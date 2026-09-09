//! Strict signed owner-only machine reads. No directory, admin or dev-auth fallback.

use crate::{
    api::{api_error, bridge, internal_error},
    state::AppState,
};
use axum::{
    extract::{Path, Query, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Json,
};
use buzz_core::TenantContext;
use nostr::PublicKey;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

type Failure = (StatusCode, Json<Value>);

/// Owner-scoped ascending machine UUID pagination.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineQuery {
    after: Option<Uuid>,
    limit: Option<i64>,
}

async fn authorize(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    path: &str,
    query: Option<&str>,
) -> Result<(TenantContext, PublicKey), Failure> {
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return Err(api_error(
            StatusCode::UNAUTHORIZED,
            "one signed Authorization header required",
        ));
    }
    let mut hosts = headers.get_all(header::HOST).iter();
    let (Some(host), None) = (hosts.next(), hosts.next()) else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "one Host header required",
        ));
    };
    let host = host
        .to_str()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid Host"))?;
    let tenant = crate::tenant::bind_community(&state.db, host)
        .await
        .map_err(|_| api_error(StatusCode::NOT_FOUND, "community unavailable"))?;
    let path = query.map_or_else(|| path.to_owned(), |query| format!("{path}?{query}"));
    let url = bridge::nip98_expected_url(&state.config.relay_url, &tenant, &path);
    // `true` is intentional even when the relay otherwise allows dev X-Pubkey.
    let auth = bridge::verify_bridge_auth_with_options(headers, "GET", &url, None, true, false)?;
    bridge::enforce_http_admission(state, &tenant, &auth.pubkey).await?;
    bridge::check_nip98_replay(state, &tenant, auth.event_id_bytes).await?;
    super::relay_members::enforce_relay_membership(
        state,
        tenant.community(),
        &auth.pubkey.to_bytes(),
        super::relay_members::extract_auth_tag_header(headers),
        auth.signed_created_at,
    )
    .await?;
    let restrictions = state
        .db
        .moderation_restriction_state(tenant.community(), &auth.pubkey.to_bytes())
        .await
        .map_err(|_| internal_error("machine read restriction lookup failed"))?;
    if restrictions.banned {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "community access unavailable",
        ));
    }
    Ok((tenant, auth.pubkey))
}

fn private_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers
}

/// GET /api/machines: ownership is applied in SQL before pagination.
pub async fn list_machines(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
    Query(query): Query<MachineQuery>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<Value>), Failure> {
    let (tenant, owner) = authorize(&state, &headers, "/api/machines", raw.as_deref()).await?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 100",
        ));
    }
    let mut machines = state
        .db
        .list_machines(
            tenant.community(),
            &owner.to_bytes(),
            query.after,
            limit + 1,
        )
        .await
        .map_err(|_| internal_error("machine list failed"))?;
    let next = if machines.len() > limit as usize {
        machines.truncate(limit as usize);
        machines
            .last()
            .and_then(|machine| machine.get("machine_id"))
            .cloned()
    } else {
        None
    };
    Ok((
        private_headers(),
        Json(json!({"machines": machines, "next_cursor": next})),
    ))
}

/// GET /api/machines/{id}: other owners and tenants receive the same missing result.
pub async fn get_machine(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<Value>), Failure> {
    let (tenant, owner) = authorize(
        &state,
        &headers,
        &format!("/api/machines/{id}"),
        raw.as_deref(),
    )
    .await?;
    let machine = state
        .db
        .get_machine(tenant.community(), &owner.to_bytes(), id)
        .await
        .map_err(|_| internal_error("machine detail failed"))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "machine not found"))?;
    Ok((private_headers(), Json(machine)))
}
