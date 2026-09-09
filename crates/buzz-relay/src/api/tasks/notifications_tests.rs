//! Actual signed HTTP mutations observed over NIP-42 authenticated WebSockets.

mod postgres_tests {
    use super::super::route_authz::{fixture, Fixture};
    use axum::http::StatusCode;
    use futures_util::{SinkExt, StreamExt};
    use nostr::{EventBuilder, Keys, Kind, Tag};
    use serde_json::{json, Value};
    use std::{sync::Arc, time::Duration};
    use tokio_tungstenite::{
        tungstenite::{client::IntoClientRequest, Message},
        MaybeTlsStream, WebSocketStream,
    };

    type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

    async fn next_json(socket: &mut Socket) -> Value {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match socket
                    .next()
                    .await
                    .expect("socket remains open")
                    .expect("websocket read")
                {
                    Message::Text(text) => return serde_json::from_str(&text).expect("JSON frame"),
                    Message::Ping(data) => socket.send(Message::Pong(data)).await.expect("pong"),
                    other => panic!("unexpected frame: {other:?}"),
                }
            }
        })
        .await
        .expect("bounded websocket response")
    }

    async fn connect(base: &str, host: &str, keys: Option<&Keys>) -> Socket {
        connect_with_delegation(base, host, keys, None).await
    }

    async fn connect_with_delegation(
        base: &str,
        host: &str,
        keys: Option<&Keys>,
        owner: Option<&Keys>,
    ) -> Socket {
        let mut request = base
            .replacen("http://", "ws://", 1)
            .into_client_request()
            .expect("WS request");
        request
            .headers_mut()
            .insert("Host", host.parse().expect("host"));
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("upgrade");
        let challenge = next_json(&mut socket).await;
        assert_eq!(challenge[0], "AUTH");
        if let Some(keys) = keys {
            let mut tags = vec![
                Tag::parse(["relay", &format!("wss://{host}")]).expect("relay tag"),
                Tag::parse(["challenge", challenge[1].as_str().expect("challenge")])
                    .expect("challenge tag"),
            ];
            if let Some(owner) = owner {
                let signed = buzz_sdk::nip_oa::compute_auth_tag(owner, &keys.public_key(), "")
                    .expect("signed delegation");
                tags.push(buzz_sdk::nip_oa::parse_auth_tag(&signed).expect("delegation tag"));
            }
            let event = EventBuilder::new(Kind::Authentication, "")
                .tags(tags)
                .sign_with_keys(keys)
                .expect("signed NIP-42");
            let id = event.id.to_hex();
            socket
                .send(Message::Text(json!(["AUTH", event]).to_string().into()))
                .await
                .expect("AUTH");
            let ack = next_json(&mut socket).await;
            assert_eq!(ack[0], "OK");
            assert_eq!(ack[1], id);
            assert_eq!(ack[2], true, "{ack}");
        }
        socket
    }

    async fn expect_sync(socket: &mut Socket, channel: uuid::Uuid) {
        assert_eq!(
            next_json(socket).await,
            json!(["BUZZ_TASKS_SYNC_REQUIRED", channel])
        );
    }

    async fn expect_community_sync(socket: &mut Socket) {
        assert_eq!(
            next_json(socket).await,
            json!(["BUZZ_TASKS_SYNC_REQUIRED", null])
        );
    }

    async fn expect_no_notification(socket: &mut Socket) {
        // Heartbeats are independent of task activity. Reject every application
        // frame and unexpected close while servicing the ordinary Ping/Pong flow.
        let outcome = tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                match socket.next().await {
                    Some(Ok(Message::Ping(data))) => {
                        socket.send(Message::Pong(data)).await.expect("pong");
                    }
                    frame => panic!("unauthorized or unchanged client received {frame:?}"),
                }
            }
        })
        .await;
        assert!(outcome.is_err());
    }

    struct Server(tokio::task::JoinHandle<()>);
    impl Drop for Server {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    struct Background(Vec<tokio::task::JoinHandle<()>>);
    impl Drop for Background {
        fn drop(&mut self) {
            for handle in &self.0 {
                handle.abort();
            }
        }
    }

    async fn start_conn_control(state: Arc<crate::state::AppState>) -> Background {
        start_conn_control_gated(state, None).await
    }

    async fn start_conn_control_gated(
        state: Arc<crate::state::AppState>,
        gate: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> Background {
        let mut rx = state.pubsub.subscribe_conn_control();
        let control_rx = state.pubsub.subscribe_conn_control();
        let subscriber = {
            let pubsub = state.pubsub.clone();
            tokio::spawn(async move {
                if let Some(gate) = gate {
                    gate.await.expect("subscriber startup released");
                }
                pubsub.run_conn_control_subscriber().await;
            })
        };
        // A fresh, nonexistent community makes the probe disjoint from every
        // fixture socket. Observe it on this exact subscriber, not merely the
        // Redis PUBLISH subscriber count (which can describe another relay).
        let probe_ctx = buzz_core::TenantContext::resolved(
            buzz_core::CommunityId::from_uuid(uuid::Uuid::new_v4()),
            "readiness.invalid",
        );
        let probe_community = probe_ctx.community();
        let publisher = state.pubsub.clone();
        let (ready_tx, mut ready_rx) = tokio::sync::oneshot::channel();
        let consumer = tokio::spawn(state.run_connection_control(control_rx));
        let readiness = tokio::spawn(async move {
            let mut ready_tx = Some(ready_tx);
            while let Ok(scoped) = rx.recv().await {
                if scoped.community_id == probe_community
                    && scoped.command == buzz_pubsub::conn_control::ConnControl::DisconnectCommunity
                {
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(());
                    }
                    continue;
                }
            }
        });
        // Own the handles before awaiting readiness, so timeout/unwind also
        // aborts both tasks. Probe retries never replay HTTP task mutations.
        let background = Background(vec![subscriber, consumer, readiness]);
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    result = &mut ready_rx => {
                        result.expect("readiness consumer remains alive");
                        break;
                    }
                    _ = interval.tick() => {
                        publisher.publish_conn_control(
                            &probe_ctx,
                            &buzz_pubsub::conn_control::ConnControl::DisconnectCommunity,
                        ).await.expect("publish isolated readiness probe");
                    }
                }
            }
        })
        .await
        .expect("this Redis subscriber must observe readiness within five seconds");
        background
    }

    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn connection_control_readiness_waits_for_the_actual_subscriber() {
        let f = fixture().await.expect("Postgres and Redis fixture");
        // Another healthy subscriber must not satisfy this relay's readiness.
        let other = peer_state(&f).await;
        let _other_background = start_conn_control(other).await;
        let (release, gate) = tokio::sync::oneshot::channel();
        let startup = start_conn_control_gated(f.state.clone(), Some(gate));
        tokio::pin!(startup);
        tokio::select! {
            _ = &mut startup => panic!("readiness reported before the subscriber was released"),
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
        release.send(()).expect("release subscriber startup");
        let _background = tokio::time::timeout(Duration::from_secs(5), startup)
            .await
            .expect("actual Redis readiness must be bounded");
    }

    async fn serve(f: &mut Fixture) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        f.http_base = Some(format!(
            "http://{}",
            listener.local_addr().expect("address")
        ));
        // Unlike the older route fixture, this test exercises the actual Redis replay guard.
        let state = Arc::get_mut(&mut f.state).expect("exclusive state before serve");
        state.nip98_replay = Arc::new(buzz_pubsub::RedisNip98ReplayGuard::new(
            state.redis_pool.clone(),
        ));
        let router = f.router();
        Server(tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve");
        }))
    }

    async fn peer_state(f: &Fixture) -> Arc<crate::state::AppState> {
        let config = (*f.state.config).clone();
        let redis_pool = f.state.redis_pool.clone();
        let pubsub = Arc::new(
            buzz_pubsub::PubSubManager::new(&config.redis_url, redis_pool.clone())
                .await
                .expect("peer pubsub"),
        );
        let audit = buzz_audit::AuditService::new(f.pool.clone());
        let auth = buzz_auth::AuthService::new(config.auth.clone());
        let search = buzz_search::SearchService::new(f.pool.clone());
        let workflow_engine = Arc::new(buzz_workflow::WorkflowEngine::new(
            f.state.db.clone(),
            buzz_workflow::WorkflowConfig::default(),
        ));
        let media_storage = buzz_media::MediaStorage::new(&config.media).expect("peer media");
        let (mut state, _audit_shutdown) = crate::state::AppState::new(
            config,
            f.state.db.clone(),
            redis_pool.clone(),
            audit,
            pubsub,
            auth,
            search,
            workflow_engine,
            Keys::generate(),
            media_storage,
        );
        state.nip98_replay = Arc::new(buzz_pubsub::RedisNip98ReplayGuard::new(redis_pool));
        Arc::new(state)
    }

    async fn serve_state(state: Arc<crate::state::AppState>) -> (Server, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        let router = crate::router::build_router(state);
        let server = Server(tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve");
        }));
        (server, base)
    }

    async fn request_at(
        f: &Fixture,
        base: &str,
        method: &str,
        path: &str,
        keys: &Keys,
        body: Option<&str>,
    ) -> (StatusCode, Value) {
        let body_bytes = body.map(str::as_bytes).unwrap_or_default();
        let auth = super::super::route_authz::nip98_auth_header(
            keys,
            method,
            &format!("https://{}{path}", f.host),
            body_bytes,
        );
        let response = reqwest::Client::new()
            .request(method.parse().expect("method"), format!("{base}{path}"))
            .header(axum::http::header::HOST, &f.host)
            .header(axum::http::header::AUTHORIZATION, auth)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(body_bytes.to_vec())
            .send()
            .await
            .expect("HTTP response");
        let status = response.status();
        let json = response.json().await.expect("HTTP JSON response");
        (status, json)
    }

    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn stored_agent_owner_does_not_authorize_a_revoked_direct_session() {
        let mut f = fixture().await.expect("Postgres and Redis fixture");
        let state = Arc::get_mut(&mut f.state).expect("exclusive fixture state");
        let mut config = (*state.config).clone();
        config.allow_nip_oa_auth = true;
        state.config = Arc::new(config);
        state
            .db
            .add_relay_member(
                f.community,
                &f.outsider.public_key().to_hex(),
                "member",
                None,
            )
            .await
            .expect("direct agent membership");
        assert!(state
            .db
            .set_agent_owner(
                f.community,
                &f.outsider.public_key().to_bytes(),
                &f.owner.public_key().to_bytes(),
            )
            .await
            .expect("stored owner relationship"));
        let _server = serve(&mut f).await;
        let base = f.http_base.as_deref().expect("HTTP base");
        let mut owner = connect(base, &f.host, Some(&f.owner)).await;
        // This NIP-42 session deliberately has no NIP-OA auth tag.
        let mut agent = connect(base, &f.host, Some(&f.outsider)).await;
        let (status, created) = f
            .request(
                "POST",
                "/api/tasks",
                &f.owner,
                Some(r#"{"title":"before direct access revocation"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        expect_community_sync(&mut owner).await;
        expect_community_sync(&mut agent).await;
        f.state
            .db
            .remove_relay_member(f.community, &f.outsider.public_key().to_hex())
            .await
            .expect("revoke direct agent membership");
        let (status, denied) = f.request("GET", "/api/tasks", &f.outsider, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
        let (status, created) = f
            .request(
                "POST",
                "/api/tasks",
                &f.owner,
                Some(r#"{"title":"after direct access revocation"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        expect_community_sync(&mut owner).await;
        expect_no_notification(&mut agent).await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn verified_delegation_is_session_scoped_and_owner_revocation_is_immediate() {
        let mut f = fixture().await.expect("Postgres and Redis fixture");
        let state = Arc::get_mut(&mut f.state).expect("exclusive fixture state");
        let mut config = (*state.config).clone();
        config.allow_nip_oa_auth = true;
        state.config = Arc::new(config);
        let agent = Keys::generate();
        state
            .db
            .add_relay_member(f.community, &agent.public_key().to_hex(), "member", None)
            .await
            .expect("direct agent membership");
        let _server = serve(&mut f).await;
        let base = f.http_base.as_deref().expect("HTTP base");
        let mut direct = connect(base, &f.host, Some(&agent)).await;
        f.state
            .db
            .remove_relay_member(f.community, &agent.public_key().to_hex())
            .await
            .expect("revoke direct membership");
        // Same key, separate live connection, and an actual owner-signed auth tag.
        let mut delegated =
            connect_with_delegation(base, &f.host, Some(&agent), Some(&f.outsider)).await;
        let mut publisher = connect(base, &f.host, Some(&f.owner)).await;
        let (status, created) = f
            .request(
                "POST",
                "/api/tasks",
                &f.owner,
                Some(r#"{"title":"verified delegation receives update"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        expect_community_sync(&mut publisher).await;
        expect_community_sync(&mut delegated).await;
        expect_no_notification(&mut direct).await;

        f.state
            .db
            .remove_relay_member(f.community, &f.outsider.public_key().to_hex())
            .await
            .expect("revoke delegation owner membership");
        let (status, created) = f
            .request(
                "POST",
                "/api/tasks",
                &f.owner,
                Some(r#"{"title":"revoked owner cannot receive update"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        expect_community_sync(&mut publisher).await;
        expect_no_notification(&mut delegated).await;
        expect_no_notification(&mut direct).await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn community_tasks_and_channel_tasks_refresh_another_relay_instance() {
        let mut f = fixture().await.expect("Postgres and Redis fixture");
        let other = fixture().await.expect("second isolated community");
        let _server_a = serve(&mut f).await;
        let peer = peer_state(&f).await;
        let _peer_conn_control = start_conn_control(peer.clone()).await;
        let (_server_b, base_b) = serve_state(peer).await;

        let mut owner = connect(&base_b, &f.host, Some(&f.owner)).await;
        let mut outsider = connect(&base_b, &f.host, Some(&f.outsider)).await;
        let mut unauthenticated = connect(&base_b, &f.host, None).await;
        let mut foreign = connect(&base_b, &other.host, Some(&other.owner)).await;

        let community_body = json!({"title":"community task from relay A"}).to_string();
        let (status, created) = f
            .request("POST", "/api/tasks", &f.owner, Some(&community_body))
            .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        expect_community_sync(&mut owner).await;
        expect_community_sync(&mut outsider).await;
        expect_no_notification(&mut unauthenticated).await;
        expect_no_notification(&mut foreign).await;
        let community_path = format!(
            "/api/tasks/{}",
            created["id"].as_str().expect("community task id")
        );
        let (status, detail) =
            request_at(&f, &base_b, "GET", &community_path, &f.owner, None).await;
        assert_eq!(status, StatusCode::OK, "{detail}");
        assert_eq!(detail["task"]["title"], "community task from relay A");

        let (status, updated) = f
            .request(
                "PATCH",
                &community_path,
                &f.owner,
                Some(r#"{"priority":4,"expected_revision":0}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        expect_community_sync(&mut owner).await;
        expect_community_sync(&mut outsider).await;
        let community_event_path = format!("{community_path}/events");
        let (status, event) = f
            .request(
                "POST",
                &community_event_path,
                &f.owner,
                Some(r#"{"body":"community event from relay A"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{event}");
        expect_community_sync(&mut owner).await;
        expect_community_sync(&mut outsider).await;
        let (status, detail) =
            request_at(&f, &base_b, "GET", &community_path, &f.owner, None).await;
        assert_eq!(status, StatusCode::OK, "{detail}");
        assert_eq!(detail["task"]["revision"], 1);
        assert!(detail["events"].as_array().is_some_and(|events| events
            .iter()
            .any(|event| event["body"] == "community event from relay A")));

        let channel_body = json!({
            "title":"channel task from relay A",
            "channel_id":f.private_channel_id,
        })
        .to_string();
        let (status, channel_task) = f
            .request("POST", "/api/tasks", &f.owner, Some(&channel_body))
            .await;
        assert_eq!(status, StatusCode::OK, "{channel_task}");
        expect_sync(&mut owner, f.private_channel_id).await;
        expect_no_notification(&mut outsider).await;

        f.state
            .db
            .add_member(
                f.community,
                f.private_channel_id,
                &f.outsider.public_key().to_bytes(),
                buzz_core::channel::MemberRole::Member,
                Some(&f.owner.public_key().to_bytes()),
            )
            .await
            .expect("grant private-channel membership");
        let channel_path = format!(
            "/api/tasks/{}",
            channel_task["id"].as_str().expect("channel task id")
        );
        let (status, updated) = f
            .request(
                "PATCH",
                &channel_path,
                &f.owner,
                Some(r#"{"priority":2,"expected_revision":0}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        expect_sync(&mut owner, f.private_channel_id).await;
        expect_sync(&mut outsider, f.private_channel_id).await;

        f.state
            .db
            .remove_member(
                f.community,
                f.private_channel_id,
                &f.outsider.public_key().to_bytes(),
                &f.owner.public_key().to_bytes(),
            )
            .await
            .expect("revoke private-channel membership");
        let channel_event_path = format!("{channel_path}/events");
        let (status, event) = f
            .request(
                "POST",
                &channel_event_path,
                &f.owner,
                Some(r#"{"body":"private event after revocation"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{event}");
        expect_sync(&mut owner, f.private_channel_id).await;
        expect_no_notification(&mut outsider).await;

        f.state
            .db
            .remove_relay_member(f.community, &f.outsider.public_key().to_hex())
            .await
            .expect("revoke relay membership");
        let (status, updated) = f
            .request(
                "PATCH",
                &community_path,
                &f.owner,
                Some(r#"{"priority":5,"expected_revision":1}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        expect_community_sync(&mut owner).await;
        expect_no_notification(&mut outsider).await;

        let tenant = buzz_core::TenantContext::resolved(f.community, &f.host);
        let duplicate = buzz_pubsub::conn_control::ConnControl::InvalidateTasks {
            channel_id: None,
            origin_generation: f.state.task_invalidation_generation,
        };
        f.state
            .pubsub
            .publish_conn_control(&tenant, &duplicate)
            .await
            .expect("first duplicate advisory");
        f.state
            .pubsub
            .publish_conn_control(&tenant, &duplicate)
            .await
            .expect("second duplicate advisory");
        expect_community_sync(&mut owner).await;
        expect_community_sync(&mut owner).await;
        expect_no_notification(&mut outsider).await;
        expect_no_notification(&mut unauthenticated).await;
        expect_no_notification(&mut foreign).await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn committed_task_http_mutations_notify_only_authorized_websockets() {
        let mut f = fixture().await.expect("Postgres and Redis fixture");
        let other = fixture().await.expect("second isolated community");
        let _server = serve(&mut f).await;
        let base = f.http_base.as_deref().expect("server base");
        let mut owner = connect(base, &f.host, Some(&f.owner)).await;
        let mut outsider = connect(base, &f.host, Some(&f.outsider)).await;
        let mut unauthenticated = connect(base, &f.host, None).await;
        let mut foreign = connect(base, &other.host, Some(&other.owner)).await;
        // A stale cached allow must not become a task-existence oracle.
        f.state.accessible_channels_cache.insert(
            (f.community, f.outsider.public_key().to_bytes().to_vec()),
            vec![f.private_channel_id],
        );

        let path = format!("/api/tasks/{}", f.task_id);
        // Hold the task UPDATE inside its transaction. Notification before the
        // commit would disclose an event while its row is still invisible.
        let gate = (uuid::Uuid::new_v4().as_u128() & i64::MAX as u128) as i64;
        let mut lock = f.pool.acquire().await.expect("gate connection");
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(gate)
            .execute(&mut *lock)
            .await
            .expect("hold gate");
        let ddl = format!(
            "CREATE FUNCTION task_notification_commit_gate() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({gate}); RETURN NEW; END $$; CREATE TRIGGER task_notification_commit_gate AFTER UPDATE ON tasks FOR EACH ROW EXECUTE FUNCTION task_notification_commit_gate()"
        );
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&f.pool)
            .await
            .expect("install gate");
        let (response, ()) = tokio::join!(
            f.request(
                "PATCH",
                &path,
                &f.owner,
                Some(r#"{"priority":7,"expected_revision":0}"#)
            ),
            async {
                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND wait_event = 'advisory')")
                            .fetch_one(&f.pool).await.expect("observe commit gate");
                        if waiting { break; }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }).await.expect("UPDATE reaches gate before any invalidation");
                expect_no_notification(&mut owner).await;
                let before = f
                    .state
                    .db
                    .get_task(f.community, f.task_id)
                    .await
                    .expect("old committed row");
                assert_eq!(before.revision, 0);
                sqlx::query("SELECT pg_advisory_unlock($1)")
                    .bind(gate)
                    .execute(&mut *lock)
                    .await
                    .expect("release commit");
                expect_sync(&mut owner, f.private_channel_id).await;
                let persisted = f
                    .state
                    .db
                    .get_task(f.community, f.task_id)
                    .await
                    .expect("committed row");
                assert_eq!((persisted.revision, persisted.priority), (1, 7));
            }
        );
        let (status, updated) = response;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["revision"], 1);
        sqlx::raw_sql("DROP TRIGGER task_notification_commit_gate ON tasks; DROP FUNCTION task_notification_commit_gate()")
            .execute(&f.pool).await.expect("remove gate");
        for socket in [&mut outsider, &mut unauthenticated, &mut foreign] {
            expect_no_notification(socket).await;
        }

        let (status, _) = f
            .request(
                "PATCH",
                &path,
                &f.owner,
                Some(r#"{"priority":9,"expected_revision":0}"#),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        expect_no_notification(&mut owner).await;
        let (status, _) = f
            .request(
                "PATCH",
                &path,
                &f.owner,
                Some(r#"{"priority":7,"expected_revision":1}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        expect_no_notification(&mut owner).await;

        let body =
            json!({"title":"created after commit", "channel_id":f.private_channel_id}).to_string();
        let (status, created) = f.request("POST", "/api/tasks", &f.owner, Some(&body)).await;
        assert_eq!(status, StatusCode::OK, "{created}");
        expect_sync(&mut owner, f.private_channel_id).await;
        let event_path = format!("{path}/events");
        let (status, event) = f
            .request(
                "POST",
                &event_path,
                &f.owner,
                Some(r#"{"body":"committed comment"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{event}");
        expect_sync(&mut owner, f.private_channel_id).await;
        let events = f
            .state
            .db
            .list_task_events(f.community, f.task_id)
            .await
            .expect("durable events");
        assert!(events
            .iter()
            .any(|e| e.body.as_deref() == Some("committed comment")));
        for socket in [&mut outsider, &mut unauthenticated, &mut foreign] {
            expect_no_notification(socket).await;
        }

        // A disconnected client recovers from an authorized GET after NIP-42 reconnect.
        owner.close(None).await.expect("close owner");
        drop(owner);
        let (status, _) = f
            .request(
                "PATCH",
                &path,
                &f.owner,
                Some(r#"{"priority":8,"expected_revision":1}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let mut reconnected = connect(base, &f.host, Some(&f.owner)).await;
        let (status, recovered) = f.request("GET", &path, &f.owner, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(recovered["task"]["revision"], 2);
        assert_eq!(recovered["task"]["priority"], 8);
        reconnected.close(None).await.expect("close");
    }
    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn task_control_queue_coalesces_duplicates_and_recovers_scoped_overflow() {
        let mut f = fixture().await.expect("Postgres and Redis fixture");
        let other = fixture().await.expect("foreign community");
        let _server = serve(&mut f).await;
        let base = f.http_base.as_deref().expect("base");
        let mut owner = connect(base, &f.host, Some(&f.owner)).await;
        let mut foreign = connect(base, &other.host, Some(&other.owner)).await;
        let mut lock = f.pool.begin().await.expect("gate transaction");
        let gate_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .expect("gate pid");
        sqlx::query("LOCK TABLE channel_members IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *lock)
            .await
            .expect("hold authorization");
        let (tx, rx) = tokio::sync::broadcast::channel(1024);
        let _consumer = Server(tokio::spawn(f.state.clone().run_connection_control(rx)));
        let command = |channel| buzz_pubsub::conn_control::ScopedConnControl {
            community_id: f.community,
            command: buzz_pubsub::conn_control::ConnControl::InvalidateTasks {
                channel_id: Some(channel),
                origin_generation: uuid::Uuid::new_v4(),
            },
        };
        tx.send(command(f.private_channel_id))
            .expect("active advisory");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))")
                    .bind(gate_pid).fetch_one(&f.pool).await.expect("observe blocked authorization");
                if waiting { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("actual fanout is held by database lock");
        for _ in 0..300 {
            tx.send(command(f.private_channel_id))
                .expect("duplicate advisory");
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while !tx.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("production consumer consumed duplicate burst");
        expect_no_notification(&mut owner).await;
        for _ in 0..257 {
            tx.send(command(uuid::Uuid::new_v4()))
                .expect("distinct scope advisory");
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match owner.next().await {
                    None | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(Message::Ping(data))) => {
                        owner.send(Message::Pong(data)).await.expect("pong")
                    }
                    frame => panic!("unexpected overflow recovery frame: {frame:?}"),
                }
            }
        })
        .await
        .expect("bounded task queue overflow forces affected socket recovery");
        expect_no_notification(&mut foreign).await;
        lock.rollback().await.expect("release authorization gate");
    }

    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn lost_control_commands_force_socket_reauthorization() {
        let mut f = fixture().await.expect("Postgres and Redis fixture");
        let _server = serve(&mut f).await;
        let mut owner = connect(
            f.http_base.as_deref().expect("base"),
            &f.host,
            Some(&f.owner),
        )
        .await;
        // A deliberately undersized receiver deterministically loses a control
        // command before the actual production consumer begins reading.
        let (tx, rx) = tokio::sync::broadcast::channel(2);
        for _ in 0..3 {
            tx.send(buzz_pubsub::conn_control::ScopedConnControl {
                community_id: f.community,
                command: buzz_pubsub::conn_control::ConnControl::InvalidateTasks {
                    channel_id: None,
                    origin_generation: f.state.task_invalidation_generation,
                },
            })
            .expect("receiver retained");
        }
        let consumer = Server(tokio::spawn(f.state.clone().run_connection_control(rx)));
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match owner.next().await {
                    None | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(Message::Ping(data))) => {
                        owner.send(Message::Pong(data)).await.expect("pong")
                    }
                    frame => panic!("unexpected recovery frame: {frame:?}"),
                }
            }
        })
        .await
        .expect("loss of control commands must force fresh authorization");
        drop(consumer);
        drop(tx);
    }

    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn urgent_disconnect_does_not_wait_for_task_authorization() {
        let mut f = fixture().await.expect("Postgres and Redis fixture");
        let _server = serve(&mut f).await;
        let _control = start_conn_control(f.state.clone()).await;
        let mut owner = connect(
            f.http_base.as_deref().expect("base"),
            &f.host,
            Some(&f.owner),
        )
        .await;
        let mut lock = f.pool.begin().await.expect("gate transaction");
        let gate_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .expect("gate pid");
        sqlx::query("LOCK TABLE channel_members IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *lock)
            .await
            .expect("hold advisory authorization");
        let ctx = buzz_core::TenantContext::resolved(f.community, &f.host);
        f.state
            .pubsub
            .publish_conn_control(
                &ctx,
                &buzz_pubsub::conn_control::ConnControl::InvalidateTasks {
                    channel_id: Some(f.private_channel_id),
                    origin_generation: uuid::Uuid::new_v4(),
                },
            )
            .await
            .expect("publish task advisory through actual Redis");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))")
                    .bind(gate_pid).fetch_one(&f.pool).await.expect("observe blocked permission query");
                if waiting { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("production consumer entered blocked authorization");
        f.state
            .pubsub
            .publish_conn_control(
                &ctx,
                &buzz_pubsub::conn_control::ConnControl::DisconnectCommunity,
            )
            .await
            .expect("publish urgent disconnect through actual Redis");
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match owner.next().await {
                    None | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(Message::Ping(data))) => {
                        owner.send(Message::Pong(data)).await.expect("pong")
                    }
                    frame => panic!("unexpected frame while awaiting disconnect: {frame:?}"),
                }
            }
        })
        .await
        .expect("urgent disconnect must not await the five-second task permission deadline");
        lock.rollback().await.expect("release authorization gate");
    }

    #[tokio::test]
    #[ignore = "requires Postgres and Redis"]
    async fn task_notification_access_deadline_preserves_committed_http_success() {
        let mut f = fixture().await.expect("Postgres and Redis fixture");
        let _server = serve(&mut f).await;
        let mut owner = connect(
            f.http_base.as_deref().expect("base"),
            &f.host,
            Some(&f.owner),
        )
        .await;
        // The existing HTTP gate has a valid cached allow; only the new fresh
        // notification lookup is held behind this real database table lock.
        f.state.accessible_channels_cache.insert(
            (f.community, f.owner.public_key().to_bytes().to_vec()),
            vec![f.private_channel_id],
        );
        let mut lock = f.pool.begin().await.expect("gate transaction");
        sqlx::query("LOCK TABLE channel_members IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *lock)
            .await
            .expect("block notification authorization");
        let start = std::time::Instant::now();
        let (status, updated) = tokio::time::timeout(
            Duration::from_secs(8),
            f.request(
                "PATCH",
                &format!("/api/tasks/{}", f.task_id),
                &f.owner,
                Some(r#"{"priority":11,"expected_revision":0}"#),
            ),
        )
        .await
        .expect("notification deadline bounds the successful HTTP response");
        assert!(
            start.elapsed() >= Duration::from_secs(5),
            "actual access lookup reached its deadline"
        );
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["revision"], 1);
        lock.rollback().await.expect("release authorization gate");
        let persisted = f
            .state
            .db
            .get_task(f.community, f.task_id)
            .await
            .expect("committed task");
        assert_eq!((persisted.revision, persisted.priority), (1, 11));
        expect_no_notification(&mut owner).await;
        owner.close(None).await.expect("close");
    }
}
