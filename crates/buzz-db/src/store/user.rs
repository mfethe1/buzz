//! User CRUD operations.

use crate::error::Result;
use crate::Db;
use buzz_core::CommunityId;
use buzz_datastore_tracing::datastore_span;
use sqlx::PgPool;
use sqlx::Row;

/// A user's profile fields.
#[derive(Debug, Clone)]
pub struct UserProfile {
    /// Raw 32-byte compressed public key.
    pub pubkey: Vec<u8>,
    /// Human-readable display name chosen by the user.
    pub display_name: Option<String>,
    /// URL of the user's avatar image.
    pub avatar_url: Option<String>,
    /// Short bio or description provided by the user.
    pub about: Option<String>,
    /// NIP-05 identifier (user@domain).
    pub nip05_handle: Option<String>,
}

/// Lightweight user record returned from search.
#[derive(Debug, Clone)]
pub struct UserSearchProfile {
    /// Raw 32-byte compressed public key.
    pub pubkey: Vec<u8>,
    /// Human-readable display name chosen by the user.
    pub display_name: Option<String>,
    /// URL of the user's avatar image.
    pub avatar_url: Option<String>,
    /// NIP-05 identifier (user@domain).
    pub nip05_handle: Option<String>,
}

/// Ensure a user record exists for the given pubkey (upsert).
/// Creates with minimal fields if not present; no-op if already exists.
///
/// Returns `true` if a new row was inserted, `false` if the user already existed.
/// The `true` case is the reliable signal for "user was just registered" — used
/// by callers to increment `buzz_users_created_total`.
pub async fn ensure_user(pool: &PgPool, community_id: CommunityId, pubkey: &[u8]) -> Result<bool> {
    ensure_user_with_operation(
        pool,
        community_id,
        pubkey,
        crate::observability::WriterOperation::EventWrite,
    )
    .await
}

async fn ensure_user_with_operation(
    pool: &PgPool,
    community_id: CommunityId,
    pubkey: &[u8],
    operation: crate::observability::WriterOperation,
) -> Result<bool> {
    let mut connection = crate::observability::acquire_writer(pool, operation).await?;
    let result = sqlx::query(
        r#"
        INSERT INTO users (community_id, pubkey)
        VALUES ($1, $2)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(community_id.as_uuid())
    .bind(pubkey)
    .execute(&mut *connection)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Get a single user record by pubkey.
pub async fn get_user(
    pool: &PgPool,
    community_id: CommunityId,
    pubkey: &[u8],
) -> Result<Option<UserProfile>> {
    let row = sqlx::query_as::<
        _,
        (
            Vec<u8>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(
        r#"
        SELECT pubkey, display_name, avatar_url, about, nip05_handle
        FROM users
        WHERE community_id = $1 AND pubkey = $2
        "#,
    )
    .bind(community_id.as_uuid())
    .bind(pubkey)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(pubkey, display_name, avatar_url, about, nip05_handle)| UserProfile {
            pubkey,
            display_name,
            avatar_url,
            about,
            nip05_handle,
        },
    ))
}

/// Update a user's profile fields (display_name, avatar_url, about, nip05_handle).
/// Only updates fields that are Some -- None fields are left unchanged.
/// At least one field must be Some, otherwise returns Ok(()) without touching the DB.
///
/// Empty strings are treated as "clear to NULL" -- this is important for kind:0
/// absolute-state semantics where absent fields must be cleared, and for the
/// `nip05_handle` column which has a UNIQUE constraint (multiple NULLs are allowed,
/// but multiple empty strings would violate uniqueness).
pub async fn update_user_profile(
    pool: &PgPool,
    community_id: CommunityId,
    pubkey: &[u8],
    display_name: Option<&str>,
    avatar_url: Option<&str>,
    about: Option<&str>,
    nip05_handle: Option<&str>,
) -> Result<()> {
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx = 1u32;

    if display_name.is_some() {
        set_parts.push(format!("display_name = ${param_idx}"));
        param_idx += 1;
    }
    if avatar_url.is_some() {
        set_parts.push(format!("avatar_url = ${param_idx}"));
        param_idx += 1;
    }
    if about.is_some() {
        set_parts.push(format!("about = ${param_idx}"));
        param_idx += 1;
    }
    if nip05_handle.is_some() {
        set_parts.push(format!("nip05_handle = ${param_idx}"));
        param_idx += 1;
    }

    if set_parts.is_empty() {
        return Ok(());
    }

    // Helper: convert empty string to None (NULL in DB). This ensures UNIQUE
    // columns like nip05_handle don't collide on empty strings, and keeps
    // semantics clean: absent profile data is NULL, not "".
    fn empty_to_none(val: Option<&str>) -> Option<&str> {
        val.filter(|s| !s.is_empty())
    }

    let sql = format!(
        "UPDATE users SET {} WHERE community_id = ${param_idx} AND pubkey = ${}",
        set_parts.join(", "),
        param_idx + 1
    );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    if display_name.is_some() {
        query = query.bind(empty_to_none(display_name));
    }
    if avatar_url.is_some() {
        query = query.bind(empty_to_none(avatar_url));
    }
    if about.is_some() {
        query = query.bind(empty_to_none(about));
    }
    if nip05_handle.is_some() {
        query = query.bind(empty_to_none(nip05_handle));
    }
    query = query.bind(community_id.as_uuid());
    query = query.bind(pubkey);
    query.execute(pool).await?;
    Ok(())
}

/// Look up a user by their full NIP-05 handle (exact match, case-insensitive).
/// Both `local_part` and `domain` must already be lowercased by the caller.
pub async fn get_user_by_nip05(
    pool: &PgPool,
    community_id: CommunityId,
    local_part: &str,
    domain: &str,
) -> Result<Option<UserProfile>> {
    let handle = format!("{}@{}", local_part, domain);
    let row = sqlx::query_as::<
        _,
        (
            Vec<u8>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(
        r#"
        SELECT pubkey, display_name, avatar_url, about, nip05_handle
        FROM users
        WHERE community_id = $1 AND LOWER(nip05_handle) = LOWER($2)
        LIMIT 1
        "#,
    )
    .bind(community_id.as_uuid())
    .bind(&handle)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(pubkey, display_name, avatar_url, about, nip05_handle)| UserProfile {
            pubkey,
            display_name,
            avatar_url,
            about,
            nip05_handle,
        },
    ))
}

/// Escape SQL LIKE metacharacters (`%`, `_`, `\`) so user input is treated
/// as literal text.  Used with `ESCAPE '\'` in the query.
///
/// Without this, a search query of `"%"` would match every row (full table
/// scan) and `"_"` would act as a single-character wildcard.
fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Search users by display name, NIP-05 handle, or pubkey prefix.
///
/// Empty queries return an empty vec and do not hit the database.
pub async fn search_users(
    pool: &PgPool,
    community_id: CommunityId,
    query: &str,
    limit: u32,
) -> Result<Vec<UserSearchProfile>> {
    let normalized = query.trim().to_lowercase();
    if normalized.is_empty() {
        return Ok(Vec::new());
    }

    let escaped = escape_like(&normalized);
    let contains_pattern = format!("%{escaped}%");
    let prefix_pattern = format!("{escaped}%");
    let limit = limit.clamp(1, 500) as i64;

    let rows = sqlx::query_as::<_, (Vec<u8>, Option<String>, Option<String>, Option<String>)>(
        r#"
        SELECT pubkey, display_name, avatar_url, nip05_handle
        FROM users
        WHERE community_id = $1
          AND (LOWER(COALESCE(display_name, '')) LIKE $2 ESCAPE '\'
           OR LOWER(COALESCE(nip05_handle, '')) LIKE $2 ESCAPE '\'
           OR LOWER(encode(pubkey, 'hex')) LIKE $2 ESCAPE '\')
        ORDER BY
            CASE
                WHEN LOWER(COALESCE(display_name, '')) = $3 THEN 0
                WHEN LOWER(COALESCE(nip05_handle, '')) = $3 THEN 1
                WHEN LOWER(encode(pubkey, 'hex')) = $3 THEN 2
                WHEN LOWER(COALESCE(display_name, '')) LIKE $4 ESCAPE '\' THEN 3
                WHEN LOWER(COALESCE(nip05_handle, '')) LIKE $4 ESCAPE '\' THEN 4
                WHEN LOWER(encode(pubkey, 'hex')) LIKE $4 ESCAPE '\' THEN 5
                ELSE 6
            END,
            COALESCE(NULLIF(display_name, ''), NULLIF(nip05_handle, ''), LOWER(encode(pubkey, 'hex')))
        LIMIT $5
        "#,
    )
    .bind(community_id.as_uuid())
    .bind(&contains_pattern)
    .bind(&normalized)
    .bind(&prefix_pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(pubkey, display_name, avatar_url, nip05_handle)| UserSearchProfile {
                pubkey,
                display_name,
                avatar_url,
                nip05_handle,
            },
        )
        .collect())
}

/// Set the owner pubkey for an agent user.
/// The owner pubkey must already exist in the users table (FK constraint).
/// Returns an error if the agent pubkey is not found (rows_affected == 0).
/// Atomically set agent owner — only if no owner is currently assigned.
///
/// Returns Ok(true) if ownership was set, Ok(false) if an owner already exists
/// (caller should check whether the existing owner matches). Returns Err if the
/// agent pubkey doesn't exist in the users table.
pub async fn set_agent_owner(
    pool: &PgPool,
    community_id: CommunityId,
    agent_pubkey: &[u8],
    owner_pubkey: &[u8],
) -> Result<bool> {
    set_agent_owner_with_operation(
        pool,
        community_id,
        agent_pubkey,
        owner_pubkey,
        crate::observability::WriterOperation::EventWrite,
    )
    .await
}

async fn set_agent_owner_with_operation(
    pool: &PgPool,
    community_id: CommunityId,
    agent_pubkey: &[u8],
    owner_pubkey: &[u8],
    operation: crate::observability::WriterOperation,
) -> Result<bool> {
    let mut connection = crate::observability::acquire_writer(pool, operation).await?;
    // Conditional UPDATE: only set owner if currently NULL. This makes
    // "first mint wins" atomic — no TOCTOU race between concurrent mints.
    let result = sqlx::query(
        r#"UPDATE users SET agent_owner_pubkey = $1 WHERE community_id = $2 AND pubkey = $3 AND agent_owner_pubkey IS NULL"#,
    )
    .bind(owner_pubkey)
    .bind(community_id.as_uuid())
    .bind(agent_pubkey)
    .execute(&mut *connection)
    .await?;

    if result.rows_affected() == 0 {
        // Could be: (a) pubkey not found, or (b) owner already set.
        // Check which case by querying the row.
        let exists = sqlx::query(r#"SELECT 1 FROM users WHERE community_id = $1 AND pubkey = $2"#)
            .bind(community_id.as_uuid())
            .bind(agent_pubkey)
            .fetch_optional(&mut *connection)
            .await?;
        if exists.is_none() {
            return Err(crate::error::DbError::NotFound(
                "agent pubkey not found in users table".into(),
            ));
        }
        // Row exists but owner already set — return false (not an error).
        return Ok(false);
    }
    Ok(true)
}

/// The machine an agent calls home: stable host id, human label, and runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineHome {
    /// Stable host identity (the desktop's device id).
    pub machine_id: String,
    /// Human-facing, renameable label.
    pub machine_label: Option<String>,
    /// Runtime serving this home (`"hermes"`, `"openclaw"`, `"claude-code"`, ...).
    pub machine_runtime: Option<String>,
}

/// Register (or re-register) `agent_pubkey` as the home agent for a machine.
///
/// One home per machine per community is a database invariant
/// (`idx_users_one_home_per_machine`), not a check performed here: a
/// read-then-write would race two concurrent registrations onto the same host.
/// A conflicting claim therefore surfaces as a unique violation, which is
/// translated to [`DbError::AccessDenied`] so callers get an actionable
/// message instead of a raw SQLSTATE.
///
/// Returns `Err(DbError::NotFound)` if the agent pubkey has no `users` row.
pub async fn set_machine_home(
    pool: &PgPool,
    community_id: CommunityId,
    agent_pubkey: &[u8],
    home: &MachineHome,
) -> Result<()> {
    let mut connection = crate::observability::acquire_writer(
        pool,
        crate::observability::WriterOperation::EventWrite,
    )
    .await?;
    let result = sqlx::query(
        r#"UPDATE users SET machine_id = $1, machine_label = $2, machine_runtime = $3, updated_at = NOW() WHERE community_id = $4 AND pubkey = $5"#,
    )
    .bind(&home.machine_id)
    .bind(home.machine_label.as_deref())
    .bind(home.machine_runtime.as_deref())
    .bind(community_id.as_uuid())
    .bind(agent_pubkey)
    .execute(&mut *connection)
    .await;

    match result {
        Ok(done) if done.rows_affected() == 0 => Err(crate::error::DbError::NotFound(
            "agent pubkey not found in users table".into(),
        )),
        Ok(_) => Ok(()),
        // 23505 = unique_violation: another agent already homes this machine.
        Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("23505") => {
            Err(crate::error::DbError::AccessDenied(format!(
                "machine {} already has a home agent in this community",
                home.machine_id
            )))
        }
        Err(e) => Err(e.into()),
    }
}

/// Clear an agent's machine home, freeing the machine for another agent.
///
/// Returns `true` if a home was cleared, `false` if the row exists but had no
/// home. All three columns drop together: the migration's
/// `chk_users_machine_fields_require_machine_id` makes a label or runtime
/// without a `machine_id` unrepresentable.
pub async fn clear_machine_home(
    pool: &PgPool,
    community_id: CommunityId,
    agent_pubkey: &[u8],
) -> Result<bool> {
    let mut connection = crate::observability::acquire_writer(
        pool,
        crate::observability::WriterOperation::EventWrite,
    )
    .await?;
    let result = sqlx::query(
        r#"UPDATE users SET machine_id = NULL, machine_label = NULL, machine_runtime = NULL, updated_at = NOW() WHERE community_id = $1 AND pubkey = $2 AND machine_id IS NOT NULL"#,
    )
    .bind(community_id.as_uuid())
    .bind(agent_pubkey)
    .execute(&mut *connection)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Look up an agent's machine home. `None` when the user is absent or unhomed.
pub async fn get_machine_home(
    pool: &PgPool,
    community_id: CommunityId,
    agent_pubkey: &[u8],
) -> Result<Option<MachineHome>> {
    let row = sqlx::query(
        r#"SELECT machine_id, machine_label, machine_runtime FROM users WHERE community_id = $1 AND pubkey = $2 AND machine_id IS NOT NULL"#,
    )
    .bind(community_id.as_uuid())
    .bind(agent_pubkey)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| MachineHome {
        machine_id: r.get("machine_id"),
        machine_label: r.get("machine_label"),
        machine_runtime: r.get("machine_runtime"),
    }))
}

/// Resolve the agent that homes `machine_id`, if any.
///
/// This is the lookup that makes a machine home addressable: given a host, find
/// the pubkey that answers for it.
pub async fn get_agent_for_machine(
    pool: &PgPool,
    community_id: CommunityId,
    machine_id: &str,
) -> Result<Option<Vec<u8>>> {
    let row =
        sqlx::query(r#"SELECT pubkey FROM users WHERE community_id = $1 AND machine_id = $2"#)
            .bind(community_id.as_uuid())
            .bind(machine_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.get::<Vec<u8>, _>("pubkey")))
}

/// Get the channel_add_policy and agent_owner_pubkey for a user.
/// Returns None if the pubkey is not in the users table.
/// Returns Some((policy_str, owner_bytes_or_none)) if found.
pub async fn get_agent_channel_policy(
    pool: &PgPool,
    community_id: CommunityId,
    pubkey: &[u8],
) -> Result<Option<(String, Option<Vec<u8>>)>> {
    let mut connection = crate::observability::acquire_writer(
        pool,
        crate::observability::WriterOperation::Authorization,
    )
    .await?;
    let row = sqlx::query(
        r#"SELECT channel_add_policy::text AS channel_add_policy, agent_owner_pubkey FROM users WHERE community_id = $1 AND pubkey = $2"#,
    )
    .bind(community_id.as_uuid())
    .bind(pubkey)
    .fetch_optional(&mut *connection)
    .await?;

    row.map(|r| -> Result<(String, Option<Vec<u8>>)> {
        let policy: String = r.try_get("channel_add_policy")?;
        let owner: Option<Vec<u8>> = r.try_get("agent_owner_pubkey").unwrap_or(None);
        Ok((policy, owner))
    })
    .transpose()
}

/// Check whether `actor_pubkey` is the `agent_owner_pubkey` of `target_pubkey`.
/// Queries `agent_owner_pubkey` directly rather than going through
/// `get_agent_channel_policy`, which would fetch unrelated fields.
pub async fn is_agent_owner(
    pool: &PgPool,
    community_id: CommunityId,
    target_pubkey: &[u8],
    actor_pubkey: &[u8],
) -> Result<bool> {
    let mut connection = crate::observability::acquire_writer(
        pool,
        crate::observability::WriterOperation::Authorization,
    )
    .await?;
    let row = sqlx::query_scalar::<_, bool>(
        "SELECT agent_owner_pubkey = $3 FROM users WHERE community_id = $1 AND pubkey = $2 AND agent_owner_pubkey IS NOT NULL",
    )
    .bind(community_id.as_uuid())
    .bind(target_pubkey)
    .bind(actor_pubkey)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(row.unwrap_or(false))
}

/// Set the channel_add_policy for a user.
/// Returns an error if the pubkey is not found (rows_affected == 0).
/// Returns an error if `policy` is not one of the valid ENUM values.
pub async fn set_channel_add_policy(
    pool: &PgPool,
    community_id: CommunityId,
    pubkey: &[u8],
    policy: &str,
) -> Result<()> {
    if !matches!(policy, "anyone" | "owner_only" | "nobody") {
        return Err(crate::error::DbError::InvalidData(format!(
            "invalid channel_add_policy: {policy}"
        )));
    }
    let result = sqlx::query(
        r#"UPDATE users SET channel_add_policy = $1::channel_add_policy WHERE community_id = $2 AND pubkey = $3"#,
    )
    .bind(policy)
    .bind(community_id.as_uuid())
    .bind(pubkey)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(crate::error::DbError::NotFound(
            "pubkey not found in users table".into(),
        ));
    }
    Ok(())
}

impl Db {
    /// Ensure a user record exists (upsert).
    ///
    /// Returns `true` if a new row was inserted (first time), `false` if it
    /// already existed. Callers use the `true` return to increment
    /// `buzz_users_created_total`.
    #[datastore_span(name = "ensure_user", system = "postgresql")]
    pub async fn ensure_user(&self, community_id: CommunityId, pubkey: &[u8]) -> Result<bool> {
        crate::user::ensure_user(&self.pool, community_id, pubkey).await
    }

    /// Ensure a principal while materializing an authenticated NIP-OA
    /// authorization relationship.
    #[datastore_span(name = "ensure_user_for_authorization", system = "postgresql")]
    pub async fn ensure_user_for_authorization(
        &self,
        community_id: CommunityId,
        pubkey: &[u8],
    ) -> Result<bool> {
        ensure_user_with_operation(
            &self.pool,
            community_id,
            pubkey,
            crate::observability::WriterOperation::Authorization,
        )
        .await
    }

    /// Get a single user record by pubkey.
    #[datastore_span(name = "get_user", system = "postgresql")]
    pub async fn get_user(
        &self,
        community_id: CommunityId,
        pubkey: &[u8],
    ) -> Result<Option<UserProfile>> {
        crate::user::get_user(&self.pool, community_id, pubkey).await
    }

    /// Update a user's profile fields.
    #[datastore_span(name = "update_user_profile", system = "postgresql")]
    pub async fn update_user_profile(
        &self,
        community_id: CommunityId,
        pubkey: &[u8],
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        about: Option<&str>,
        nip05_handle: Option<&str>,
    ) -> Result<()> {
        crate::user::update_user_profile(
            &self.pool,
            community_id,
            pubkey,
            display_name,
            avatar_url,
            about,
            nip05_handle,
        )
        .await
    }

    /// Look up a user by NIP-05 handle.
    #[datastore_span(name = "get_user_by_nip05", system = "postgresql")]
    pub async fn get_user_by_nip05(
        &self,
        community_id: CommunityId,
        local_part: &str,
        domain: &str,
    ) -> Result<Option<UserProfile>> {
        crate::user::get_user_by_nip05(&self.pool, community_id, local_part, domain).await
    }

    /// Search users by display name, NIP-05 handle, or pubkey prefix.
    #[datastore_span(name = "search_users", system = "postgresql")]
    pub async fn search_users(
        &self,
        community_id: CommunityId,
        query: &str,
        limit: u32,
    ) -> Result<Vec<UserSearchProfile>> {
        crate::user::search_users(&self.pool, community_id, query, limit).await
    }

    /// Atomically set agent owner — only if no owner is currently assigned.
    /// Returns Ok(true) if set, Ok(false) if an owner already exists.
    #[datastore_span(name = "set_agent_owner", system = "postgresql")]
    pub async fn set_agent_owner(
        &self,
        community_id: CommunityId,
        agent_pubkey: &[u8],
        owner_pubkey: &[u8],
    ) -> Result<bool> {
        crate::user::set_agent_owner(&self.pool, community_id, agent_pubkey, owner_pubkey).await
    }

    /// Materialize an authenticated NIP-OA agent-owner relationship under
    /// authorization attribution.
    #[datastore_span(name = "set_agent_owner_for_authorization", system = "postgresql")]
    pub async fn set_agent_owner_for_authorization(
        &self,
        community_id: CommunityId,
        agent_pubkey: &[u8],
        owner_pubkey: &[u8],
    ) -> Result<bool> {
        set_agent_owner_with_operation(
            &self.pool,
            community_id,
            agent_pubkey,
            owner_pubkey,
            crate::observability::WriterOperation::Authorization,
        )
        .await
    }

    /// Get the channel_add_policy and agent_owner_pubkey for a user.
    #[datastore_span(name = "get_agent_channel_policy", system = "postgresql")]
    pub async fn get_agent_channel_policy(
        &self,
        community_id: CommunityId,
        pubkey: &[u8],
    ) -> Result<Option<(String, Option<Vec<u8>>)>> {
        crate::user::get_agent_channel_policy(&self.pool, community_id, pubkey).await
    }

    /// Check whether `actor_pubkey` is the agent owner of `target_pubkey`.
    #[datastore_span(name = "is_agent_owner", system = "postgresql")]
    pub async fn is_agent_owner(
        &self,
        community_id: CommunityId,
        target_pubkey: &[u8],
        actor_pubkey: &[u8],
    ) -> Result<bool> {
        crate::user::is_agent_owner(&self.pool, community_id, target_pubkey, actor_pubkey).await
    }

    /// Set the channel_add_policy for a user.
    #[datastore_span(name = "set_channel_add_policy", system = "postgresql")]
    pub async fn set_channel_add_policy(
        &self,
        community_id: CommunityId,
        pubkey: &[u8],
        policy: &str,
    ) -> Result<()> {
        crate::user::set_channel_add_policy(&self.pool, community_id, pubkey, policy).await
    }
}

#[cfg(test)]
mod postgres_tests;
