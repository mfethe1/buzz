use super::*;
use nostr::{EventBuilder, Keys, Kind, Timestamp};

struct Fixture {
    db: Db,
    pool: sqlx::PgPool,
    community: CommunityId,
    owner: Keys,
    coordinator: Keys,
    machine: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
            .await
            .unwrap();
        let community = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id,host) VALUES($1,$2)")
            .bind(community.as_uuid())
            .bind(format!("machine-{}.invalid", community.as_uuid()))
            .execute(&pool)
            .await
            .unwrap();
        Self {
            db: Db::from_pool(pool.clone()),
            pool,
            community,
            owner: Keys::generate(),
            coordinator: Keys::generate(),
            machine: Uuid::new_v4(),
        }
    }
    fn enrollment(&self) -> Event {
        // DB seam requires transport-verified proof; cryptographic owner proof is
        // exercised by the real relay tests, not this transaction fixture.
        let mut payload = json!({"version":1,"community_id":self.community.as_uuid(),"machine_id":self.machine,"coordinator_pubkey":self.coordinator.public_key().to_hex(),"label":"Mack","runtime":"hermes","owner_auth":["auth",self.owner.public_key().to_hex(),"kind=47210","a".repeat(128)]});
        let mut consent = payload.clone();
        consent["owner_pubkey"] = json!(self.owner.public_key().to_hex());
        consent["expires_at"] = json!(Timestamp::now().as_secs() + 300);
        payload["coordinator_consent"] = serde_json::to_value(
            EventBuilder::new(Kind::Custom(47212), consent.to_string())
                .sign_with_keys(&self.coordinator)
                .unwrap(),
        )
        .unwrap();
        EventBuilder::new(Kind::Custom(47210), payload.to_string())
            .sign_with_keys(&self.owner)
            .unwrap()
    }
    fn observation(&self, enrollment: &Event, sequence: i64, timestamp: u64, keys: &Keys) -> Event {
        EventBuilder::new(Kind::Custom(47211),json!({"version":1,"community_id":self.community.as_uuid(),"machine_id":self.machine,"registration_event_id":enrollment.id.to_hex(),"sequence":sequence,"state":"ready"}).to_string()).custom_created_at(Timestamp::from(timestamp)).sign_with_keys(keys).unwrap()
    }
    async fn read(&self) -> Value {
        self.db
            .get_machine(
                self.community,
                &self.owner.public_key().to_bytes(),
                self.machine,
            )
            .await
            .unwrap()
            .unwrap()
    }
    async fn count(&self, table: &str) -> i64 {
        assert!([
            "machine_control_events",
            "machines",
            "events",
            "agent_capability_grants"
        ]
        .contains(&table));
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM {table} WHERE community_id=$1"
        )))
        .bind(self.community.as_uuid())
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn private_machine_atomic_enrollment_duplicate_and_owner_pagination() {
    let f = Fixture::new().await;
    let event = f.enrollment();
    let (one, two) = tokio::join!(
        f.db.apply_machine_command(f.community, &event),
        f.db.apply_machine_command(f.community, &event)
    );
    assert_eq!(
        [one.unwrap(), two.unwrap()].iter().filter(|v| **v).count(),
        1
    );
    assert_eq!(f.count("machine_control_events").await, 1);
    assert_eq!(f.count("events").await, 0);
    assert_eq!(f.count("agent_capability_grants").await, 0);
    let row = f.read().await;
    assert_eq!(row["fresh"], false);
    assert_eq!(row["enrollment_event"]["id"], event.id.to_hex());
    let user=sqlx::query("SELECT agent_owner_pubkey,agent_type,machine_id FROM users WHERE community_id=$1 AND pubkey=$2").bind(f.community.as_uuid()).bind(f.coordinator.public_key().to_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
    assert_eq!(
        user.get::<Vec<u8>, _>("agent_owner_pubkey"),
        f.owner.public_key().to_bytes()
    );
    assert_eq!(user.get::<String, _>("agent_type"), "hermes");
    assert_eq!(user.get::<String, _>("machine_id"), f.machine.to_string());
    assert!(f
        .db
        .list_machines(
            f.community,
            &f.coordinator.public_key().to_bytes(),
            None,
            50
        )
        .await
        .unwrap()
        .is_empty());
    assert!(f
        .db
        .get_machine(
            f.community,
            &f.coordinator.public_key().to_bytes(),
            f.machine
        )
        .await
        .unwrap()
        .is_none());
    assert!(f
        .db
        .get_machine(
            CommunityId::from_uuid(Uuid::new_v4()),
            &f.owner.public_key().to_bytes(),
            f.machine
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        f.db.list_machines(f.community, &f.owner.public_key().to_bytes(), None, 1)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(f
        .db
        .list_machines(
            f.community,
            &f.owner.public_key().to_bytes(),
            Some(f.machine),
            1
        )
        .await
        .unwrap()
        .is_empty());
    // Interleave another owner's machine between two owned IDs; owner filtering
    // must happen before LIMIT, including on subsequent cursor pages.
    let mut page = Fixture {
        db: f.db.clone(),
        pool: f.pool.clone(),
        community: f.community,
        owner: f.owner.clone(),
        coordinator: Keys::generate(),
        machine: Uuid::from_u128(1),
    };
    let first_id = page.machine;
    page.db
        .apply_machine_command(page.community, &page.enrollment())
        .await
        .unwrap();
    page.owner = Keys::generate();
    page.coordinator = Keys::generate();
    page.machine = Uuid::from_u128(2);
    page.db
        .apply_machine_command(page.community, &page.enrollment())
        .await
        .unwrap();
    page.owner = f.owner.clone();
    page.coordinator = Keys::generate();
    page.machine = Uuid::from_u128(3);
    page.db
        .apply_machine_command(page.community, &page.enrollment())
        .await
        .unwrap();
    let first_page =
        f.db.list_machines(f.community, &f.owner.public_key().to_bytes(), None, 1)
            .await
            .unwrap();
    assert_eq!(first_page[0]["machine_id"], first_id.to_string());
    let next_page =
        f.db.list_machines(
            f.community,
            &f.owner.public_key().to_bytes(),
            Some(first_id),
            1,
        )
        .await
        .unwrap();
    assert_eq!(next_page[0]["machine_id"], page.machine.to_string());
    f.db.validate_deletion_catalog().await.unwrap();
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn private_machine_replacements_moves_and_foreign_ownership_roll_back() {
    let mut f = Fixture::new().await;
    let event = f.enrollment();
    f.db.apply_machine_command(f.community, &event)
        .await
        .unwrap();
    let original = f.read().await;
    let original_coordinator = f.coordinator.clone();
    f.coordinator = Keys::generate();
    assert!(f
        .db
        .apply_machine_command(f.community, &f.enrollment())
        .await
        .is_err());
    let absent: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS(SELECT 1 FROM users WHERE community_id=$1 AND pubkey=$2)",
    )
    .bind(f.community.as_uuid())
    .bind(f.coordinator.public_key().to_bytes().as_slice())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(absent);
    assert_eq!(f.read().await, original);
    f.coordinator = original_coordinator;
    f.machine = Uuid::new_v4();
    // The coordinator consents to the proposed new machine, so rejection must
    // come from the existing durable registration rather than tampered consent.
    let moved = f.enrollment();
    assert!(f
        .db
        .apply_machine_command(f.community, &moved)
        .await
        .is_err());
    assert_eq!(f.count("machine_control_events").await, 1);
    let other = Fixture::new().await;
    other
        .db
        .ensure_user(other.community, &f.owner.public_key().to_bytes())
        .await
        .unwrap();
    sqlx::query("INSERT INTO users(community_id,pubkey,agent_owner_pubkey) VALUES($1,$2,$3)")
        .bind(other.community.as_uuid())
        .bind(other.coordinator.public_key().to_bytes().as_slice())
        .bind(f.owner.public_key().to_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(other
        .db
        .apply_machine_command(other.community, &other.enrollment())
        .await
        .is_err());
    assert_eq!(other.count("machines").await, 0);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn private_machine_observation_binding_order_expiry_and_replay() {
    let f = Fixture::new().await;
    let enrollment = f.enrollment();
    f.db.apply_machine_command(f.community, &enrollment)
        .await
        .unwrap();
    let now = Timestamp::now().as_secs();
    let observation = f.observation(&enrollment, 1, now, &f.coordinator);
    assert!(f
        .db
        .apply_machine_command(f.community, &observation)
        .await
        .unwrap());
    let first = f.read().await;
    assert_eq!(first["fresh"], true);
    assert!(!f
        .db
        .apply_machine_command(f.community, &observation)
        .await
        .unwrap());
    assert_eq!(f.read().await, first);
    for event in [
        f.observation(&enrollment, 2, now, &f.owner),
        f.observation(&enrollment, 1, now + 1, &f.coordinator),
        f.observation(&enrollment, 2, now - 31, &f.coordinator),
        f.observation(&enrollment, 2, now + 6, &f.coordinator),
    ] {
        assert!(f
            .db
            .apply_machine_command(f.community, &event)
            .await
            .is_err());
        assert_eq!(f.read().await, first);
    }
    let fake = EventBuilder::new(Kind::Custom(47210), "fake")
        .sign_with_keys(&f.owner)
        .unwrap();
    assert!(f
        .db
        .apply_machine_command(f.community, &f.observation(&fake, 2, now, &f.coordinator))
        .await
        .is_err());
    let new = f.observation(&enrollment, 2, now, &f.coordinator);
    assert!(f.db.apply_machine_command(f.community, &new).await.unwrap());
    sqlx::query("UPDATE machines SET observed_at=clock_timestamp()-interval '130 seconds',received_at=clock_timestamp()-interval '130 seconds',expires_at=clock_timestamp()-interval '10 seconds' WHERE community_id=$1").bind(f.community.as_uuid()).execute(&f.pool).await.unwrap();
    assert_eq!(f.read().await["fresh"], false);
    assert!(!f.db.apply_machine_command(f.community, &new).await.unwrap());
    assert_eq!(f.read().await["fresh"], false);
    sqlx::query("UPDATE users SET machine_id=NULL,machine_label=NULL,machine_runtime=NULL WHERE community_id=$1 AND pubkey=$2").bind(f.community.as_uuid()).bind(f.coordinator.public_key().to_bytes().as_slice()).execute(&f.pool).await.unwrap();
    assert!(f
        .db
        .apply_machine_command(
            f.community,
            &f.observation(&enrollment, 3, Timestamp::now().as_secs(), &f.coordinator)
        )
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn private_machine_journal_failure_rolls_back_owner_type_and_home() {
    let f = Fixture::new().await;
    sqlx::raw_sql("CREATE FUNCTION reject_machine_journal() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned test rollback'; END $$; CREATE TRIGGER reject_machine_journal BEFORE INSERT ON machine_control_events FOR EACH ROW EXECUTE FUNCTION reject_machine_journal()") .execute(&f.pool).await.unwrap();
    assert!(f
        .db
        .apply_machine_command(f.community, &f.enrollment())
        .await
        .is_err());
    assert_eq!(f.count("machines").await, 0);
    assert_eq!(f.count("machine_control_events").await, 0);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE community_id=$1")
        .bind(f.community.as_uuid())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    sqlx::raw_sql("DROP TRIGGER reject_machine_journal ON machine_control_events; DROP FUNCTION reject_machine_journal()").execute(&f.pool).await.unwrap();
}
