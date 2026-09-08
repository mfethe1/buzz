//! Signed HTTP ingress and primary admission exercised over a real local socket.
mod postgres_tests {
    use super::super::route_authz::{fixture, Fixture};
    use axum::http::StatusCode;
    use buzz_core::{
        cml::{CmlStatus, CmlTask, Lease},
        cml_event::{CmlRole, CmlTransition},
        fleet::{self, FleetReceipt, Qualification, ReceiptStatus},
    };
    use nostr::{Event, EventBuilder, Kind, Tag, Timestamp};
    use serde_json::{json, Value};
    use std::sync::Arc;

    struct Server(tokio::task::JoinHandle<()>);
    impl Drop for Server {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    async fn serve(f: &mut Fixture) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        f.http_base = Some(format!("http://{}", listener.local_addr().unwrap()));
        let state = Arc::get_mut(&mut f.state).unwrap();
        state.nip98_replay = Arc::new(buzz_pubsub::RedisNip98ReplayGuard::new(
            state.redis_pool.clone(),
        ));
        let router = f.router();
        Server(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }))
    }
    fn plan(f: &Fixture) -> CmlTask {
        let now = Timestamp::now().as_secs();
        serde_json::from_value(json!({
            "protocol":"buzz-cml","version":1,"id":f.task_id,"title":"Qualify repository","objective":"Read tracked file count",
            "status":"planned","priority":"P2","updated_at":now,
            "roles":{"planner":f.owner.public_key().to_hex(),"worker":f.outsider.public_key().to_hex(),"reviewer":nostr::Keys::generate().public_key().to_hex(),"fixer":null},
            "git":{"repo":"mfethe1/buzz","branch":"codex/qualify","base_sha":"a".repeat(40),"head_sha":null,"worktree_alias":"buzz"},
            "lease":null,"evidence":[],"blockers":[],"acceptance":[],"review":{"round":0,"max_rounds":3},
            "runtime":{"host_id":null,"last_heartbeat_at":null,"presence":"offline","ttl_seconds":180},
            "extensions":{fleet::EXTENSION:{"target":"mack","machine_id":"mack","repository":"buzz","capability":"qualify","expires_at":now+300,"task_revision":0,"policy_digest":"b".repeat(64)}}
        })).unwrap()
    }
    fn signed(
        f: &Fixture,
        task: &CmlTask,
        transition: CmlTransition,
        previous: Option<&Event>,
    ) -> Event {
        let planner = transition == CmlTransition::Plan;
        buzz_sdk::build_cml_transition(
            f.private_channel_id,
            task,
            transition,
            if planner {
                CmlRole::Planner
            } else {
                CmlRole::Worker
            },
            previous.map(|e| e.id),
        )
        .unwrap()
        .sign_with_keys(if planner { &f.owner } else { &f.outsider })
        .unwrap()
    }
    async fn post(f: &Fixture, event: &Event, planner: bool) -> Value {
        let (status, body) = f
            .request(
                "POST",
                "/events",
                if planner { &f.owner } else { &f.outsider },
                Some(&serde_json::to_string(event).unwrap()),
            )
            .await;
        println!("fleet HTTP POST: {status} {body}");
        body
    }
    async fn count(f: &Fixture, event: &Event) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM events WHERE community_id=$1 AND id=$2")
            .bind(f.community.as_uuid())
            .bind(event.id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap()
    }
    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn signed_http_atomic_start_ack_and_primary_admission_fail_closed() {
        let mut f = fixture().await.expect("real isolated PG/Redis fixture");
        let _server = serve(&mut f).await;
        sqlx::query("UPDATE users SET agent_type='hermes',machine_id='mack' WHERE community_id=$1 AND pubkey=$2")
            .bind(f.community.as_uuid()).bind(f.outsider.public_key().to_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO channel_members(community_id,channel_id,pubkey,role) VALUES($1,$2,$3,'bot')")
            .bind(f.community.as_uuid()).bind(f.private_channel_id).bind(f.outsider.public_key().to_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let mut task = plan(&f);
        let plan = signed(&f, &task, CmlTransition::Plan, None);
        assert_ne!(post(&f, &plan, true).await["accepted"], true);
        assert_eq!(count(&f, &plan).await, 0);
        f.state
            .db
            .capability_grant(
                f.community,
                &f.owner.public_key().to_bytes(),
                "cross_ssh",
                "mack",
                &f.owner.public_key().to_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(post(&f, &plan, true).await["accepted"], true);
        let attempt = fleet::attempt_id(f.community, f.task_id, plan.id.as_bytes());
        task.status = CmlStatus::Claimed;
        task.lease = Some(Lease {
            id: attempt.clone(),
            holder: f.outsider.public_key().to_hex(),
            issued_at: task.updated_at,
            expires_at: task.updated_at + 300,
        });
        let claim = signed(&f, &task, CmlTransition::Claim, Some(&plan));
        assert_eq!(post(&f, &claim, false).await["accepted"], true);
        task.status = CmlStatus::Working;
        let start = signed(&f, &task, CmlTransition::Start, Some(&claim));
        let (one, two) = tokio::join!(post(&f, &start, false), post(&f, &start, false));
        assert_eq!(one["accepted"], true, "{one}");
        assert_eq!(two["accepted"], true, "{two}");
        let fresh = [&one, &two]
            .into_iter()
            .filter(|v| v["message"].as_str() == Some(""))
            .count();
        assert_eq!(
            fresh, 1,
            "one newly accepted ACK, other duplicate: {one} {two}"
        );
        assert_eq!(count(&f, &start).await, 1);
        let path = format!(
            "/api/tasks/{}/attempts/{attempt}/admission?start_event_id={}",
            f.task_id,
            start.id.to_hex()
        );
        let (status, admission) = f.request("GET", &path, &f.outsider, None).await;
        assert_eq!(status, StatusCode::OK, "{admission}");
        assert_eq!(admission["start_event_id"], start.id.to_hex());
        assert_eq!(
            f.request("GET", &path, &f.owner, None).await.0,
            StatusCode::FORBIDDEN
        );
        let mut submitted = task.clone();
        submitted.status = CmlStatus::Review;
        submitted.git.head_sha = Some("c".repeat(40));
        let early = signed(&f, &submitted, CmlTransition::Submit, Some(&start));
        assert_ne!(post(&f, &early, false).await["accepted"], true);
        assert_eq!(count(&f, &early).await, 0);
        f.state
            .db
            .capability_revoke(
                f.community,
                &f.owner.public_key().to_bytes(),
                "cross_ssh",
                "mack",
                &f.owner.public_key().to_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(
            f.request("GET", &path, &f.outsider, None).await.0,
            StatusCode::FORBIDDEN
        );
        let (status, display) = f
            .request("GET", &format!("/api/tasks/{}", f.task_id), &f.owner, None)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(display["attempts"][0]["state"], "started");
        let wire = FleetReceipt {
            attempt_id: attempt.clone(),
            task_id: f.task_id,
            plan_event_id: plan.id.to_hex(),
            start_event_id: Some(start.id.to_hex()),
            machine_id: "mack".into(),
            policy_digest: "b".repeat(64),
            status: ReceiptStatus::Success,
            qualification: Some(Qualification {
                repository: "mfethe1/buzz".into(),
                head_sha: "c".repeat(40),
                tracked_files: 7,
                python: "3.14".into(),
            }),
            error: None,
            completed_at: Timestamp::now().as_secs(),
        };
        let receipt = EventBuilder::new(
            Kind::Custom(buzz_core::kind::KIND_JOB_RESULT as u16),
            wire.to_canonical_json().unwrap(),
        )
        .tags([
            Tag::parse(["protocol", fleet::RECEIPT_PROTOCOL, "1"]).unwrap(),
            Tag::parse(["h", &f.private_channel_id.to_string()]).unwrap(),
            Tag::parse(["d", &attempt]).unwrap(),
        ])
        .sign_with_keys(&f.outsider)
        .unwrap();
        // Revocation blocks new execution, while an already completed worker may
        // still append its verified terminal fact through normal member ingress.
        assert_eq!(post(&f, &receipt, false).await["accepted"], true);
        assert_ne!(post(&f, &early, false).await["accepted"], true);
        assert_eq!(count(&f, &early).await, 0);
        submitted.evidence.push(buzz_core::cml::Evidence {
            kind: "fleet-qualification-receipt".into(),
            reference: receipt.id.to_hex(),
        });
        let mut wrong = submitted.clone();
        wrong.git.head_sha = Some("d".repeat(40));
        let wrong = signed(&f, &wrong, CmlTransition::Submit, Some(&start));
        assert_ne!(post(&f, &wrong, false).await["accepted"], true);
        assert_eq!(count(&f, &wrong).await, 0);
        let valid = signed(&f, &submitted, CmlTransition::Submit, Some(&start));
        assert_eq!(post(&f, &valid, false).await["accepted"], true);
        let (_, display) = f
            .request("GET", &format!("/api/tasks/{}", f.task_id), &f.owner, None)
            .await;
        assert_eq!(display["attempts"][0]["state"], "success");
        assert_eq!(
            display["attempts"][0]["receipt_event_id"],
            receipt.id.to_hex()
        );
        println!("SIGNED_HTTP_FLEET_PASS: ungranted denied; one fresh start ACK; wrong worker and revoked grant denied; signed terminal outcome persisted");
    }
}
