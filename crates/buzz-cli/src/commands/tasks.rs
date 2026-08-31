use serde_json::{json, Value};

use crate::{client::BuzzClient, error::CliError, TasksCmd};

const TASKS_PATH: &str = "/api/tasks";

pub async fn dispatch(cmd: TasksCmd, client: &BuzzClient) -> Result<String, CliError> {
    match cmd {
        TasksCmd::Create {
            title,
            body,
            channel,
            assignee,
            priority,
            source,
            source_ref,
        } => {
            let payload = create_payload(
                title,
                body,
                channel,
                assignee,
                priority,
                source,
                source_ref.clone(),
            );
            match client.post_authed_json(TASKS_PATH, &payload).await {
                Ok(raw) => Ok(raw),
                Err(error) => {
                    // A create response can be lost after the relay commits. If
                    // the caller supplied a durable source reference, reconcile
                    // it before surfacing the error so an automatic connector
                    // never retries into a duplicate task.
                    if let Some(source_ref) = source_ref.as_deref() {
                        let path = task_list_path(None, None, None, Some(source_ref), 2);
                        if let Ok(raw) = client.get_authed(&path).await {
                            if let Some(existing) = exactly_one_task(&raw)? {
                                return Ok(existing.to_string());
                            }
                        }
                    }
                    Err(error)
                }
            }
        }
        TasksCmd::List {
            status,
            assignee,
            channel,
            source_ref,
            limit,
        } => {
            let path = task_list_path(
                status.as_deref(),
                assignee.as_deref(),
                channel.as_deref(),
                source_ref.as_deref(),
                limit,
            );
            client.get_authed(&path).await
        }
    }
}

fn create_payload(
    title: String,
    body: Option<String>,
    channel: Option<String>,
    assignee: Option<String>,
    priority: i32,
    source: Option<String>,
    source_ref: Option<String>,
) -> Value {
    json!({
        "title": title,
        "body": body,
        "channel_id": channel,
        "assignee": assignee,
        "priority": priority,
        "source": source,
        "source_ref": source_ref,
    })
}

fn task_list_path(
    status: Option<&str>,
    assignee: Option<&str>,
    channel: Option<&str>,
    source_ref: Option<&str>,
    limit: u32,
) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("limit", &limit.clamp(1, 200).to_string());
    if let Some(value) = status {
        serializer.append_pair("status", value);
    }
    if let Some(value) = assignee {
        serializer.append_pair("assignee", value);
    }
    if let Some(value) = channel {
        serializer.append_pair("channel", value);
    }
    if let Some(value) = source_ref {
        serializer.append_pair("source_ref", value);
    }
    format!("{TASKS_PATH}?{}", serializer.finish())
}

fn exactly_one_task(raw: &str) -> Result<Option<Value>, CliError> {
    let payload: Value = serde_json::from_str(raw)
        .map_err(|error| CliError::Other(format!("task response is not JSON: {error}")))?;
    let tasks = payload
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| CliError::Other("task list response is missing tasks[]".into()))?;
    Ok((tasks.len() == 1).then(|| tasks[0].clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_query_is_stable_bounded_and_encoded() {
        assert_eq!(
            task_list_path(
                Some("in progress"),
                Some("ab&status=done"),
                Some("chan/1"),
                Some("telegram:7991290678:79398:42"),
                999,
            ),
            "/api/tasks?limit=200&status=in+progress&assignee=ab%26status%3Ddone&channel=chan%2F1&source_ref=telegram%3A7991290678%3A79398%3A42"
        );
    }

    #[test]
    fn create_payload_preserves_request_provenance() {
        let payload = create_payload(
            "Build intake".into(),
            Some("Original request".into()),
            Some("channel-1".into()),
            Some("deadbeef".into()),
            10,
            Some("telegram".into()),
            Some("telegram:7991290678:79398:42".into()),
        );
        assert_eq!(payload["source"], "telegram");
        assert_eq!(payload["source_ref"], "telegram:7991290678:79398:42");
        assert_eq!(payload["assignee"], "deadbeef");
        assert_eq!(payload["body"], "Original request");
    }

    #[test]
    fn reconciliation_accepts_exactly_one_existing_task() {
        let one = r#"{"tasks":[{"id":"task-1"}]}"#;
        assert_eq!(exactly_one_task(one).unwrap().unwrap()["id"], "task-1");
        assert!(exactly_one_task(r#"{"tasks":[]}"#).unwrap().is_none());
        assert!(exactly_one_task(r#"{"tasks":[{},{}]}"#).unwrap().is_none());
    }
}
