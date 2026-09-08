//! Primary, freshly authorized admission for a newly accepted signed start.

use super::*;
use axum::http::{header, HeaderValue};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionQuery {
    start_event_id: String,
}

/// `GET /api/tasks/{task}/attempts/{attempt}/admission`.
///
/// This read is execution-sensitive. Display state and cache hits must never
/// replace its writer transaction or its exact authenticated worker binding.
/// It is necessary but not sufficient: an adapter must also have received the
/// newly accepted response to its own signed start, never a duplicate/replay.
pub async fn get_fleet_admission(
    State(state): State<Arc<AppState>>,
    Path((task_id, attempt_id)): Path<(Uuid, String)>,
    RawQuery(raw_query): RawQuery,
    Query(query): Query<AdmissionQuery>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<Value>), (StatusCode, Json<Value>)> {
    let path = format!("/api/tasks/{task_id}/attempts/{attempt_id}/admission");
    let (tenant, worker) =
        authorize_task_request(&state, &headers, "GET", &path, raw_query.as_deref(), None).await?;
    if !buzz_core::fleet::valid_attempt_id(&attempt_id)
        || query.start_event_id.len() != 64
        || !query
            .start_event_id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid attempt or start event id",
        ));
    }
    let start = hex::decode(&query.start_event_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid start event id"))?;
    let admission = state
        .db
        .fleet_start_admission(
            tenant.community(),
            task_id,
            &attempt_id,
            &start,
            &worker.to_bytes(),
        )
        .await
        .map_err(|error| match error {
            buzz_db::DbError::AccessDenied(_) | buzz_db::DbError::NotFound(_) => api_error(
                StatusCode::FORBIDDEN,
                "fleet execution admission unavailable",
            ),
            other => internal_error(&format!("fleet admission: {other}")),
        })?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, Json(admission)))
}
