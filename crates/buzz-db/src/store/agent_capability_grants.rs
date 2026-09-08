//! Per-machine capability grants for agents.
//!
//! Default deny: [`is_granted`] returns `false` unless an active (non-revoked)
//! row exists. Revocation is a tombstone rather than a delete, so a revoked
//! grant stays visible to [`list_grants`]. The database appends each changed
//! grant/revoke image to `agent_capability_events` atomically, retaining history
//! even after a re-grant replaces the current tombstone.
//!
//! Every function is community-scoped; a grant in one community never
//! authorizes anything in another.

use crate::error::Result;
use crate::Db;
use buzz_core::CommunityId;
use buzz_datastore_tracing::datastore_span;
use sqlx::{PgPool, Row};

/// The `cross_ssh` capability: execute a command on another machine.
pub const CAP_CROSS_SSH: &str = "cross_ssh";

/// Wildcard target: any machine in the community.
pub const TARGET_ANY: &str = "*";

/// A capability grant row, including revoked ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGrant {
    /// The agent the grant applies to.
    pub agent_pubkey: Vec<u8>,
    /// Capability name, e.g. [`CAP_CROSS_SSH`].
    pub capability: String,
    /// Target machine id, or [`TARGET_ANY`].
    pub target: String,
    /// Who granted it.
    pub granted_by: Vec<u8>,
    /// True when the grant has been revoked (tombstoned).
    pub revoked: bool,
}

/// Grant `capability` on `target` to `agent_pubkey`.
///
/// Idempotent in effective permission. Each changed grant records a new history
/// fact; re-granting clears the current tombstone without erasing its history.
pub async fn grant(
    pool: &PgPool,
    community_id: CommunityId,
    agent_pubkey: &[u8],
    capability: &str,
    target: &str,
    granted_by: &[u8],
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO agent_capability_grants
               (community_id, agent_pubkey, capability, target, granted_by)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (community_id, agent_pubkey, capability, target)
           DO UPDATE SET revoked_at = NULL,
                         revoked_by = NULL,
                         granted_by = EXCLUDED.granted_by,
                         granted_at = NOW()"#,
    )
    .bind(community_id.as_uuid())
    .bind(agent_pubkey)
    .bind(capability)
    .bind(target)
    .bind(granted_by)
    .execute(pool)
    .await?;
    Ok(())
}

/// Revoke a grant by tombstoning it. Revoking a nonexistent grant is a no-op
/// (the effective state — denied — is already correct).
pub async fn revoke(
    pool: &PgPool,
    community_id: CommunityId,
    agent_pubkey: &[u8],
    capability: &str,
    target: &str,
    revoked_by: &[u8],
) -> Result<bool> {
    let result = sqlx::query(
        r#"UPDATE agent_capability_grants
              SET revoked_at = NOW(), revoked_by = $5
            WHERE community_id = $1 AND agent_pubkey = $2
              AND capability = $3 AND target = $4
              AND revoked_at IS NULL"#,
    )
    .bind(community_id.as_uuid())
    .bind(agent_pubkey)
    .bind(capability)
    .bind(target)
    .bind(revoked_by)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// The authorization check. **Default deny.**
///
/// Returns `true` only when an active grant exists for the exact target or for
/// [`TARGET_ANY`]. No row, or a revoked row, means `false`.
pub async fn is_granted(
    pool: &PgPool,
    community_id: CommunityId,
    agent_pubkey: &[u8],
    capability: &str,
    target: &str,
) -> Result<bool> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
               SELECT 1 FROM agent_capability_grants
                WHERE community_id = $1 AND agent_pubkey = $2
                  AND capability = $3 AND (target = $4 OR target = '*')
                  AND revoked_at IS NULL
           ) AS granted"#,
    )
    .bind(community_id.as_uuid())
    .bind(agent_pubkey)
    .bind(capability)
    .bind(target)
    .fetch_one(pool)
    .await?;
    Ok(row.get::<bool, _>("granted"))
}

/// List every grant for a community, including revoked tombstones.
pub async fn list_grants(pool: &PgPool, community_id: CommunityId) -> Result<Vec<CapabilityGrant>> {
    let rows = sqlx::query(
        r#"SELECT agent_pubkey, capability, target, granted_by,
                  (revoked_at IS NOT NULL) AS revoked
             FROM agent_capability_grants
            WHERE community_id = $1
            ORDER BY granted_at DESC"#,
    )
    .bind(community_id.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CapabilityGrant {
            agent_pubkey: r.get("agent_pubkey"),
            capability: r.get("capability"),
            target: r.get("target"),
            granted_by: r.get("granted_by"),
            revoked: r.get("revoked"),
        })
        .collect())
}

impl Db {
    /// Returns `true` only if an active grant exists. Default deny.
    #[datastore_span(name = "capability_is_granted", system = "postgresql")]
    pub async fn capability_is_granted(
        &self,
        community_id: CommunityId,
        agent_pubkey: &[u8],
        capability: &str,
        target: &str,
    ) -> Result<bool> {
        is_granted(&self.pool, community_id, agent_pubkey, capability, target).await
    }

    /// Grant `capability` on `target` to `agent_pubkey`. Idempotent: re-granting
    /// a revoked grant reactivates it.
    #[datastore_span(name = "capability_grant", system = "postgresql")]
    pub async fn capability_grant(
        &self,
        community_id: CommunityId,
        agent_pubkey: &[u8],
        capability: &str,
        target: &str,
        granted_by: &[u8],
    ) -> Result<()> {
        grant(
            &self.pool,
            community_id,
            agent_pubkey,
            capability,
            target,
            granted_by,
        )
        .await
    }

    /// Revoke a grant (tombstone). Returns `true` if an active grant was revoked.
    #[datastore_span(name = "capability_revoke", system = "postgresql")]
    pub async fn capability_revoke(
        &self,
        community_id: CommunityId,
        agent_pubkey: &[u8],
        capability: &str,
        target: &str,
        revoked_by: &[u8],
    ) -> Result<bool> {
        revoke(
            &self.pool,
            community_id,
            agent_pubkey,
            capability,
            target,
            revoked_by,
        )
        .await
    }

    /// List all grants (including revoked tombstones) for an agent.
    #[datastore_span(name = "capability_list_grants", system = "postgresql")]
    pub async fn capability_list_grants(
        &self,
        community_id: CommunityId,
    ) -> Result<Vec<CapabilityGrant>> {
        list_grants(&self.pool, community_id).await
    }
}

#[cfg(test)]
mod postgres_tests;
