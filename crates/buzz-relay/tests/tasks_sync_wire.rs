//! HW-018 live two-client wire-level non-leak proof (spec promotion gate 3).
//!
//! Serves the REAL relay router (`buzz_relay::router::build_router`) on a real
//! TCP socket with a real Postgres + Redis backing state, then connects real
//! WebSocket clients over the wire:
//!
//!   * `owner`    — authenticated, can see the private channel,
//!   * `outsider` — authenticated in the same community, CANNOT see it,
//!   * `lurker`   — connected but never authenticated.
//!
//! A task mutation on the private channel (NIP-98-signed `PATCH`, then `POST`)
//! must produce exactly one `BUZZ_TASKS_SYNC_REQUIRED` frame per mutation on
//! `owner`'s socket — and ZERO frames on `outsider`'s and `lurker`'s sockets.
//! The assertion observes the raw WebSocket stream, not a rendered client, so
//! it proves "zero unauthorized frame bytes", not "zero rendered UI".
//!
//! Substrate: the `buzz-harness` compose stack
//! (`docker compose -p buzz-harness -f docker-compose.harness.yml up -d`),
//! postgres on :5471, redis on :6471. Override with
//! `BUZZ_TEST_DATABASE_URL` / `BUZZ_TEST_REDIS_URL`.
//!
//! Run: `cargo test -p buzz-relay --test tasks_sync_wire -- --ignored --nocapture`

use std::sync::Arc;
use std::time::Duration;

use buzz_core::channel::{ChannelType, ChannelVisibility, MemberRole};
use buzz_db::task::NewTask;
use buzz_relay::config::Config;
use buzz_relay::router::build_router;
use buzz_relay::state::AppState;
use buzz_test_client::{BuzzTestClient, RelayMessage, TestClientError};
use nostr::{EventBuilder, Keys, Kind};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const DEFAULT_DB_URL: &str = "postgres://buzz:buzz_dev@127.0.0.1:5471/buzz"; // sadscan:disable np.postgres.2 -- buzz-harness local test stack
const DEFAULT_REDIS_URL: &str = "redis://127.0.0.1:6471";
/// Window in which the positive control (owner's frame) must arrive.
const OWNER_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
/// Window in which the negative controls must observe NOTHING. Heartbeat pings
/// fire every 30 s and are answered transparently inside the client, so any
/// application-level Text frame inside this window is a leak.
const NON_LEAK_WINDOW: Duration = Duration::from_millis(1500);

struct WireFixture {
    state: Arc<AppState>,
    pool: sqlx::PgPool,
    host: String,
    community: buzz_core::CommunityId,
    channel_id: Uuid,
    task_id: Uuid,
    owner: Keys,
    member: Keys,
    outsider: Keys,
}

/// Boot a real AppState + router listener. `Ok` = listener bound and serving.
async fn boot() -> Option<(WireFixture, String, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
    let port = listener.local_addr().ok()?.port();
    // The community host IS the live socket authority, so the Host-header
    // tenant bind, the NIP-42 relay-URL check, and the client's own view of
    // the relay URL all agree with zero mocking.
    let host = format!("127.0.0.1:{port}");
    let ws_url = format!("ws://{host}");
    let http_url = format!("http://{host}");

    let database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
        .unwrap_or_else(|_| DEFAULT_DB_URL.to_string());
    let redis_url =
        std::env::var("BUZZ_TEST_REDIS_URL").unwrap_or_else(|_| DEFAULT_REDIS_URL.to_string());

    let mut config = Config::from_env().ok()?;
    config.database_url = database_url.clone();
    config.redis_url = redis_url.clone();
    config.relay_url = ws_url;
    config.require_relay_membership = true;
    config.require_auth_token = false;

    let pool = sqlx::PgPool::connect(&database_url).await.ok()?;
    let db = buzz_db::Db::from_pool(pool.clone());
    let ensured = db.ensure_configured_community(&host).await.ok()?;

    let redis_pool = deadpool_redis::Config::from_url(&redis_url)
        .create_pool(Some(deadpool_redis::Runtime::Tokio1))
        .ok()?;
    let pubsub = Arc::new(
        buzz_pubsub::PubSubManager::new(&redis_url, redis_pool.clone())
            .await
            .ok()?,
    );

    let owner = Keys::generate();
    let member = Keys::generate();
    let outsider = Keys::generate();
    let owner_pk = owner.public_key().to_bytes().to_vec();
    let member_pk = member.public_key().to_bytes().to_vec();
    let outsider_pk = outsider.public_key().to_bytes().to_vec();

    // Both are relay members (outer gate passes for both) so the frame
    // fan-out below isolates the CHANNEL gate exactly.
    buzz_db::user::ensure_user(&pool, ensured.id, &owner_pk)
        .await
        .ok()?;
    buzz_db::user::ensure_user(&pool, ensured.id, &member_pk)
        .await
        .ok()?;
    buzz_db::user::ensure_user(&pool, ensured.id, &outsider_pk)
        .await
        .ok()?;
    db.add_relay_member(ensured.id, &owner.public_key().to_hex(), "member", None)
        .await
        .ok()?;
    db.add_relay_member(ensured.id, &member.public_key().to_hex(), "member", None)
        .await
        .ok()?;
    db.add_relay_member(ensured.id, &outsider.public_key().to_hex(), "member", None)
        .await
        .ok()?;

    let channel = buzz_db::channel::create_channel(
        &pool,
        ensured.id,
        "hw018-wire-private",
        ChannelType::Stream,
        ChannelVisibility::Private,
        None,
        &owner_pk,
        None,
    )
    .await
    .ok()?;

    // Real channel membership for the second authorized client: the fan-out
    // must reach channel members who are NOT the mutating actor, which rules
    // out a vacuous "echo to self" pass.
    buzz_db::channel::add_member(
        &pool,
        ensured.id,
        channel.id,
        &member_pk,
        MemberRole::Member,
        Some(&owner_pk),
    )
    .await
    .ok()?;

    let task = db
        .create_task(
            ensured.id,
            NewTask {
                channel_id: Some(channel.id),
                created_by_pubkey: Some(owner_pk.clone()),
                title: "hw018 wire probe".to_owned(),
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
    let (state, _audit_shutdown) = AppState::new(
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
    let state = Arc::new(state);

    // Serve the production router on the real socket, with per-connection
    // address info exactly like the binary's serve path.
    let app = build_router(state.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve relay router");
    });

    Some((
        WireFixture {
            state,
            pool,
            host,
            community: ensured.id,
            channel_id: channel.id,
            task_id: task.id,
            owner,
            member,
            outsider,
        },
        http_url,
        server,
    ))
}

/// NIP-98 `Authorization: Nostr <b64>` header over the exact URL the relay
/// reconstructs (scheme from `relay_url` — `ws://` ⇒ `http://` — plus the
/// tenant host and path).
fn nip98_auth_header(keys: &Keys, method: &str, url: &str, body: &[u8]) -> String {
    let hash: [u8; 32] = Sha256::digest(body).into();
    let tags = vec![
        nostr::Tag::parse(["u", url]).expect("u tag"),
        nostr::Tag::parse(["method", method]).expect("method tag"),
        nostr::Tag::parse(["payload", hex::encode(hash).as_str()]).expect("payload tag"),
    ];
    let event = EventBuilder::new(Kind::HttpAuth, "")
        .tags(tags)
        .sign_with_keys(keys)
        .expect("sign NIP-98 event");
    let event_json = serde_json::to_string(&event).expect("serialize NIP-98 event");
    use base64::Engine as _;
    format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(event_json)
    )
}

/// Signed HTTP request against the live router. Returns (status, body).
async fn signed_request(
    http_url: &str,
    path_and_query: &str,
    keys: &Keys,
    method: &str,
    body: Option<&Value>,
) -> (reqwest::StatusCode, Value) {
    let url = format!("{http_url}{path_and_query}");
    let body_bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
    let auth = nip98_auth_header(keys, method, &url, &body_bytes);
    let client = reqwest::Client::new();
    let mut request = client
        .request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), &url)
        .header(reqwest::header::AUTHORIZATION, auth);
    if let Some(b) = body {
        request = request.json(b);
    }
    let response = request.send().await.expect("send request");
    let status = response.status();
    let bytes = response.bytes().await.expect("read body");
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// Drain every application frame within `window`; fail on ANY Text frame.
fn assert_silent(result: Result<RelayMessage, TestClientError>, who: &str) {
    match result {
        Err(TestClientError::Timeout) => {}
        Err(TestClientError::ConnectionClosed) => {}
        Err(other) => panic!("{who} transport error while asserting silence: {other}"),
        Ok(frame) => panic!(
            "LEAK: {who} received a frame within the non-leak window: {frame:?}"
        ),
    }
}

/// HW-018 promotion gate 3: live two-client wire-level non-leak proof.
#[tokio::test]
#[ignore = "requires Postgres + Redis (buzz-harness compose stack, :5471/:6471)"]
async fn task_invalidation_is_invisible_on_the_wire_to_unauthorized_clients() {
    let Some((f, http_url, server)) = boot().await else {
        eprintln!("SKIP: Postgres/Redis unavailable");
        return;
    };
    let result = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
        wire_assertions(&f, &http_url),
    ))
    .await;
    server.abort();
    let _ = server.await;
    cleanup(&f).await;
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
    let _ = f.state; // keep the state alive until assertions finish
}

async fn wire_assertions(f: &WireFixture, http_url: &str) {
    let ws_url = format!("ws://{}", f.host);

    // Two authenticated clients over the real wire.
    let mut owner = BuzzTestClient::connect(&ws_url, &f.owner)
        .await
        .expect("owner authenticates");
    let mut member = BuzzTestClient::connect(&ws_url, &f.member)
        .await
        .expect("member authenticates");
    let mut outsider = BuzzTestClient::connect(&ws_url, &f.outsider)
        .await
        .expect("outsider authenticates");

    // A third socket that never authenticates. Consume its AUTH challenge so
    // any later frame is unambiguous. AUTH_TIMEOUT (5 s) closes it if it
    // stays unauthenticated; the non-leak window below sits inside that.
    let mut lurker = BuzzTestClient::connect_unauthenticated(&ws_url)
        .await
        .expect("lurker connects");
    match lurker.recv_event(Duration::from_secs(2)).await {
        Ok(RelayMessage::Auth { .. }) => {}
        other => panic!("lurker expected the AUTH challenge first, got {other:?}"),
    }

    // --- Trigger 1: PATCH the task (owner, private channel).
    let patch_path = format!("/api/tasks/{}", f.task_id);
    let (status, body) = signed_request(
        http_url,
        &patch_path,
        &f.owner,
        "PATCH",
        Some(&json!({"title": "hw018 wire probe (renamed)"})),
    )
    .await;
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "owner PATCH must succeed; got {status} {body}"
    );

    // --- Positive control: the owner receives exactly one signal frame.
    let frame = owner
        .recv_event(OWNER_FRAME_TIMEOUT)
        .await
        .expect("owner must receive BUZZ_TASKS_SYNC_REQUIRED after the PATCH")
        .clone();
    match &frame {
        RelayMessage::TasksSyncRequired { channel_id } => {
            assert_eq!(
                channel_id, &f.channel_id.to_string(),
                "signal must carry the mutated channel's UUID, nothing else"
            );
        }
        other => panic!("owner expected TasksSyncRequired, got {other:?}"),
    }
    // Exactly one: a second frame inside the window means double-fanout.
    assert_silent(owner.recv_event(Duration::from_millis(300)).await, "owner (second frame)");
    // Cross-actor fan-out: the member (authorized, not the actor) gets the
    // same signal, proving the signal genuinely fans out beyond self-echo.
    let frame = member
        .recv_event(OWNER_FRAME_TIMEOUT)
        .await
        .expect("member must receive BUZZ_TASKS_SYNC_REQUIRED after the PATCH")
        .clone();
    assert!(
        matches!(&frame, RelayMessage::TasksSyncRequired { channel_id } if channel_id == &f.channel_id.to_string()),
        "member's frame must be the channel signal, got {frame:?}"
    );
    assert_silent(member.recv_event(Duration::from_millis(300)).await, "member (second frame)");

    // --- Trigger 2: POST a new channel-bound task (owner).
    let (status, body) = signed_request(
        http_url,
        "/api/tasks",
        &f.owner,
        "POST",
        Some(&json!({"title": "hw018 second probe", "channel_id": f.channel_id})),
    )
    .await;
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "owner POST must succeed; got {status} {body}"
    );
    let frame = owner
        .recv_event(OWNER_FRAME_TIMEOUT)
        .await
        .expect("owner must receive a second signal after the POST")
        .clone();
    assert!(
        matches!(&frame, RelayMessage::TasksSyncRequired { channel_id } if channel_id == &f.channel_id.to_string()),
        "owner's second frame must be the channel signal, got {frame:?}"
    );
    let frame = member
        .recv_event(OWNER_FRAME_TIMEOUT)
        .await
        .expect("member must receive a second signal after the POST")
        .clone();
    assert!(
        matches!(&frame, RelayMessage::TasksSyncRequired { channel_id } if channel_id == &f.channel_id.to_string()),
        "member's second frame must be the channel signal, got {frame:?}"
    );

    // --- The non-leak core: ZERO unauthorized frame bytes.
    assert_silent(outsider.recv_event(NON_LEAK_WINDOW).await, "outsider");
    assert_silent(lurker.recv_event(NON_LEAK_WINDOW).await, "lurker");

    let _ = owner.disconnect().await;
    let _ = member.disconnect().await;
    let _ = outsider.disconnect().await;
    let _ = lurker.disconnect().await;
}

async fn cleanup(f: &WireFixture) {
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
        .remove_relay_member(f.community, &f.member.public_key().to_hex())
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
