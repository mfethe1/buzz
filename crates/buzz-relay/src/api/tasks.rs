//! Authorized reads and writes for task state.
//!
//! Tasks are relay-owned database rows, not Nostr events — the same modeling
//! choice `api::workflows` makes for runs and approvals, and for the same
//! reason: there is no synthetic event worth inventing for a work item whose
//! whole value is a queryable, mutable read model.
//!
//! Every route is scoped to the **host-derived** tenant, like the rest of the
//! relay's HTTP surface. There is no `/communities/{id}/…` path segment
//! anywhere in Buzz: `crate::tenant::bind_community` resolves the community
//! from the `Host` header, and NIP-98 signatures are bound to that same host,
//! so a client cannot name a community it did not connect to.
//!
//! Channel-bound tasks additionally require the caller to have access to the
//! bound channel, mirroring `api::workflows::authorize_workflow_read`.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::Json,
};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use buzz_core::task::{TaskAction, TaskStatus};
use buzz_core::TenantContext;
use buzz_db::task::{NewTask, TaskCursor, TaskEventRecord, TaskFilter, TaskPatch, TaskRecord};

use crate::{
    api::{api_error, bridge, internal_error},
    state::AppState,
};

const DEFAULT_TASK_LIMIT: i64 = 50;
const MAX_TASK_LIMIT: i64 = 200;
const MAX_TITLE_CHARS: usize = 200;

/// Query filters for `GET /api/tasks`.
#[derive(Debug, Deserialize, Default)]
pub struct TasksQuery {
    status: Option<String>,
    assignee: Option<String>,
    channel: Option<Uuid>,
    source_ref: Option<String>,
    include_archived: Option<bool>,
    limit: Option<i64>,
    before: Option<String>,
}

/// Body of `POST /api/tasks`.
#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    title: String,
    body: Option<String>,
    channel_id: Option<Uuid>,
    parent_task_id: Option<Uuid>,
    assignee: Option<String>,
    priority: Option<i32>,
    due_at: Option<DateTime<Utc>>,
    source: Option<String>,
    source_ref: Option<String>,
}

/// Body of `PATCH /api/tasks/{id}`.
///
/// `assignee` and `due_at` are doubly optional on the wire: an absent key
/// leaves the field alone, while an explicit `null` clears it. `serde`'s
/// `double_option` shape (`Option<Option<T>>` with
/// `skip_serializing_if`/`default`) is what distinguishes the two.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateTaskRequest {
    expected_revision: Option<i32>,
    status: Option<String>,
    title: Option<String>,
    priority: Option<i32>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    due_at: Option<Option<DateTime<Utc>>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    assignee: Option<Option<String>>,
}

/// Body of `POST /api/tasks/{id}/events`.
#[derive(Debug, Deserialize)]
pub struct AppendTaskEventRequest {
    action: Option<String>,
    body: Option<String>,
}

fn deserialize_double_option<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn request_path(path: &str, raw_query: Option<&str>) -> String {
    match raw_query {
        Some(query) if !query.is_empty() => format!("{path}?{query}"),
        _ => path.to_string(),
    }
}

/// Parse a 32-byte pubkey from lowercase hex, rejecting anything else.
fn parse_pubkey(field: &str, raw: &str) -> Result<Vec<u8>, (StatusCode, Json<Value>)> {
    let bytes = hex::decode(raw)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, &format!("{field} must be hex")))?;
    if bytes.len() != 32 {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            &format!("{field} must be a 32-byte pubkey"),
        ));
    }
    Ok(bytes)
}

fn parse_status(raw: &str) -> Result<TaskStatus, (StatusCode, Json<Value>)> {
    raw.parse::<TaskStatus>()
        .map_err(|message| api_error(StatusCode::BAD_REQUEST, &message))
}

/// Reject titles the database's `CHECK (length(title) BETWEEN 1 AND 200)`
/// would reject, so the caller gets a 400 instead of a 500.
///
/// The check counts characters, matching PostgreSQL's `length()` on `TEXT`
/// (which counts characters, not bytes) — using `String::len` here would let a
/// 200-character multi-byte title fail in the database after passing this gate.
fn validate_title(title: &str) -> Result<String, (StatusCode, Json<Value>)> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "title must not be empty",
        ));
    }
    if trimmed.chars().count() > MAX_TITLE_CHARS {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "title must be at most 200 characters",
        ));
    }
    Ok(trimmed.to_owned())
}

/// Map a database error onto the narrowest status the caller can act on.
///
/// Foreign-key violations here always mean the caller named a channel, parent
/// task, or assignee that does not exist in this community — a request error,
/// not a server fault.
fn map_task_error(context: &str, error: buzz_db::DbError) -> (StatusCode, Json<Value>) {
    match &error {
        buzz_db::DbError::StaleRevision { task_id, expected, actual } => api_error(
            StatusCode::CONFLICT,
            &format!("task {task_id} was modified (expected revision {expected}, actual {actual}); re-fetch and retry"),
        ),
        buzz_db::DbError::NotFound(_) => api_error(StatusCode::NOT_FOUND, "task not found"),
        buzz_db::DbError::InvalidData(message) => api_error(StatusCode::BAD_REQUEST, message),
        buzz_db::DbError::AccessDenied(_) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "community writes are temporarily unavailable",
        ),
        buzz_db::DbError::Sqlx(sqlx::Error::Database(db_error))
            if db_error.code().as_deref() == Some("23503") =>
        {
            api_error(
                StatusCode::BAD_REQUEST,
                "channel, parent task, or assignee does not exist in this community",
            )
        }
        buzz_db::DbError::Sqlx(sqlx::Error::Database(db_error))
            if db_error.code().as_deref() == Some("23514") =>
        {
            api_error(StatusCode::BAD_REQUEST, "task violates a field constraint")
        }
        _ => internal_error(&format!("{context}: {error}")),
    }
}

/// Authenticate the caller and bind the request to its host-derived tenant.
///
/// `body` is `Some` for writes; NIP-98 then additionally requires a `payload`
/// tag covering it, so a signature cannot be replayed against a different body.
async fn authorize_task_request(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    raw_query: Option<&str>,
    body: Option<&[u8]>,
) -> Result<(TenantContext, nostr::PublicKey), (StatusCode, Json<Value>)> {
    let raw_host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let tenant = crate::tenant::bind_community(&state.db, raw_host)
        .await
        .map_err(|_| {
            api_error(
                StatusCode::NOT_FOUND,
                "relay: no community is configured for this host",
            )
        })?;

    let path_with_query = request_path(path, raw_query);
    let url = bridge::nip98_expected_url(&state.config.relay_url, &tenant, &path_with_query);
    let bridge::VerifiedBridgeAuth {
        pubkey,
        event_id_bytes,
        signed_created_at,
    } = bridge::verify_bridge_auth_with_options(
        headers,
        method,
        &url,
        body,
        state.config.require_auth_token,
        body.is_some(),
    )?;
    bridge::enforce_http_admission(state, &tenant, &pubkey).await?;
    bridge::check_nip98_replay(state, &tenant, event_id_bytes).await?;

    let pubkey_bytes = pubkey.to_bytes().to_vec();
    let auth_tag = headers
        .get("x-auth-tag")
        .and_then(|value| value.to_str().ok());
    super::relay_members::enforce_relay_membership(
        state,
        tenant.community(),
        &pubkey_bytes,
        auth_tag,
        signed_created_at,
    )
    .await?;

    Ok((tenant, pubkey))
}

/// Reject access to a channel-bound task the caller cannot see.
///
/// A task with no channel is community-wide and needs no further check; relay
/// membership already gated it.
async fn enforce_channel_access(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    pubkey: &nostr::PublicKey,
    channel_id: Option<Uuid>,
) -> Result<(), (StatusCode, Json<Value>)> {
    let Some(channel_id) = channel_id else {
        return Ok(());
    };
    let accessible = state
        .get_accessible_channel_ids_cached(tenant.community(), &pubkey.to_bytes())
        .await
        .map_err(|error| internal_error(&format!("task channel access lookup: {error}")))?;
    if !accessible.contains(&channel_id) {
        // 404, not 403: the caller cannot see this channel, so it must not
        // learn that a task exists in it.
        return Err(api_error(StatusCode::NOT_FOUND, "task not found"));
    }
    Ok(())
}

/// `POST /api/tasks` — create a task. Requires relay membership.
pub async fn create_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (tenant, pubkey) =
        authorize_task_request(&state, &headers, "POST", "/api/tasks", None, Some(&body)).await?;

    let request: CreateTaskRequest = serde_json::from_slice(&body)
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, &format!("invalid task JSON: {e}")))?;

    let title = validate_title(&request.title)?;
    let assignee = request
        .assignee
        .as_deref()
        .map(|raw| parse_pubkey("assignee", raw))
        .transpose()?;

    enforce_channel_access(&state, &tenant, &pubkey, request.channel_id).await?;

    // `tasks.created_by_pubkey` is a community-scoped FK into `users`. An
    // authenticated member may still have no `users` row yet (it is created
    // lazily on first profile write), so materialize it before the insert.
    let creator = pubkey.to_bytes().to_vec();
    state
        .db
        .ensure_user(tenant.community(), &creator)
        .await
        .map_err(|error| internal_error(&format!("ensure task creator: {error}")))?;

    let task = state
        .db
        .create_task(
            tenant.community(),
            NewTask {
                channel_id: request.channel_id,
                created_by_pubkey: Some(creator),
                assignee_pubkey: assignee,
                parent_task_id: request.parent_task_id,
                title,
                body: request.body,
                priority: request.priority.unwrap_or(0),
                // 'app' marks a task typed by a person in a Buzz client, as
                // distinct from one a harness opened on their behalf.
                source: Some(request.source.unwrap_or_else(|| "app".to_owned())),
                source_ref: request.source_ref,
                due_at: request.due_at,
            },
        )
        .await
        .map_err(|error| map_task_error("create task", error))?;

    Ok(Json(task_json(&task)))
}

// Cursor timestamps retain subsecond precision; the task wire format intentionally
// keeps its existing seconds representation for old clients.
fn decode_task_cursor(raw: &str) -> Result<TaskCursor, (StatusCode, Json<Value>)> {
    let invalid = || api_error(StatusCode::BAD_REQUEST, "invalid task cursor");
    if raw.len() > 256 {
        return Err(invalid());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

fn encode_task_cursor(task: &TaskRecord) -> Result<String, (StatusCode, Json<Value>)> {
    let bytes = serde_json::to_vec(&TaskCursor {
        updated_at: task.updated_at,
        id: task.id,
    })
    .map_err(|error| internal_error(&format!("encode task cursor: {error}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// `GET /api/tasks` — list this community's tasks, newest-modified first.
pub async fn list_tasks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    Query(query): Query<TasksQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let limit = query.limit.unwrap_or(DEFAULT_TASK_LIMIT);
    if !(1..=MAX_TASK_LIMIT).contains(&limit) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 200",
        ));
    }
    let before = query
        .before
        .as_deref()
        .map(decode_task_cursor)
        .transpose()?;
    let status = query.status.as_deref().map(parse_status).transpose()?;
    let assignee = query
        .assignee
        .as_deref()
        .map(|raw| parse_pubkey("assignee", raw))
        .transpose()?;

    let (tenant, pubkey) = authorize_task_request(
        &state,
        &headers,
        "GET",
        "/api/tasks",
        raw_query.as_deref(),
        None,
    )
    .await?;

    if let Some(channel_id) = query.channel {
        enforce_channel_access(&state, &tenant, &pubkey, Some(channel_id)).await?;
    }

    // The visibility predicate must be part of the database query: filtering
    // an already-limited page can hide all accessible work behind private tasks.
    let accessible = state
        .get_accessible_channel_ids_cached(tenant.community(), &pubkey.to_bytes())
        .await
        .map_err(|error| internal_error(&format!("task channel access lookup: {error}")))?;
    let mut tasks = state
        .db
        .list_tasks(
            tenant.community(),
            &TaskFilter {
                status,
                assignee_pubkey: assignee,
                channel_id: query.channel,
                source_ref: query.source_ref.clone(),
                include_archived: query.include_archived.unwrap_or(false),
                visible_channel_ids: Some(accessible.into_iter().collect()),
                before,
                limit: limit + 1,
            },
        )
        .await
        .map_err(|error| map_task_error("list tasks", error))?;

    let has_more = tasks.len() > limit as usize;
    tasks.truncate(limit as usize);
    let next_cursor = if has_more {
        tasks.last().map(encode_task_cursor).transpose()?
    } else {
        None
    };
    let visible: Vec<Value> = tasks.iter().map(task_json).collect();
    Ok(Json(
        serde_json::json!({ "tasks": visible, "next_cursor": next_cursor }),
    ))
}

/// `GET /api/tasks/{id}` — one task plus its full event history.
pub async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let path = format!("/api/tasks/{task_id}");
    let (tenant, pubkey) =
        authorize_task_request(&state, &headers, "GET", &path, None, None).await?;

    let task = state
        .db
        .get_task(tenant.community(), task_id)
        .await
        .map_err(|error| map_task_error("get task", error))?;
    enforce_channel_access(&state, &tenant, &pubkey, task.channel_id).await?;

    let events = state
        .db
        .list_task_events(tenant.community(), task_id)
        .await
        .map_err(|error| map_task_error("list task events", error))?;

    Ok(Json(serde_json::json!({
        "task": task_json(&task),
        "events": events.iter().map(task_event_json).collect::<Vec<_>>(),
    })))
}

/// `PATCH /api/tasks/{id}` — update a task, appending its history entries.
pub async fn update_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let path = format!("/api/tasks/{task_id}");
    let (tenant, pubkey) =
        authorize_task_request(&state, &headers, "PATCH", &path, None, Some(&body)).await?;

    let request: UpdateTaskRequest = serde_json::from_slice(&body)
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, &format!("invalid task JSON: {e}")))?;

    let patch = TaskPatch {
        expected_revision: request.expected_revision,
        status: request.status.as_deref().map(parse_status).transpose()?,
        title: request.title.as_deref().map(validate_title).transpose()?,
        priority: request.priority,
        due_at: request.due_at,
        assignee_pubkey: match request.assignee {
            None => None,
            Some(None) => Some(None),
            Some(Some(raw)) => Some(Some(parse_pubkey("assignee", &raw)?)),
        },
    };
    if patch.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "patch must change at least one field",
        ));
    }

    // Authorize against the task's *current* channel before mutating it.
    let existing = state
        .db
        .get_task(tenant.community(), task_id)
        .await
        .map_err(|error| map_task_error("get task", error))?;
    enforce_channel_access(&state, &tenant, &pubkey, existing.channel_id).await?;

    let task = state
        .db
        .update_task(
            tenant.community(),
            task_id,
            &patch,
            Some(&pubkey.to_bytes()),
        )
        .await
        .map_err(|error| map_task_error("update task", error))?;

    Ok(Json(task_json(&task)))
}

/// `POST /api/tasks/{id}/events` — append a comment or summary.
pub async fn append_task_event(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let path = format!("/api/tasks/{task_id}/events");
    let (tenant, pubkey) =
        authorize_task_request(&state, &headers, "POST", &path, None, Some(&body)).await?;

    let request: AppendTaskEventRequest = serde_json::from_slice(&body)
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, &format!("invalid event JSON: {e}")))?;

    let action = match request.action.as_deref() {
        None => TaskAction::Commented,
        Some(raw) => raw
            .parse::<TaskAction>()
            .map_err(|message| api_error(StatusCode::BAD_REQUEST, &message))?,
    };
    // Lifecycle actions are derived from the mutation that caused them; letting
    // a caller post one directly would let it fabricate a transition history
    // that never happened.
    if !matches!(action, TaskAction::Commented | TaskAction::SummaryPersisted) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "only 'commented' and 'summary_persisted' may be posted directly",
        ));
    }
    let event_body = request
        .body
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "body must not be empty"))?;

    let task = state
        .db
        .get_task(tenant.community(), task_id)
        .await
        .map_err(|error| map_task_error("get task", error))?;
    enforce_channel_access(&state, &tenant, &pubkey, task.channel_id).await?;

    let actor = pubkey.to_bytes().to_vec();
    state
        .db
        .ensure_user(tenant.community(), &actor)
        .await
        .map_err(|error| internal_error(&format!("ensure task actor: {error}")))?;

    let event = state
        .db
        .append_task_event(
            tenant.community(),
            task_id,
            Some(&actor),
            action,
            Some(event_body),
        )
        .await
        .map_err(|error| map_task_error("append task event", error))?;

    Ok(Json(task_event_json(&event)))
}

fn task_json(task: &TaskRecord) -> Value {
    serde_json::json!({
        "id": task.id,
        "channel_id": task.channel_id,
        "created_by": task.created_by_pubkey.as_ref().map(hex::encode),
        "assignee": task.assignee_pubkey.as_ref().map(hex::encode),
        "parent_task_id": task.parent_task_id,
        "title": task.title,
        "body": task.body,
        "status": task.status.as_str(),
        "priority": task.priority,
        "source": task.source,
        "source_ref": task.source_ref,
        "due_at": task.due_at.map(|value| value.timestamp()),
        "done_at": task.done_at.map(|value| value.timestamp()),
        "archived_at": task.archived_at.map(|value| value.timestamp()),
        "created_at": task.created_at.timestamp(),
        "updated_at": task.updated_at.timestamp(),
        "revision": task.revision,
    })
}

fn task_event_json(event: &TaskEventRecord) -> Value {
    serde_json::json!({
        "id": event.id,
        "task_id": event.task_id,
        "actor": event.actor_pubkey.as_ref().map(hex::encode),
        "action": event.action.as_str(),
        "from_status": event.from_status.map(|status| status.as_str()),
        "to_status": event.to_status.map(|status| status.as_str()),
        "body": event.body,
        "changes": event.changes,
        "created_at": event.created_at.timestamp(),
    })
}

#[cfg(test)]
#[path = "tasks/tests.rs"]
mod tests;
