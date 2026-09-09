use super::*;
use buzz_core::{
    channel::{ChannelType, ChannelVisibility},
    cml::{CmlStatus, CmlTask, Lease},
    cml_event::CmlRole,
};
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::PgPool;

pub(crate) struct Fixture {
    pub db: Db,
    pub pool: PgPool,
    pub community: CommunityId,
    pub channel: Uuid,
    pub planner: Keys,
    pub worker: Keys,
    pub task: CmlTask,
}

impl Fixture {
    pub async fn new() -> Self {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .unwrap();
        let community = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id,host) VALUES($1,$2)")
            .bind(community.as_uuid())
            .bind(format!("fleet-{}.invalid", community.as_uuid()))
            .execute(&pool)
            .await
            .unwrap();
        Self::in_community(pool, community).await
    }

    pub async fn in_community(pool: PgPool, community: CommunityId) -> Self {
        let db = Db::from_pool(pool.clone());
        let planner = Keys::generate();
        let worker = Keys::generate();
        for keys in [&planner, &worker] {
            db.ensure_user(community, &keys.public_key().to_bytes())
                .await
                .unwrap();
        }
        sqlx::query("UPDATE users SET machine_id='mack',agent_type='hermes' WHERE community_id=$1 AND pubkey=$2")
            .bind(community.as_uuid()).bind(worker.public_key().to_bytes().as_slice()).execute(&pool).await.unwrap();
        let channel = db
            .create_channel(
                community,
                "Fleet qualification",
                ChannelType::Stream,
                ChannelVisibility::Private,
                None,
                &planner.public_key().to_bytes(),
                None,
            )
            .await
            .unwrap()
            .id;
        sqlx::query("INSERT INTO channel_members(community_id,channel_id,pubkey,role) VALUES($1,$2,$3,'bot')")
            .bind(community.as_uuid()).bind(channel).bind(worker.public_key().to_bytes().as_slice()).execute(&pool).await.unwrap();
        let row = db
            .create_task(
                community,
                crate::task::NewTask {
                    title: "Qualify repository".into(),
                    channel_id: Some(channel),
                    created_by_pubkey: Some(planner.public_key().to_bytes().to_vec()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let now = Timestamp::now().as_secs();
        let task = serde_json::from_value(json!({
            "protocol":"buzz-cml","version":1,"id":row.id,"title":"Qualify repository",
            "objective":"Observe tracked files","status":"planned","priority":"P2","updated_at":now,
            "roles":{"planner":planner.public_key().to_hex(),"worker":worker.public_key().to_hex(),"reviewer":Keys::generate().public_key().to_hex(),"fixer":null},
            "git":{"repo":"mfethe1/buzz","branch":"codex/qualification","base_sha":"a".repeat(40),"head_sha":null,"worktree_alias":"qualification"},
            "lease":null,"evidence":[],"blockers":[],"acceptance":[],"review":{"round":0,"max_rounds":3},
            "runtime":{"host_id":null,"last_heartbeat_at":null,"presence":"offline","ttl_seconds":180},
            "extensions":{fleet::EXTENSION:{"target":"mack","machine_id":"mack","repository":"buzz","capability":"qualify","expires_at":now+300,"task_revision":row.revision,"policy_digest":"b".repeat(64)}}
        })).unwrap();
        Self {
            db,
            pool,
            community,
            channel,
            planner,
            worker,
            task,
        }
    }
    pub async fn grant(&self) {
        crate::agent_capability_grants::grant(
            &self.pool,
            self.community,
            &self.planner.public_key().to_bytes(),
            "cross_ssh",
            "mack",
            &self.planner.public_key().to_bytes(),
        )
        .await
        .unwrap();
    }
    pub fn event(
        &self,
        task: &CmlTask,
        transition: CmlTransition,
        previous: Option<&Event>,
    ) -> Event {
        let role = if matches!(
            transition,
            CmlTransition::Plan | CmlTransition::Cancel | CmlTransition::LeaseExpired
        ) {
            CmlRole::Planner
        } else {
            CmlRole::Worker
        };
        let keys = if role == CmlRole::Planner {
            &self.planner
        } else {
            &self.worker
        };
        let status = serde_json::to_value(task.status)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        let mut tags = vec![
            Tag::parse(["h", &self.channel.to_string()]).unwrap(),
            Tag::parse(["d", &task.id.to_string()]).unwrap(),
            Tag::parse(["protocol", "buzz-cml", "1"]).unwrap(),
            Tag::parse(["transition", transition.as_str()]).unwrap(),
            Tag::parse(["status", &status]).unwrap(),
            Tag::parse(["role", role.as_str()]).unwrap(),
        ];
        if let Some(previous) = previous {
            tags.push(Tag::parse(["e", &previous.id.to_hex(), "prev"]).unwrap());
        }
        let event = EventBuilder::new(
            Kind::Custom(transition.event_kind() as u16),
            task.to_canonical_json().unwrap(),
        )
        .tags(tags)
        .custom_created_at(Timestamp::from(task.updated_at))
        .sign_with_keys(keys)
        .unwrap();
        cml_event::validate_cml_event(&event).unwrap();
        event
    }
    pub async fn persist(&self, event: &Event) -> Result<(StoredEvent, bool)> {
        self.db
            .insert_fleet_event(self.community, event, Some(self.channel), None)
            .await
    }
    pub async fn plan_claim(&self) -> (Event, Event, CmlTask) {
        self.grant().await;
        let plan = self.event(&self.task, CmlTransition::Plan, None);
        assert!(self.persist(&plan).await.unwrap().1);
        let mut claimed = self.task.clone();
        claimed.status = CmlStatus::Claimed;
        claimed.lease = Some(Lease {
            id: fleet::attempt_id(self.community, self.task.id, plan.id.as_bytes()),
            holder: self.worker.public_key().to_hex(),
            issued_at: self.task.updated_at,
            expires_at: self.task.updated_at + 300,
        });
        let claim = self.event(&claimed, CmlTransition::Claim, Some(&plan));
        assert!(self.persist(&claim).await.unwrap().1);
        (plan, claim, claimed)
    }
    pub async fn start(&self) -> (Event, Event, CmlTask) {
        let (plan, claim, mut task) = self.plan_claim().await;
        task.status = CmlStatus::Working;
        let start = self.event(&task, CmlTransition::Start, Some(&claim));
        assert!(self.persist(&start).await.unwrap().1);
        (plan, start, task)
    }
    pub fn receipt(&self, plan: &Event, start: Option<&Event>, status: ReceiptStatus) -> Event {
        let body = FleetReceipt {
            attempt_id: fleet::attempt_id(self.community, self.task.id, plan.id.as_bytes()),
            task_id: self.task.id,
            plan_event_id: plan.id.to_hex(),
            start_event_id: start.map(|e| e.id.to_hex()),
            machine_id: "mack".into(),
            policy_digest: "b".repeat(64),
            status,
            qualification: if status == ReceiptStatus::Success {
                Some(fleet::Qualification {
                    repository: "mfethe1/buzz".into(),
                    head_sha: "c".repeat(40),
                    tracked_files: 3,
                    python: "3.14.0".into(),
                })
            } else {
                None
            },
            error: None,
            completed_at: Timestamp::now().as_secs(),
        };
        EventBuilder::new(
            Kind::Custom(buzz_core::kind::KIND_JOB_RESULT as u16),
            body.to_canonical_json().unwrap(),
        )
        .tags([
            Tag::parse(["protocol", fleet::RECEIPT_PROTOCOL, "1"]).unwrap(),
            Tag::parse(["h", &self.channel.to_string()]).unwrap(),
            Tag::parse(["d", &body.attempt_id]).unwrap(),
        ])
        .custom_created_at(Timestamp::from(body.completed_at))
        .sign_with_keys(&self.worker)
        .unwrap()
    }
    pub async fn admission(&self, plan: &Event, start: &Event) -> Result<Value> {
        self.db
            .fleet_start_admission(
                self.community,
                self.task.id,
                &fleet::attempt_id(self.community, self.task.id, plan.id.as_bytes()),
                start.id.as_bytes(),
                &self.worker.public_key().to_bytes(),
            )
            .await
    }
    pub async fn event_count(&self, event: &Event) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM events WHERE community_id=$1 AND id=$2")
            .bind(self.community.as_uuid())
            .bind(event.id.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}
