//! MCP Streamable HTTP endpoint — OAuth 2.0 Protected Resource Metadata spike
//! (RFC 9728).
//!
//! This is the S1 spike for BUZZ-W-012 / REG-3: publish RFC 9728
//! protected-resource metadata and return a 401 with a `WWW-Authenticate`
//! challenge pointing clients at the metadata document. No token validation,
//! no authorization server, and no MCP protocol handling is implemented yet —
//! the endpoint exists solely so external MCP clients can discover that the
//! relay *will* speak OAuth-protected MCP, and so the 401 shape matches what
//! those clients expect per the MCP 2025-06-18 authorization spec.
//!
//! The resource identifier is derived from the relay's configured public URL
//! (`Config::relay_url`, the same value advertised in NIP-11) by converting
//! the `ws`/`wss` scheme to `http`/`https` and appending `/mcp`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Json;
use serde_json::json;

use crate::state::AppState;

/// Normalize the relay's public URL (ws/wss) to an http(s) resource URL for
/// the MCP endpoint. Used by both the metadata document and the 401 challenge
/// so the resource identifier is consistent.
pub(crate) fn mcp_resource_url(relay_url: &str) -> String {
    let base = relay_url
        .trim_end_matches('/')
        .replace("wss://", "https://")
        .replace("ws://", "http://");
    format!("{base}/mcp")
}

/// Derive the well-known metadata URL from the resource URL per RFC 9728 §3:
/// `/.well-known/oauth-protected-resource` is published relative to the
/// resource server's origin.
pub(crate) fn metadata_url(relay_url: &str) -> String {
    let resource = mcp_resource_url(relay_url);
    // The origin is everything up to the path.
    let origin = resource
        .find("://")
        .map(|i| {
            let after_scheme = &resource[i + 3..];
            // Find the first '/' after the host (handles :port and bare host).
            let end = after_scheme.find('/').unwrap_or(after_scheme.len());
            &resource[..i + 3 + end]
        })
        .unwrap_or(&resource);
    format!("{origin}/.well-known/oauth-protected-resource")
}

/// `GET /.well-known/oauth-protected-resource`
///
/// Returns the RFC 9728 protected-resource metadata JSON. The `resource`
/// field is the MCP endpoint URL; `authorization_servers` is intentionally a
/// placeholder pointing back at the relay's own metadata (a real authorization
/// server will be wired in a later stage).
pub async fn protected_resource_metadata(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, HeaderMap, Json<serde_json::Value>) {
    let resource = mcp_resource_url(&state.config.relay_url);
    let meta_url = metadata_url(&state.config.relay_url);

    let body = json!({
        "resource": resource,
        "authorization_servers": [meta_url],
        "bearer_methods_supported": ["header"],
        "scopes_supported": [
            "buzz.channels.read",
            "buzz.messages.read",
            "buzz.tasks.read",
        ],
        "resource_documentation": "https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization",
    });

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("public, max-age=300"),
    );

    (StatusCode::OK, headers, Json(body))
}

/// `POST /mcp`
///
/// Spike: always returns 401 Unauthorized with a `WWW-Authenticate` header
/// pointing the client to the RFC 9728 protected-resource metadata document.
/// No Bearer token validation is performed — this is the discovery-only stage.
pub async fn mcp_unauthorized(State(state): State<Arc<AppState>>) -> (StatusCode, HeaderMap) {
    let meta_url = metadata_url(&state.config.relay_url);

    // RFC 9728 §5.1: the WWW-Authenticate response uses the
    // `resource_metadata` parameter to direct the client to the metadata.
    let challenge = format!(r#"Bearer resource_metadata="{meta_url}""#);

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("www-authenticate"),
        HeaderValue::from_str(&challenge).expect("valid header value"),
    );

    (StatusCode::UNAUTHORIZED, headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_resource_url_converts_ws_to_http() {
        assert_eq!(
            mcp_resource_url("ws://localhost:3000"),
            "http://localhost:3000/mcp"
        );
        assert_eq!(
            mcp_resource_url("wss://relay.example.com"),
            "https://relay.example.com/mcp"
        );
    }

    #[test]
    fn mcp_resource_url_strips_trailing_slash() {
        assert_eq!(
            mcp_resource_url("wss://relay.example.com/"),
            "https://relay.example.com/mcp"
        );
    }

    #[test]
    fn metadata_url_derives_from_origin() {
        assert_eq!(
            metadata_url("wss://relay.example.com"),
            "https://relay.example.com/.well-known/oauth-protected-resource"
        );
        assert_eq!(
            metadata_url("ws://localhost:3000"),
            "http://localhost:3000/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn metadata_url_handles_port() {
        assert_eq!(
            metadata_url("ws://localhost:8080"),
            "http://localhost:8080/.well-known/oauth-protected-resource"
        );
    }
}
