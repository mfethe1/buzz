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

async fn history(pool: &PgPool, community: CommunityId) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT to_jsonb(e) FROM agent_capability_events e WHERE community_id = $1 ORDER BY id",
    )
    .bind(community.as_uuid())
    .fetch_all(pool)
    .await
    .expect("history")
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn grant_revoke_regrant_preserves_actors_and_complete_images() {
    let pool = setup_pool().await;
    let community = make_community(&pool).await;
    let agent = random_pubkey();
    let actors = [random_pubkey(), random_pubkey(), random_pubkey()];
    grant(&pool, community, &agent, CAP_CROSS_SSH, "mack", &actors[0])
        .await
        .expect("grant");
    assert!(
        revoke(&pool, community, &agent, CAP_CROSS_SSH, "mack", &actors[1])
            .await
            .expect("revoke")
    );
    assert!(
        !revoke(&pool, community, &agent, CAP_CROSS_SSH, "mack", &actors[1])
            .await
            .expect("repeat revoke")
    );
    grant(&pool, community, &agent, CAP_CROSS_SSH, "mack", &actors[2])
        .await
        .expect("regrant");
    let events = history(&pool, community).await;
    assert_eq!(events.len(), 3);
    for (index, action) in ["grant", "revoke", "grant"].into_iter().enumerate() {
        assert_eq!(events[index]["action"], action);
        assert_eq!(
            events[index]["actor_pubkey"],
            format!("\\x{}", hex::encode(&actors[index]))
        );
    }
    assert!(events[0]["before_state"].is_null());
    assert_eq!(events[1]["before_state"], events[0]["after_state"]);
    assert_eq!(events[2]["before_state"], events[1]["after_state"]);
    assert!(!events[1]["after_state"]["revoked_at"].is_null());
    assert!(events[2]["after_state"]["revoked_at"].is_null());
    assert!(is_granted(&pool, community, &agent, CAP_CROSS_SSH, "mack")
        .await
        .expect("current permission"));
    let other = make_community(&pool).await;
    assert!(history(&pool, other).await.is_empty());
    assert!(!is_granted(&pool, other, &agent, CAP_CROSS_SSH, "mack")
        .await
        .expect("tenant isolation"));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn history_failure_rolls_back_grant_and_revoke() {
    let pool = setup_pool().await;
    let community = make_community(&pool).await;
    let agent = random_pubkey();
    let actor = random_pubkey();
    grant(&pool, community, &agent, CAP_CROSS_SSH, "mack", &actor)
        .await
        .expect("initial grant");
    let before = history(&pool, community).await;
    sqlx::raw_sql("CREATE FUNCTION reject_capability_history_fixture() RETURNS TRIGGER LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected history write failure'; END $$; CREATE TRIGGER reject_history_fixture BEFORE INSERT ON agent_capability_events FOR EACH ROW EXECUTE FUNCTION reject_capability_history_fixture();")
        .execute(&pool).await.expect("install failure at actual append seam");
    assert!(
        revoke(&pool, community, &agent, CAP_CROSS_SSH, "mack", &actor)
            .await
            .is_err()
    );
    assert!(is_granted(&pool, community, &agent, CAP_CROSS_SSH, "mack")
        .await
        .expect("revoke rollback"));
    assert!(
        grant(&pool, community, &agent, CAP_CROSS_SSH, "rosie", &actor)
            .await
            .is_err()
    );
    assert!(
        !is_granted(&pool, community, &agent, CAP_CROSS_SSH, "rosie")
            .await
            .expect("grant rollback")
    );
    assert_eq!(history(&pool, community).await, before);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn concurrent_first_grants_have_one_serial_history_chain() {
    let pool = setup_pool().await;
    let community = make_community(&pool).await;
    let agent = random_pubkey();
    let a = random_pubkey();
    let b = random_pubkey();
    let (first, second) = tokio::join!(
        grant(&pool, community, &agent, CAP_CROSS_SSH, "mack", &a),
        grant(&pool, community, &agent, CAP_CROSS_SSH, "mack", &b)
    );
    first.expect("first grant");
    second.expect("second grant");
    let events = history(&pool, community).await;
    assert_eq!(events.len(), 2);
    assert!(events[0]["before_state"].is_null());
    assert_eq!(events[1]["before_state"], events[0]["after_state"]);
    let grants = list_grants(&pool, community).await.expect("projection");
    assert_eq!(
        events[1]["after_state"]["granted_by"],
        format!("\\x{}", hex::encode(&grants[0].granted_by))
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn capability_history_cannot_be_rewritten_or_deleted_by_serving_writes() {
    let pool = setup_pool().await;
    let community = make_community(&pool).await;
    grant(
        &pool,
        community,
        &random_pubkey(),
        CAP_CROSS_SSH,
        "mack",
        &random_pubkey(),
    )
    .await
    .expect("grant");
    let before = history(&pool, community).await;
    assert!(sqlx::query(
        "UPDATE agent_capability_events SET action = 'revoke' WHERE community_id = $1"
    )
    .bind(community.as_uuid())
    .execute(&pool)
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM agent_capability_events WHERE community_id = $1")
            .bind(community.as_uuid())
            .execute(&pool)
            .await
            .is_err()
    );
    assert_eq!(history(&pool, community).await, before);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn migration_schema_upgrade_from_task_system_preserves_rows_and_adds_grant_history() {
    let pool = setup_pool().await;
    crate::migration::run_migrations_through(&pool, 46)
        .await
        .expect("deployed task schema");
    let community = make_community(&pool).await;
    let task_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tasks (community_id, id, title) VALUES ($1, $2, 'existing task')")
        .bind(community.as_uuid())
        .bind(task_id)
        .execute(&pool)
        .await
        .expect("old task");
    crate::migration::run_migrations(&pool)
        .await
        .expect("additive upgrade");
    let task = crate::task::get_task(&pool, community, task_id)
        .await
        .expect("preserved task");
    assert_eq!(task.title, "existing task");
    assert_eq!(task.revision, 0);
    let agent = random_pubkey();
    crate::user::ensure_user(&pool, community, &agent)
        .await
        .expect("agent");
    crate::user::set_machine_home(
        &pool,
        community,
        &agent,
        &crate::user::MachineHome {
            machine_id: "mack".into(),
            machine_label: None,
            machine_runtime: Some("hermes".into()),
        },
    )
    .await
    .expect("migrated home");
    grant(&pool, community, &agent, CAP_CROSS_SSH, "mack", &agent)
        .await
        .expect("migrated grant");
    assert_eq!(history(&pool, community).await.len(), 1);
    Db::from_pool(pool.clone())
        .validate_deletion_catalog()
        .await
        .expect("current deletion catalog");
}
