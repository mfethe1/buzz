//! Real signed HTTP workflow approvals against PostgreSQL, Redis and the native router.
use crate::state::AppState;
use base64::Engine;
use buzz_core::{
    channel::{ChannelType, ChannelVisibility, MemberRole},
    CommunityId,
};
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

struct Fixture {
    state: Arc<AppState>,
    pool: sqlx::PgPool,
    host: String,
    community: CommunityId,
    channel: Uuid,
    workflow: Uuid,
    owner: Keys,
    outsider: Keys,
    address: std::net::SocketAddr,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn fixture(timeout: &str, from_any: bool) -> Fixture {
    let host = format!("workflow-http-{}.example", Uuid::new_v4());
    let mut config = crate::config::Config::from_env().expect("config");
    config.database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("isolated test DB URL");
    config.redis_url = std::env::var("BUZZ_TEST_REDIS_URL").expect("isolated real Redis URL");
    config.relay_url = format!("ws://{host}");
    config.require_relay_membership = true;
    config.require_auth_token = false;
    let pool = sqlx::PgPool::connect(&config.database_url)
        .await
        .expect("Postgres");
    let db = buzz_db::Db::from_pool(pool.clone());
    if std::env::var("BUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
        db.migrate().await.expect("migrations");
    }
    let community = db
        .ensure_configured_community(&host)
        .await
        .expect("community")
        .id;
    let owner = Keys::generate();
    let outsider = Keys::generate();
    for key in [&owner, &outsider] {
        db.ensure_user(community, &key.public_key().to_bytes())
            .await
            .expect("user");
        db.add_relay_member(community, &key.public_key().to_hex(), "member", None)
            .await
            .expect("relay membership");
    }
    let channel = db
        .create_channel(
            community,
            "approval-http",
            ChannelType::Stream,
            ChannelVisibility::Private,
            None,
            &owner.public_key().to_bytes(),
            None,
        )
        .await
        .expect("channel")
        .id;
    let definition = json!({"name":"approval-http","trigger":{"on":"webhook"},"enabled":true,
        "steps":[{"id":"before","action":"delay","duration":"0s"},
        {"id":"review","action":"request_approval","from":if from_any {"any".into()}else{owner.public_key().to_hex()},"message":"Approve {{trigger.request}}","timeout":timeout},
        {"id":"after","action":"send_message","text":"Approved {{trigger.request}} with prior {{steps.before.output.slept_secs}} and decision {{steps.review.output.approved}}"}]});
    let hash = Sha256::digest(definition.to_string().as_bytes());
    let workflow = db
        .create_workflow(
            community,
            Some(channel),
            &owner.public_key().to_bytes(),
            "approval-http",
            &definition.to_string(),
            &hash,
        )
        .await
        .expect("workflow");
    let redis_pool = deadpool_redis::Config::from_url(&config.redis_url)
        .create_pool(Some(deadpool_redis::Runtime::Tokio1))
        .expect("Redis pool");
    let pubsub = Arc::new(
        buzz_pubsub::PubSubManager::new(&config.redis_url, redis_pool.clone())
            .await
            .expect("pubsub"),
    );
    let audit = buzz_audit::AuditService::new(pool.clone());
    let auth = buzz_auth::AuthService::new(config.auth.clone());
    let search = buzz_search::SearchService::new(pool.clone());
    let engine = Arc::new(buzz_workflow::WorkflowEngine::new(
        db.clone(),
        buzz_workflow::WorkflowConfig::default(),
    ));
    let media = buzz_media::MediaStorage::new(&config.media).expect("media");
    let (state, _) = AppState::new(
        config,
        db,
        redis_pool,
        audit,
        pubsub,
        auth,
        search,
        engine,
        Keys::generate(),
        media,
    );
    let state = Arc::new(state);
    state
        .workflow_engine
        .set_action_sink(Arc::new(crate::workflow_sink::RelayActionSink::new(&state)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("TCP listener");
    let address = listener.local_addr().expect("address");
    let router = crate::router::build_router(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("HTTP server");
    });
    Fixture {
        state,
        pool,
        host,
        community,
        channel,
        workflow,
        owner,
        outsider,
        address,
        server,
    }
}

impl Fixture {
    async fn request(
        &self,
        keys: &Keys,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let body = body
            .map(|v| serde_json::to_vec(&v).expect("JSON"))
            .unwrap_or_default();
        let signed_url = format!("http://{}{path}", self.host);
        let auth = EventBuilder::new(Kind::HttpAuth, "")
            .tags([
                Tag::parse(["u", &signed_url]).expect("u"),
                Tag::parse(["method", method]).expect("method"),
                Tag::parse(["payload", &hex::encode(Sha256::digest(&body))]).expect("payload"),
                Tag::parse(["nonce", &Uuid::new_v4().to_string()]).expect("nonce"),
            ])
            .sign_with_keys(keys)
            .expect("signed NIP-98");
        let response = reqwest::Client::new()
            .request(
                method.parse().expect("method"),
                format!("http://{}{path}", self.address),
            )
            .header("Host", &self.host)
            .header(
                "Authorization",
                format!(
                    "Nostr {}",
                    base64::engine::general_purpose::STANDARD
                        .encode(serde_json::to_vec(&auth).expect("auth JSON"))
                ),
            )
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .expect("real HTTP response");
        let status = response.status().as_u16();
        let body = response.text().await.expect("body");
        let value = serde_json::from_str(&body).unwrap_or_else(|_| json!({"raw":body}));
        (status, value)
    }
    async fn submit(&self, keys: &Keys, event: &Event) -> (u16, Value) {
        self.request(
            keys,
            "POST",
            "/events",
            Some(serde_json::to_value(event).expect("event JSON")),
        )
        .await
    }
    async fn start(&self) -> (Uuid, Value) {
        let event = EventBuilder::new(Kind::Custom(46020), r#"{"request":"production-test"}"#)
            .tags([Tag::parse(["d", &self.workflow.to_string()]).expect("d")])
            .sign_with_keys(&self.owner)
            .expect("signed trigger");
        let (status, response) = self.submit(&self.owner, &event).await;
        assert_eq!(status, 200, "trigger: {response}");
        assert_eq!(response["accepted"], true, "{response}");
        let message = response["message"]
            .as_str()
            .expect("message")
            .strip_prefix("response:")
            .expect("receipt");
        let receipt: Value = serde_json::from_str(message).expect("receipt JSON");
        let run = Uuid::parse_str(receipt["run_id"].as_str().expect("run ID")).expect("UUID");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let row = self
                    .state
                    .db
                    .get_workflow_run(self.community, run)
                    .await
                    .expect("run");
                if row.status == buzz_db::workflow::RunStatus::WaitingApproval {
                    break;
                }
                assert!(
                    !matches!(
                        row.status,
                        buzz_db::workflow::RunStatus::Failed
                            | buzz_db::workflow::RunStatus::Completed
                    ),
                    "unexpected run: {row:?}"
                );
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("wait persisted");
        let path = format!("/workflows/{}/runs/{run}/approvals", self.workflow);
        let (status, body) = self.request(&self.owner, "GET", &path, None).await;
        assert_eq!(status, 200, "approval read: {body}");
        (run, body["approvals"][0].clone())
    }
    fn decision(&self, key: &Keys, approval: &Value, grant: bool, note: &str) -> Event {
        EventBuilder::new(Kind::Custom(if grant { 46030 } else { 46031 }), note)
            .tags([
                Tag::parse(["d", approval["approval_ref"].as_str().expect("reference")])
                    .expect("d"),
            ])
            .sign_with_keys(key)
            .expect("signed decision")
    }
    fn restarted_engine(&self) -> Arc<buzz_workflow::WorkflowEngine> {
        let engine = Arc::new(buzz_workflow::WorkflowEngine::new(
            self.state.db.clone(),
            buzz_workflow::WorkflowConfig::default(),
        ));
        engine.set_action_sink(Arc::new(crate::workflow_sink::RelayActionSink::new(
            &self.state,
        )));
        engine
    }
    async fn terminal(&self, run: Uuid) -> buzz_db::workflow::WorkflowRunRecord {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let row = self
                    .state
                    .db
                    .get_workflow_run(self.community, run)
                    .await
                    .expect("run");
                if matches!(
                    row.status,
                    buzz_db::workflow::RunStatus::Completed
                        | buzz_db::workflow::RunStatus::Cancelled
                        | buzz_db::workflow::RunStatus::Failed
                ) {
                    return row;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("terminal run")
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn workflow_approval_signed_http_restart_replay_and_exactly_one_effect() {
    let f = fixture("1h", false).await;
    let (run, approval) = f.start().await;
    assert_eq!(approval["message"], "Approve production-test");
    let request_id = hex::decode(
        approval["request_event_id"]
            .as_str()
            .expect("request identity"),
    )
    .expect("hex");
    let request = f
        .state
        .db
        .get_event_by_id(f.community, &request_id)
        .await
        .expect("lookup")
        .expect("signed request");
    request.event.verify().expect("valid relay signature");
    assert!(request.event.tags.iter().any(|tag| tag.as_slice()
        == [
            "workflow-revision",
            approval["definition_hash"].as_str().expect("revision")
        ]));
    let saved: Value = sqlx::query_scalar(
        "SELECT continuation FROM workflow_approvals WHERE community_id=$1 AND token=$2",
    )
    .bind(f.community.as_uuid())
    .bind(hex::decode(approval["approval_ref"].as_str().expect("ref")).expect("hex"))
    .fetch_one(&f.pool)
    .await
    .expect("saved snapshot");
    let digest = hex::encode(Sha256::digest(
        serde_json::to_vec(&saved).expect("snapshot JSON"),
    ));
    assert!(request
        .event
        .tags
        .iter()
        .any(|tag| tag.as_slice() == ["continuation-sha256", digest.as_str()]));
    assert_eq!(request.channel_id, Some(f.channel));
    let outsider = f.decision(&f.outsider, &approval, true, "unauthorized");
    let (status, body) = f.submit(&f.outsider, &outsider).await;
    assert!(
        status >= 400 || body["accepted"] == false,
        "outsider: {body}"
    );
    let grant = f.decision(&f.owner, &approval, true, "approved exact request");
    let (status, body) = f.submit(&f.owner, &grant).await;
    assert_eq!(status, 200, "grant: {body}");
    assert_eq!(body["accepted"], true, "{body}");
    let (status, body) = f.submit(&f.owner, &grant).await;
    assert_eq!(status, 200, "replay: {body}");
    assert!(
        body["message"]
            .as_str()
            .expect("receipt")
            .contains("\"duplicate\":true"),
        "{body}"
    );
    let a = f.restarted_engine();
    let b = f.restarted_engine();
    let recovering_a = tokio::spawn(async move { a.run().await });
    let recovering_b = tokio::spawn(async move { b.run().await });
    let result = f.terminal(run).await;
    recovering_a.abort();
    recovering_b.abort();
    assert_eq!(
        result.status,
        buzz_db::workflow::RunStatus::Completed,
        "{result:?}"
    );
    let trace = result.execution_trace.as_array().expect("trace");
    let after = trace
        .iter()
        .find(|v| v["step_id"] == "after")
        .expect("after step");
    let id = hex::decode(after["output"]["event_id"].as_str().expect("effect ID")).expect("hex");
    let effect = f
        .state
        .db
        .get_event_by_id(f.community, &id)
        .await
        .expect("lookup")
        .expect("effect");
    assert_eq!(
        effect.event.content,
        "Approved production-test with prior 0 and decision true"
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE community_id=$1 AND channel_id=$2 AND kind=9",
    )
    .bind(f.community.as_uuid())
    .bind(f.channel)
    .fetch_one(&f.pool)
    .await
    .expect("effect count");
    assert_eq!(count, 1);
    let deny = f.decision(&f.owner, &approval, false, "competing denial");
    let (status, body) = f.submit(&f.owner, &deny).await;
    assert!(
        status >= 400 || body["accepted"] == false,
        "competing decision: {body}"
    );
    eprintln!("WF-08 actual signed HTTP: persisted request -> grant -> exact replay -> recreated engines -> completed; one kind9 effect and preserved trigger/prior output");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn workflow_approval_signed_http_second_wait_survives_late_finalizer() {
    let f = fixture("1h", false).await;
    let row = f
        .state
        .db
        .get_workflow(f.community, f.workflow)
        .await
        .expect("workflow");
    let mut definition = row.definition;
    definition["steps"].as_array_mut().expect("steps").insert(
        2,
        json!({
            "id":"review_again", "action":"request_approval", "from":f.owner.public_key().to_hex(),
            "message":"Confirm {{steps.review.output.approved}}", "timeout":"1h",
        }),
    );
    sqlx::query(
        "UPDATE workflows SET definition=$3,definition_hash=$4 WHERE community_id=$1 AND id=$2",
    )
    .bind(f.community.as_uuid())
    .bind(f.workflow)
    .bind(&definition)
    .bind(Sha256::digest(definition.to_string().as_bytes()).as_slice())
    .execute(&f.pool)
    .await
    .expect("two approval definition");
    let (run, first) = f.start().await;
    let grant = f.decision(&f.owner, &first, true, "first approved");
    let (status, body) = f.submit(&f.owner, &grant).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    f.restarted_engine()
        .recover_approvals()
        .await
        .expect("first restart");
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let row = f
                .state
                .db
                .get_workflow_run(f.community, run)
                .await
                .expect("run");
            if row.status == buzz_db::workflow::RunStatus::WaitingApproval && row.current_step == 2
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("second wait committed");
    let path = format!("/workflows/{}/runs/{run}/approvals", f.workflow);
    let (status, body) = f.request(&f.owner, "GET", &path, None).await;
    assert_eq!(status, 200, "{body}");
    let approvals = body["approvals"].as_array().expect("approvals");
    assert_eq!(approvals.len(), 2);
    let second = approvals
        .iter()
        .find(|a| a["step_id"] == "review_again")
        .expect("second wait");
    assert_eq!(second["message"], "Confirm true");
    assert_ne!(second["approval_ref"], first["approval_ref"]);
    // Model an old executor returning an error after its sink committed a wait.
    f.state
        .workflow_engine
        .finalize_run(
            f.community,
            run,
            Err((
                buzz_workflow::WorkflowError::Database("late fanout failure".into()),
                buzz_workflow::error::PartialProgress {
                    step_index: 2,
                    trace: vec![],
                },
            )),
            None,
        )
        .await;
    assert_eq!(
        f.state
            .db
            .get_workflow_run(f.community, run)
            .await
            .expect("preserved wait")
            .status,
        buzz_db::workflow::RunStatus::WaitingApproval
    );
    let grant = f.decision(&f.owner, second, true, "second approved");
    let (status, body) = f.submit(&f.owner, &grant).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    f.restarted_engine()
        .recover_approvals()
        .await
        .expect("second restart");
    assert_eq!(
        f.terminal(run).await.status,
        buzz_db::workflow::RunStatus::Completed
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE community_id=$1 AND channel_id=$2 AND kind=9",
    )
    .bind(f.community.as_uuid())
    .bind(f.channel)
    .fetch_one(&f.pool)
    .await
    .expect("effects");
    assert_eq!(count, 1);
    eprintln!("WF-08 signed HTTP: two distinct persisted waits and approvals across recreated engines; late finalizer preserved second wait; one final effect");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn workflow_approval_signed_http_expiration_is_durable() {
    let f = fixture("1s", true).await;
    let (run, approval) = f.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let grant = f.decision(&f.owner, &approval, true, "too late");
    let (status, body) = f.submit(&f.owner, &grant).await;
    assert!(
        status >= 400 || body["accepted"] == false,
        "expired decision: {body}"
    );
    f.restarted_engine()
        .recover_approvals()
        .await
        .expect("recover expiry");
    assert_eq!(
        f.terminal(run).await.error_code.as_deref(),
        Some("approval_expired")
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn workflow_approval_signed_http_revoked_approver_cannot_resume() {
    let f = fixture("1h", true).await;
    f.state
        .db
        .add_member(
            f.community,
            f.channel,
            &f.outsider.public_key().to_bytes(),
            MemberRole::Member,
            Some(&f.owner.public_key().to_bytes()),
        )
        .await
        .expect("approver membership");
    let (run, approval) = f.start().await;
    let grant = f.decision(&f.outsider, &approval, true, "approved before revocation");
    let (status, body) = f.submit(&f.outsider, &grant).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    sqlx::query("UPDATE channel_members SET removed_at=clock_timestamp() WHERE community_id=$1 AND channel_id=$2 AND pubkey=$3").bind(f.community.as_uuid()).bind(f.channel).bind(f.outsider.public_key().to_bytes().as_slice()).execute(&f.pool).await.expect("revoke fixture membership");
    f.restarted_engine()
        .recover_approvals()
        .await
        .expect("recover");
    let run = f.terminal(run).await;
    assert_eq!(run.status, buzz_db::workflow::RunStatus::Failed);
    assert_eq!(run.error_code.as_deref(), Some("owner_unauthorized"));
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE community_id=$1 AND channel_id=$2 AND kind=9",
    )
    .bind(f.community.as_uuid())
    .bind(f.channel)
    .fetch_one(&f.pool)
    .await
    .expect("count");
    assert_eq!(count, 0);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn workflow_approval_signed_http_changed_definition_cannot_resume() {
    let f = fixture("1h", false).await;
    let (run, approval) = f.start().await;
    assert_eq!(
        approval["definition_hash"]
            .as_str()
            .expect("approved revision")
            .len(),
        64
    );
    let grant = f.decision(&f.owner, &approval, true, "approve original version");
    let (status, body) = f.submit(&f.owner, &grant).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    sqlx::query("UPDATE workflows SET definition_hash=$3 WHERE community_id=$1 AND id=$2")
        .bind(f.community.as_uuid())
        .bind(f.workflow)
        .bind([9_u8; 32].as_slice())
        .execute(&f.pool)
        .await
        .expect("change fixture version");
    f.restarted_engine()
        .recover_approvals()
        .await
        .expect("recover");
    let row = f.terminal(run).await;
    assert_eq!(row.status, buzz_db::workflow::RunStatus::Failed);
    assert_eq!(row.error_code.as_deref(), Some("owner_unauthorized"));
    assert!(!row.execution_trace.to_string().contains("\"sent\":true"));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn workflow_approval_signed_http_denial_commits_terminal_audit() {
    let f = fixture("1h", false).await;
    let (run, approval) = f.start().await;
    let deny = f.decision(&f.owner, &approval, false, "do not execute");
    let (status, body) = f.submit(&f.owner, &deny).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["accepted"], true, "{body}");
    let row = f.terminal(run).await;
    assert_eq!(row.status, buzz_db::workflow::RunStatus::Cancelled);
    assert_eq!(row.error_code.as_deref(), Some("approval_denied"));
    let (status, replay) = f.submit(&f.owner, &deny).await;
    assert_eq!(status, 200, "{replay}");
    assert!(replay["message"]
        .as_str()
        .expect("receipt")
        .contains("\"duplicate\":true"));
    let path = format!("/workflows/{}/runs/{run}/approvals", f.workflow);
    let (status, read) = f.request(&f.owner, "GET", &path, None).await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["approvals"][0]["status"], "denied");
    assert_eq!(read["approvals"][0]["decision_event_id"], deny.id.to_hex());
    let (status, read) = f.request(&f.outsider, "GET", &path, None).await;
    assert_eq!(status, 403, "private approval must not leak: {read}");
}
