//! Jev routing storage: decision audit rows, labels, the classify queue,
//! per-channel routing policy and owner-authored agent profiles (plan J1).
//!
//! Every function is community-scoped. Decisions are idempotent on
//! `(community, subject_kind, subject_id, question_set_version, model_version)`.

use crate::error::{DbError, Result};
use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use buzz_core::CommunityId;

/// Per-channel routing mode. A channel with no policy row is [`RoutingMode::Off`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingMode {
    /// Jev is not consulted.
    Off,
    /// Decisions are recorded but never shown.
    Shadow,
    /// The top route is offered to a human.
    Suggest,
    /// The top route is applied; requires a named owner.
    Auto,
}

impl RoutingMode {
    /// The stored `mode` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Shadow => "shadow",
            Self::Suggest => "suggest",
            Self::Auto => "auto",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "off" => Ok(Self::Off),
            "shadow" => Ok(Self::Shadow),
            "suggest" => Ok(Self::Suggest),
            "auto" => Ok(Self::Auto),
            other => Err(DbError::InvalidData(format!("routing mode {other:?}"))),
        }
    }
}

/// One Jev judgment to record (plan §6.4). Field names match the columns.
#[derive(Debug, Clone)]
pub struct NewDecision<'a> {
    /// `message`, `task` or `watch`.
    pub subject_kind: &'a str,
    /// Event id or task id of the subject.
    pub subject_id: &'a [u8],
    /// Channel the subject lives in, if any.
    pub channel_id: Option<Uuid>,
    /// Pinned question-set version.
    pub question_set_version: &'a str,
    /// Pinned Jev model version.
    pub model_version: &'a str,
    /// Mode in force: `shadow`, `suggest` or `auto`.
    pub mode: &'a str,
    /// `route`, `abstain` or `deferred`.
    pub outcome: &'a str,
    /// §2.5 reason; required iff `outcome != "route"`.
    pub reason_code: Option<&'a str>,
    /// Argmax option.
    pub top_option: Option<&'a str>,
    /// Mass of the argmax option.
    pub top_mass: Option<f32>,
    /// Mass of the runner-up.
    pub second_mass: Option<f32>,
    /// Injection-question mass.
    pub injection_score: Option<f32>,
    /// Full option → mass map.
    pub masses: serde_json::Value,
    /// Floor in force when decided.
    pub floor_at_decision: f32,
    /// Error class when the call failed closed.
    pub error_class: Option<&'a str>,
    /// Roster snapshot the options came from.
    pub roster_version: Option<&'a str>,
    /// Digest of the state sent to Jev.
    pub state_digest: &'a [u8],
    /// Hash of the full request body.
    pub request_hash: &'a [u8],
    /// Raw Jev response JSON.
    pub response: Option<serde_json::Value>,
    /// Call latency.
    pub latency_ms: Option<i32>,
    /// `.usage` input tokens.
    pub input_tokens: Option<i32>,
    /// `.usage` output tokens.
    pub output_tokens: Option<i32>,
}

/// Insert a decision. Returns `Ok(false)` (no error) when the idempotency key
/// already exists, so a re-delivered event is a no-op.
pub async fn insert_decision(
    pool: &PgPool,
    community_id: CommunityId,
    d: &NewDecision<'_>,
) -> Result<bool> {
    let result = sqlx::query(
        r#"
        INSERT INTO jev_decisions (community_id, subject_kind, subject_id, channel_id,
            question_set_version, model_version, mode, outcome, reason_code, top_option,
            top_mass, second_mass, injection_score, masses, floor_at_decision, error_class,
            roster_version, state_digest, request_hash, response, latency_ms, input_tokens,
            output_tokens)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
            $17, $18, $19, $20, $21, $22, $23)
        ON CONFLICT (community_id, subject_kind, subject_id, question_set_version, model_version)
        DO NOTHING
        "#,
    )
    .bind(community_id.as_uuid())
    .bind(d.subject_kind)
    .bind(d.subject_id)
    .bind(d.channel_id)
    .bind(d.question_set_version)
    .bind(d.model_version)
    .bind(d.mode)
    .bind(d.outcome)
    .bind(d.reason_code)
    .bind(d.top_option)
    .bind(d.top_mass)
    .bind(d.second_mass)
    .bind(d.injection_score)
    .bind(&d.masses)
    .bind(d.floor_at_decision)
    .bind(d.error_class)
    .bind(d.roster_version)
    .bind(d.state_digest)
    .bind(d.request_hash)
    .bind(&d.response)
    .bind(d.latency_ms)
    .bind(d.input_tokens)
    .bind(d.output_tokens)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// The channel's routing mode; `Off` when no policy row exists.
pub async fn routing_mode(
    pool: &PgPool,
    community_id: CommunityId,
    channel_id: Uuid,
) -> Result<RoutingMode> {
    let mode: Option<String> = sqlx::query_scalar(
        "SELECT mode FROM channel_routing_policy WHERE community_id = $1 AND channel_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(channel_id)
    .fetch_optional(pool)
    .await?;
    mode.map_or(Ok(RoutingMode::Off), |m| RoutingMode::parse(&m))
}

/// Set a channel's routing mode. `owner_pubkey` is required for `Auto`.
pub async fn set_routing_mode(
    pool: &PgPool,
    community_id: CommunityId,
    channel_id: Uuid,
    mode: RoutingMode,
    owner_pubkey: Option<&[u8]>,
    updated_by: &[u8],
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO channel_routing_policy (community_id, channel_id, mode, owner_pubkey, updated_by)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (community_id, channel_id) DO UPDATE
        SET mode = EXCLUDED.mode, owner_pubkey = EXCLUDED.owner_pubkey,
            updated_by = EXCLUDED.updated_by, updated_at = NOW()
        "#,
    )
    .bind(community_id.as_uuid())
    .bind(channel_id)
    .bind(mode.as_str())
    .bind(owner_pubkey)
    .bind(updated_by)
    .execute(pool)
    .await?;
    Ok(())
}

/// Create or replace an owner-authored routing profile for an agent.
pub async fn upsert_agent_profile(
    pool: &PgPool,
    community_id: CommunityId,
    agent_pubkey: &[u8],
    description: &str,
    enabled: bool,
    authored_by: &[u8],
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO agent_routing_profiles (community_id, agent_pubkey, description, enabled, authored_by)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (community_id, agent_pubkey) DO UPDATE
        SET description = EXCLUDED.description, enabled = EXCLUDED.enabled,
            authored_by = EXCLUDED.authored_by, updated_at = NOW()
        "#,
    )
    .bind(community_id.as_uuid())
    .bind(agent_pubkey)
    .bind(description)
    .bind(enabled)
    .bind(authored_by)
    .execute(pool)
    .await?;
    Ok(())
}

/// Queue rows are tried at most this many times (1 + 2 retries), then `error`.
pub const MAX_CLASSIFY_ATTEMPTS: i32 = 3;

/// Record `actor`'s label on a decision. Last write wins: the previous current
/// label is superseded (kept as history). Re-sending the current label is a
/// no-op. Returns the id of the current feedback row.
pub async fn record_feedback(
    pool: &PgPool,
    community_id: CommunityId,
    decision_id: Uuid,
    actor_pubkey: &[u8],
    labeller_kind: &str,
    verdict: &str,
    label: Option<&str>,
) -> Result<Uuid> {
    let mut tx = pool.begin().await?;
    // Serialises concurrent labels on one decision and proves it exists.
    sqlx::query("SELECT 1 FROM jev_decisions WHERE community_id = $1 AND id = $2 FOR UPDATE")
        .bind(community_id.as_uuid())
        .bind(decision_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("jev decision {decision_id}")))?;
    let current = sqlx::query(
        "SELECT id, labeller_kind, verdict, label FROM jev_decision_feedback
         WHERE community_id = $1 AND decision_id = $2 AND actor_pubkey = $3
           AND superseded_at IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(decision_id)
    .bind(actor_pubkey)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(row) = current {
        let same = row.get::<String, _>("labeller_kind") == labeller_kind
            && row.get::<String, _>("verdict") == verdict
            && row.get::<Option<String>, _>("label").as_deref() == label;
        if same {
            tx.commit().await?;
            return Ok(row.get("id"));
        }
        sqlx::query(
            "UPDATE jev_decision_feedback SET superseded_at = NOW()
             WHERE community_id = $1 AND id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(row.get::<Uuid, _>("id"))
        .execute(&mut *tx)
        .await?;
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO jev_decision_feedback
            (community_id, decision_id, actor_pubkey, labeller_kind, verdict, label)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(community_id.as_uuid())
    .bind(decision_id)
    .bind(actor_pubkey)
    .bind(labeller_kind)
    .bind(verdict)
    .bind(label)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

/// A claimed queue row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifyItem {
    /// `message`, `task` or `watch`.
    pub subject_kind: String,
    /// Event id or task id.
    pub subject_id: Vec<u8>,
    /// Channel of the subject, if any.
    pub channel_id: Option<Uuid>,
    /// Attempts including this claim.
    pub attempts: i32,
}

/// Enqueue a subject for classification. Returns `false` if already queued.
pub async fn enqueue_classify(
    pool: &PgPool,
    community_id: CommunityId,
    subject_kind: &str,
    subject_id: &[u8],
    channel_id: Option<Uuid>,
) -> Result<bool> {
    let result = sqlx::query(
        "INSERT INTO jev_classify_queue (community_id, subject_kind, subject_id, channel_id)
         VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(community_id.as_uuid())
    .bind(subject_kind)
    .bind(subject_id)
    .bind(channel_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Claim up to `limit` due rows under `claim_id` until `lease_until`. Pending
/// rows and rows whose lease expired are eligible; concurrent claimers get
/// disjoint rows (`FOR UPDATE SKIP LOCKED`).
pub async fn claim_classify_batch<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    community_id: CommunityId,
    claim_id: Uuid,
    lease_until: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<ClassifyItem>> {
    let rows = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT subject_kind, subject_id
            FROM jev_classify_queue
            WHERE community_id = $1
              AND community_write_allowed(community_id)
              AND attempts < $4
              AND next_attempt_at <= NOW()
              AND (state = 'pending' OR (state = 'claimed' AND lease_until < NOW()))
            ORDER BY next_attempt_at, created_at
            FOR UPDATE SKIP LOCKED
            LIMIT $5
        )
        UPDATE jev_classify_queue q
        SET state = 'claimed', claim_id = $2, lease_until = $3, attempts = q.attempts + 1
        FROM candidates c
        WHERE q.community_id = $1 AND q.subject_kind = c.subject_kind
          AND q.subject_id = c.subject_id
        RETURNING q.subject_kind, q.subject_id, q.channel_id, q.attempts
        "#,
    )
    .bind(community_id.as_uuid())
    .bind(claim_id)
    .bind(lease_until)
    .bind(MAX_CLASSIFY_ATTEMPTS)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ClassifyItem {
            subject_kind: row.get("subject_kind"),
            subject_id: row.get("subject_id"),
            channel_id: row.get("channel_id"),
            attempts: row.get("attempts"),
        })
        .collect())
}

/// Finish a claimed row. `error = None` marks it done; otherwise it is retried
/// with backoff, or left as `error` once attempts are exhausted. Returns
/// `false` if `claim_id` no longer holds the row.
pub async fn finish_classify(
    pool: &PgPool,
    community_id: CommunityId,
    subject_kind: &str,
    subject_id: &[u8],
    claim_id: Uuid,
    error: Option<&str>,
    backoff: Duration,
) -> Result<bool> {
    let result = sqlx::query(
        r#"
        UPDATE jev_classify_queue
        SET state = CASE WHEN $5::text IS NULL THEN 'done'
                         WHEN attempts >= $6 THEN 'error' ELSE 'pending' END,
            last_error = $5, claim_id = NULL, lease_until = NULL,
            next_attempt_at = NOW() + $7 * INTERVAL '1 millisecond'
        WHERE community_id = $1 AND subject_kind = $2 AND subject_id = $3
          AND claim_id = $4 AND state = 'claimed'
        "#,
    )
    .bind(community_id.as_uuid())
    .bind(subject_kind)
    .bind(subject_id)
    .bind(claim_id)
    .bind(error)
    .bind(MAX_CLASSIFY_ATTEMPTS)
    .bind(backoff.num_milliseconds() as f64)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod postgres_tests {
    use super::*;
    use std::collections::HashSet;

    async fn setup_pool() -> PgPool {
        PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("connect to test DB")
    }

    async fn make_community(pool: &PgPool) -> CommunityId {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(id)
            .bind(format!("jev-test-{}.example", id.simple()))
            .execute(pool)
            .await
            .expect("insert test community");
        CommunityId::from_uuid(id)
    }

    async fn make_channel(pool: &PgPool, community: CommunityId) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO channels (id, community_id, name, channel_type, visibility, created_by) \
             VALUES ($1, $2, $3, 'stream'::channel_type, 'open'::channel_visibility, $4)",
        )
        .bind(id)
        .bind(community.as_uuid())
        .bind(format!("jev-test-{}", id.simple()))
        .bind([0x11u8; 32].as_slice())
        .execute(pool)
        .await
        .expect("insert test channel");
        id
    }

    fn decision<'a>(subject: &'a [u8], model: &'a str, outcome: &'a str) -> NewDecision<'a> {
        NewDecision {
            subject_kind: "message",
            subject_id: subject,
            channel_id: None,
            question_set_version: "qs-1",
            model_version: model,
            mode: "shadow",
            outcome,
            reason_code: (outcome != "route").then_some("BelowFloor"),
            top_option: Some("agent:a"),
            top_mass: Some(0.4),
            second_mass: Some(0.3),
            injection_score: Some(0.01),
            masses: serde_json::json!({"agent:a": 0.4, "agent:b": 0.3}),
            floor_at_decision: 0.6,
            error_class: None,
            roster_version: Some("r1"),
            state_digest: &[1; 32],
            request_hash: &[2; 32],
            response: Some(serde_json::json!({"answers": {}})),
            latency_ms: Some(310),
            input_tokens: Some(516),
            output_tokens: Some(12),
        }
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn duplicate_decision_key_returns_false_without_error() {
        let pool = setup_pool().await;
        let community = make_community(&pool).await;
        let subject = Uuid::new_v4().as_bytes().to_vec();

        let first = decision(&subject, "jev-1.13.0", "abstain");
        assert!(insert_decision(&pool, community, &first)
            .await
            .expect("insert"));
        // Same key, different payload: a re-delivery must be a silent no-op.
        let dup = decision(&subject, "jev-1.13.0", "route");
        let dup = insert_decision(&pool, community, &dup).await;
        assert!(!dup.expect("duplicate must not error"));
        // A model bump is a new judgment, not a duplicate.
        let bumped = decision(&subject, "jev-1.14.0", "route");
        assert!(insert_decision(&pool, community, &bumped)
            .await
            .expect("bump"));

        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT model_version, outcome FROM jev_decisions
             WHERE community_id = $1 ORDER BY model_version",
        )
        .bind(community.as_uuid())
        .fetch_all(&pool)
        .await
        .expect("read back");
        let expected = [("jev-1.13.0", "abstain"), ("jev-1.14.0", "route")];
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(m, o)| (m.to_string(), o.to_string()))
            .collect();
        assert_eq!(rows, expected, "first write wins; no row was overwritten");
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn routing_policy_defaults_to_off_for_existing_channels() {
        let pool = setup_pool().await;
        let community = make_community(&pool).await;
        let channel = make_channel(&pool, community).await;

        let mode = routing_mode(&pool, community, channel).await.expect("mode");
        assert_eq!(mode, RoutingMode::Off, "no policy row means off");

        // A row written without a mode takes the column default.
        sqlx::query(
            "INSERT INTO channel_routing_policy (community_id, channel_id, updated_by)
             VALUES ($1, $2, $3)",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .bind([0x22u8; 32].as_slice())
        .execute(&pool)
        .await
        .expect("insert policy with defaults");
        let mode = routing_mode(&pool, community, channel).await.expect("mode");
        assert_eq!(mode, RoutingMode::Off, "column default is off");

        let admin = [0x33u8; 32];
        set_routing_mode(&pool, community, channel, RoutingMode::Shadow, None, &admin)
            .await
            .expect("opt in to shadow");
        let mode = routing_mode(&pool, community, channel).await.expect("mode");
        assert_eq!(mode, RoutingMode::Shadow);
        let auto_without_owner =
            set_routing_mode(&pool, community, channel, RoutingMode::Auto, None, &admin).await;
        let err = auto_without_owner.expect_err("auto needs a named owner");
        assert!(
            format!("{err:?}").contains("chk_channel_routing_policy_auto_owner"),
            "rejected by the auto-owner CHECK, got {err:?}"
        );
        assert_eq!(
            routing_mode(&pool, community, channel).await.expect("mode"),
            RoutingMode::Shadow,
            "a rejected write leaves the policy unchanged"
        );
        set_routing_mode(
            &pool,
            community,
            channel,
            RoutingMode::Auto,
            Some(&admin),
            &admin,
        )
        .await
        .expect("auto with an owner");
        let mode = routing_mode(&pool, community, channel).await.expect("mode");
        assert_eq!(mode, RoutingMode::Auto);
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn concurrent_claimers_get_disjoint_rows() {
        let pool = setup_pool().await;
        let community = make_community(&pool).await;
        let mut all = HashSet::new();
        for _ in 0..10 {
            let subject = Uuid::new_v4().as_bytes().to_vec();
            assert!(
                enqueue_classify(&pool, community, "message", &subject, None)
                    .await
                    .expect("enqueue")
            );
            all.insert(subject);
        }
        let lease = Utc::now() + Duration::minutes(5);
        let key = |items: &[ClassifyItem]| -> HashSet<Vec<u8>> {
            items.iter().map(|i| i.subject_id.clone()).collect()
        };

        // Claimer A holds its rows locked in an open transaction.
        let mut tx_a = pool.begin().await.expect("begin a");
        let a = claim_classify_batch(&mut *tx_a, community, Uuid::new_v4(), lease, 4)
            .await
            .expect("claim a");
        assert_eq!(a.len(), 4);
        // Claimer B runs while A is uncommitted: it must skip A's rows, not
        // wait for them and not take them.
        let b = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            claim_classify_batch(&pool, community, Uuid::new_v4(), lease, 10),
        )
        .await
        .expect("claimer b must not block on a's locks")
        .expect("claim b");
        tx_a.commit().await.expect("commit a");
        let (a, b) = (key(&a), key(&b));
        assert!(a.is_disjoint(&b), "claimers overlapped");
        assert_eq!(a.len() + b.len(), 10);
        assert_eq!(&a | &b, all);

        // Two pool claimers racing on a fresh set also never overlap.
        let mut fresh = HashSet::new();
        for _ in 0..20 {
            let subject = Uuid::new_v4().as_bytes().to_vec();
            enqueue_classify(&pool, community, "message", &subject, None)
                .await
                .expect("enqueue");
            fresh.insert(subject);
        }
        let (c, d) = tokio::join!(
            claim_classify_batch(&pool, community, Uuid::new_v4(), lease, 20),
            claim_classify_batch(&pool, community, Uuid::new_v4(), lease, 20),
        );
        let (c, d) = (key(&c.expect("claim c")), key(&d.expect("claim d")));
        assert!(c.is_disjoint(&d), "racing claimers overlapped");
        assert_eq!(&c | &d, fresh, "every row claimed exactly once");
        let rest = claim_classify_batch(&pool, community, Uuid::new_v4(), lease, 50)
            .await
            .expect("claim rest");
        assert!(rest.is_empty(), "leased rows must not be reclaimed");

        // Failure with zero backoff re-queues; success is terminal.
        let mut tx = pool.begin().await.expect("begin");
        let one = claim_classify_batch(&mut *tx, community, Uuid::new_v4(), lease, 1).await;
        tx.rollback().await.expect("rollback");
        assert!(one.expect("claim").is_empty(), "all rows are leased");
        let claim = Uuid::new_v4();
        let target = fresh.iter().next().expect("a fresh row").clone();
        let expired = sqlx::query(
            "UPDATE jev_classify_queue SET lease_until = NOW() - INTERVAL '1 second' \
             WHERE community_id = $1 AND subject_kind = 'message' AND subject_id = $2",
        )
        .bind(community.as_uuid())
        .bind(&target)
        .execute(&pool)
        .await
        .expect("expire one lease");
        assert_eq!(expired.rows_affected(), 1);
        let item = claim_classify_batch(&pool, community, claim, lease, 50)
            .await
            .expect("reclaim expired");
        assert_eq!(item.len(), 1, "only the expired lease is reclaimable");
        let item = &item[0];
        assert_eq!(item.subject_id, target);
        assert_eq!(item.attempts, 2);
        let (kind, id) = (item.subject_kind.as_str(), item.subject_id.as_slice());
        let stale = finish_classify(
            &pool,
            community,
            kind,
            id,
            Uuid::new_v4(),
            None,
            lease_zero(),
        );
        assert!(
            !stale.await.expect("stale finish"),
            "only the holder may finish"
        );
        let failed = finish_classify(
            &pool,
            community,
            kind,
            id,
            claim,
            Some("timeout"),
            lease_zero(),
        );
        assert!(failed.await.expect("finish err"));
        let again = claim_classify_batch(&pool, community, claim, lease, 50).await;
        let again = again.expect("claim retry");
        assert_eq!(again.len(), 1, "only the failed row is due again");
        assert_eq!(again[0].attempts, 3);

        // A failure on the last attempt is terminal: the row parks in 'error'.
        let last = finish_classify(&pool, community, kind, id, claim, Some("x"), lease_zero());
        assert!(last.await.expect("finish last"));
        let state_of = |subject: Vec<u8>| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT state FROM jev_classify_queue \
                     WHERE community_id = $1 AND subject_kind = 'message' AND subject_id = $2",
                )
                .bind(community.as_uuid())
                .bind(subject)
                .fetch_one(&pool)
                .await
                .expect("state")
            }
        };
        assert_eq!(state_of(id.to_vec()).await, "error");

        // Success is terminal: a done row is never claimed again.
        let late = b"late-subject".to_vec();
        assert!(enqueue_classify(&pool, community, kind, &late, None)
            .await
            .expect("enqueue late"));
        let ok_claim = Uuid::new_v4();
        let got = claim_classify_batch(&pool, community, ok_claim, lease, 50).await;
        assert_eq!(got.expect("claim late").len(), 1, "only the new row is due");
        let done = finish_classify(&pool, community, kind, &late, ok_claim, None, lease_zero());
        assert!(done.await.expect("finish ok"));
        assert_eq!(state_of(late).await, "done");
        let none = claim_classify_batch(&pool, community, Uuid::new_v4(), lease, 50).await;
        assert!(
            none.expect("claim none").is_empty(),
            "done and error rows stay put"
        );
    }

    fn lease_zero() -> Duration {
        Duration::zero()
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn feedback_is_last_write_wins_per_actor_with_history() {
        let pool = setup_pool().await;
        let community = make_community(&pool).await;
        let subject = Uuid::new_v4().as_bytes().to_vec();
        let d = decision(&subject, "jev-1.13.0", "route");
        assert!(insert_decision(&pool, community, &d).await.expect("insert"));
        let decision_id: Uuid =
            sqlx::query_scalar("SELECT id FROM jev_decisions WHERE community_id = $1")
                .bind(community.as_uuid())
                .fetch_one(&pool)
                .await
                .expect("decision id");
        const ALICE: [u8; 32] = [0xa1; 32];
        const BOT: [u8; 32] = [0xb2; 32];
        let alice = ALICE;
        let fb = |actor: &'static [u8], kind, verdict| {
            record_feedback(&pool, community, decision_id, actor, kind, verdict, None)
        };
        let first = fb(&ALICE, "human", "correct").await.expect("label");
        let resent = fb(&ALICE, "human", "correct").await.expect("resend");
        assert_eq!(first, resent, "re-sending the current label is a no-op");
        let changed = fb(&ALICE, "human", "wrong").await.expect("relabel");
        assert_ne!(first, changed);
        fb(&BOT, "agent", "correct").await.expect("agent label");

        let rows: Vec<(Vec<u8>, String, bool)> = sqlx::query_as(
            "SELECT actor_pubkey, verdict, superseded_at IS NULL FROM jev_decision_feedback
             WHERE community_id = $1 ORDER BY created_at, verdict",
        )
        .bind(community.as_uuid())
        .fetch_all(&pool)
        .await
        .expect("feedback rows");
        let current: Vec<_> = rows.iter().filter(|r| r.2).collect();
        assert_eq!(rows.len(), 3, "history kept, resend not duplicated");
        assert_eq!(current.len(), 2, "one current label per actor");
        assert!(current.contains(&&(alice.to_vec(), "wrong".to_string(), true)));
        let missing = record_feedback(
            &pool,
            community,
            Uuid::new_v4(),
            &alice,
            "human",
            "wrong",
            None,
        );
        assert!(matches!(missing.await, Err(DbError::NotFound(_))));
    }
}
