//! Atomic signed-event projection and primary-database execution admission.

use buzz_core::{
    cml_event::{self, CmlTransition},
    fleet::{self, FleetReceipt, FleetScope, ReceiptStatus},
    CommunityId, StoredEvent,
};
use buzz_datastore_tracing::datastore_span;
use nostr::Event;
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    event::ThreadMetadataParams,
    observability::{acquire_writer, WriterOperation},
    Db, DbError, Result,
};

fn deny(message: impl Into<String>) -> DbError {
    DbError::AccessDenied(message.into())
}
fn invalid(error: impl std::fmt::Display) -> DbError {
    DbError::InvalidData(error.to_string())
}

async fn lock_task(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    task: Uuid,
) -> Result<PgRow> {
    // Take the existing lifecycle lock before task, home, grant, or event rows.
    let active: bool = sqlx::query_scalar("SELECT community_write_allowed($1)")
        .bind(community.as_uuid())
        .fetch_one(&mut **tx)
        .await?;
    if !active {
        return Err(deny("community does not admit fleet execution"));
    }
    sqlx::query("SELECT channel_id, revision, archived_at FROM tasks WHERE community_id=$1 AND id=$2 FOR UPDATE")
        .bind(community.as_uuid()).bind(task).fetch_optional(&mut **tx).await?
        .ok_or_else(|| deny("task unavailable"))
}

/// Lock positive authorization evidence so revocation/home moves serialize with
/// admission. No permission cache or replica may be used by this path.
async fn lock_authority(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    channel: Uuid,
    planner: &[u8],
    worker: &[u8],
    scope: &FleetScope,
) -> Result<Value> {
    let now: i64 = sqlx::query_scalar("SELECT extract(epoch FROM clock_timestamp())::bigint")
        .fetch_one(&mut **tx)
        .await?;
    if now < 0 || now as u64 >= scope.expires_at {
        return Err(deny("fleet grant expired"));
    }
    let channel_live = sqlx::query("SELECT id FROM channels WHERE community_id=$1 AND id=$2 AND deleted_at IS NULL AND archived_at IS NULL FOR SHARE")
        .bind(community.as_uuid()).bind(channel).fetch_optional(&mut **tx).await?;
    if channel_live.is_none() {
        return Err(deny("fleet channel unavailable"));
    }
    // An opt-in execution plan requires both participants to be explicit current
    // channel members. Directory presence or an inherited display cache is not
    // execution authority.
    for key in [planner, worker] {
        let user = sqlx::query("SELECT pubkey FROM users WHERE community_id=$1 AND pubkey=$2 AND deactivated_at IS NULL FOR SHARE")
            .bind(community.as_uuid()).bind(key).fetch_optional(&mut **tx).await?;
        let membership = sqlx::query("SELECT pubkey FROM channel_members WHERE community_id=$1 AND channel_id=$2 AND pubkey=$3 AND removed_at IS NULL FOR SHARE")
            .bind(community.as_uuid()).bind(channel).bind(key).fetch_optional(&mut **tx).await?;
        if user.is_none() || membership.is_none() {
            return Err(deny("fleet participant is not an active channel member"));
        }
    }
    let home = sqlx::query("SELECT pubkey FROM users WHERE community_id=$1 AND pubkey=$2 AND machine_id=$3 AND agent_type IS NOT NULL AND deactivated_at IS NULL FOR SHARE")
        .bind(community.as_uuid()).bind(worker).bind(&scope.machine_id).fetch_optional(&mut **tx).await?;
    if home.is_none() {
        return Err(deny(
            "fleet worker does not own the registered machine home",
        ));
    }
    let grants = sqlx::query("SELECT to_jsonb(g) AS grant FROM agent_capability_grants g WHERE community_id=$1 AND agent_pubkey=$2 AND capability='cross_ssh' AND target IN ($3,'*') AND revoked_at IS NULL ORDER BY (target=$3) DESC FOR SHARE")
        .bind(community.as_uuid()).bind(planner).bind(&scope.machine_id).fetch_all(&mut **tx).await?;
    grants
        .first()
        .map(|row| row.get("grant"))
        .ok_or_else(|| deny("planner has no active cross_ssh capability for this machine"))
}

fn row_scope(row: &PgRow) -> Result<FleetScope> {
    serde_json::from_value(row.get("scope")).map_err(invalid)
}
fn row_event(row: &PgRow, column: &str) -> Result<Event> {
    serde_json::from_value(row.get(column)).map_err(invalid)
}
fn same_revision(task: &PgRow, scope: &FleetScope) -> Result<()> {
    if task
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("archived_at")
        .is_some()
        || task.get::<i32, _>("revision") != scope.task_revision
    {
        return Err(deny("task revision no longer matches approved plan"));
    }
    Ok(())
}

async fn project_cml(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    task: &PgRow,
    event: &Event,
) -> Result<()> {
    let cml = cml_event::validate_cml_event_after_signature(event).map_err(invalid)?;
    let scope = FleetScope::from_task(&cml.task)
        .map_err(invalid)?
        .ok_or_else(|| invalid("fleet scope missing"))?;
    if task.get::<Option<Uuid>, _>("channel_id") != Some(cml.channel_id) {
        return Err(deny("fleet plan must name its task's channel"));
    }
    let planner = hex::decode(&cml.task.roles.planner).map_err(invalid)?;
    let worker = hex::decode(
        cml.task
            .roles
            .worker
            .as_deref()
            .ok_or_else(|| deny("fleet worker required"))?,
    )
    .map_err(invalid)?;
    if cml.transition == CmlTransition::Plan {
        same_revision(task, &scope)?;
        if scope.expires_at <= cml.task.updated_at
            || scope.expires_at > cml.task.updated_at.saturating_add(3600)
        {
            return Err(deny("fleet plan deadline must be within one hour"));
        }
        let grant =
            lock_authority(tx, community, cml.channel_id, &planner, &worker, &scope).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM fleet_attempts WHERE community_id=$1 AND task_id=$2)",
        )
        .bind(community.as_uuid())
        .bind(cml.task.id)
        .fetch_one(&mut **tx)
        .await?;
        if exists {
            return Err(deny(
                "task already has a qualification attempt; automatic replacement is forbidden",
            ));
        }
        let id = fleet::attempt_id(community, cml.task.id, event.id.as_bytes());
        let raw = serde_json::to_value(event)?;
        sqlx::query("INSERT INTO fleet_attempts (community_id,id,task_id,channel_id,task_revision,plan_event_id,plan_event,cml_head,cml_event,planner_pubkey,worker_pubkey,machine_id,scope,state,permission_grants) VALUES ($1,$2,$3,$4,$5,$6,$7,$6,$7,$8,$9,$10,$11,'planned',$12)")
            .bind(community.as_uuid()).bind(id).bind(cml.task.id).bind(cml.channel_id).bind(scope.task_revision).bind(event.id.as_bytes().as_slice()).bind(raw).bind(planner).bind(worker).bind(&scope.machine_id).bind(serde_json::to_value(&scope)?).bind(json!({"plan":grant})).execute(&mut **tx).await?;
        return Ok(());
    }
    let row =
        sqlx::query("SELECT * FROM fleet_attempts WHERE community_id=$1 AND task_id=$2 FOR UPDATE")
            .bind(community.as_uuid())
            .bind(cml.task.id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| deny("fleet plan has no relay admission"))?;
    let previous = cml_event::validate_cml_event_after_signature(&row_event(&row, "cml_event")?)
        .map_err(invalid)?;
    cml_event::validate_successor(&previous, &cml).map_err(|e| deny(e.to_string()))?;
    if row_scope(&row)? != scope {
        return Err(deny("fleet scope changed"));
    }
    let state: String = row.get("state");
    let mut next_state = state.clone();
    let mut grant = None;
    if matches!(cml.transition, CmlTransition::Claim | CmlTransition::Start) {
        let expected = if cml.transition == CmlTransition::Claim {
            "planned"
        } else {
            "claimed"
        };
        if state != expected || row.get::<Option<Vec<u8>>, _>("cancel_event_id").is_some() {
            return Err(deny("fleet attempt cannot claim or start again"));
        }
        same_revision(task, &scope)?;
        grant =
            Some(lock_authority(tx, community, cml.channel_id, &planner, &worker, &scope).await?);
        let lease = cml
            .task
            .lease
            .as_ref()
            .ok_or_else(|| deny("fleet lease required"))?;
        if lease.id != row.get::<String, _>("id") || lease.expires_at != scope.expires_at {
            return Err(deny("fleet lease binding mismatch"));
        }
        next_state = if cml.transition == CmlTransition::Claim {
            "claimed"
        } else {
            "started"
        }
        .into();
    } else if cml.transition == CmlTransition::LeaseExpired {
        let now: i64 = sqlx::query_scalar("SELECT extract(epoch FROM clock_timestamp())::bigint")
            .fetch_one(&mut **tx)
            .await?;
        if now < 0 || (now as u64) < scope.expires_at {
            return Err(deny("fleet lease has not expired"));
        }
        if state != "claimed" {
            return Err(deny("started fleet attempts are never re-leased"));
        }
        next_state = "expired".into();
    } else if cml.transition == CmlTransition::Submit {
        if state != "success" {
            return Err(deny(
                "worker submission requires a durable successful receipt",
            ));
        }
        let signed_receipt = row_event(&row, "receipt")?;
        let (_, receipt) =
            FleetReceipt::from_event_after_signature(&signed_receipt).map_err(invalid)?;
        let receipt_id = signed_receipt.id.to_hex();
        if row.get::<Option<Vec<u8>>, _>("receipt_event_id").as_deref()
            != Some(signed_receipt.id.as_bytes().as_slice())
            || receipt.status != ReceiptStatus::Success
            || receipt.qualification.as_ref().map(|q| &q.head_sha) != cml.task.git.head_sha.as_ref()
            || !cml.task.evidence.iter().any(|evidence| {
                evidence.kind == "fleet-qualification-receipt" && evidence.reference == receipt_id
            })
        {
            return Err(deny(
                "worker submission must bind the observed commit and signed receipt",
            ));
        }
    } else if cml.transition == CmlTransition::Block
        && !matches!(state.as_str(), "error" | "cancelled")
    {
        return Err(deny("worker blocker requires a durable terminal receipt"));
    }
    let mut permissions: Value = row.get("permission_grants");
    if let Some(grant) = grant {
        permissions[if cml.transition == CmlTransition::Claim {
            "claim"
        } else {
            "start"
        }] = grant;
    }
    sqlx::query("UPDATE fleet_attempts SET cml_head=$3,cml_event=$4,state=$5,permission_grants=$6,claim_event_id=CASE WHEN $7 THEN $3 ELSE claim_event_id END,start_event_id=CASE WHEN $8 THEN $3 ELSE start_event_id END,cancel_event_id=CASE WHEN $9 THEN $3 ELSE cancel_event_id END,updated_at=clock_timestamp() WHERE community_id=$1 AND task_id=$2")
        .bind(community.as_uuid()).bind(cml.task.id).bind(event.id.as_bytes().as_slice()).bind(serde_json::to_value(event)?).bind(next_state).bind(permissions)
        .bind(cml.transition == CmlTransition::Claim).bind(cml.transition == CmlTransition::Start).bind(cml.transition == CmlTransition::Cancel).execute(&mut **tx).await?;
    Ok(())
}

async fn project_receipt(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    task: &PgRow,
    event: &Event,
) -> Result<()> {
    let (channel, receipt) = FleetReceipt::from_event_after_signature(event).map_err(invalid)?;
    let row = sqlx::query(
        "SELECT * FROM fleet_attempts WHERE community_id=$1 AND task_id=$2 AND id=$3 FOR UPDATE",
    )
    .bind(community.as_uuid())
    .bind(receipt.task_id)
    .bind(&receipt.attempt_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| deny("fleet attempt not found"))?;
    let scope = row_scope(&row)?;
    let start: Option<Vec<u8>> = row.get("start_event_id");
    if task.get::<Option<Uuid>, _>("channel_id") != Some(channel)
        || channel != row.get::<Uuid, _>("channel_id")
        || event.pubkey.to_bytes().as_slice() != row.get::<Vec<u8>, _>("worker_pubkey")
        || receipt.plan_event_id != hex::encode(row.get::<Vec<u8>, _>("plan_event_id"))
        || receipt.start_event_id != start.as_ref().map(hex::encode)
        || receipt.policy_digest != scope.policy_digest
        || receipt.machine_id != scope.machine_id
    {
        return Err(deny("fleet receipt scope/worker/start mismatch"));
    }
    let state: String = row.get("state");
    if matches!(
        state.as_str(),
        "success" | "error" | "cancelled" | "cancelled_before_execution" | "expired"
    ) {
        return Err(deny("fleet attempt already has a terminal outcome"));
    }
    let cancelled = row.get::<Option<Vec<u8>>, _>("cancel_event_id").is_some();
    if matches!(
        receipt.status,
        ReceiptStatus::Cancelled | ReceiptStatus::CancelledBeforeExecution
    ) && !cancelled
    {
        return Err(deny(
            "stop acknowledgement requires signed planner cancellation",
        ));
    }
    if receipt.status == ReceiptStatus::CancelledBeforeExecution {
        if start.is_some() || !matches!(state.as_str(), "planned" | "claimed") {
            return Err(deny("attempt already has execution admission"));
        }
    } else if start.is_none() || !matches!(state.as_str(), "started" | "unknown") {
        return Err(deny("receipt has no matching execution admission"));
    }
    if let Some(q) = &receipt.qualification {
        let plan = cml_event::validate_cml_event_after_signature(&row_event(&row, "plan_event")?)
            .map_err(invalid)?;
        if q.repository != plan.task.git.repo {
            return Err(deny("receipt repository mismatch"));
        }
    }
    let next = match receipt.status {
        ReceiptStatus::Success => "success",
        ReceiptStatus::Error => "error",
        ReceiptStatus::Cancelled => "cancelled",
        ReceiptStatus::CancelledBeforeExecution => "cancelled_before_execution",
        ReceiptStatus::Unknown => "unknown",
    };
    sqlx::query("UPDATE fleet_attempts SET state=$3,receipt_event_id=$4,receipt=$5,updated_at=clock_timestamp() WHERE community_id=$1 AND id=$2")
        .bind(community.as_uuid()).bind(&receipt.attempt_id).bind(next).bind(event.id.as_bytes().as_slice()).bind(serde_json::to_value(event)?).execute(&mut **tx).await?;
    Ok(())
}

impl Db {
    /// Persist a signature-verified fleet event and its projection atomically.
    /// The transport-neutral ingest pipeline must run its ordinary admission,
    /// scope, membership, moderation and thread checks before calling this seam.
    #[datastore_span(name = "insert_fleet_event", system = "postgresql")]
    pub async fn insert_fleet_event(
        &self,
        community: CommunityId,
        event: &Event,
        channel: Option<Uuid>,
        thread: Option<ThreadMetadataParams<'_>>,
    ) -> Result<(StoredEvent, bool)> {
        let task_id = if fleet::is_receipt(event) {
            FleetReceipt::from_event_after_signature(event)
                .map_err(invalid)?
                .1
                .task_id
        } else {
            cml_event::validate_cml_event_after_signature(event)
                .map_err(invalid)?
                .task
                .id
        };
        let connection = acquire_writer(&self.pool, WriterOperation::EventWrite).await?;
        let mut tx = Transaction::begin(connection, None).await?;
        let task = lock_task(&mut tx, community, task_id).await?;
        let result = crate::event::insert_event_with_thread_metadata_tx(
            &mut tx, community, event, channel, thread,
        )
        .await?;
        if result.1 {
            if fleet::is_receipt(event) {
                project_receipt(&mut tx, community, &task, event).await?;
            } else {
                project_cml(&mut tx, community, &task, event).await?;
            }
        }
        tx.commit().await?;
        Ok(result)
    }

    /// Display/audit facts only. This response can never authorize a spawn.
    #[datastore_span(name = "list_task_attempts", system = "postgresql")]
    pub async fn list_task_attempts(
        &self,
        community: CommunityId,
        task: Uuid,
    ) -> Result<Vec<Value>> {
        let mut connection = acquire_writer(&self.pool, WriterOperation::EventWrite).await?;
        let rows = sqlx::query("SELECT * FROM fleet_attempts WHERE community_id=$1 AND task_id=$2 ORDER BY created_at,id")
            .bind(community.as_uuid()).bind(task).fetch_all(&mut *connection).await?;
        Ok(rows.iter().map(|r| json!({"id":r.get::<String,_>("id"),"task_id":task,"plan_event_id":hex::encode(r.get::<Vec<u8>,_>("plan_event_id")),"worker":hex::encode(r.get::<Vec<u8>,_>("worker_pubkey")),"machine_id":r.get::<String,_>("machine_id"),"state":r.get::<String,_>("state"),"start_event_id":r.get::<Option<Vec<u8>>,_>("start_event_id").map(hex::encode),"cancel_event_id":r.get::<Option<Vec<u8>>,_>("cancel_event_id").map(hex::encode),"receipt_event_id":r.get::<Option<Vec<u8>>,_>("receipt_event_id").map(hex::encode),"receipt":r.get::<Option<Value>,_>("receipt")})).collect())
    }

    /// Fresh primary-database check of a particular newly accepted start.
    /// This is separate from display reads. Callers must additionally possess
    /// their own accepted-new start response; replayed/lost responses fail shut.
    #[datastore_span(name = "fleet_start_admission", system = "postgresql")]
    pub async fn fleet_start_admission(
        &self,
        community: CommunityId,
        task_id: Uuid,
        attempt: &str,
        start: &[u8],
        worker: &[u8],
    ) -> Result<Value> {
        let connection = acquire_writer(&self.pool, WriterOperation::Authorization).await?;
        let mut tx = Transaction::begin(connection, None).await?;
        let task = lock_task(&mut tx, community, task_id).await?;
        let row = sqlx::query(
            "SELECT * FROM fleet_attempts WHERE community_id=$1 AND id=$2 AND task_id=$3 FOR SHARE",
        )
        .bind(community.as_uuid())
        .bind(attempt)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| deny("fleet admission not found"))?;
        let scope = row_scope(&row)?;
        if row.get::<String, _>("state") != "started"
            || row.get::<Option<Vec<u8>>, _>("cancel_event_id").is_some()
            || row.get::<Option<Vec<u8>>, _>("start_event_id").as_deref() != Some(start)
            || row.get::<Vec<u8>, _>("worker_pubkey").as_slice() != worker
        {
            return Err(deny("fleet execution admission unavailable"));
        }
        same_revision(&task, &scope)?;
        let planner: Vec<u8> = row.get("planner_pubkey");
        let channel: Uuid = row.get("channel_id");
        if task.get::<Option<Uuid>, _>("channel_id") != Some(channel) {
            return Err(deny("task channel changed"));
        }
        lock_authority(&mut tx, community, channel, &planner, worker, &scope).await?;
        let result = json!({"attempt_id":attempt,"task_id":task_id,"plan_event_id":hex::encode(row.get::<Vec<u8>,_>("plan_event_id")),"start_event_id":hex::encode(start),"worker":hex::encode(worker),"machine_id":scope.machine_id,"policy_digest":scope.policy_digest,"expires_at":scope.expires_at});
        tx.commit().await?;
        Ok(result)
    }
}

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod postgres_tests;
