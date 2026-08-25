//! `buzz-mcp` — MCP context server exposing Buzz channel/thread/task history
//! to local agents over stdio, relay-backed over the existing NIP-98 HTTP
//! bridge (`POST /query`, `GET /api/tasks`).
//!
//! v1 is READ-ONLY by design (see `registry/work/REG-3/reflecting.md`): the
//! blast radius of a prompt-injected agent is bounded by what the operator can
//! already read. Writes are deferred to v2.
//!
//! Identity: the operator's own nostr key (`BUZZ_PRIVATE_KEY`), exactly like
//! the `buzz` CLI. The signing key never leaves this process — the
//! `buzz-dev-mcp` shim's key-injection into children (`shim.rs`) is a dev-tool
//! pattern and is deliberately NOT reused here.
//!
//! No new `BUZZ_ACP_MCP_*` env knob is minted: the server is configured by the
//! client's own MCP config plus the two env vars the CLI already defines.

use base64::Engine;
use nostr::prelude::*;
use serde_json::Value;
use sha2::Digest;

/// Hard server-side clamp on `limit` for every tool, mirroring the relay's
/// own clamps (`api/tasks.rs` MAX_TASK_LIMIT=200, CLI messages limit min 200).
pub(crate) const MAX_LIMIT: u32 = 200;
pub(crate) const DEFAULT_LIMIT: u32 = 50;

/// Total request timeout (relay-side default `BUZZ_TIMEOUT_SECS` is 30s; we
/// bound the whole tool call so an agent never hangs on a dead relay).
const REQUEST_TIMEOUT_SECS: u64 = 30;

#[derive(Debug)]
pub enum RelayError {
    /// Relay unreachable / timed out / connection refused — transient.
    Transport(String),
    /// Relay answered with a non-2xx status (authz denial, bad request).
    Status(u16, String),
    /// Response body was not the expected shape.
    Malformed(String),
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayError::Transport(e) => write!(f, "relay unreachable: {e}"),
            RelayError::Status(code, body) => write!(f, "relay returned {code}: {body}"),
            RelayError::Malformed(e) => write!(f, "malformed relay response: {e}"),
        }
    }
}

/// Thin NIP-98-signed HTTP client for the relay bridge, modeled on
/// `buzz-cli/src/client.rs` (`sign_nip98` :84-110, `query` :767-798,
/// `get_authed` :836-850). The CLI's `BuzzClient` is crate-private, so this
/// is a re-implementation of exactly the two calls v1 needs, no more.
pub struct RelayClient {
    http: reqwest::Client,
    relay_url: String,
    keys: Keys,
}

impl RelayClient {
    pub fn new(relay_url: String, keys: Keys) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
                .build()
                .expect("reqwest client builder"),
            relay_url: relay_url.trim_end_matches('/').to_string(),
            keys,
        }
    }

    /// Sign a NIP-98 HTTP auth event (kind:27235) — same tag set as the CLI's
    /// `sign_nip98`: `u`, `method`, `nonce`, and `payload` (sha256 of body).
    fn sign_nip98(&self, method: &str, url: &str, body: Option<&[u8]>) -> Result<String, String> {
        let mk = |k: &str, v: &str| Tag::parse([k, v]).map_err(|e| format!("tag error: {e}"));
        let mut tags = vec![
            mk("u", url)?,
            mk("method", method)?,
            // Nonce prevents replay rejection for rapid-fire requests with
            // identical bodies (relay's Nip98ReplayGuard sees each event once).
            mk("nonce", &uuid::Uuid::new_v4().to_string())?,
        ];
        if let Some(b) = body {
            let hash = hex::encode(<sha2::Sha256 as Digest>::digest(b));
            tags.push(mk("payload", &hash)?);
        }
        let event = EventBuilder::new(Kind::Custom(27235), "")
            .tags(tags)
            .sign_with_keys(&self.keys)
            .map_err(|e| format!("NIP-98 signing failed: {e}"))?;
        let json = event.as_json();
        Ok(format!(
            "Nostr {}",
            base64::engine::general_purpose::STANDARD.encode(json.as_bytes())
        ))
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Value, RelayError> {
        let url = format!("{}{path}", self.relay_url);
        let auth = self
            .sign_nip98(method.as_str(), &url, body.as_deref())
            .map_err(|e| RelayError::Transport(e))?;
        let mut req = self
            .http
            .request(method, &url)
            .header("Authorization", auth)
            .header("Accept", "application/json");
        if let Some(b) = body {
            req = req.header("Content-Type", "application/json").body(b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| RelayError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| RelayError::Transport(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(RelayError::Status(status, text));
        }
        serde_json::from_str(&text)
            .map_err(|e| RelayError::Malformed(format!("non-JSON body: {e}")))
    }

    /// `POST /query` with a single Nostr filter. Returns the raw event array.
    pub async fn query(&self, filter: &Value) -> Result<Vec<Value>, RelayError> {
        let body = serde_json::to_vec(&[filter])
            .map_err(|e| RelayError::Malformed(format!("filter serialization: {e}")))?;
        let v = self
            .send(reqwest::Method::POST, "/query", Some(body))
            .await?;
        v.as_array()
            .cloned()
            .ok_or_else(|| RelayError::Malformed("expected JSON array of events".into()))
    }

    /// `GET` an authed JSON endpoint (e.g. `/api/tasks`).
    pub async fn get_authed(&self, path: &str) -> Result<Value, RelayError> {
        self.send(reqwest::Method::GET, path, None).await
    }
}

/// Clamp a user-supplied limit exactly like the CLI (`min(200)`).
pub fn clamp_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Extract `[["d", value]]` from an event's tags.
pub fn extract_d_tag(event: &Value) -> Option<String> {
    event
        .get("tags")?
        .as_array()?
        .iter()
        .find(|t| {
            t.as_array()
                .map(|a| a.first().and_then(|v| v.as_str()) == Some("d"))
                .unwrap_or(false)
        })
        .and_then(|t| t.get(1).and_then(|v| v.as_str()))
        .map(str::to_owned)
}

/// Parse the relay's task JSON into the projection we surface (keeps tool
/// output stable and small for the consuming agent).
pub fn project_task(v: &Value) -> Value {
    let pick = |k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    serde_json::json!({
        "id": pick("id"),
        "title": pick("title"),
        "status": pick("status"),
        "priority": pick("priority"),
        "channel_id": pick("channel_id"),
        "assignee_pubkey": pick("assignee_pubkey"),
        "created_by_pubkey": pick("created_by_pubkey"),
        "parent_task_id": pick("parent_task_id"),
        "due_at": pick("due_at"),
        "created_at": pick("created_at"),
        "updated_at": pick("updated_at"),
    })
}

/// Message kinds the relay serves for channel history — same set the CLI's
/// `cmd_get_messages` uses (`messages.rs:369`).
pub(crate) const MESSAGE_KINDS: [u64; 5] = [9, 40002, 40008, 45001, 45003];
/// Thread reply kinds (`messages.rs:441`) — 40003 threads, 45003 reactions.
pub(crate) const THREAD_KINDS: [u64; 5] = [9, 40002, 40003, 40008, 45003];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_limit_defaults_and_clamps() {
        assert_eq!(clamp_limit(None), 50);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(10_000)), 200);
    }

    #[test]
    fn extract_d_tag_finds_first_d() {
        let e = serde_json::json!({
            "tags": [["h","abc"],["d","chan-1"],["d","chan-2"]]
        });
        assert_eq!(extract_d_tag(&e).as_deref(), Some("chan-1"));
        assert_eq!(extract_d_tag(&serde_json::json!({"tags": []})), None);
    }

    #[test]
    fn project_task_picks_known_fields_and_nulls_missing() {
        let t = serde_json::json!({"id": "t1", "title": "T", "bogus": 1});
        let p = project_task(&t);
        assert_eq!(p["id"], "t1");
        assert_eq!(p["title"], "T");
        assert!(p["status"].is_null());
        assert!(p.get("bogus").is_none());
    }
}
