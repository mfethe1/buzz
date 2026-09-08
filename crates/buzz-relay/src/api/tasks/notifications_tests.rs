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
            let event = EventBuilder::new(Kind::Authentication, "")
                .tags([
                    Tag::parse(["relay", &format!("wss://{host}")]).expect("relay tag"),
                    Tag::parse(["challenge", challenge[1].as_str().expect("challenge")])
                        .expect("challenge tag"),
                ])
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
