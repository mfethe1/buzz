//! Per-machine capability grants for agents.
//!
//! Default deny: [`is_granted`] returns `false` unless an active (non-revoked)
//! row exists. Revocation is a tombstone rather than a delete, so a revoked
//! grant stays visible to [`list_grants`] and to the audit trail.
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
/// Idempotent, and re-granting a previously revoked pair clears the tombstone
/// and records the new granter.
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
mod postgres_tests {
    use super::*;
    use nostr::Keys;
    use uuid::Uuid;

    async fn setup_pool() -> PgPool {
        PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("connect to test DB")
    }

    fn random_pubkey() -> Vec<u8> {
        Keys::generate().public_key().to_bytes().to_vec()
    }

    async fn make_community(pool: &PgPool) -> CommunityId {
        let id = Uuid::new_v4();
        let host = format!("cap-test-{}.example", id.simple());
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(id)
            .bind(host)
            .execute(pool)
            .await
            .expect("insert test community");
        CommunityId::from_uuid(id)
    }

    /// The PR's core assertion: with no grants at all, every check denies.
    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn test_default_deny_with_no_grants() {
        let pool = setup_pool().await;
        let community = make_community(&pool).await;
        let agent = random_pubkey();

        let allowed = is_granted(&pool, community, &agent, CAP_CROSS_SSH, "winnie-desktop")
            .await
            .expect("check");
        assert!(!allowed, "no grant must deny");
    }

    /// grant -> allow, revoke -> deny again.
    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn test_grant_then_revoke_round_trip() {
        let pool = setup_pool().await;
        let community = make_community(&pool).await;
        let agent = random_pubkey();
        let admin = random_pubkey();

        grant(
            &pool,
            community,
            &agent,
            CAP_CROSS_SSH,
            "winnie-desktop",
            &admin,
        )
        .await
        .expect("grant");
        assert!(
            is_granted(&pool, community, &agent, CAP_CROSS_SSH, "winnie-desktop")
                .await
                .expect("check"),
            "grant must allow"
        );

        revoke(
            &pool,
            community,
            &agent,
            CAP_CROSS_SSH,
            "winnie-desktop",
            &admin,
        )
        .await
        .expect("revoke");
        assert!(
            !is_granted(&pool, community, &agent, CAP_CROSS_SSH, "winnie-desktop")
                .await
                .expect("check"),
            "revoke must deny again"
        );
    }

    /// A grant authorizes only the machine it names.
    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn test_grant_does_not_leak_to_other_targets() {
        let pool = setup_pool().await;
        let community = make_community(&pool).await;
        let agent = random_pubkey();
        let admin = random_pubkey();

        grant(
            &pool,
            community,
            &agent,
            CAP_CROSS_SSH,
            "winnie-desktop",
            &admin,
        )
        .await
        .expect("grant");

        assert!(
            !is_granted(&pool, community, &agent, CAP_CROSS_SSH, "rosie-pi")
                .await
                .expect("check"),
            "grant on one machine must not authorize another"
        );
    }

    /// A grant in one community never authorizes anything in another.
    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn test_grant_is_community_scoped() {
        let pool = setup_pool().await;
        let community_a = make_community(&pool).await;
        let community_b = make_community(&pool).await;
        let agent = random_pubkey();
        let admin = random_pubkey();

        grant(
            &pool,
            community_a,
            &agent,
            CAP_CROSS_SSH,
            "winnie-desktop",
            &admin,
        )
        .await
        .expect("grant");

        assert!(
            !is_granted(&pool, community_b, &agent, CAP_CROSS_SSH, "winnie-desktop")
                .await
                .expect("check"),
            "a grant must not cross community boundaries"
        );
    }

    /// The wildcard target authorizes any machine in that community.
    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn test_wildcard_target_authorizes_any_machine() {
        let pool = setup_pool().await;
        let community = make_community(&pool).await;
        let agent = random_pubkey();
        let admin = random_pubkey();

        grant(&pool, community, &agent, CAP_CROSS_SSH, TARGET_ANY, &admin)
            .await
            .expect("grant");

        assert!(
            is_granted(
                &pool,
                community,
                &agent,
                CAP_CROSS_SSH,
                "any-machine-at-all"
            )
            .await
            .expect("check"),
            "wildcard must authorize an unnamed machine"
        );
    }

    /// A different capability is not authorized by a cross_ssh grant.
    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn test_grant_does_not_leak_across_capabilities() {
        let pool = setup_pool().await;
        let community = make_community(&pool).await;
        let agent = random_pubkey();
        let admin = random_pubkey();

        grant(
            &pool,
            community,
            &agent,
            CAP_CROSS_SSH,
            "winnie-desktop",
            &admin,
        )
        .await
        .expect("grant");

        assert!(
            !is_granted(&pool, community, &agent, "read_secrets", "winnie-desktop")
                .await
                .expect("check"),
            "cross_ssh must not authorize a different capability"
        );
    }

    /// Re-granting a revoked pair clears the tombstone rather than duplicating.
    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn test_regrant_clears_tombstone() {
        let pool = setup_pool().await;
        let community = make_community(&pool).await;
        let agent = random_pubkey();
        let admin = random_pubkey();

        grant(
            &pool,
            community,
            &agent,
            CAP_CROSS_SSH,
            "winnie-desktop",
            &admin,
        )
        .await
        .expect("grant");
        revoke(
            &pool,
            community,
            &agent,
            CAP_CROSS_SSH,
            "winnie-desktop",
            &admin,
        )
        .await
        .expect("revoke");
        grant(
            &pool,
            community,
            &agent,
            CAP_CROSS_SSH,
            "winnie-desktop",
            &admin,
        )
        .await
        .expect("re-grant");

        assert!(
            is_granted(&pool, community, &agent, CAP_CROSS_SSH, "winnie-desktop")
                .await
                .expect("check"),
            "re-grant must allow again"
        );
        let grants = list_grants(&pool, community).await.expect("list");
        assert_eq!(
            grants.len(),
            1,
            "re-grant must reuse the row, not duplicate"
        );
        assert!(!grants[0].revoked, "tombstone must be cleared");
    }

    /// Revoked grants stay listed, so the audit trail survives revocation.
    #[tokio::test]
    #[ignore = "requires PostgreSQL"]
    async fn test_revoked_grants_remain_listed() {
        let pool = setup_pool().await;
        let community = make_community(&pool).await;
        let agent = random_pubkey();
        let admin = random_pubkey();

        grant(
            &pool,
            community,
            &agent,
            CAP_CROSS_SSH,
            "winnie-desktop",
            &admin,
        )
        .await
        .expect("grant");
        revoke(
            &pool,
            community,
            &agent,
            CAP_CROSS_SSH,
            "winnie-desktop",
            &admin,
        )
        .await
        .expect("revoke");

        let grants = list_grants(&pool, community).await.expect("list");
        assert_eq!(grants.len(), 1, "revoked grant must remain visible");
        assert!(grants[0].revoked, "it must be marked revoked");
    }
}
