use super::*;
use crate::Db;
use nostr::Keys;

async fn setup_db() -> Db {
    let pool = PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("connect to test DB");
    Db::from_pool(pool)
}

fn random_pubkey() -> Vec<u8> {
    Keys::generate().public_key().to_bytes().to_vec()
}

async fn make_community(pool: &PgPool) -> CommunityId {
    let id = uuid::Uuid::new_v4();
    let host = format!("user-test-{}.example", id.simple());
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
        .bind(id)
        .bind(host)
        .execute(pool)
        .await
        .expect("insert test community");
    CommunityId::from_uuid(id)
}

/// Setting an agent owner then reading back the policy should return
/// the default "anyone" policy and the owner pubkey.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_set_agent_owner_and_get_policy() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let agent_pk = random_pubkey();
    let owner_pk = random_pubkey();

    ensure_user(&db.pool, community, &agent_pk)
        .await
        .expect("ensure agent");
    ensure_user(&db.pool, community, &owner_pk)
        .await
        .expect("ensure owner");

    let was_set = set_agent_owner(&db.pool, community, &agent_pk, &owner_pk)
        .await
        .expect("set_agent_owner");
    assert!(was_set, "first set_agent_owner should return true");

    let result = get_agent_channel_policy(&db.pool, community, &agent_pk)
        .await
        .expect("get_agent_channel_policy");

    let (policy, owner) = result.expect("should return Some for known pubkey");
    assert_eq!(policy, "anyone", "default policy should be 'anyone'");
    assert_eq!(
        owner,
        Some(owner_pk),
        "owner pubkey should match what was set"
    );
}

/// set_channel_add_policy should persist each of the three valid policies.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_set_channel_add_policy() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let pk = random_pubkey();
    ensure_user(&db.pool, community, &pk)
        .await
        .expect("ensure user");

    // owner_only
    set_channel_add_policy(&db.pool, community, &pk, "owner_only")
        .await
        .expect("set owner_only");
    let (policy, owner) = get_agent_channel_policy(&db.pool, community, &pk)
        .await
        .expect("get policy")
        .expect("should be Some");
    assert_eq!(policy, "owner_only");
    assert!(owner.is_none(), "no owner was set");

    // nobody
    set_channel_add_policy(&db.pool, community, &pk, "nobody")
        .await
        .expect("set nobody");
    let (policy, owner) = get_agent_channel_policy(&db.pool, community, &pk)
        .await
        .expect("get policy")
        .expect("should be Some");
    assert_eq!(policy, "nobody");
    assert!(owner.is_none());

    // anyone (reset to default)
    set_channel_add_policy(&db.pool, community, &pk, "anyone")
        .await
        .expect("set anyone");
    let (policy, owner) = get_agent_channel_policy(&db.pool, community, &pk)
        .await
        .expect("get policy")
        .expect("should be Some");
    assert_eq!(policy, "anyone");
    assert!(owner.is_none());
}

/// get_agent_channel_policy should return None for a pubkey that has
/// never been inserted into the users table.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_get_policy_unknown_pubkey() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let pk = random_pubkey();

    let result = get_agent_channel_policy(&db.pool, community, &pk)
        .await
        .expect("query should not error");

    assert!(result.is_none(), "unknown pubkey should return None");
}

/// set_agent_owner should return Err when the agent pubkey does not exist
/// in the users table.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_set_agent_owner_nonexistent_agent() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let agent_pk = random_pubkey();
    let owner_pk = random_pubkey();

    // Only ensure the owner exists -- agent is intentionally absent.
    ensure_user(&db.pool, community, &owner_pk)
        .await
        .expect("ensure owner");

    let result = set_agent_owner(&db.pool, community, &agent_pk, &owner_pk).await;
    assert!(
        result.is_err(),
        "should error when agent pubkey is not in users table"
    );
}

/// set_agent_owner should return Ok(false) when the agent already has an owner.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_set_agent_owner_already_owned() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let agent_pk = random_pubkey();
    let owner1 = random_pubkey();
    let owner2 = random_pubkey();

    ensure_user(&db.pool, community, &agent_pk)
        .await
        .expect("ensure agent");
    ensure_user(&db.pool, community, &owner1)
        .await
        .expect("ensure owner1");
    ensure_user(&db.pool, community, &owner2)
        .await
        .expect("ensure owner2");

    let first = set_agent_owner(&db.pool, community, &agent_pk, &owner1)
        .await
        .expect("first set");
    assert!(first, "first set should succeed");

    let second = set_agent_owner(&db.pool, community, &agent_pk, &owner2)
        .await
        .expect("second set should not error");
    assert!(!second, "second set should return false (already owned)");

    // Verify original owner is preserved.
    let (_, owner) = get_agent_channel_policy(&db.pool, community, &agent_pk)
        .await
        .expect("get policy")
        .expect("should be Some");
    assert_eq!(owner, Some(owner1), "original owner should be preserved");
}

/// set_channel_add_policy should return Err when the pubkey does not exist
/// in the users table (0 rows affected -> NotFound).
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_set_channel_add_policy_nonexistent_user() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let pk = random_pubkey();

    let result = set_channel_add_policy(&db.pool, community, &pk, "nobody").await;
    assert!(
        result.is_err(),
        "should error when pubkey is not in users table"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_set_channel_add_policy_rejects_invalid() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let pubkey = nostr::Keys::generate().public_key().to_bytes().to_vec();
    ensure_user(&db.pool, community, &pubkey).await.unwrap();
    let result = set_channel_add_policy(&db.pool, community, &pubkey, "invalid_policy").await;
    assert!(result.is_err(), "should reject invalid policy value");
}

// Use the production `escape_like` function directly — no local mirror.
use super::escape_like;

#[test]
fn like_escape_percent() {
    assert_eq!(escape_like("%"), "\\%");
    assert_eq!(escape_like("100%match"), "100\\%match");
}

#[test]
fn like_escape_underscore() {
    assert_eq!(escape_like("_"), "\\_");
    assert_eq!(escape_like("a_b"), "a\\_b");
}

#[test]
fn like_escape_backslash() {
    assert_eq!(escape_like("\\"), "\\\\");
    assert_eq!(escape_like("a\\b"), "a\\\\b");
}

#[test]
fn like_escape_combined() {
    // All three metacharacters in one string
    assert_eq!(escape_like("%_\\"), "\\%\\_\\\\");
}

#[test]
fn like_escape_normal_input_unchanged() {
    assert_eq!(escape_like("alice"), "alice");
    assert_eq!(escape_like("bob@example.com"), "bob@example.com");
    assert_eq!(escape_like(""), "");
}

/// A user with "owner_only" policy but no agent_owner_pubkey set should
/// return Some(("owner_only", None)).
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_owner_only_with_no_owner() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let pk = random_pubkey();
    ensure_user(&db.pool, community, &pk)
        .await
        .expect("ensure user");

    set_channel_add_policy(&db.pool, community, &pk, "owner_only")
        .await
        .expect("set owner_only");

    let result = get_agent_channel_policy(&db.pool, community, &pk)
        .await
        .expect("get policy")
        .expect("should be Some");

    assert_eq!(result.0, "owner_only");
    assert!(result.1.is_none(), "owner should be None when never set");
}

/// A registered machine home round-trips, and the machine resolves back to
/// the agent that homes it.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_machine_home_round_trip() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let agent = random_pubkey();
    ensure_user(&db.pool, community, &agent)
        .await
        .expect("ensure agent");

    let home = MachineHome {
        machine_id: format!("machine-{}", uuid::Uuid::new_v4()),
        machine_label: Some("Winnie".to_owned()),
        machine_runtime: Some("openclaw".to_owned()),
    };
    set_machine_home(&db.pool, community, &agent, &home)
        .await
        .expect("set home");

    let got = get_machine_home(&db.pool, community, &agent)
        .await
        .expect("get home")
        .expect("home should be Some");
    assert_eq!(got, home);

    let resolved = get_agent_for_machine(&db.pool, community, &home.machine_id)
        .await
        .expect("resolve machine")
        .expect("machine should resolve");
    assert_eq!(resolved, agent, "machine must resolve to its home agent");
}

/// The core PR-3 invariant: one home agent per machine, per community. The
/// second claim must be rejected rather than silently stealing the host.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_second_agent_cannot_claim_same_machine() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let first = random_pubkey();
    let second = random_pubkey();
    ensure_user(&db.pool, community, &first).await.expect("a");
    ensure_user(&db.pool, community, &second).await.expect("b");

    let machine_id = format!("machine-{}", uuid::Uuid::new_v4());
    let home = MachineHome {
        machine_id: machine_id.clone(),
        machine_label: None,
        machine_runtime: None,
    };
    set_machine_home(&db.pool, community, &first, &home)
        .await
        .expect("first claim wins");

    let conflict = set_machine_home(&db.pool, community, &second, &home).await;
    assert!(
        matches!(conflict, Err(crate::error::DbError::AccessDenied(_))),
        "second claim on the same machine must be denied, got {conflict:?}"
    );

    // The original home is untouched by the failed claim.
    let still = get_agent_for_machine(&db.pool, community, &machine_id)
        .await
        .expect("resolve")
        .expect("still homed");
    assert_eq!(still, first);
}

/// Admission confinement: the same machine id in a different community is a
/// different machine, so the unique index must not collide across tenants.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_same_machine_id_allowed_in_other_community() {
    let db = setup_db().await;
    let community_a = make_community(&db.pool).await;
    let community_b = make_community(&db.pool).await;
    let agent_a = random_pubkey();
    let agent_b = random_pubkey();
    ensure_user(&db.pool, community_a, &agent_a)
        .await
        .expect("a");
    ensure_user(&db.pool, community_b, &agent_b)
        .await
        .expect("b");

    let home = MachineHome {
        machine_id: format!("machine-{}", uuid::Uuid::new_v4()),
        machine_label: None,
        machine_runtime: None,
    };
    set_machine_home(&db.pool, community_a, &agent_a, &home)
        .await
        .expect("community A claim");
    set_machine_home(&db.pool, community_b, &agent_b, &home)
        .await
        .expect("community B must be independent of A");
}

/// Clearing a home frees the machine for another agent to claim.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_clear_machine_home_frees_the_machine() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let first = random_pubkey();
    let second = random_pubkey();
    ensure_user(&db.pool, community, &first).await.expect("a");
    ensure_user(&db.pool, community, &second).await.expect("b");

    let home = MachineHome {
        machine_id: format!("machine-{}", uuid::Uuid::new_v4()),
        machine_label: None,
        machine_runtime: None,
    };
    set_machine_home(&db.pool, community, &first, &home)
        .await
        .expect("first claim");

    assert!(
        clear_machine_home(&db.pool, community, &first)
            .await
            .expect("clear"),
        "clearing an existing home reports true"
    );
    assert!(
        get_machine_home(&db.pool, community, &first)
            .await
            .expect("get")
            .is_none(),
        "home is gone after clear"
    );
    assert!(
        !clear_machine_home(&db.pool, community, &first)
            .await
            .expect("clear again"),
        "clearing an unhomed agent reports false"
    );

    set_machine_home(&db.pool, community, &second, &home)
        .await
        .expect("machine is free for the next agent");
}

/// Registering a home for a pubkey with no users row is an error, not a
/// silent no-op that would leave the machine unhomed.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_set_machine_home_nonexistent_agent() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let ghost = random_pubkey();

    let home = MachineHome {
        machine_id: format!("machine-{}", uuid::Uuid::new_v4()),
        machine_label: None,
        machine_runtime: None,
    };
    let result = set_machine_home(&db.pool, community, &ghost, &home).await;
    assert!(
        matches!(result, Err(crate::error::DbError::NotFound(_))),
        "unknown agent must not be homed, got {result:?}"
    );
}

/// A label or runtime with no machine_id is unaddressable, and the database
/// must reject it rather than storing a half-registered home.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn test_label_without_machine_id_is_rejected() {
    let db = setup_db().await;
    let community = make_community(&db.pool).await;
    let agent = random_pubkey();
    ensure_user(&db.pool, community, &agent)
        .await
        .expect("ensure agent");

    let result =
        sqlx::query("UPDATE users SET machine_label = $1 WHERE community_id = $2 AND pubkey = $3")
            .bind("orphan-label")
            .bind(community.as_uuid())
            .bind(&agent)
            .execute(&db.pool)
            .await;
    assert!(
        result.is_err(),
        "a label with no machine_id must violate chk_users_machine_fields_require_machine_id"
    );
}
