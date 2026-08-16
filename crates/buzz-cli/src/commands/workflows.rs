use sha2::{Digest, Sha256};

use crate::client::{
    extract_d_tag, extract_relay_response_field, normalize_write_response, print_create_response,
    BuzzClient,
};
use crate::error::CliError;
use crate::validate::{parse_uuid, percent_encode, read_or_stdin, sdk_err, validate_uuid};

// TODO(phase-4): Replace raw nostr::EventBuilder usage with buzz-sdk builder functions

/// List workflows in a channel — query kind:30620 workflow definition events.
pub async fn cmd_list_workflows(client: &BuzzClient, channel_id: &str) -> Result<(), CliError> {
    validate_uuid(channel_id)?;
    let filter = serde_json::json!({
        "kinds": [30620],
        "#h": [channel_id]
    });
    let resp = client.query(&filter).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&resp).unwrap_or_default();
    let workflows: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "workflow_id": extract_d_tag(e),
                "content": e.get("content").and_then(|v| v.as_str()).unwrap_or(""),
                "created_at": e.get("created_at").and_then(|v| v.as_u64()).unwrap_or(0),
                "pubkey": e.get("pubkey").and_then(|v| v.as_str()).unwrap_or(""),
            })
        })
        .collect();
    let output = serde_json::to_string(&workflows).unwrap_or_default();
    println!("{output}");
    Ok(())
}

/// Get a single workflow definition.
pub async fn cmd_get_workflow(client: &BuzzClient, workflow_id: &str) -> Result<(), CliError> {
    validate_uuid(workflow_id)?;
    let filter = serde_json::json!({
        "kinds": [30620],
        "#d": [workflow_id]
    });
    let resp = client.query(&filter).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&resp).unwrap_or_default();
    if let Some(e) = events.first() {
        let normalized = serde_json::json!({
            "workflow_id": extract_d_tag(e),
            "content": e.get("content").and_then(|v| v.as_str()).unwrap_or(""),
            "created_at": e.get("created_at").and_then(|v| v.as_u64()).unwrap_or(0),
            "pubkey": e.get("pubkey").and_then(|v| v.as_str()).unwrap_or(""),
        });
        println!("{normalized}");
    } else {
        println!("null");
    }
    Ok(())
}

/// Get workflow run history — reads the relay's structured run endpoint.
///
/// Run history lives in `workflow_runs` (trigger context + trace), not as Nostr
/// events. The relay exposes `GET /workflows/{workflow_id}/runs` which returns
/// `{"runs": [...], "next": {"before": <ts>, "before_id": <uuid>}}`. This
/// command preserves that JSON unchanged so status, trace, timestamps, and
/// stable failure fields survive.
pub async fn cmd_get_workflow_runs(
    client: &BuzzClient,
    workflow_id: &str,
    limit: Option<u32>,
    before: Option<&str>,
    before_id: Option<&str>,
) -> Result<(), CliError> {
    validate_uuid(workflow_id)?;
    let limit = match limit {
        Some(v) => {
            if !(1..=100).contains(&v) {
                return Err(CliError::Usage(
                    "limit must be between 1 and 100".to_string(),
                ));
            }
            v
        }
        None => 20,
    };

    // Paired keyset cursor — both or neither, matching relay's 400 contract.
    match (before, before_id) {
        (Some(_), None) | (None, Some(_)) => {
            return Err(CliError::Usage(
                "before and before_id must be supplied together".to_string(),
            ))
        }
        (Some(b), Some(bid)) => {
            // Validate cursor shapes client-side so a typo fails before the network.
            if b.parse::<chrono::DateTime<chrono::Utc>>().is_err() {
                return Err(CliError::Usage(format!(
                    "before must be an RFC3339 timestamp: {b}"
                )));
            }
            validate_uuid(bid)?;
        }
        (None, None) => {}
    }

    let path = build_workflow_runs_path(workflow_id, limit, before, before_id);
    let resp = client.get_authed(&path).await?;
    // Preserve relay's structured JSON verbatim — no synthetic #run tags or event synthesis.
    println!("{resp}");
    Ok(())
}

/// Build a root-relative GET path for the workflow runs endpoint.
///
/// Mutation-sensitive tests in this module pin exact path/query encoding.
fn build_workflow_runs_path(
    workflow_id: &str,
    limit: u32,
    before: Option<&str>,
    before_id: Option<&str>,
) -> String {
    let mut path = format!("/workflows/{workflow_id}/runs?limit={limit}");
    if let (Some(b), Some(bid)) = (before, before_id) {
        // before contains colons (RFC3339) — percent-encode for safe query transmission.
        // Use the shared RFC3986 encoder so signing and request encoding cannot drift.
        path.push_str("&before=");
        path.push_str(&percent_encode(b));
        path.push_str("&before_id=");
        path.push_str(&percent_encode(bid));
    }
    path
}

/// Create a workflow — sign and submit a kind:30620 event.
pub async fn cmd_create_workflow(
    client: &BuzzClient,
    channel_id: &str,
    yaml: &str,
) -> Result<(), CliError> {
    let channel_uuid = parse_uuid(channel_id)?;
    let yaml_definition = read_or_stdin(yaml)?;

    let workflow_id = uuid::Uuid::new_v4();
    let builder = buzz_sdk::build_workflow_def(channel_uuid, workflow_id, &yaml_definition)
        .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;

    let resp = client.submit_event(event).await?;
    let final_workflow_id = extract_relay_response_field(&resp, "workflow_id")
        .unwrap_or_else(|| workflow_id.to_string());
    print_create_response(&resp, "workflow_id", &final_workflow_id);
    Ok(())
}

/// Update a workflow — sign and submit an updated kind:30620 event with same d-tag.
pub async fn cmd_update_workflow(
    client: &BuzzClient,
    channel_id: &str,
    workflow_id: &str,
    yaml: &str,
) -> Result<(), CliError> {
    let channel_uuid = parse_uuid(channel_id)?;
    let wf_uuid = parse_uuid(workflow_id)?;
    let yaml_definition = read_or_stdin(yaml)?;

    let builder = buzz_sdk::build_workflow_update(channel_uuid, wf_uuid, &yaml_definition)
        .map_err(sdk_err)?;
    let event = client.sign_event(builder)?;

    let resp = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&resp));
    Ok(())
}

/// Delete a workflow — sign and submit a kind:5 deletion event.
pub async fn cmd_delete_workflow(client: &BuzzClient, workflow_id: &str) -> Result<(), CliError> {
    let wf_uuid = parse_uuid(workflow_id)?;
    let keys = client.keys();

    let builder =
        buzz_sdk::build_workflow_delete(&keys.public_key().to_hex(), wf_uuid).map_err(sdk_err)?;
    let event = client.sign_event(builder)?;

    let resp = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&resp));
    Ok(())
}

/// Trigger a workflow — sign and submit a kind:46020 event.
///
/// When `inputs` is provided, it is parsed as a JSON object and used as the
/// event content (MCP parity). When omitted, the event content is `{}`.
pub async fn cmd_trigger_workflow(
    client: &BuzzClient,
    workflow_id: &str,
    inputs: Option<&str>,
) -> Result<(), CliError> {
    let wf_uuid = parse_uuid(workflow_id)?;

    if let Some(raw) = inputs {
        // Parse and validate it is a JSON object, then build the event manually
        // so we can embed the inputs as the event content.
        let parsed: serde_json::Value = serde_json::from_str(raw)
            .map_err(|e| CliError::Usage(format!("--inputs is not valid JSON: {e}")))?;
        if !parsed.is_object() {
            return Err(CliError::Usage("--inputs must be a JSON object".into()));
        }
        let content = serde_json::to_string(&parsed).unwrap_or_default();
        use nostr::{EventBuilder, Kind, Tag};
        let tags = vec![Tag::parse(["d", &wf_uuid.to_string()])
            .map_err(|e| CliError::Other(format!("tag error: {e}")))?];
        let builder = EventBuilder::new(
            Kind::Custom(buzz_sdk::kind::KIND_WORKFLOW_TRIGGER as u16),
            &content,
        )
        .tags(tags);
        let event = client.sign_event(builder)?;
        let resp = client.submit_event(event).await?;
        println!("{}", normalize_write_response(&resp));
    } else {
        let builder = buzz_sdk::build_workflow_trigger(wf_uuid).map_err(sdk_err)?;
        let event = client.sign_event(builder)?;
        let resp = client.submit_event(event).await?;
        println!("{}", normalize_write_response(&resp));
    }
    Ok(())
}

/// Approve or deny a workflow step — sign and submit a kind:46030 (grant) or 46031 (deny) event.
pub async fn cmd_approve_step(
    client: &BuzzClient,
    approval_token: &str,
    approved: bool,
    note: Option<&str>,
) -> Result<(), CliError> {
    validate_uuid(approval_token)?;

    let content = note.unwrap_or("");

    // The relay expects d-tag = hex(SHA256(token)), not the raw token UUID.
    let token_hash = hex::encode(Sha256::digest(approval_token.as_bytes()));
    let builder =
        buzz_sdk::build_workflow_approval(&token_hash, approved, content).map_err(sdk_err)?;
    let event = client.sign_event(builder)?;

    let resp = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&resp));
    Ok(())
}

pub async fn dispatch(cmd: crate::WorkflowsCmd, client: &BuzzClient) -> Result<(), CliError> {
    use crate::WorkflowsCmd;
    match cmd {
        WorkflowsCmd::List { channel } => cmd_list_workflows(client, &channel).await,
        WorkflowsCmd::Get { workflow } => cmd_get_workflow(client, &workflow).await,
        WorkflowsCmd::Create { channel, yaml } => {
            cmd_create_workflow(client, &channel, &yaml).await
        }
        WorkflowsCmd::Update {
            channel,
            workflow,
            yaml,
        } => cmd_update_workflow(client, &channel, &workflow, &yaml).await,
        WorkflowsCmd::Delete { workflow } => cmd_delete_workflow(client, &workflow).await,
        WorkflowsCmd::Trigger { workflow, inputs } => {
            cmd_trigger_workflow(client, &workflow, inputs.as_deref()).await
        }
        WorkflowsCmd::Runs {
            workflow,
            limit,
            before,
            before_id,
        } => {
            cmd_get_workflow_runs(
                client,
                &workflow,
                limit,
                before.as_deref(),
                before_id.as_deref(),
            )
            .await
        }
        WorkflowsCmd::Approve {
            token,
            approved,
            note,
        } => {
            // approved is already a bool — no parse_bool_flag needed
            cmd_approve_step(client, &token, approved, note.as_deref()).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_path_default_limit() {
        let wf = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            build_workflow_runs_path(wf, 20, None, None),
            format!("/workflows/{wf}/runs?limit=20")
        );
    }

    #[test]
    fn build_path_custom_limit() {
        let wf = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            build_workflow_runs_path(wf, 1, None, None),
            format!("/workflows/{wf}/runs?limit=1")
        );
        assert_eq!(
            build_workflow_runs_path(wf, 100, None, None),
            format!("/workflows/{wf}/runs?limit=100")
        );
    }

    #[test]
    fn build_path_with_cursor_encodes_before() {
        let wf = "550e8400-e29b-41d4-a716-446655440000";
        let bid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let before = "2026-01-15T10:00:00Z";
        let path = build_workflow_runs_path(wf, 20, Some(before), Some(bid));
        // ':' must be percent-encoded, otherwise NIP-98 signed URL would mismatch
        assert!(
            path.contains("before=2026-01-15T10%3A00%3A00Z"),
            "path={path}"
        );
        assert!(path.contains(&format!("before_id={bid}")));
        assert!(path.starts_with(&format!("/workflows/{wf}/runs?limit=20&before=")));
    }

    #[test]
    fn build_path_never_invents_run_tags() {
        let wf = "550e8400-e29b-41d4-a716-446655440000";
        let path = build_workflow_runs_path(wf, 20, None, None);
        assert!(
            !path.contains("#run"),
            "must not invent #run tags, got {path}"
        );
        assert!(
            !path.contains("kinds"),
            "must not use Nostr REQ kinds, got {path}"
        );
    }

    #[test]
    fn build_path_encodes_plus_offset() {
        // + in +00:00 must be encoded to %2B, otherwise it decodes as space
        let wf = "550e8400-e29b-41d4-a716-446655440000";
        let bid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let before = "2026-01-15T10:00:00+00:00";
        let path = build_workflow_runs_path(wf, 20, Some(before), Some(bid));
        assert!(
            path.contains("before=2026-01-15T10%3A00%3A00%2B00%3A00"),
            "path={path}"
        );
    }

    #[tokio::test]
    async fn limit_bounds_reject_zero_and_over_100() {
        // Exact harness: cmd_get_workflow_runs validates before networking, so zero and 101 must be Usage, not clamped
        let wf = "550e8400-e29b-41d4-a716-446655440000";
        let client = crate::client::BuzzClient::new(
            "http://localhost:1".into(),
            nostr::Keys::generate(),
            None,
            None,
        )
        .unwrap();
        let err0 = cmd_get_workflow_runs(&client, wf, Some(0), None, None)
            .await
            .unwrap_err();
        assert!(
            matches!(err0, crate::error::CliError::Usage(ref msg) if msg.contains("limit")),
            "got {err0:?}"
        );
        let err101 = cmd_get_workflow_runs(&client, wf, Some(101), None, None)
            .await
            .unwrap_err();
        assert!(
            matches!(err101, crate::error::CliError::Usage(ref msg) if msg.contains("limit")),
            "got {err101:?}"
        );
    }

    #[tokio::test]
    async fn paired_cursor_rejected_when_only_one_present() {
        let wf = "550e8400-e29b-41d4-a716-446655440000";
        let bid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let before = "2026-01-15T10:00:00Z";
        let client = crate::client::BuzzClient::new(
            "http://localhost:1".into(),
            nostr::Keys::generate(),
            None,
            None,
        )
        .unwrap();
        let e1 = cmd_get_workflow_runs(&client, wf, Some(20), Some(before), None)
            .await
            .unwrap_err();
        assert!(
            matches!(e1, crate::error::CliError::Usage(ref msg) if msg.contains("before and before_id")),
            "got {e1:?}"
        );
        let e2 = cmd_get_workflow_runs(&client, wf, Some(20), None, Some(bid))
            .await
            .unwrap_err();
        assert!(
            matches!(e2, crate::error::CliError::Usage(ref msg) if msg.contains("before and before_id")),
            "got {e2:?}"
        );
        // Invalid timestamp must be Usage, not relay 400
        let e3 = cmd_get_workflow_runs(&client, wf, Some(20), Some("not-a-time"), Some(bid))
            .await
            .unwrap_err();
        assert!(
            matches!(e3, crate::error::CliError::Usage(ref msg) if msg.contains("RFC3339")),
            "got {e3:?}"
        );
        // Invalid UUID for before_id must be Usage
        let e4 = cmd_get_workflow_runs(&client, wf, Some(20), Some(before), Some("not-a-uuid"))
            .await
            .unwrap_err();
        assert!(matches!(e4, crate::error::CliError::Usage(_)), "got {e4:?}");
    }

    #[tokio::test]
    async fn workflow_id_shape_rejected() {
        let bad = "not-a-uuid";
        let client = crate::client::BuzzClient::new(
            "http://localhost:1".into(),
            nostr::Keys::generate(),
            None,
            None,
        )
        .unwrap();
        let err = cmd_get_workflow_runs(&client, bad, Some(20), None, None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, crate::error::CliError::Usage(_)),
            "got {err:?}"
        );
    }

    // --- NIP-98 GET, structured output, and relay error propagation (mutation-sensitive) ---

    async fn runs_test_server<F>(
        f: F,
    ) -> (
        String,
        std::sync::Arc<std::sync::atomic::AtomicU32>,
        std::sync::Arc<tokio::sync::Mutex<Option<(String, String)>>>,
    )
    where
        F: Fn(u32) -> (axum::http::StatusCode, String) + Send + Sync + 'static,
    {
        use axum::{
            body::Body,
            extract::State,
            http::{HeaderMap, Response, StatusCode, Uri},
            Router,
        };
        use std::net::SocketAddr;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        use tokio::net::TcpListener;

        let counter = Arc::new(AtomicU32::new(0));
        let last_req: Arc<tokio::sync::Mutex<Option<(String, String)>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        let handler: Arc<dyn Fn(u32) -> (StatusCode, String) + Send + Sync> = Arc::new(f);
        let state = (handler, counter.clone(), last_req.clone());
        type S = (
            Arc<dyn Fn(u32) -> (StatusCode, String) + Send + Sync>,
            Arc<AtomicU32>,
            Arc<tokio::sync::Mutex<Option<(String, String)>>>,
        );
        let app = Router::new()
            .route(
                "/{*path}",
                axum::routing::get(
                    |State((handler, ctr, req_cap)): State<S>, uri: Uri, headers: HeaderMap| async move {
                        let n = ctr.fetch_add(1, Ordering::SeqCst) + 1;
                        let path_and_query = uri.path_and_query().map(|pq| pq.to_string()).unwrap_or_else(|| uri.path().to_string());
                        let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                        *req_cap.lock().await = Some((path_and_query, auth));
                        let (status, body) = handler(n);
                        Response::builder()
                            .status(status)
                            .header("content-type", "application/json")
                            .body(Body::from(body))
                            .unwrap()
                    },
                ),
            )
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), counter, last_req)
    }

    #[tokio::test]
    async fn runs_get_uses_nip98_and_preserves_structured_json() {
        let expected_body = r#"{"runs":[{"id":"550e8400-e29b-41d4-a716-446655440001","workflow_id":"550e8400-e29b-41d4-a716-446655440000","status":"completed","current_step":2,"execution_trace":[],"started_at":1700000000,"completed_at":1700000001,"error_code":null,"error_message":null,"created_at":1700000000}],"next":null}"#;
        let (url, attempts, last_req) = runs_test_server({
            let body = expected_body.to_string();
            move |_| (axum::http::StatusCode::OK, body.clone())
        })
        .await;
        let client =
            crate::client::BuzzClient::new(url, nostr::Keys::generate(), None, None).unwrap();
        let wf = "550e8400-e29b-41d4-a716-446655440000";
        // Use the client directly to verify verbatim preservation; cmd_get_workflow_runs does the same via println!
        let path = build_workflow_runs_path(wf, 20, None, None);
        let resp = client.get_authed(&path).await.expect("get_authed ok");
        assert_eq!(
            resp, expected_body,
            "structured JSON must survive unchanged"
        );
        // Verify exact path/query was sent and NIP-98 header was present
        let (sent_path, auth) = last_req.lock().await.clone().unwrap();
        assert_eq!(
            sent_path,
            format!("/workflows/{wf}/runs?limit=20"),
            "exact path/query, got {sent_path}"
        );
        assert!(
            auth.starts_with("Nostr "),
            "NIP-98 GET must send Nostr Authorization, got {auth:?}"
        );
        // Structured fields must be present — proves no event normalization
        let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert!(v.get("runs").is_some());
        assert!(v.get("next").is_some());
        assert_eq!(v["runs"][0]["status"].as_str(), Some("completed"));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn runs_get_encodes_cursor_and_verifies_path() {
        let wf = "550e8400-e29b-41d4-a716-446655440000";
        let bid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let before = "2026-01-15T10:00:00Z";
        let (url, _, last_req) = runs_test_server(|_| {
            (
                axum::http::StatusCode::OK,
                r#"{"runs":[],"next":null}"#.to_string(),
            )
        })
        .await;
        let client =
            crate::client::BuzzClient::new(url, nostr::Keys::generate(), None, None).unwrap();
        let path = build_workflow_runs_path(wf, 5, Some(before), Some(bid));
        let _ = client.get_authed(&path).await.unwrap();
        let (sent_path, _) = last_req.lock().await.clone().unwrap();
        assert_eq!(
            sent_path, path,
            "cursor path must be byte-identical to built path"
        );
        assert!(sent_path.contains("before=2026-01-15T10%3A00%3A00Z"));
        assert!(sent_path.contains(&format!("before_id={bid}")));
    }

    #[tokio::test]
    async fn runs_relay_errors_propagate_with_correct_status_and_no_retry() {
        for (status, label) in [
            (400u16, "bad_request"),
            (401, "unauthorized"),
            (403, "forbidden"),
            (404, "not_found"),
        ] {
            let (url, attempts, _) = runs_test_server(move |_| {
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    format!(r#"{{"error":"{label}"}}"#),
                )
            })
            .await;
            let client =
                crate::client::BuzzClient::new(url, nostr::Keys::generate(), None, None).unwrap();
            let wf = "550e8400-e29b-41d4-a716-446655440000";
            let path = build_workflow_runs_path(wf, 20, None, None);
            let err = client.get_authed(&path).await.unwrap_err();
            match err {
                crate::error::CliError::Relay { status: s, body } => {
                    assert_eq!(s, status, "must propagate exact status");
                    assert!(body.contains(label), "body={body}");
                }
                other => panic!("expected Relay error for {status}, got {other:?}"),
            }
            // 400/401/403/404 are definitive — must not be retried
            assert_eq!(
                attempts.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "status {status} must not retry"
            );
        }
    }
}
