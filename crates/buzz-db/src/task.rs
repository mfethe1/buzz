//! Task and task-event persistence.
//!
//! Tasks are durable work items owned by a human or a harness agent (Claude
//! Code, Codex, the ACP mesh). They are relay-owned rows rather than Nostr
//! events, the same modeling choice already made for `workflow_runs` and
//! `workflow_approvals`, and they are unrelated to `buzz-workflow`'s scheduled
//! execution engine.
//!
//! Every statement here binds `community_id` first, matching the tenant
//! invariant that `(community_id, id)` — never a bare `id` — names a task. A
//! task id presented against the wrong tenant reads as absent, not as another
//! community's row.
//!
//! `task_events` is append-only: mutations record what changed instead of
//! overwriting history. `update_task` therefore runs the read, the write, and
//! the event append in one transaction with `SELECT … FOR UPDATE` on the task
//! row, so two concurrent PATCHes cannot interleave into a log that claims a
//! transition neither of them made.

use buzz_core::task::{status_change_action, TaskAction, TaskStatus};
use chrono::{DateTime, SubsecRound, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, QueryBuilder, Row as _, Transaction};
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::CommunityId;

/// Columns selected for every [`TaskRecord`]. Kept in one place so the row
/// parser and every query cannot drift apart.
///
/// A macro rather than a `const` so callers can splice it with `concat!` and
/// keep every statement a true string literal — sqlx only accepts `&'static
/// str` without an `AssertSqlSafe` escape hatch, and there is nothing dynamic
/// here worth asserting past.
macro_rules! task_columns {
    () => {
        "community_id, id, channel_id, created_by_pubkey, assignee_pubkey, \
         parent_task_id, title, body, status, priority, source, source_ref, \
         due_at, done_at, archived_at, created_at, updated_at, revision"
    };
}

/// Columns selected for every [`TaskEventRecord`].
macro_rules! task_event_columns {
    () => {
        "id, task_id, actor_pubkey, action, from_status, to_status, body, changes, created_at"
    };
}

/// A durable work item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    /// Task id, unique within its community.
    pub id: Uuid,
    /// Channel this task is bound to, if any.
    pub channel_id: Option<Uuid>,
    /// Creator's pubkey. Agents are users, so this covers both.
    pub created_by_pubkey: Option<Vec<u8>>,
    /// Current assignee's pubkey.
    pub assignee_pubkey: Option<Vec<u8>>,
    /// Parent task, for subtasks.
    pub parent_task_id: Option<Uuid>,
    /// Short title (1–200 characters).
    pub title: String,
    /// Long-form description.
    pub body: Option<String>,
    /// Lifecycle status.
    pub status: TaskStatus,
    /// Sort priority; higher sorts first.
    pub priority: i32,
    /// Harness origin (`manual`, `claude`, `codex`, `acp`, `mesh`, …).
    pub source: Option<String>,
    /// External reference owned by that harness.
    pub source_ref: Option<String>,
    /// Due date.
    pub due_at: Option<DateTime<Utc>>,
    /// Completion timestamp. Set exactly when `status` is `done`.
    pub done_at: Option<DateTime<Utc>>,
    /// Archive timestamp.
    pub archived_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last-modification timestamp.
    pub updated_at: DateTime<Utc>,
    /// Monotonic version, advanced only when persisted task fields change.
    pub revision: i32,
}

/// One entry in a task's append-only history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEventRecord {
    /// Monotonic event id within the community.
    pub id: i64,
    /// The task this event belongs to.
    pub task_id: Uuid,
    /// Who performed the action.
    pub actor_pubkey: Option<Vec<u8>>,
    /// What happened.
    pub action: TaskAction,
    /// Status before a `status_changed` event.
    pub from_status: Option<TaskStatus>,
    /// Status after a `status_changed` event.
    pub to_status: Option<TaskStatus>,
    /// Comment or summary text.
    pub body: Option<String>,
    /// Structured before/after values, absent for legacy events and comments.
    pub changes: Option<Value>,
    /// When it happened.
    pub created_at: DateTime<Utc>,
}

/// Fields accepted when creating a task.
#[derive(Debug, Clone, Default)]
pub struct NewTask {
    /// Channel to bind the task to.
    pub channel_id: Option<Uuid>,
    /// Creator's pubkey (the authenticated caller).
    pub created_by_pubkey: Option<Vec<u8>>,
    /// Initial assignee.
    pub assignee_pubkey: Option<Vec<u8>>,
    /// Parent task, for subtasks.
    pub parent_task_id: Option<Uuid>,
    /// Short title (1–200 characters).
    pub title: String,
    /// Long-form description.
    pub body: Option<String>,
    /// Sort priority.
    pub priority: i32,
    /// Harness origin.
    pub source: Option<String>,
    /// External reference owned by that harness.
    pub source_ref: Option<String>,
    /// Due date.
    pub due_at: Option<DateTime<Utc>>,
}

/// Exclusive boundary for newest-modified-first task pagination.
///
/// The full timestamp precision and id tie-breaker must both survive the wire.
/// This is a live keyset, not a snapshot: tasks modified between pages move
/// ahead of the cursor and can be found by refreshing the first page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCursor {
    /// Last task's modification timestamp, with database precision.
    pub updated_at: DateTime<Utc>,
    /// Last task's id, breaking equal-timestamp ties.
    pub id: Uuid,
}

/// Filters for [`list_tasks`]. `None` means "do not filter on this field".
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    /// Restrict to one status.
    pub status: Option<TaskStatus>,
    /// Restrict to one assignee.
    pub assignee_pubkey: Option<Vec<u8>>,
    /// Restrict to one channel.
    pub channel_id: Option<Uuid>,
    /// Restrict to tasks originating from one harness reference.
    ///
    /// Exact equality only. `source_ref` is opaque TEXT (see
    /// `migrations/0046_task_system.sql`), so no parsing or prefix matching is
    /// applied here.
    pub source_ref: Option<String>,
    /// Include archived tasks. Archived tasks are hidden by default.
    pub include_archived: bool,
    /// Caller-visible channels, applied before the limit. `Some([])` permits
    /// only channel-less tasks; `None` leaves visibility to a trusted caller.
    pub visible_channel_ids: Option<Vec<Uuid>>,
    /// Return rows strictly after this boundary in newest-modified order.
    pub before: Option<TaskCursor>,
    /// Maximum rows to return.
    pub limit: i64,
}

/// Fields a PATCH may change. `None` means "leave unchanged".
///
/// `assignee_pubkey` is a nested `Option` because unassigning is a real
/// operation: `Some(None)` clears the assignee, while `None` leaves it alone.
/// The same distinction applies to `due_at`.
#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    /// Optional optimistic concurrency guard; omission preserves unguarded writes.
    pub expected_revision: Option<i32>,
    /// New status.
    pub status: Option<TaskStatus>,
    /// New title.
    pub title: Option<String>,
    /// New priority.
    pub priority: Option<i32>,
    /// New due date, or `Some(None)` to clear it.
    pub due_at: Option<Option<DateTime<Utc>>>,
    /// New assignee, or `Some(None)` to unassign.
    pub assignee_pubkey: Option<Option<Vec<u8>>>,
}

impl TaskPatch {
    /// Whether the patch asks for any change at all.
    pub fn is_empty(&self) -> bool {
        self.status.is_none()
            && self.title.is_none()
            && self.priority.is_none()
            && self.due_at.is_none()
            && self.assignee_pubkey.is_none()
    }
}

fn parse_status(raw: &str) -> Result<TaskStatus> {
    raw.parse::<TaskStatus>().map_err(DbError::InvalidData)
}

fn parse_task_row(row: &sqlx::postgres::PgRow) -> Result<TaskRecord> {
    let status: String = row.try_get("status")?;
    Ok(TaskRecord {
        id: row.try_get("id")?,
        channel_id: row.try_get("channel_id")?,
        created_by_pubkey: row.try_get("created_by_pubkey")?,
        assignee_pubkey: row.try_get("assignee_pubkey")?,
        parent_task_id: row.try_get("parent_task_id")?,
        title: row.try_get("title")?,
        body: row.try_get("body")?,
        status: parse_status(&status)?,
        priority: row.try_get("priority")?,
        source: row.try_get("source")?,
        source_ref: row.try_get("source_ref")?,
        due_at: row.try_get("due_at")?,
        done_at: row.try_get("done_at")?,
        archived_at: row.try_get("archived_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        revision: row.try_get("revision")?,
    })
}

fn parse_task_event_row(row: &sqlx::postgres::PgRow) -> Result<TaskEventRecord> {
    let action: String = row.try_get("action")?;
    let from_status: Option<String> = row.try_get("from_status")?;
    let to_status: Option<String> = row.try_get("to_status")?;
    Ok(TaskEventRecord {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        actor_pubkey: row.try_get("actor_pubkey")?,
        action: action.parse::<TaskAction>().map_err(DbError::InvalidData)?,
        from_status: from_status.as_deref().map(parse_status).transpose()?,
        to_status: to_status.as_deref().map(parse_status).transpose()?,
        body: row.try_get("body")?,
        changes: row.try_get("changes")?,
        created_at: row.try_get("created_at")?,
    })
}

#[derive(Default)]
struct TaskEventContent<'a> {
    transition: Option<(TaskStatus, TaskStatus)>,
    body: Option<&'a str>,
    changes: Option<Value>,
}

/// Append a history row in the same transaction as its task mutation.
async fn insert_task_event(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    task_id: Uuid,
    actor_pubkey: Option<&[u8]>,
    action: TaskAction,
    content: TaskEventContent<'_>,
) -> Result<TaskEventRecord> {
    let (from_status, to_status) = match content.transition {
        Some((from, to)) => (Some(from), Some(to)),
        None => (None, None),
    };
    let row = sqlx::query(concat!(
        "INSERT INTO task_events \
           (community_id, task_id, actor_pubkey, action, from_status, to_status, body, changes) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING ",
        task_event_columns!()
    ))
    .bind(community.as_uuid())
    .bind(task_id)
    .bind(actor_pubkey)
    .bind(action.as_str())
    .bind(from_status.map(|status| status.as_str()))
    .bind(to_status.map(|status| status.as_str()))
    .bind(content.body)
    .bind(content.changes)
    .fetch_one(&mut **tx)
    .await?;
    parse_task_event_row(&row)
}

/// Create a task and its opening `created` history entry in one transaction.
///
/// The two must commit together: a task with no history would be invisible to
/// the task feed, which reads `task_events`.
pub async fn create_task(
    pool: &PgPool,
    community: CommunityId,
    new_task: NewTask,
) -> Result<TaskRecord> {
    let mut tx = pool.begin().await?;

    let row = sqlx::query(concat!(
        "INSERT INTO tasks \
           (community_id, channel_id, created_by_pubkey, assignee_pubkey, parent_task_id, \
            title, body, priority, source, source_ref, due_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
         RETURNING ",
        task_columns!()
    ))
    .bind(community.as_uuid())
    .bind(new_task.channel_id)
    .bind(new_task.created_by_pubkey.as_deref())
    .bind(new_task.assignee_pubkey.as_deref())
    .bind(new_task.parent_task_id)
    .bind(&new_task.title)
    .bind(new_task.body.as_deref())
    .bind(new_task.priority)
    .bind(new_task.source.as_deref())
    .bind(new_task.source_ref.as_deref())
    .bind(new_task.due_at)
    .fetch_one(&mut *tx)
    .await?;
    let task = parse_task_row(&row)?;

    insert_task_event(
        &mut tx,
        community,
        task.id,
        new_task.created_by_pubkey.as_deref(),
        TaskAction::Created,
        TaskEventContent {
            changes: Some(json!({
                "title": {"from": null, "to": task.title},
                "assignee": {"from": null, "to": task.assignee_pubkey.as_ref().map(hex::encode)},
                "priority": {"from": null, "to": task.priority},
                "due_at": {"from": null, "to": task.due_at},
            })),
            ..TaskEventContent::default()
        },
    )
    .await?;

    tx.commit().await?;
    Ok(task)
}

/// Read one task, or [`DbError::NotFound`] when no such task exists *in this
/// community*.
pub async fn get_task(pool: &PgPool, community: CommunityId, id: Uuid) -> Result<TaskRecord> {
    let row = sqlx::query(concat!(
        "SELECT ",
        task_columns!(),
        " FROM tasks WHERE community_id = $1 AND id = $2"
    ))
    .bind(community.as_uuid())
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("task {id}")))?;
    parse_task_row(&row)
}

/// List tasks newest-modified first, filtered by `filter`.
pub async fn list_tasks(
    pool: &PgPool,
    community: CommunityId,
    filter: &TaskFilter,
) -> Result<Vec<TaskRecord>> {
    let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
    builder.push(task_columns!());
    builder.push(" FROM tasks WHERE community_id = ");
    builder.push_bind(community.as_uuid());

    if let Some(status) = filter.status {
        builder.push(" AND status = ");
        builder.push_bind(status.as_str());
    }
    if let Some(assignee) = filter.assignee_pubkey.as_deref() {
        builder.push(" AND assignee_pubkey = ");
        builder.push_bind(assignee);
    }
    if let Some(channel_id) = filter.channel_id {
        builder.push(" AND channel_id = ");
        builder.push_bind(channel_id);
    }
    if let Some(source_ref) = filter.source_ref.as_deref() {
        builder.push(" AND source_ref = ");
        builder.push_bind(source_ref.to_owned());
    }
    if !filter.include_archived {
        builder.push(" AND archived_at IS NULL");
    }
    if let Some(channels) = &filter.visible_channel_ids {
        builder.push(" AND (channel_id IS NULL OR channel_id = ANY(");
        builder.push_bind(channels);
        builder.push("))");
    }
    if let Some(cursor) = &filter.before {
        builder.push(" AND (updated_at, id) < (");
        builder.push_bind(cursor.updated_at);
        builder.push(", ");
        builder.push_bind(cursor.id);
        builder.push(")");
    }
    builder.push(" ORDER BY updated_at DESC, id DESC LIMIT ");
    builder.push_bind(filter.limit);

    builder
        .build()
        .fetch_all(pool)
        .await?
        .iter()
        .map(parse_task_row)
        .collect()
}

/// Read one task's history oldest-first.
pub async fn list_task_events(
    pool: &PgPool,
    community: CommunityId,
    task_id: Uuid,
) -> Result<Vec<TaskEventRecord>> {
    sqlx::query(concat!(
        "SELECT ",
        task_event_columns!(),
        " FROM task_events \
         WHERE community_id = $1 AND task_id = $2 \
         ORDER BY created_at ASC, id ASC"
    ))
    .bind(community.as_uuid())
    .bind(task_id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(parse_task_event_row)
    .collect()
}

/// Apply a patch, appending one history row per field that actually changed.
///
/// Runs under `SELECT … FOR UPDATE` so the before-image the history records is
/// the one this transaction actually replaced. A patch whose every field
/// already holds the requested value commits no history at all, which keeps a
/// client retry from inflating the log.
///
/// `done_at` is derived from the new status rather than accepted from the
/// caller — the database's `chk_tasks_done_at_matches_status` requires the two
/// to agree, and deriving it is the only way a caller cannot violate that.
pub async fn update_task(
    pool: &PgPool,
    community: CommunityId,
    id: Uuid,
    patch: &TaskPatch,
    actor_pubkey: Option<&[u8]>,
) -> Result<TaskRecord> {
    let mut tx = pool.begin().await?;

    let current = sqlx::query(concat!(
        "SELECT ",
        task_columns!(),
        " FROM tasks WHERE community_id = $1 AND id = $2 FOR UPDATE"
    ))
    .bind(community.as_uuid())
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("task {id}")))?;
    let current = parse_task_row(&current)?;
    if let Some(expected) = patch.expected_revision {
        if expected != current.revision {
            return Err(DbError::StaleRevision {
                task_id: id,
                expected,
                actual: current.revision,
            });
        }
    }

    let new_status = patch.status.unwrap_or(current.status);
    let new_title = patch.title.clone().unwrap_or_else(|| current.title.clone());
    let new_priority = patch.priority.unwrap_or(current.priority);
    // PostgreSQL stores microseconds; normalize before comparison so a client
    // retry with nanoseconds neither inflates history nor records phantom digits.
    let new_due_at = patch
        .due_at
        .unwrap_or(current.due_at)
        .map(|value| value.trunc_subsecs(6));
    let new_assignee = patch
        .assignee_pubkey
        .clone()
        .unwrap_or_else(|| current.assignee_pubkey.clone());
    // Derived, never caller-supplied: keeps `chk_tasks_done_at_matches_status`
    // satisfiable. Re-entering `done` preserves the original completion time.
    let new_done_at = if new_status.requires_done_at() {
        current.done_at.or_else(|| Some(Utc::now()))
    } else {
        None
    };

    if new_status == current.status
        && new_title == current.title
        && new_priority == current.priority
        && new_due_at == current.due_at
        && new_assignee == current.assignee_pubkey
    {
        tx.commit().await?;
        return Ok(current);
    }

    let row = sqlx::query(concat!(
        "UPDATE tasks SET status = $3, title = $4, priority = $5, due_at = $6, \
                          assignee_pubkey = $7, done_at = $8 \
         WHERE community_id = $1 AND id = $2 \
         RETURNING ",
        task_columns!()
    ))
    .bind(community.as_uuid())
    .bind(id)
    .bind(new_status.as_str())
    .bind(&new_title)
    .bind(new_priority)
    .bind(new_due_at)
    .bind(new_assignee.as_deref())
    .bind(new_done_at)
    .fetch_one(&mut *tx)
    .await?;
    let updated = parse_task_row(&row)?;

    if let Some(action) = status_change_action(current.status, new_status) {
        insert_task_event(
            &mut tx,
            community,
            id,
            actor_pubkey,
            action,
            TaskEventContent {
                transition: Some((current.status, new_status)),
                ..TaskEventContent::default()
            },
        )
        .await?;
    }
    if new_title != current.title {
        insert_task_event(
            &mut tx,
            community,
            id,
            actor_pubkey,
            TaskAction::TitleChanged,
            TaskEventContent {
                body: Some(&new_title),
                changes: Some(json!({"title": {"from": current.title, "to": new_title}})),
                ..TaskEventContent::default()
            },
        )
        .await?;
    }
    if new_assignee != current.assignee_pubkey {
        insert_task_event(
            &mut tx,
            community,
            id,
            actor_pubkey,
            TaskAction::Assigned,
            TaskEventContent {
                changes: Some(json!({"assignee": {
                    "from": current.assignee_pubkey.as_ref().map(hex::encode),
                    "to": new_assignee.as_ref().map(hex::encode),
                }})),
                ..TaskEventContent::default()
            },
        )
        .await?;
    }

    for (action, changes) in [
        (
            TaskAction::PriorityChanged,
            (new_priority != current.priority)
                .then(|| json!({"priority": {"from": current.priority, "to": new_priority}})),
        ),
        (
            TaskAction::DueAtChanged,
            (new_due_at != current.due_at)
                .then(|| json!({"due_at": {"from": current.due_at, "to": new_due_at}})),
        ),
    ] {
        if let Some(changes) = changes {
            insert_task_event(
                &mut tx,
                community,
                id,
                actor_pubkey,
                action,
                TaskEventContent {
                    changes: Some(changes),
                    ..TaskEventContent::default()
                },
            )
            .await?;
        }
    }

    tx.commit().await?;
    Ok(updated)
}

/// Append a caller-supplied history entry (a comment, or an agent summary).
///
/// Returns [`DbError::NotFound`] when the task does not exist in this
/// community, so a comment can never create history for another tenant's task.
/// A second [`TaskAction::SummaryPersisted`] for the same task is rejected by
/// `idx_task_events_one_summary_per_task` and surfaces as
/// [`DbError::InvalidData`] rather than an opaque driver error.
pub async fn append_task_event(
    pool: &PgPool,
    community: CommunityId,
    task_id: Uuid,
    actor_pubkey: Option<&[u8]>,
    action: TaskAction,
    body: Option<&str>,
) -> Result<TaskEventRecord> {
    let mut tx = pool.begin().await?;

    let exists: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM tasks WHERE community_id = $1 AND id = $2 FOR UPDATE")
            .bind(community.as_uuid())
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await?;
    if exists.is_none() {
        return Err(DbError::NotFound(format!("task {task_id}")));
    }

    let event = insert_task_event(
        &mut tx,
        community,
        task_id,
        actor_pubkey,
        action,
        TaskEventContent {
            body,
            ..TaskEventContent::default()
        },
    )
    .await
    .map_err(|error| match &error {
        DbError::Sqlx(sqlx::Error::Database(db_error))
            if db_error.constraint() == Some("idx_task_events_one_summary_per_task") =>
        {
            DbError::InvalidData(format!("task {task_id} already has a persisted summary"))
        }
        _ => error,
    })?;

    tx.commit().await?;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_patch_asks_for_nothing() {
        assert!(TaskPatch::default().is_empty());
    }

    #[test]
    fn a_guard_only_patch_is_empty() {
        assert!(TaskPatch {
            expected_revision: Some(0),
            ..TaskPatch::default()
        }
        .is_empty());
    }

    #[test]
    fn clearing_a_field_is_not_an_empty_patch() {
        // `Some(None)` means "unassign", which is a real change. Treating it as
        // empty would silently drop the operation.
        let patch = TaskPatch {
            assignee_pubkey: Some(None),
            ..TaskPatch::default()
        };
        assert!(!patch.is_empty());

        let patch = TaskPatch {
            due_at: Some(None),
            ..TaskPatch::default()
        };
        assert!(!patch.is_empty());
    }

    #[test]
    fn every_selected_task_column_is_named_once() {
        // The row parser reads these by name; a duplicate or a stray comma
        // here would surface as a runtime decode error on every read.
        let columns: Vec<&str> = task_columns!().split(',').map(str::trim).collect();
        assert!(columns.contains(&"community_id"));
        assert!(columns.contains(&"status"));
        assert!(columns.contains(&"done_at"));
        let mut sorted = columns.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            columns.len(),
            "duplicate column in projection"
        );
    }
}

use crate::Db;
use buzz_datastore_tracing::datastore_span;

// ---------------------------------------------------------------------------
// Fork addition (PR #6425): Db facade methods for the task system.
// These impl blocks live with the task module so upstream's restructured
// lib.rs facade stays untouched.
// ---------------------------------------------------------------------------
impl Db {
    /// Creates a task in the given community (fork task-system facade).
    #[datastore_span(name = "create_task", system = "postgresql")]
    pub async fn create_task(
        &self,
        community: CommunityId,
        new_task: crate::task::NewTask,
    ) -> Result<crate::task::TaskRecord> {
        crate::task::create_task(&self.pool, community, new_task).await
    }

    /// Read one task scoped to `community`.
    #[datastore_span(name = "get_task", system = "postgresql")]
    pub async fn get_task(
        &self,
        community: CommunityId,
        id: Uuid,
    ) -> Result<crate::task::TaskRecord> {
        crate::task::get_task(&self.pool, community, id).await
    }

    /// List a community's tasks, newest-modified first.
    #[datastore_span(name = "list_tasks", system = "postgresql")]
    pub async fn list_tasks(
        &self,
        community: CommunityId,
        filter: &crate::task::TaskFilter,
    ) -> Result<Vec<crate::task::TaskRecord>> {
        crate::task::list_tasks(&self.pool, community, filter).await
    }

    /// Read one task's append-only history, oldest first.
    #[datastore_span(name = "list_task_events", system = "postgresql")]
    pub async fn list_task_events(
        &self,
        community: CommunityId,
        task_id: Uuid,
    ) -> Result<Vec<crate::task::TaskEventRecord>> {
        crate::task::list_task_events(&self.pool, community, task_id).await
    }

    /// Apply a task patch, appending one history row per field that changed.
    #[datastore_span(name = "update_task", system = "postgresql")]
    pub async fn update_task(
        &self,
        community: CommunityId,
        id: Uuid,
        patch: &crate::task::TaskPatch,
        actor_pubkey: Option<&[u8]>,
    ) -> Result<crate::task::TaskRecord> {
        crate::task::update_task(&self.pool, community, id, patch, actor_pubkey).await
    }

    /// Append a comment or summary to a task's history.
    #[datastore_span(name = "append_task_event", system = "postgresql")]
    pub async fn append_task_event(
        &self,
        community: CommunityId,
        task_id: Uuid,
        actor_pubkey: Option<&[u8]>,
        action: buzz_core::task::TaskAction,
        body: Option<&str>,
    ) -> Result<crate::task::TaskEventRecord> {
        crate::task::append_task_event(&self.pool, community, task_id, actor_pubkey, action, body)
            .await
    }
}

#[cfg(test)]
#[path = "task/postgres_tests.rs"]
mod postgres_tests;
