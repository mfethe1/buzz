//! Actual private machine HTTP ingress/readback with real PostgreSQL and Redis.
mod postgres_tests {
    use super::super::route_authz::{fixture, Fixture};
    use axum::http::StatusCode;
    use futures_util::StreamExt;
    use nostr::{Event, EventBuilder, Keys, Kind, Timestamp};
    use serde_json::{json, Value};
    use std::{sync::Arc, time::Duration};
    use uuid::Uuid;

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
    fn enrollment(f: &Fixture, machine: Uuid, coordinator: &Keys, conditions: &str) -> Event {
        let proof =
            buzz_sdk::nip_oa::compute_auth_tag(&f.owner, &coordinator.public_key(), conditions)
                .unwrap();
        let mut payload = json!({"version":1,"community_id":f.community.as_uuid(),"machine_id":machine,"coordinator_pubkey":coordinator.public_key().to_hex(),"label":"Private computer marker","runtime":"hermes","owner_auth":serde_json::from_str::<Value>(&proof).unwrap()});
        let mut consent = payload.clone();
        consent["owner_pubkey"] = json!(f.owner.public_key().to_hex());
        consent["expires_at"] = json!(Timestamp::now().as_secs() + 300);
        payload["coordinator_consent"] = serde_json::to_value(
            EventBuilder::new(Kind::Custom(47212), consent.to_string())
                .sign_with_keys(coordinator)
                .unwrap(),
        )
        .unwrap();
        EventBuilder::new(Kind::Custom(47210), payload.to_string())
            .sign_with_keys(&f.owner)
            .unwrap()
    }
    async fn post(f: &Fixture, event: &Event, signer: &Keys) -> (StatusCode, Value) {
        f.request(
            "POST",
            "/events",
            signer,
            Some(&serde_json::to_string(event).unwrap()),
        )
        .await
    }
    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn signed_machine_enrollment_observation_owner_reads_and_no_public_side_effects() {
        let mut f = fixture().await.expect("owned PG/Redis");
        let _server = serve(&mut f).await;
        let machine = Uuid::new_v4();
        let enrolled = enrollment(&f, machine, &f.outsider, "kind=47210");
        let mut local = f.state.pubsub.subscribe_local();
        let redis_url = std::env::var("BUZZ_TEST_REDIS_URL").expect("owned Redis URL");
        let mut redis = deadpool_redis::redis::Client::open(redis_url)
            .unwrap()
            .get_async_pubsub()
            .await
            .unwrap();
        redis.psubscribe("*").await.unwrap();
        assert_eq!(post(&f, &enrolled, &f.owner).await.1["accepted"], true);
        assert!(post(&f, &enrolled, &f.owner).await.1["message"]
            .as_str()
            .unwrap()
            .starts_with("duplicate:"));
        let path = format!("/api/machines/{machine}");
        let (status, registered) = f.request("GET", &path, &f.owner, None).await;
        assert_eq!(status, StatusCode::OK, "{registered}");
        assert_eq!(registered["enrollment_event"]["id"], enrolled.id.to_hex());
        assert_eq!(registered["fresh"], false);
        assert_eq!(
            f.request("GET", &path, &f.outsider, None).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            f.request("GET", "/api/machines", &f.outsider, None).await.1["machines"],
            json!([])
        );
        let dev = reqwest::Client::new()
            .get(format!("{}{path}", f.http_base.as_ref().unwrap()))
            .header("Host", &f.host)
            .header("X-Pubkey", f.owner.public_key().to_hex())
            .send()
            .await
            .unwrap();
        assert_eq!(
            dev.status(),
            StatusCode::UNAUTHORIZED,
            "dev fallback must stay closed"
        );
        let obs=EventBuilder::new(Kind::Custom(47211),json!({"version":1,"community_id":f.community.as_uuid(),"machine_id":machine,"registration_event_id":enrolled.id.to_hex(),"sequence":1,"state":"ready"}).to_string()).sign_with_keys(&f.outsider).unwrap();
        assert_eq!(post(&f, &obs, &f.outsider).await.1["accepted"], true);
        let current = f.request("GET", &path, &f.owner, None).await.1;
        assert_eq!(current["fresh"], true);
        assert_eq!(current["reported_state"], "ready");
        assert!(post(&f, &obs, &f.outsider).await.1["message"]
            .as_str()
            .unwrap()
            .starts_with("duplicate:"));
        // Only the response clock may advance. Replay must preserve all durable
        // projection fields, including observation expiry and signed events.
        let mut replayed = f.request("GET", &path, &f.owner, None).await.1;
        let mut current = current;
        assert!(replayed
            .as_object_mut()
            .unwrap()
            .remove("server_now")
            .is_some());
        assert!(current
            .as_object_mut()
            .unwrap()
            .remove("server_now")
            .is_some());
        assert_eq!(replayed, current);
        for filter in [
            json!({"ids":[enrolled.id.to_hex(),obs.id.to_hex()]}),
            json!({"kinds":[47210,47211]}),
            json!({"kinds":[9,47210,47211]}),
            json!({}),
            json!({"kinds":[47210,47211],"search":"Private"}),
            json!({"kinds":[47210,47211],"feed_types":["activity","mentions","needs_action"]}),
            json!({"ids":[enrolled.id.to_hex()],"include_history":true}),
        ] {
            for keys in [&f.owner, &f.outsider] {
                let (status, body) = f
                    .request("POST", "/query", keys, Some(&json!([filter]).to_string()))
                    .await;
                assert!(
                    [StatusCode::OK, StatusCode::FORBIDDEN].contains(&status),
                    "{status} {body}"
                );
                let raw = body.to_string();
                assert!(
                    !raw.contains("Private computer marker")
                        && !raw.contains(&enrolled.id.to_hex())
                        && !raw.contains(&obs.id.to_hex()),
                    "private query leak: {body}"
                );
            }
        }
        let (status, count) = f
            .request(
                "POST",
                "/count",
                &f.owner,
                Some(&json!([{"kinds":[47210,47211]}]).to_string()),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{count}");
        assert_eq!(count["count"], 0);
        for table in [
            "events",
            "event_mentions",
            "push_match_queue",
            "workflow_runs",
            "agent_capability_grants",
            "agent_capability_events",
        ] {
            let sql = format!("SELECT count(*) FROM {table} WHERE community_id=$1");
            let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                .bind(f.community.as_uuid())
                .fetch_one(&f.pool)
                .await
                .unwrap();
            assert_eq!(count, 0, "{table} must remain empty");
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), local.recv())
                .await
                .is_err(),
            "local fanout leak"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), redis.on_message().next())
                .await
                .is_err(),
            "Redis fanout leak"
        );
        f.state.db.validate_deletion_catalog().await.unwrap();
        eprintln!("PASS real signed HTTP enrollment→observation→private owner GET; X-Pubkey401, foreign owner404, duplicate expiry stable, ordinary query/COUNT/search/feed and local/Redis fanout empty; no grants/push/workflow rows");
    }

    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn machine_owner_proof_audience_signature_timestamp_and_home_rejections_are_atomic() {
        let mut f = fixture().await.expect("owned PG/Redis");
        let _server = serve(&mut f).await;
        for conditions in ["kind=27235", "kind=47211", "kind=47210&created_at<1"] {
            let event = enrollment(&f, Uuid::new_v4(), &f.outsider, conditions);
            assert_ne!(
                post(&f, &event, &f.owner).await.1["accepted"],
                true,
                "{conditions}"
            );
        }
        let event = enrollment(&f, Uuid::new_v4(), &f.outsider, "kind=47210");
        let payload: Value = serde_json::from_str(&event.content).unwrap();
        // Owner-only assertions must not capture an existing human/unowned key.
        for case in [
            "missing",
            "wrong_signer",
            "tampered",
            "expired",
            "wrong_metadata",
            "foreign_consent",
        ] {
            let mut bad = payload.clone();
            let consent: Event =
                serde_json::from_value(bad["coordinator_consent"].clone()).unwrap();
            let mut consent_body: Value = serde_json::from_str(&consent.content).unwrap();
            match case {
                "missing" => {
                    bad.as_object_mut().unwrap().remove("coordinator_consent");
                }
                "wrong_signer" => {
                    bad["coordinator_consent"] = serde_json::to_value(
                        EventBuilder::new(Kind::Custom(47212), consent.content)
                            .sign_with_keys(&f.owner)
                            .unwrap(),
                    )
                    .unwrap()
                }
                "tampered" => {
                    bad["coordinator_consent"]["content"] = json!(format!("{} ", consent.content));
                }
                "expired" => {
                    consent_body["expires_at"] = json!(Timestamp::now().as_secs() - 1);
                    bad["coordinator_consent"] = serde_json::to_value(
                        EventBuilder::new(Kind::Custom(47212), consent_body.to_string())
                            .sign_with_keys(&f.outsider)
                            .unwrap(),
                    )
                    .unwrap();
                }
                "wrong_metadata" => bad["label"] = json!("Changed without coordinator"),
                "foreign_consent" => {
                    consent_body["community_id"] = json!(Uuid::new_v4());
                    bad["coordinator_consent"] = serde_json::to_value(
                        EventBuilder::new(Kind::Custom(47212), consent_body.to_string())
                            .sign_with_keys(&f.outsider)
                            .unwrap(),
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
            let bad = EventBuilder::new(Kind::Custom(47210), bad.to_string())
                .sign_with_keys(&f.owner)
                .unwrap();
            assert_ne!(post(&f, &bad, &f.owner).await.1["accepted"], true, "{case}");
        }
        let standalone: Event =
            serde_json::from_value(payload["coordinator_consent"].clone()).unwrap();
        assert_ne!(post(&f, &standalone, &f.outsider).await.1["accepted"], true);

        let foreign = EventBuilder::new(Kind::Custom(47210), event.content.clone())
            .sign_with_keys(&f.outsider)
            .unwrap();
        assert_ne!(post(&f, &foreign, &f.outsider).await.1["accepted"], true);
        let mut wrong = payload.clone();
        wrong["community_id"] = json!(Uuid::new_v4());
        let wrong = EventBuilder::new(Kind::Custom(47210), wrong.to_string())
            .sign_with_keys(&f.owner)
            .unwrap();
        assert_ne!(post(&f, &wrong, &f.owner).await.1["accepted"], true);
        let mut invalid = event.clone();
        invalid.content.push(' ');
        assert_ne!(post(&f, &invalid, &f.owner).await.1["accepted"], true);
        assert_eq!(
            f.request("GET", "/api/machines", &f.owner, None).await.1["machines"],
            json!([])
        );
        let materialized:bool=sqlx::query_scalar("SELECT agent_owner_pubkey IS NOT NULL OR agent_type IS NOT NULL OR machine_id IS NOT NULL FROM users WHERE community_id=$1 AND pubkey=$2").bind(f.community.as_uuid()).bind(f.outsider.public_key().to_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert!(!materialized);
        assert_eq!(post(&f, &event, &f.owner).await.1["accepted"], true);
        let delayed=EventBuilder::new(Kind::Custom(47211),json!({"version":1,"community_id":f.community.as_uuid(),"machine_id":payload["machine_id"],"registration_event_id":event.id.to_hex(),"sequence":10,"state":"ready"}).to_string()).custom_created_at(Timestamp::from(Timestamp::now().as_secs()-31)).sign_with_keys(&f.outsider).unwrap();
        assert_ne!(post(&f, &delayed, &f.outsider).await.1["accepted"], true);
        let replacement = enrollment(
            &f,
            Uuid::parse_str(payload["machine_id"].as_str().unwrap()).unwrap(),
            &Keys::generate(),
            "kind=47210",
        );
        assert_ne!(post(&f, &replacement, &f.owner).await.1["accepted"], true);
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM machine_control_events WHERE community_id=$1")
                .bind(f.community.as_uuid())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        eprintln!("PASS invalid action proof, signer, signature, tenant, delayed observation and replacement leave no partial machine materialization");
    }
    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn standalone_machine_consent_does_not_materialize_transport_owner() {
        let mut f = fixture().await.expect("owned PG/Redis");
        Arc::make_mut(&mut Arc::get_mut(&mut f.state).unwrap().config).allow_nip_oa_auth = true;
        let _server = serve(&mut f).await;
        let coordinator = Keys::generate();
        let outer = enrollment(&f, Uuid::new_v4(), &coordinator, "kind=47210");
        let document: Value = serde_json::from_str(&outer.content).unwrap();
        let body = document["coordinator_consent"].to_string();
        let proof =
            buzz_sdk::nip_oa::compute_auth_tag(&f.owner, &coordinator.public_key(), "kind=27235")
                .unwrap();
        let auth = super::super::route_authz::nip98_auth_header(
            &coordinator,
            "POST",
            &format!("https://{}/events", f.host),
            body.as_bytes(),
        );
        let response = reqwest::Client::new()
            .post(format!("{}/events", f.http_base.as_ref().unwrap()))
            .header("Host", &f.host)
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .header("x-auth-tag", proof)
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = response.json().await.unwrap();
        assert_ne!(body["accepted"], true);
        let materialized: i64 =
            sqlx::query_scalar("SELECT count(*) FROM users WHERE community_id=$1 AND pubkey=$2")
                .bind(f.community.as_uuid())
                .bind(coordinator.public_key().to_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(
            materialized, 0,
            "rejected standalone consent must not materialize the transport owner"
        );
    }
}
