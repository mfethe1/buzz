//! Atomic approval waits and one-use continuation admission.

use buzz_core::CommunityId;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{Db, DbError, Result};

/// Complete immutable state saved by the executor before returning suspended.
#[derive(Debug, Clone)]
pub struct ApprovalWait {
    /// Owning tenant, derived from the run.
    pub community_id: CommunityId,
    /// Owning workflow.
    pub workflow_id: Uuid,
    /// Suspended run.
    pub run_id: Uuid,
    /// Channel for the signed request and decision audit events.
    pub channel_id: Uuid,
    /// Random public approval reference (not a bearer credential).
    pub reference: Vec<u8>,
    /// Authored step ID.
    pub step_id: String,
    /// Suspended step index.
    pub step_index: i32,
    /// Explicit approver pubkey or `any` current channel member.
    pub approver_spec: String,
    /// Rendered customer-facing request.
    pub message: String,
    /// Bounded lifetime, in seconds.
    pub timeout_secs: i64,
    /// Immutable definition, owner, trigger context, outputs and next step.
    pub continuation: Value,
    /// Full trace including the pending approval.
    pub trace: Value,
}

/// Durable outcome returned for a signed decision, including exact replay.
#[derive(Debug, Clone)]
pub struct DecisionReceipt {
    /// Run affected by this decision.
    pub run_id: Uuid,
    /// Terminal approval status.
    pub status: String,
    /// Whether this exact signed decision had already committed.
    pub duplicate: bool,
}

/// An admitted continuation. The database never returns a claim twice.
#[derive(Debug)]
pub struct ApprovalContinuation {
    /// Owning tenant.
    pub community_id: CommunityId,
    /// One-use approval reference.
    pub reference: Vec<u8>,
    /// Run to continue.
    pub run_id: Uuid,
    /// Owning workflow.
    pub workflow_id: Uuid,
    /// Saved, immutable execution state.
    pub snapshot: Value,
    /// Trace prefix, including the signed decision.
    pub trace: Value,
    /// First step after the approval.
    pub next_step: i32,
    /// Signer whose current channel membership is rechecked at admission.
    pub approver_pubkey: Vec<u8>,
    /// Exact accepted native decision event.
    pub decision_event_id: Vec<u8>,
}

/// Persist a native request event, approval row, continuation and run wait in
/// the caller's lifecycle-fenced transaction. No partial wait can commit.
pub async fn save_wait(
    tx: &mut Transaction<'_, Postgres>,
    wait: &ApprovalWait,
    request_event: &nostr::Event,
) -> Result<buzz_core::StoredEvent> {
    let row = sqlx::query(
        "SELECT r.status::text AS status, w.channel_id FROM workflow_runs r \
         JOIN workflows w ON (w.community_id,w.id)=(r.community_id,r.workflow_id) \
         WHERE r.community_id=$1 AND r.id=$2 AND r.workflow_id=$3 FOR UPDATE OF r",
    )
    .bind(wait.community_id.as_uuid())
    .bind(wait.run_id)
    .bind(wait.workflow_id)
    .fetch_one(&mut **tx)
    .await?;
    if row.try_get::<String, _>("status")? != "running"
        || row.try_get::<Option<Uuid>, _>("channel_id")? != Some(wait.channel_id)
        || wait.timeout_secs <= 0
        || wait.timeout_secs > 604800
        || wait.reference.len() != 32
    {
        return Err(DbError::InvalidData(
            "run cannot enter approval wait".into(),
        ));
    }
    let (stored, _) = crate::event::insert_event_in_transaction(
        tx,
        wait.community_id,
        request_event,
        Some(wait.channel_id),
    )
    .await?;
    sqlx::query(
        "INSERT INTO workflow_approvals \
         (community_id,token,workflow_id,run_id,step_id,step_index,approver_spec,expires_at, \
          request_event_id,request_message,continuation) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,clock_timestamp()+make_interval(secs=>$8),$9,$10,$11)",
    )
    .bind(wait.community_id.as_uuid())
    .bind(&wait.reference)
    .bind(wait.workflow_id)
    .bind(wait.run_id)
    .bind(&wait.step_id)
    .bind(wait.step_index)
    .bind(&wait.approver_spec)
    .bind(wait.timeout_secs as f64)
    .bind(request_event.id.as_bytes().as_slice())
    .bind(&wait.message)
    .bind(&wait.continuation)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE workflow_runs SET status='waiting_approval',current_step=$3,execution_trace=$4 \
        WHERE community_id=$1 AND id=$2",
    )
    .bind(wait.community_id.as_uuid())
    .bind(wait.run_id)
    .bind(wait.step_index)
    .bind(&wait.trace)
    .execute(&mut **tx)
    .await?;
    Ok(stored)
}

/// Decide and store the signed event in ONE transaction. Locks both approval and
/// run, and the current membership row, so concurrent revocation/decisions cannot
/// use stale membership or turn a previously granted wait into a second resume.
pub async fn decide(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    reference: &[u8],
    event: &nostr::Event,
    grant: bool,
    allowed_channels: Option<&[Uuid]>,
) -> Result<DecisionReceipt> {
    let row = sqlx::query(
        "SELECT a.run_id,a.status::text AS status,a.decision_event_id,a.approver_spec, \
         a.expires_at,a.continuation,a.step_id,r.status::text AS run_status,r.current_step,a.step_index,w.channel_id \
         FROM workflow_approvals a JOIN workflow_runs r ON (r.community_id,r.id)=(a.community_id,a.run_id) \
         JOIN workflows w ON (w.community_id,w.id)=(a.community_id,a.workflow_id) \
         WHERE a.community_id=$1 AND a.token=$2 FOR UPDATE OF a,r",
    ).bind(community.as_uuid()).bind(reference).fetch_optional(&mut **tx).await?
        .ok_or_else(|| DbError::NotFound("approval".into()))?;
    let run_id: Uuid = row.try_get("run_id")?;
    let channel_id: Uuid = row.try_get("channel_id")?;
    if allowed_channels.is_some_and(|ids| !ids.contains(&channel_id)) {
        return Err(DbError::AccessDenied(
            "approval is outside token channel scope".into(),
        ));
    }
    let expected_kind = if grant { 46030 } else { 46031 };
    if event.kind.as_u16() != expected_kind
        || event.tags.iter().any(|tag| {
            let parts = tag.as_slice();
            parts.first().is_some_and(|key| key == "h")
                && (parts.len() != 2 || parts[1] != channel_id.to_string())
        })
    {
        return Err(DbError::InvalidData(
            "decision kind or channel does not match approval".into(),
        ));
    }
    let member: Option<String> = sqlx::query_scalar(
        "SELECT cm.role::text FROM channel_members cm JOIN channels c \
         ON (c.community_id,c.id)=(cm.community_id,cm.channel_id) \
         WHERE cm.community_id=$1 AND cm.channel_id=$2 AND cm.pubkey=$3 \
         AND cm.removed_at IS NULL AND c.deleted_at IS NULL AND c.archived_at IS NULL FOR SHARE OF cm,c",
    ).bind(community.as_uuid()).bind(channel_id).bind(event.pubkey.to_bytes().as_slice())
        .fetch_optional(&mut **tx).await?;
    let spec: String = row.try_get("approver_spec")?;
    if member.is_none() || !approver_matches(&spec, &event.pubkey.to_hex()) {
        return Err(DbError::AccessDenied(
            "not a current designated channel approver".into(),
        ));
    }
    let previous: Option<Vec<u8>> = row.try_get("decision_event_id")?;
    let status: String = row.try_get("status")?;
    if previous.as_deref() == Some(event.id.as_bytes().as_slice()) {
        return Ok(DecisionReceipt {
            run_id,
            status,
            duplicate: true,
        });
    }
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    if status != "pending"
        || row.try_get::<DateTime<Utc>, _>("expires_at")? <= now
        || row.try_get::<String, _>("run_status")? != "waiting_approval"
        || row.try_get::<i32, _>("current_step")? != row.try_get::<i32, _>("step_index")?
        || row.try_get::<Option<Value>, _>("continuation")?.is_none()
    {
        return Err(DbError::InvalidData(
            "approval is resolved, expired, or lacks a continuation".into(),
        ));
    }
    let (_, inserted) =
        crate::event::insert_event_in_transaction(tx, community, event, Some(channel_id)).await?;
    if !inserted {
        return Err(DbError::InvalidData(
            "decision event already used outside this approval".into(),
        ));
    }
    let status = if grant { "granted" } else { "denied" };
    sqlx::query("UPDATE workflow_approvals SET status=$3::approval_status,approver_pubkey=$4,note=$5, \
        decision_event_id=$6,granted_at=CASE WHEN $3='granted' THEN clock_timestamp() ELSE NULL END, \
        denied_at=CASE WHEN $3='denied' THEN clock_timestamp() ELSE NULL END WHERE community_id=$1 AND token=$2")
        .bind(community.as_uuid()).bind(reference).bind(status).bind(event.pubkey.to_bytes().as_slice())
        .bind(&event.content).bind(event.id.as_bytes().as_slice()).execute(&mut **tx).await?;
    let decision_trace = serde_json::json!([{"step_id": row.try_get::<String,_>("step_id")?, "status": status,
        "output":{"approved":grant,"decision_event_id":event.id.to_hex()},
        "approval_ref":hex::encode(reference),"decision_event_id":event.id.to_hex(),"approver_pubkey":event.pubkey.to_hex()}]);
    sqlx::query("UPDATE workflow_runs SET execution_trace=execution_trace || $3::jsonb, \
        status=CASE WHEN $4 THEN status ELSE 'cancelled'::run_status END, \
        error_code=CASE WHEN $4 THEN NULL ELSE 'approval_denied' END, \
        error_message=CASE WHEN $4 THEN NULL ELSE 'Workflow approval was denied' END, \
        completed_at=CASE WHEN $4 THEN NULL ELSE clock_timestamp() END WHERE community_id=$1 AND id=$2")
        .bind(community.as_uuid()).bind(run_id).bind(decision_trace).bind(grant).execute(&mut **tx).await?;
    Ok(DecisionReceipt {
        run_id,
        status: status.into(),
        duplicate: false,
    })
}

/// Supported approver formats deliberately exclude display-name/role guessing.
pub fn approver_matches(spec: &str, pubkey: &str) -> bool {
    spec == "any"
        || (spec.len() == 64
            && spec.bytes().all(|b| b.is_ascii_hexdigit())
            && spec.eq_ignore_ascii_case(pubkey))
}

impl Db {
    /// Bounded writer-side recovery scan; each returned row is only a candidate.
    pub async fn workflow_approval_candidates(&self) -> Result<Vec<(CommunityId, Vec<u8>)>> {
        let rows = sqlx::query("SELECT a.community_id,a.token FROM workflow_approvals a \
            JOIN workflow_runs r ON (r.community_id,r.id)=(a.community_id,a.run_id) \
            JOIN communities c ON c.id=a.community_id AND c.deletion_state='active' \
            WHERE a.continuation IS NOT NULL AND \
            ((a.status='pending' AND a.expires_at<=clock_timestamp() AND r.status='waiting_approval' AND r.current_step=a.step_index) \
            OR (a.status='granted' AND a.resume_claimed_at IS NULL AND r.status='waiting_approval' AND r.current_step=a.step_index) \
            OR (a.status='granted' AND a.resume_deadline_at<=clock_timestamp() AND r.status='running' AND r.current_step=a.step_index+1)) \
            ORDER BY a.created_at LIMIT 100")
            .fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| {
                Ok((
                    CommunityId::from_uuid(r.try_get("community_id")?),
                    r.try_get("token")?,
                ))
            })
            .collect()
    }

    /// Atomically expire an old wait or claim a granted continuation once.
    /// A claimed execution is NEVER retried: after its deadline the durable
    /// result is explicitly unknown/interrupted, as external effects may exist.
    pub async fn claim_workflow_approval(
        &self,
        community: CommunityId,
        reference: &[u8],
        budget_secs: i64,
    ) -> Result<Option<ApprovalContinuation>> {
        if !(1..=3600).contains(&budget_secs) {
            return Err(DbError::InvalidData("invalid resume budget".into()));
        }
        let mut tx = self.begin_event_write_transaction().await?;
        crate::deletion::DeletionStore::new(self.pool.clone())
            .guard_transaction(&mut tx, community)
            .await?;
        let row = sqlx::query("SELECT a.*,a.status::text AS approval_status,r.status::text AS run_status,r.current_step,r.execution_trace \
            FROM workflow_approvals a JOIN workflow_runs r ON (r.community_id,r.id)=(a.community_id,a.run_id) \
            WHERE a.community_id=$1 AND a.token=$2 FOR UPDATE OF a,r")
            .bind(community.as_uuid()).bind(reference).fetch_one(&mut *tx).await?;
        let run_id: Uuid = row.try_get("run_id")?;
        let step: i32 = row.try_get("step_index")?;
        let current: i32 = row.try_get("current_step")?;
        let run_status: String = row.try_get("run_status")?;
        let status: String = row.try_get("approval_status")?;
        let claimed: Option<DateTime<Utc>> = row.try_get("resume_claimed_at")?;
        let deadline: Option<DateTime<Utc>> = row.try_get("resume_deadline_at")?;
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let expired = status == "pending"
            && row.try_get::<DateTime<Utc>, _>("expires_at")? <= now
            && run_status == "waiting_approval"
            && current == step;
        let interrupted = status == "granted"
            && claimed.is_some()
            && deadline.is_some_and(|d| d <= now)
            && run_status == "running"
            && current == step + 1;
        if expired || interrupted {
            let code = if expired {
                "approval_expired"
            } else {
                "approval_resume_outcome_unknown"
            };
            sqlx::query("UPDATE workflow_runs SET status='failed',error_code=$3,error_message=$4,completed_at=clock_timestamp() WHERE community_id=$1 AND id=$2")
                .bind(community.as_uuid()).bind(run_id).bind(code)
                .bind(if expired {"Workflow approval expired"}else{"Continuation interrupted; effects may have occurred and will not be replayed"})
                .execute(&mut *tx).await?;
            if expired {
                sqlx::query("UPDATE workflow_approvals SET status='expired' WHERE community_id=$1 AND token=$2").bind(community.as_uuid()).bind(reference).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            return Ok(None);
        }
        if status != "granted"
            || claimed.is_some()
            || run_status != "waiting_approval"
            || current != step
        {
            return Ok(None);
        }
        let snapshot: Value = row
            .try_get::<Option<Value>, _>("continuation")?
            .ok_or_else(|| DbError::InvalidData("missing continuation".into()))?;
        sqlx::query("UPDATE workflow_approvals SET resume_claimed_at=clock_timestamp(),resume_deadline_at=clock_timestamp()+make_interval(secs=>$3) WHERE community_id=$1 AND token=$2")
            .bind(community.as_uuid()).bind(reference).bind((budget_secs+30) as f64).execute(&mut *tx).await?;
        sqlx::query("UPDATE workflow_runs SET status='running',current_step=$3 WHERE community_id=$1 AND id=$2")
            .bind(community.as_uuid()).bind(run_id).bind(step+1).execute(&mut *tx).await?;
        let claim = ApprovalContinuation {
            approver_pubkey: row.try_get("approver_pubkey")?,
            decision_event_id: row.try_get("decision_event_id")?,
            community_id: community,
            reference: reference.into(),
            run_id,
            workflow_id: row.try_get("workflow_id")?,
            snapshot,
            trace: row.try_get("execution_trace")?,
            next_step: step + 1,
        };
        tx.commit().await?;
        Ok(Some(claim))
    }
}

#[cfg(test)]
mod postgres_tests {
    use super::*;
    use crate::workflow::RunStatus;
    use nostr::{Event, EventBuilder, Keys, Kind, Tag};

    struct Fixture {
        db: Db,
        pool: sqlx::PgPool,
        wait: ApprovalWait,
        owner: Keys,
        other: Keys,
        request: Event,
    }

    async fn fixture() -> Fixture {
        fixture_with_schema(true).await
    }

    async fn fixture_with_schema(migrate: bool) -> Fixture {
        let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("Postgres");
        let db = Db::from_pool(pool.clone());
        if migrate && std::env::var("BUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            db.migrate().await.expect("migrations");
        }
        let owner = Keys::generate();
        let other = Keys::generate();
        let community = db
            .ensure_configured_community(&format!("approval-{}.example", Uuid::new_v4()))
            .await
            .expect("community")
            .id;
        for keys in [&owner, &other] {
            db.ensure_user(community, &keys.public_key().to_bytes())
                .await
                .expect("user");
        }
        let channel = db
            .create_channel(
                community,
                "approval-test",
                buzz_core::channel::ChannelType::Stream,
                buzz_core::channel::ChannelVisibility::Private,
                None,
                &owner.public_key().to_bytes(),
                None,
            )
            .await
            .expect("channel");
        let workflow_id = db
            .create_workflow(
                community,
                Some(channel.id),
                &owner.public_key().to_bytes(),
                "approval-test",
                "{}",
                &[1; 32],
            )
            .await
            .expect("workflow");
        let run_id = db
            .create_workflow_run(community, workflow_id, None, None)
            .await
            .expect("run");
        db.update_workflow_run(
            community,
            run_id,
            RunStatus::Running,
            0,
            &serde_json::json!([]),
            None,
        )
        .await
        .expect("running");
        let reference = super::super::hash_approval_token(&Uuid::new_v4().to_string());
        let request = EventBuilder::new(Kind::Custom(46010), "Approve exact operation")
            .tags([Tag::parse(["d", &hex::encode(&reference)]).expect("tag")])
            .sign_with_keys(&owner)
            .expect("signed request");
        let wait = ApprovalWait {
            community_id: community,
            workflow_id,
            run_id,
            channel_id: channel.id,
            reference,
            step_id: "review".into(),
            step_index: 0,
            approver_spec: owner.public_key().to_hex(),
            message: "Approve exact operation".into(),
            timeout_secs: 3600,
            continuation: serde_json::json!({"definition_hash":"original","next_step":1}),
            trace: serde_json::json!([{ "step_id":"review","status":"waiting_approval" }]),
        };
        Fixture {
            db,
            pool,
            wait,
            owner,
            other,
            request,
        }
    }

    async fn suspend(f: &Fixture) {
        let mut tx = f.db.begin_event_write_transaction().await.expect("tx");
        save_wait(&mut tx, &f.wait, &f.request)
            .await
            .expect("save atomic wait");
        tx.commit().await.expect("commit wait");
    }

    fn decision(f: &Fixture, keys: &Keys, grant: bool, note: &str) -> Event {
        EventBuilder::new(Kind::Custom(if grant { 46030 } else { 46031 }), note)
            .tags([Tag::parse(["d", &hex::encode(&f.wait.reference)]).expect("tag")])
            .sign_with_keys(keys)
            .expect("signed decision")
    }

    async fn commit_decision(
        db: &Db,
        community: CommunityId,
        reference: &[u8],
        event: &Event,
        grant: bool,
    ) -> Result<DecisionReceipt> {
        let mut tx = db.begin_event_write_transaction().await?;
        let receipt = decide(&mut tx, community, reference, event, grant, None).await?;
        tx.commit().await?;
        Ok(receipt)
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn migration_schema_workflow_approval_legacy_wait_fails_closed() {
        let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("PG");
        crate::migration::run_migrations_through(&pool, 46)
            .await
            .expect("old schema");
        let f = fixture_with_schema(false).await;
        f.db.create_approval(crate::workflow::CreateApprovalParams {
            community_id: f.wait.community_id,
            token: "legacy-test-reference",
            workflow_id: f.wait.workflow_id,
            run_id: f.wait.run_id,
            step_id: "review",
            step_index: 0,
            approver_spec: "any",
            expires_at: Utc::now() + chrono::Duration::hours(1),
        })
        .await
        .expect("legacy approval");
        f.db.update_workflow_run(
            f.wait.community_id,
            f.wait.run_id,
            RunStatus::WaitingApproval,
            0,
            &serde_json::json!([]),
            None,
        )
        .await
        .expect("legacy wait");
        f.db.migrate().await.expect("additive approval migration");
        let run =
            f.db.get_workflow_run(f.wait.community_id, f.wait.run_id)
                .await
                .expect("run");
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(
            run.error_code.as_deref(),
            Some("approval_continuation_unavailable")
        );
        let approval =
            f.db.get_approval(f.wait.community_id, "legacy-test-reference")
                .await
                .expect("legacy history retained");
        assert_eq!(approval.status, super::super::ApprovalStatus::Expired);
        assert!(approval.request_event_id.is_none());
        let indexes:i64=sqlx::query_scalar("SELECT count(*) FROM pg_indexes WHERE tablename='workflow_approvals' AND indexname IN ('idx_workflow_approvals_decision_event','idx_workflow_approvals_recovery')").fetch_one(&pool).await.expect("indexes");
        assert_eq!(indexes, 2);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn workflow_approval_wait_rolls_back_all_state_and_persists_snapshot() {
        let f = fixture().await;
        let mut tx = f.db.begin_event_write_transaction().await.expect("tx");
        save_wait(&mut tx, &f.wait, &f.request).await.expect("save");
        tx.rollback().await.expect("rollback");
        assert_eq!(
            f.db.get_workflow_run(f.wait.community_id, f.wait.run_id)
                .await
                .expect("run")
                .status,
            RunStatus::Running
        );
        assert!(f
            .db
            .get_approval_by_stored_hash(f.wait.community_id, &f.wait.reference)
            .await
            .is_err());
        assert!(f
            .db
            .get_event_by_id(f.wait.community_id, f.request.id.as_bytes())
            .await
            .expect("event lookup")
            .is_none());
        suspend(&f).await;
        let row =
            f.db.get_approval_by_stored_hash(f.wait.community_id, &f.wait.reference)
                .await
                .expect("approval");
        assert_eq!(row.request_event_id, Some(f.request.id.as_bytes().to_vec()));
        assert_eq!(
            row.request_message.as_deref(),
            Some("Approve exact operation")
        );
        let snapshot: Value = sqlx::query_scalar(
            "SELECT continuation FROM workflow_approvals WHERE community_id=$1 AND token=$2",
        )
        .bind(f.wait.community_id.as_uuid())
        .bind(&f.wait.reference)
        .fetch_one(&f.pool)
        .await
        .expect("snapshot");
        assert_eq!(snapshot, f.wait.continuation);
        assert_eq!(
            f.db.get_workflow_run(f.wait.community_id, f.wait.run_id)
                .await
                .expect("run")
                .status,
            RunStatus::WaitingApproval
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn workflow_approval_membership_designation_and_token_scope_are_independent() {
        let mut f = fixture().await;
        f.wait.approver_spec = "any".into();
        suspend(&f).await;
        let outsider = decision(&f, &f.other, true, "outsider");
        assert!(matches!(
            commit_decision(
                &f.db,
                f.wait.community_id,
                &f.wait.reference,
                &outsider,
                true
            )
            .await,
            Err(DbError::AccessDenied(_))
        ));
        f.db.add_member(
            f.wait.community_id,
            f.wait.channel_id,
            &f.other.public_key().to_bytes(),
            buzz_core::channel::MemberRole::Member,
            Some(&f.owner.public_key().to_bytes()),
        )
        .await
        .expect("member");
        sqlx::query(
            "UPDATE workflow_approvals SET approver_spec=$3 WHERE community_id=$1 AND token=$2",
        )
        .bind(f.wait.community_id.as_uuid())
        .bind(&f.wait.reference)
        .bind(f.owner.public_key().to_hex())
        .execute(&f.pool)
        .await
        .expect("designated owner");
        assert!(matches!(
            commit_decision(
                &f.db,
                f.wait.community_id,
                &f.wait.reference,
                &outsider,
                true
            )
            .await,
            Err(DbError::AccessDenied(_))
        ));
        let valid = decision(&f, &f.owner, true, "owner");
        let mut tx = f.db.begin_event_write_transaction().await.expect("tx");
        assert!(matches!(
            decide(
                &mut tx,
                f.wait.community_id,
                &f.wait.reference,
                &valid,
                true,
                Some(&[])
            )
            .await,
            Err(DbError::AccessDenied(_))
        ));
        tx.rollback().await.expect("rollback");
        assert_eq!(
            commit_decision(&f.db, f.wait.community_id, &f.wait.reference, &valid, true)
                .await
                .expect("authorized")
                .status,
            "granted"
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn workflow_approval_decision_rollback_is_atomic() {
        let f = fixture().await;
        suspend(&f).await;
        let event = decision(&f, &f.owner, true, "approved");
        let mut tx = f.db.begin_event_write_transaction().await.expect("tx");
        decide(
            &mut tx,
            f.wait.community_id,
            &f.wait.reference,
            &event,
            true,
            None,
        )
        .await
        .expect("decision");
        tx.rollback().await.expect("rollback");
        assert_eq!(
            f.db.get_approval_by_stored_hash(f.wait.community_id, &f.wait.reference)
                .await
                .expect("approval")
                .status,
            super::super::ApprovalStatus::Pending
        );
        assert!(f
            .db
            .get_event_by_id(f.wait.community_id, event.id.as_bytes())
            .await
            .expect("lookup")
            .is_none());
        assert!(f
            .db
            .workflow_approval_candidates()
            .await
            .expect("candidates")
            .iter()
            .all(|(_, r)| r != &f.wait.reference));
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn workflow_approval_rejects_outsider_wrong_tenant_and_expired_decisions() {
        let f = fixture().await;
        suspend(&f).await;
        let outsider = decision(&f, &f.other, true, "outsider");
        assert!(matches!(
            commit_decision(
                &f.db,
                f.wait.community_id,
                &f.wait.reference,
                &outsider,
                true
            )
            .await,
            Err(DbError::AccessDenied(_))
        ));
        let valid = decision(&f, &f.owner, true, "owner");
        assert!(commit_decision(
            &f.db,
            CommunityId::from_uuid(Uuid::new_v4()),
            &f.wait.reference,
            &valid,
            true
        )
        .await
        .is_err());
        sqlx::query("UPDATE workflow_approvals SET expires_at=clock_timestamp()-interval '1 second' WHERE community_id=$1 AND token=$2").bind(f.wait.community_id.as_uuid()).bind(&f.wait.reference).execute(&f.pool).await.expect("expire fixture");
        assert!(
            commit_decision(&f.db, f.wait.community_id, &f.wait.reference, &valid, true)
                .await
                .is_err()
        );
        assert!(f
            .db
            .claim_workflow_approval(f.wait.community_id, &f.wait.reference, 30)
            .await
            .expect("expire")
            .is_none());
        let run =
            f.db.get_workflow_run(f.wait.community_id, f.wait.run_id)
                .await
                .expect("run");
        assert_eq!(run.error_code.as_deref(), Some("approval_expired"));
        assert_eq!(
            f.db.get_approval_by_stored_hash(f.wait.community_id, &f.wait.reference)
                .await
                .expect("approval")
                .status,
            super::super::ApprovalStatus::Expired
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn workflow_approval_competing_decisions_have_one_winner_and_exact_replay() {
        let f = fixture().await;
        suspend(&f).await;
        let grant = decision(&f, &f.owner, true, "grant");
        let deny = decision(&f, &f.owner, false, "deny");
        let (granted, denied) = tokio::join!(
            commit_decision(&f.db, f.wait.community_id, &f.wait.reference, &grant, true),
            commit_decision(&f.db, f.wait.community_id, &f.wait.reference, &deny, false)
        );
        assert_ne!(
            granted.is_ok(),
            denied.is_ok(),
            "exactly one competing decision commits"
        );
        let (winner, approved) = if granted.is_ok() {
            (&grant, true)
        } else {
            (&deny, false)
        };
        let replay = commit_decision(
            &f.db,
            f.wait.community_id,
            &f.wait.reference,
            winner,
            approved,
        )
        .await
        .expect("exact replay");
        assert!(replay.duplicate);
        assert_eq!(replay.run_id, f.wait.run_id);
        let event =
            f.db.get_event_by_id(f.wait.community_id, winner.id.as_bytes())
                .await
                .expect("lookup")
                .expect("durable signed decision");
        assert_eq!(event.channel_id, Some(f.wait.channel_id));
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn workflow_approval_recovery_claims_once_and_never_replays_interrupted_effects() {
        let f = fixture().await;
        suspend(&f).await;
        let event = decision(&f, &f.owner, true, "grant");
        commit_decision(&f.db, f.wait.community_id, &f.wait.reference, &event, true)
            .await
            .expect("commit");
        // Recreated Db handles model another process observing only committed state.
        let restarted = Db::from_pool(f.pool.clone());
        let (a, b) = tokio::join!(
            restarted.claim_workflow_approval(f.wait.community_id, &f.wait.reference, 30),
            f.db.claim_workflow_approval(f.wait.community_id, &f.wait.reference, 30)
        );
        assert_ne!(a.expect("first").is_some(), b.expect("second").is_some());
        sqlx::query("UPDATE workflow_approvals SET resume_deadline_at=clock_timestamp()-interval '1 second' WHERE community_id=$1 AND token=$2").bind(f.wait.community_id.as_uuid()).bind(&f.wait.reference).execute(&f.pool).await.expect("interrupt fixture");
        assert!(restarted
            .claim_workflow_approval(f.wait.community_id, &f.wait.reference, 30)
            .await
            .expect("recover")
            .is_none());
        let run =
            f.db.get_workflow_run(f.wait.community_id, f.wait.run_id)
                .await
                .expect("run");
        assert_eq!(
            run.error_code.as_deref(),
            Some("approval_resume_outcome_unknown")
        );
        assert!(
            f.db.update_workflow_run(
                f.wait.community_id,
                f.wait.run_id,
                RunStatus::Completed,
                2,
                &serde_json::json!([]),
                None
            )
            .await
            .is_err(),
            "late writer must not overwrite unknown outcome"
        );
        assert!(restarted
            .claim_workflow_approval(f.wait.community_id, &f.wait.reference, 30)
            .await
            .expect("repeat recovery")
            .is_none());
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn workflow_approval_denial_is_terminal_before_returning_receipt() {
        let f = fixture().await;
        suspend(&f).await;
        let event = decision(&f, &f.owner, false, "do not run");
        let receipt = commit_decision(&f.db, f.wait.community_id, &f.wait.reference, &event, false)
            .await
            .expect("deny");
        assert_eq!(receipt.status, "denied");
        let run =
            f.db.get_workflow_run(f.wait.community_id, f.wait.run_id)
                .await
                .expect("run");
        assert_eq!(run.status, RunStatus::Cancelled);
        assert_eq!(run.error_code.as_deref(), Some("approval_denied"));
        assert!(f
            .db
            .claim_workflow_approval(f.wait.community_id, &f.wait.reference, 30)
            .await
            .expect("no continuation")
            .is_none());
    }
}
