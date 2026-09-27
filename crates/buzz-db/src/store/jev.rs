//! Jev routing storage: decision audit rows, labels, the classify queue,
//! per-channel routing policy and owner-authored agent profiles (plan J1).
//!
//! Every function is community-scoped. Decisions are idempotent on
//! `(community, subject_kind, subject_id, question_set_version, model_version)`.

use crate::error::{DbError, Result};
use sqlx::PgPool;
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

#[cfg(test)]
mod postgres_tests {
    use super::*;

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
}
