use super::*;
use axum::http::Uri;

#[test]
fn request_path_preserves_signed_query_verbatim() {
    assert_eq!(
        request_path("/api/tasks", Some("status=todo&limit=10")),
        "/api/tasks?status=todo&limit=10"
    );
    assert_eq!(request_path("/api/tasks", None), "/api/tasks");
    assert_eq!(request_path("/api/tasks", Some("")), "/api/tasks");
}

#[test]
fn source_ref_survives_verbatim_into_the_signed_path() {
    // The client signs the raw query, so the relay must reconstruct it
    // byte-for-byte. A normalised or re-ordered `source_ref` would break
    // the NIP-98 signature rather than merely filter differently.
    let raw = "channel=6f1b0e2c-0000-4000-8000-000000000001&source_ref=abc123";
    assert_eq!(
        request_path("/api/tasks", Some(raw)),
        format!("/api/tasks?{raw}")
    );
}

#[test]
fn source_ref_is_parsed_as_an_opaque_optional_string() {
    // Opaque TEXT by design (migrations/0046_task_system.sql): the relay
    // must not validate it as an event id, and its absence must stay
    // distinct from a present value.
    fn parse(query: &str) -> TasksQuery {
        let uri: Uri = format!("http://relay.invalid/api/tasks?{query}")
            .parse()
            .expect("valid uri");
        Query::<TasksQuery>::try_from_uri(&uri).expect("parses").0
    }

    assert_eq!(parse("status=todo").source_ref, None);
    assert_eq!(
        parse("source_ref=not-an-event-id").source_ref.as_deref(),
        Some("not-an-event-id")
    );
}

#[test]
fn title_length_is_counted_in_characters_not_bytes() {
    // 200 multi-byte characters is 600 bytes but a legal title; counting
    // bytes here would 400 a request the database would have accepted.
    let multibyte = "é".repeat(200);
    assert_eq!(
        validate_title(&multibyte).expect("200 chars is legal"),
        multibyte
    );
    assert!(validate_title(&"é".repeat(201)).is_err());
}

#[test]
fn title_is_trimmed_and_must_not_be_blank() {
    assert_eq!(validate_title("  ship it  ").expect("trims"), "ship it");
    assert!(validate_title("   ").is_err());
    assert!(validate_title("").is_err());
}

#[test]
fn assignee_must_be_a_32_byte_hex_pubkey() {
    let valid = "ab".repeat(32);
    assert_eq!(
        parse_pubkey("assignee", &valid).expect("valid"),
        vec![0xab; 32]
    );
    assert!(parse_pubkey("assignee", "not-hex").is_err());
    assert!(parse_pubkey("assignee", &"ab".repeat(31)).is_err());
    assert!(parse_pubkey("assignee", &"ab".repeat(33)).is_err());
}

#[test]
fn absent_and_null_assignee_are_different_patches() {
    // The whole point of the double option: `{}` leaves the assignee
    // alone, `{"assignee": null}` unassigns.
    let absent: UpdateTaskRequest = serde_json::from_str("{}").expect("absent");
    assert_eq!(absent.assignee, None);

    let cleared: UpdateTaskRequest = serde_json::from_str(r#"{"assignee": null}"#).expect("null");
    assert_eq!(cleared.assignee, Some(None));

    let set: UpdateTaskRequest = serde_json::from_str(r#"{"assignee": "abc"}"#).expect("set");
    assert_eq!(set.assignee, Some(Some("abc".to_owned())));
}

#[test]
fn absent_and_null_due_at_are_different_patches() {
    let absent: UpdateTaskRequest = serde_json::from_str("{}").expect("absent");
    assert_eq!(absent.due_at, None);

    let cleared: UpdateTaskRequest = serde_json::from_str(r#"{"due_at": null}"#).expect("null");
    assert_eq!(cleared.due_at, Some(None));
}

#[test]
fn task_wire_renders_status_and_hex_pubkeys() {
    let task = TaskRecord {
        id: Uuid::nil(),
        channel_id: None,
        created_by_pubkey: Some(vec![0xab; 32]),
        assignee_pubkey: None,
        parent_task_id: None,
        title: "ship it".to_owned(),
        body: None,
        status: TaskStatus::InProgress,
        priority: 3,
        source: Some("claude".to_owned()),
        source_ref: None,
        due_at: None,
        done_at: None,
        archived_at: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        revision: 0,
    };
    let wire = task_json(&task);
    assert_eq!(wire["status"], "in_progress");
    assert_eq!(wire["created_by"], hex::encode([0xab; 32]));
    assert!(wire["assignee"].is_null());
    assert_eq!(wire["priority"], 3);
    // Raw bytes must never reach the wire.
    assert!(wire.get("created_by_pubkey").is_none());
}

#[test]
fn task_event_wire_renders_both_status_ends() {
    let event = TaskEventRecord {
        id: 7,
        task_id: Uuid::nil(),
        actor_pubkey: None,
        action: TaskAction::StatusChanged,
        changes: None,
        from_status: Some(TaskStatus::Todo),
        to_status: Some(TaskStatus::Done),
        body: None,
        created_at: Utc::now(),
    };
    let wire = task_event_json(&event);
    assert_eq!(wire["action"], "status_changed");
    assert_eq!(wire["from_status"], "todo");
    assert_eq!(wire["to_status"], "done");
}

/// Route-level private-channel authorization (COMPAT LANE 3, §7 closure).
///
/// Drives the REAL router (`build_router` + `oneshot`) with REAL NIP-98
/// auth headers against a REAL Postgres community containing a private
/// channel and a channel-bound task. Proves at the route seam — not the
/// db seam — that a relay member who is NOT a channel member:
///   * gets 404 (never 403, never the task) on GET/PATCH/POST-events,
///   * gets the task silently filtered out of a channel list, and
///   * cannot even create a task bound to the private channel.
///
/// The relay-membership gate is exercised with `require_relay_membership
/// = true` so the 404s below are authz verdicts, not gate bypasses.
///
/// Postgres + Redis are required: run with
/// `cargo test -p buzz-relay --lib api::tasks -- --ignored`.
mod route_authz {
    use super::super::*;
    use crate::state::AppState;
    use buzz_core::channel::{ChannelType, ChannelVisibility};
    use buzz_db::task::NewTask;
    use nostr::Keys;
    use sha2::{Digest, Sha256};

    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;

    const TEST_DB_URL: &str = "postgres://buzz:***@localhost:5432/buzz"; // sadscan:disable np.postgres.1

    /// Same trick as the invites tests: the shared AlwaysFreshReplayGuard
    /// is gated behind buzz-auth/test-utils, which this crate doesn't
    /// enable, so define the pass-through locally.
    struct AlwaysFreshReplayGuard;

    impl buzz_auth::Nip98ReplayGuard for AlwaysFreshReplayGuard {
        fn try_mark_in_scope<'a>(
            &'a self,
            _scope: &'a str,
            _event_id: &'a nostr::EventId,
            _ttl_secs: u64,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<bool, buzz_auth::AuthError>> + Send + 'a>,
        > {
            Box::pin(async { Ok(true) })
        }
    }

    /// Clone of the invites.rs NIP-98 helper: signs kind:27235 over the
    /// exact URL the relay will reconstruct (scheme from config.relay_url,
    /// host from the tenant, path + raw query).
    fn nip98_auth_header(keys: &Keys, method: &str, url: &str, body: &[u8]) -> String {
        let hash: [u8; 32] = Sha256::digest(body).into();
        let tags = vec![
            nostr::Tag::parse(["u", url]).expect("u tag"),
            nostr::Tag::parse(["method", method]).expect("method tag"),
            nostr::Tag::parse(["payload", hex::encode(hash).as_str()]).expect("payload tag"),
        ];
        let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign NIP-98 event");
        let event_json = serde_json::to_string(&event).expect("serialize NIP-98 event");
        let encoded =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, event_json);
        format!("Nostr {encoded}")
    }

    #[allow(dead_code)] // AGENT-HOMES-001: shared fixture; fields used by sibling test mods
    pub(super) struct Fixture {
        pub(super) state: Arc<AppState>,
        #[allow(dead_code)]
        pub(super) pool: sqlx::PgPool,
        pub(super) host: String,
        pub(super) community: buzz_core::CommunityId,
        pub(super) private_channel_id: Uuid,
        pub(super) task_id: Uuid,
        pub(super) owner: Keys,
        pub(super) outsider: Keys,
        pub(super) http_base: Option<String>,
    }

    /// Boot an AppState bound to a fresh community whose Postgres + Redis
    /// are live. Redis must be real: the HTTP admission gate fails closed
    /// (503) when the shared limiter is unavailable, which would mask the
    /// authorization verdict under test.
    pub(super) async fn fixture() -> Option<Fixture> {
        let host = format!("task-authz-{}.example", Uuid::new_v4().simple());
        let database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| TEST_DB_URL.to_string());
        let redis_url = std::env::var("BUZZ_TEST_REDIS_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

        let mut config = crate::config::Config::from_env().ok()?;
        config.database_url = database_url.clone();
        config.redis_url = redis_url.clone();
        config.relay_url = format!("wss://{host}");
        config.require_relay_membership = true;
        config.require_auth_token = false;

        let pool = sqlx::PgPool::connect(&database_url).await.ok()?;
        let db = buzz_db::Db::from_pool(pool.clone());
        let ensured = db.ensure_configured_community(&host).await.ok()?;

        // Live Redis pool for admission + pubsub, mirroring invite tests.
        let redis_pool = deadpool_redis::Config::from_url(&redis_url)
            .create_pool(Some(deadpool_redis::Runtime::Tokio1))
            .ok()?;
        let pubsub = Arc::new(
            buzz_pubsub::PubSubManager::new(&redis_url, redis_pool.clone())
                .await
                .ok()?,
        );

        let owner = Keys::generate();
        let outsider = Keys::generate();
        let owner_pk = owner.public_key().to_bytes().to_vec();
        let outsider_pk = outsider.public_key().to_bytes().to_vec();

        // Both are relay members (the outer gate) so every response below
        // isolates the CHANNEL gate, not relay membership.
        buzz_db::user::ensure_user(&pool, ensured.id, &owner_pk)
            .await
            .ok()?;
        buzz_db::user::ensure_user(&pool, ensured.id, &outsider_pk)
            .await
            .ok()?;
        db.add_relay_member(ensured.id, &owner.public_key().to_hex(), "member", None)
            .await
            .ok()?;
        db.add_relay_member(ensured.id, &outsider.public_key().to_hex(), "member", None)
            .await
            .ok()?;

        // Private channel owned by `owner` — outsider is not a member.
        let channel = buzz_db::channel::create_channel(
            &pool,
            ensured.id,
            "task-authz-private",
            ChannelType::Stream,
            ChannelVisibility::Private,
            None,
            &owner_pk,
            None,
        )
        .await
        .ok()?;

        // A task bound to that private channel.
        let task = db
            .create_task(
                ensured.id,
                NewTask {
                    channel_id: Some(channel.id),
                    created_by_pubkey: Some(owner_pk.clone()),
                    title: "route authz probe".to_owned(),
                    ..NewTask::default()
                },
            )
            .await
            .ok()?;

        let audit = buzz_audit::AuditService::new(pool.clone());
        let auth = buzz_auth::AuthService::new(config.auth.clone());
        let search = buzz_search::SearchService::new(pool.clone());
        let workflow_engine = Arc::new(buzz_workflow::WorkflowEngine::new(
            db.clone(),
            buzz_workflow::WorkflowConfig::default(),
        ));
        let media_storage = buzz_media::MediaStorage::new(&config.media).ok()?;
        let (mut state, _audit_shutdown) = AppState::new(
            config,
            db,
            redis_pool,
            audit,
            pubsub,
            auth,
            search,
            workflow_engine,
            Keys::generate(),
            media_storage,
        );
        state.nip98_replay = Arc::new(AlwaysFreshReplayGuard);
        let state = Arc::new(state);

        Some(Fixture {
            state,
            pool,
            host,
            community: ensured.id,
            private_channel_id: channel.id,
            task_id: task.id,
            owner,
            outsider,
            http_base: None,
        })
    }

    #[allow(dead_code)] // AGENT-HOMES-001: retained for future integration tests
    async fn cleanup(f: &Fixture) {
        for table in ["task_events", "tasks", "channel_members", "channels"] {
            let sql = format!("DELETE FROM {table} WHERE community_id = $1");
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(f.community.as_uuid())
                .execute(&f.pool)
                .await
                .expect("cleanup channel/task rows");
        }
        let _ = f
            .state
            .db
            .remove_relay_member(f.community, &f.outsider.public_key().to_hex())
            .await;
        let _ = f
            .state
            .db
            .remove_relay_member(f.community, &f.owner.public_key().to_hex())
            .await;
        sqlx::query("DELETE FROM users WHERE community_id = $1")
            .bind(f.community.as_uuid())
            .execute(&f.pool)
            .await
            .expect("cleanup users");
        sqlx::query("DELETE FROM communities WHERE id = $1")
            .bind(f.community.as_uuid())
            .execute(&f.pool)
            .await
            .expect("cleanup community");
    }

    impl Fixture {
        pub(super) fn router(&self) -> axum::Router {
            crate::router::build_router(self.state.clone())
        }

        pub(super) async fn request(
            &self,
            method: &str,
            path_and_query: &str,
            keys: &Keys,
            body: Option<&str>,
        ) -> (StatusCode, serde_json::Value) {
            let url = format!("https://{}{}", self.host, path_and_query);
            let body_bytes = body.map(str::as_bytes).unwrap_or_default();
            let auth = nip98_auth_header(keys, method, &url, body_bytes);
            if let Some(base) = &self.http_base {
                let client = reqwest::Client::new();
                let response = client
                    .request(
                        method.parse().expect("method"),
                        format!("{base}{path_and_query}"),
                    )
                    .header(header::HOST, &self.host)
                    .header(header::AUTHORIZATION, auth)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body_bytes.to_vec())
                    .send()
                    .await
                    .expect("HTTP response");
                let status = response.status();
                let json = response.json().await.expect("HTTP JSON response");
                return (status, json);
            }
            let mut builder = Request::builder()
                .method(method)
                .uri(path_and_query)
                .header(header::HOST, &self.host)
                .header(header::AUTHORIZATION, auth);
            if body.is_some() {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
            }
            let response = crate::router::build_router(self.state.clone())
                .oneshot(
                    builder
                        .body(Body::from(body_bytes.to_vec()))
                        .expect("request"),
                )
                .await
                .expect("response");
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("read body");
            let json: serde_json::Value = if bytes.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
            };
            (status, json)
        }
    }

    pub(super) async fn pagination_and_history_assertions(f: &Fixture) {
        let mut ids = Vec::new();
        // Insert equal subsecond timestamps directly. UPDATE timestamps are
        // derived by the revision trigger and cannot serve as a fixture setter.
        for title in ["visible one", "visible two", "visible three"] {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO tasks (community_id, id, title, updated_at) VALUES ($1, $2, $3, '2026-01-01T00:00:00.123456Z')")
                .bind(f.community.as_uuid()).bind(id).bind(title)
                .execute(&f.pool).await.expect("visible fixture task");
            sqlx::query("INSERT INTO task_events (community_id, task_id, action) VALUES ($1, $2, 'created')")
                .bind(f.community.as_uuid()).bind(id).execute(&f.pool).await.expect("fixture creation history");
            ids.push(id.to_string());
        }
        ids.sort_by(|a, b| b.cmp(a));
        let (status, first) = f
            .request("GET", "/api/tasks?limit=2", &f.outsider, None)
            .await;
        assert_eq!(status, StatusCode::OK, "first page: {first}");
        let rows = first["tasks"].as_array().expect("tasks array");
        assert_eq!(
            rows.len(),
            2,
            "invisible newer tasks must not consume the limit"
        );
        assert_eq!(rows[0]["id"], ids[0]);
        assert_eq!(rows[1]["id"], ids[1]);
        let cursor = first["next_cursor"].as_str().expect("next cursor");
        let path = format!("/api/tasks?limit=2&before={cursor}");
        let (status, second) = f.request("GET", &path, &f.outsider, None).await;
        assert_eq!(status, StatusCode::OK, "second page: {second}");
        assert_eq!(second["tasks"].as_array().expect("second tasks").len(), 1);
        assert_eq!(second["tasks"][0]["id"], ids[2]);
        assert!(second["next_cursor"].is_null());
        let (status, _) = f
            .request("GET", "/api/tasks?before=invalid", &f.owner, None)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let task_path = format!("/api/tasks/{}", ids[0]);
        let payload = serde_json::json!({
            "assignee": f.owner.public_key().to_hex(), "priority": 4,
            "due_at": "2026-09-10T11:12:13.123456Z",
        })
        .to_string();
        let (status, updated) = f
            .request("PATCH", &task_path, &f.owner, Some(&payload))
            .await;
        assert_eq!(status, StatusCode::OK, "patch: {updated}");
        assert_eq!(updated["priority"], 4);
        let (status, detail) = f.request("GET", &task_path, &f.owner, None).await;
        assert_eq!(status, StatusCode::OK, "detail: {detail}");
        let events = detail["events"].as_array().expect("event history");
        assert_eq!(events.len(), 4);
        for (action, expected) in [
            (
                "assigned",
                serde_json::json!({"assignee": {"from": null, "to": f.owner.public_key().to_hex()}}),
            ),
            (
                "priority_changed",
                serde_json::json!({"priority": {"from": 0, "to": 4}}),
            ),
            (
                "due_at_changed",
                serde_json::json!({"due_at": {"from": null, "to": "2026-09-10T11:12:13.123456Z"}}),
            ),
        ] {
            let event = events
                .iter()
                .find(|event| event["action"] == action)
                .expect("change event");
            assert_eq!(event["changes"], expected);
            assert_eq!(event["actor"], f.owner.public_key().to_hex());
        }
        let (status, _) = f
            .request("PATCH", &task_path, &f.owner, Some(&payload))
            .await;
        assert_eq!(status, StatusCode::OK);
        let (_, retry) = f.request("GET", &task_path, &f.owner, None).await;
        assert_eq!(
            retry["events"], detail["events"],
            "signed retry must not append duplicate history"
        );
        // Exercise actual signed creation and guarded updates over this HTTP
        // listener in addition to the deliberately tied pagination fixtures.
        let (status, created) = f
            .request(
                "POST",
                "/api/tasks",
                &f.owner,
                Some(r#"{"title":"guarded task"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "create: {created}");
        assert_eq!(created["revision"], 0);
        let guarded_path = format!("/api/tasks/{}", created["id"].as_str().expect("id"));
        let (status, changed) = f
            .request(
                "PATCH",
                &guarded_path,
                &f.owner,
                Some(r#"{"priority":7,"expected_revision":0}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "guarded change: {changed}");
        assert_eq!(changed["revision"], 1);
        let (_, before_conflict) = f.request("GET", &guarded_path, &f.owner, None).await;
        assert_eq!(
            before_conflict["events"].as_array().expect("history").len(),
            2
        );
        let (status, conflict) = f
            .request(
                "PATCH",
                &guarded_path,
                &f.owner,
                Some(r#"{"priority":9,"expected_revision":0}"#),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "stale update: {conflict}");
        assert!(conflict["error"]
            .as_str()
            .expect("error")
            .contains("actual 1"));
        let (_, after_conflict) = f.request("GET", &guarded_path, &f.owner, None).await;
        assert_eq!(
            after_conflict, before_conflict,
            "a 409 cannot mutate task or history"
        );
        let (status, no_op) = f
            .request(
                "PATCH",
                &guarded_path,
                &f.owner,
                Some(r#"{"priority":7,"expected_revision":1}"#),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "no-op: {no_op}");
        assert_eq!(no_op, changed);
        let (status, _) = f
            .request(
                "PATCH",
                &guarded_path,
                &f.owner,
                Some(r#"{"expected_revision":1}"#),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        eprintln!("PASS signed HTTP create/update/history; private pagination 2+1; revision 0→1; stale write 409; guarded no-op unchanged; guard-only 400");
    }

    pub(super) async fn private_channel_assertions(f: &Fixture) {
        let task_path = format!("/api/tasks/{}", f.task_id);

        // --- Positive control: the owner sees the task. Without this, 404s
        // for the outsider could be any breakage at all.
        let (status, body) = f.request("GET", &task_path, &f.owner, None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "owner must see the task; got {status} {body}"
        );
        assert_eq!(body["task"]["title"], "route authz probe");

        // --- GET detail as outsider: 404, never 403, never the task.
        let (status, body) = f.request("GET", &task_path, &f.outsider, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "got {status} {body}");
        assert_eq!(body["error"], "task not found");

        // --- PATCH as outsider: 404 too.
        let (status, body) = f
            .request(
                "PATCH",
                &task_path,
                &f.outsider,
                Some(r#"{"status":"done"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "got {status} {body}");

        // --- POST comment as outsider: 404.
        let (status, body) = f
            .request(
                "POST",
                &format!("{task_path}/events"),
                &f.outsider,
                Some(r#"{"action":"commented","body":"leak?"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "got {status} {body}");

        // --- Channel-filtered list as outsider: 404, not 200-with-empty.
        // The explicit channel filter is itself gated by
        // enforce_channel_access (list_tasks), so an outsider cannot even
        // probe whether a channel exists — same anti-oracle rule as the
        // detail routes. The invisible-channel task is simply unreadable.
        let list_path = format!("/api/tasks?channel={}", f.private_channel_id);
        let (status, body) = f.request("GET", &list_path, &f.outsider, None).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "channel filter must 404 for an invisible channel; got {status} {body}"
        );

        // --- Unfiltered list as outsider: the task must also vanish.
        let (status, body) = f.request("GET", "/api/tasks", &f.outsider, None).await;
        assert_eq!(status, StatusCode::OK);
        let titles: Vec<&str> = body["tasks"]
            .as_array()
            .map(|tasks| tasks.iter().filter_map(|t| t["title"].as_str()).collect())
            .unwrap_or_default();
        assert!(
            !titles.contains(&"route authz probe"),
            "task leaked into unfiltered list"
        );

        // --- The owner's list DOES contain it (control for both lists).
        let (_, body) = f.request("GET", "/api/tasks", &f.owner, None).await;
        let titles: Vec<&str> = body["tasks"]
            .as_array()
            .map(|tasks| tasks.iter().filter_map(|t| t["title"].as_str()).collect())
            .unwrap_or_default();
        assert!(
            titles.contains(&"route authz probe"),
            "owner must see the task in the unfiltered list"
        );

        // --- Create bound to the private channel as outsider: 404.
        let (status, body) = f
            .request(
                "POST",
                "/api/tasks",
                &f.outsider,
                Some(
                    &serde_json::json!({
                        "title": "should not exist",
                        "channel_id": f.private_channel_id,
                    })
                    .to_string(),
                ),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "create must not bind to an invisible channel; got {status} {body}"
        );
    }
}

mod postgres_tests {
    use super::route_authz::{
        fixture, pagination_and_history_assertions, private_channel_assertions,
    };

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn signed_task_routes_page_visible_work_and_expose_durable_changes() {
        let mut f = fixture()
            .await
            .expect("Postgres and Redis fixture must be available");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test HTTP listener");
        f.http_base = Some(format!(
            "http://{}",
            listener.local_addr().expect("bound address")
        ));
        let router = f.router();
        struct Server(tokio::task::JoinHandle<()>);
        impl Drop for Server {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _server = Server(tokio::spawn(async move {
            axum::serve(listener, router).await.expect("HTTP server");
        }));
        pagination_and_history_assertions(&f).await;
    }

    /// The single route-level scenario: a relay member outside a private
    /// channel must receive 404 (not 403, not data) on every task route,
    /// and the channel-bound task must vanish from listings. The owner's
    /// positive control proves the 404s are authz, not breakage.
    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn private_channel_task_is_invisible_to_non_members_at_the_route() {
        let f = fixture()
            .await
            .expect("Postgres and Redis fixture must be available");
        // Catch assertion panics so cleanup ALWAYS runs, then resume them.
        let result = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
            private_channel_assertions(&f),
        ))
        .await;
        drop(f);
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }
}

#[path = "notifications_tests.rs"]
mod task_notifications;
