//! `buzz tasks` — durable community work items shared by humans and agents.

use serde_json::{json, Map, Value};
use url::form_urlencoded;
use uuid::Uuid;

use crate::client::BuzzClient;
use crate::error::CliError;
use crate::validate::read_or_stdin;
use crate::{OutputFormat, TasksCmd};

fn uuid(field: &str, raw: &str) -> Result<String, CliError> {
    Uuid::parse_str(raw)
        .map(|value| value.to_string())
        .map_err(|_| CliError::Usage(format!("{field} must be a UUID")))
}

fn list_path(
    status: Option<&str>,
    assignee: Option<&str>,
    channel: Option<&str>,
    source_ref: Option<&str>,
    include_archived: bool,
    limit: i64,
) -> Result<String, CliError> {
    if !(1..=200).contains(&limit) {
        return Err(CliError::Usage("limit must be between 1 and 200".into()));
    }
    let mut query = form_urlencoded::Serializer::new(String::new());
    if let Some(value) = status {
        query.append_pair("status", value);
    }
    if let Some(value) = assignee {
        query.append_pair("assignee", value);
    }
    if let Some(value) = channel {
        query.append_pair("channel", &uuid("channel", value)?);
    }
    if let Some(value) = source_ref {
        query.append_pair("source_ref", value);
    }
    if include_archived {
        query.append_pair("include_archived", "true");
    }
    query.append_pair("limit", &limit.to_string());
    Ok(format!("/api/tasks?{}", query.finish()))
}

fn parse_response(raw: &str) -> Result<Value, CliError> {
    serde_json::from_str(raw)
        .map_err(|e| CliError::Other(format!("task API returned malformed JSON: {e}")))
}

fn print_value(value: &Value, _format: &OutputFormat) {
    println!(
        "{}",
        serde_json::to_string(value).expect("JSON value serializes")
    );
}

struct CreateTaskInput {
    title: String,
    body: Option<String>,
    channel: Option<String>,
    parent: Option<String>,
    assignee: Option<String>,
    priority: i32,
    due_at: Option<String>,
    source: String,
    source_ref: Option<String>,
    if_absent: bool,
}

async fn cmd_create(
    client: &BuzzClient,
    input: CreateTaskInput,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let CreateTaskInput {
        title,
        body,
        channel,
        parent,
        assignee,
        priority,
        due_at,
        source,
        source_ref,
        if_absent,
    } = input;
    if title.trim().is_empty() || title.chars().count() > 200 {
        return Err(CliError::Usage(
            "title must contain between 1 and 200 characters".into(),
        ));
    }
    if if_absent {
        let source_ref_value = source_ref
            .as_deref()
            .ok_or_else(|| CliError::Usage("--if-absent requires --source-ref".into()))?;
        let path = list_path(None, None, None, Some(source_ref_value), false, 2)?;
        let existing = parse_response(&client.get_authed(&path).await?)?;
        if let Some(task) = existing
            .get("tasks")
            .and_then(Value::as_array)
            .and_then(|tasks| tasks.first())
        {
            print_value(&json!({"created": false, "task": task}), format);
            return Ok(());
        }
    }

    let body = match body {
        Some(value) => Some(read_or_stdin(&value)?),
        None => None,
    };
    let payload = json!({
        "title": title,
        "body": body,
        "channel_id": channel.as_deref().map(|v| uuid("channel", v)).transpose()?,
        "parent_task_id": parent.as_deref().map(|v| uuid("parent", v)).transpose()?,
        "assignee": assignee,
        "priority": priority,
        "due_at": due_at,
        "source": source,
        "source_ref": source_ref,
    });
    let created = parse_response(&client.post_authed_json("/api/tasks", &payload).await?)?;
    print_value(&json!({"created": true, "task": created}), format);
    Ok(())
}

async fn append_event(
    client: &BuzzClient,
    task: &str,
    action: &str,
    body: &str,
    format: &OutputFormat,
) -> Result<(), CliError> {
    let task = uuid("task", task)?;
    let body = read_or_stdin(body)?;
    if body.trim().is_empty() {
        return Err(CliError::Usage("event body cannot be empty".into()));
    }
    let response = client
        .post_authed_json(
            &format!("/api/tasks/{task}/events"),
            &json!({"action": action, "body": body}),
        )
        .await?;
    print_value(&parse_response(&response)?, format);
    Ok(())
}

pub async fn dispatch(
    cmd: TasksCmd,
    client: &BuzzClient,
    format: &OutputFormat,
) -> Result<(), CliError> {
    match cmd {
        TasksCmd::List {
            status,
            assignee,
            channel,
            source_ref,
            include_archived,
            limit,
        } => {
            let path = list_path(
                status.as_deref(),
                assignee.as_deref(),
                channel.as_deref(),
                source_ref.as_deref(),
                include_archived,
                limit,
            )?;
            let response = client.get_authed(&path).await?;
            print_value(&parse_response(&response)?, format);
            Ok(())
        }
        TasksCmd::Get { task } => {
            let task = uuid("task", &task)?;
            let response = client.get_authed(&format!("/api/tasks/{task}")).await?;
            print_value(&parse_response(&response)?, format);
            Ok(())
        }
        TasksCmd::Create {
            title,
            body,
            channel,
            parent,
            assignee,
            priority,
            due_at,
            source,
            source_ref,
            if_absent,
        } => {
            cmd_create(
                client,
                CreateTaskInput {
                    title,
                    body,
                    channel,
                    parent,
                    assignee,
                    priority,
                    due_at,
                    source,
                    source_ref,
                    if_absent,
                },
                format,
            )
            .await
        }
        TasksCmd::Update {
            task,
            status,
            title,
            priority,
            assignee,
            clear_assignee,
            due_at,
            clear_due,
        } => {
            let task = uuid("task", &task)?;
            let mut payload = Map::new();
            if let Some(value) = status {
                payload.insert("status".into(), Value::String(value));
            }
            if let Some(value) = title {
                payload.insert("title".into(), Value::String(value));
            }
            if let Some(value) = priority {
                payload.insert("priority".into(), json!(value));
            }
            if clear_assignee {
                payload.insert("assignee".into(), Value::Null);
            } else if let Some(value) = assignee {
                payload.insert("assignee".into(), Value::String(value));
            }
            if clear_due {
                payload.insert("due_at".into(), Value::Null);
            } else if let Some(value) = due_at {
                payload.insert("due_at".into(), Value::String(value));
            }
            if payload.is_empty() {
                return Err(CliError::Usage(
                    "update requires at least one mutable field".into(),
                ));
            }
            let response = client
                .patch_authed_json(&format!("/api/tasks/{task}"), &Value::Object(payload))
                .await?;
            print_value(&parse_response(&response)?, format);
            Ok(())
        }
        TasksCmd::Comment { task, body } => {
            append_event(client, &task, "commented", &body, format).await
        }
        TasksCmd::Summarize { task, body } => {
            append_event(client, &task, "summary_persisted", &body, format).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_path_encodes_opaque_source_refs_and_bounds_limit() {
        let path = list_path(
            Some("in_progress"),
            None,
            None,
            Some("telegram:chat/thread 7"),
            false,
            20,
        )
        .unwrap();
        assert_eq!(
            path,
            "/api/tasks?status=in_progress&source_ref=telegram%3Achat%2Fthread+7&limit=20"
        );
        assert!(list_path(None, None, None, None, false, 0).is_err());
        assert!(list_path(None, None, None, None, false, 201).is_err());
    }

    #[test]
    fn uuid_fields_fail_before_network_io() {
        assert!(uuid("task", "not-a-uuid").is_err());
        assert!(list_path(None, None, Some("bad"), None, false, 10).is_err());
    }
}
