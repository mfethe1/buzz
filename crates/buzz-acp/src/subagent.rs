//! Subagent delegation detection → parent-tagged lifecycle events.
//!
//! Workstream A of SPEC-nested-subagents.md: when the wrapped agent delegates
//! to a subagent (Hermes `delegate_task`, Claude Code `Task`, OpenClaw
//! `spawn`), the harness publishes a kind:20003 ephemeral event carrying the
//! `["parent", <parent-agent-pubkey-hex>]` nesting tag and a payload of
//! `{subagent_name, parent_pubkey, status, summary?}`.
//!
//! Detection happens in [`crate::acp::AcpClient`]'s session-update handling:
//! ACP session updates stream tool calls, and delegation tools appear there
//! like any other tool. This module holds the per-client correlation state
//! (`toolCallId` → subagent) and maps the tool-call lifecycle onto subagent
//! statuses:
//!
//! - `tool_call` (pending) with a delegation-shaped title → `spawned`
//! - `tool_call_update` `in_progress` → `running` (first time only)
//! - `tool_call_update` `completed` → `complete` (+ summary text)
//! - `tool_call_update` `failed` → `failed` (+ summary text)
//!
//! The events are emitted onto the local observer feed as
//! `subagent_lifecycle` and ride the existing owner-scoped encrypted
//! kind:24200 observer frames to clients (see `publish_relay_observer_event`
//! in `lib.rs`). The frame's agent tag IS the parent identity, so payloads
//! here carry only `{subagent_name, status, summary?}`; clients derive
//! nesting from the frame's agent tag. No separate kind:20003 publish
//! exists by design — 20003 was reserved for a future standalone ephemeral
//! fan-out and is currently unused.

use serde_json::Value;

/// Observer-feed kind for detected subagent lifecycle transitions.
pub(crate) const OBSERVER_KIND_SUBAGENT_LIFECYCLE: &str = "subagent_lifecycle";

/// Maximum characters of tool-call output kept as the subagent `summary`.
const SUMMARY_MAX_CHARS: usize = 280;

/// A delegated subagent being tracked across tool-call updates.
#[derive(Debug, Clone)]
struct TrackedSubagent {
    /// Display name parsed from the delegation tool call, or the tool title.
    name: String,
    /// Last lifecycle status emitted for this subagent.
    status: &'static str,
}

/// Per-client correlation of delegation tool calls to subagent lifecycle.
#[derive(Debug, Default)]
pub(crate) struct SubagentTracker {
    /// `toolCallId` → tracked subagent. Entries are removed on terminal
    /// statuses (`complete`/`failed`).
    tasks: std::collections::HashMap<String, TrackedSubagent>,
}

impl SubagentTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Feed one `session/update` `update` object; returns the observer
    /// payload to emit when a subagent lifecycle transition occurred.
    pub(crate) fn observe_update(&mut self, update: &Value) -> Option<Value> {
        match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("tool_call") => self.on_tool_call(update),
            Some("tool_call_update") => self.on_tool_call_update(update),
            _ => None,
        }
    }

    fn on_tool_call(&mut self, update: &Value) -> Option<Value> {
        let title = update.get("title").and_then(Value::as_str)?;
        if !is_delegation_tool(title) {
            return None;
        }
        let tool_call_id = update.get("toolCallId").and_then(Value::as_str)?;
        // A re-emitted pending call for an already-tracked id is a duplicate
        // spawn notification, not a new subagent.
        if self.tasks.contains_key(tool_call_id) {
            return None;
        }
        let name = extract_subagent_name(update).unwrap_or_else(|| title.to_string());
        self.tasks.insert(
            tool_call_id.to_string(),
            TrackedSubagent {
                name: name.clone(),
                status: "spawned",
            },
        );
        Some(lifecycle_payload(&name, "spawned", None))
    }

    fn on_tool_call_update(&mut self, update: &Value) -> Option<Value> {
        let tool_call_id = update.get("toolCallId").and_then(Value::as_str)?;
        let tracked = self.tasks.get(tool_call_id)?;
        let name = tracked.name.clone();
        let status = update.get("status").and_then(Value::as_str)?;
        match status {
            // Pending repeats carry no new information.
            "pending" => None,
            "in_progress" => {
                if tracked.status == "running" {
                    return None;
                }
                let tracked = self.tasks.get_mut(tool_call_id)?;
                tracked.status = "running";
                Some(lifecycle_payload(&name, "running", None))
            }
            "completed" => {
                self.tasks.remove(tool_call_id);
                let summary = extract_summary(update);
                Some(lifecycle_payload(&name, "complete", summary.as_deref()))
            }
            "failed" => {
                self.tasks.remove(tool_call_id);
                let summary = extract_summary(update);
                Some(lifecycle_payload(&name, "failed", summary.as_deref()))
            }
            _ => None,
        }
    }
}

/// Whether a `tool_call` title looks like a delegation/subagent tool.
///
/// Matches the delegation families named in the SPEC: `delegate_task`
/// (Hermes), `Task` (Claude Code), and `spawn`-style tools (OpenClaw).
/// `task`/`spawn` are matched exactly to avoid flagging unrelated tools that
/// merely contain those substrings.
pub(crate) fn is_delegation_tool(title: &str) -> bool {
    let normalized = title.trim().to_ascii_lowercase();
    normalized == "task"
        || normalized == "spawn"
        || normalized.contains("delegate")
        || normalized.contains("subagent")
}

/// Parse the subagent display name from the delegation call's raw input.
fn extract_subagent_name(update: &Value) -> Option<String> {
    let input = update
        .get("rawInput")
        .or_else(|| update.get("raw_input"))
        .or_else(|| update.get("arguments"))?;
    for key in ["subagent", "subagent_name", "agent", "agent_name", "name"] {
        if let Some(name) = input.get(key).and_then(Value::as_str) {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// First text block of a tool-call update, truncated for use as `summary`.
fn extract_summary(update: &Value) -> Option<String> {
    let text = update
        .get("content")?
        .as_array()?
        .iter()
        .find_map(|block| block.pointer("/content/text").and_then(Value::as_str))?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= SUMMARY_MAX_CHARS {
        return Some(text.to_string());
    }
    Some(text.chars().take(SUMMARY_MAX_CHARS).collect())
}

fn lifecycle_payload(name: &str, status: &str, summary: Option<&str>) -> Value {
    let mut payload = serde_json::json!({
        "subagent_name": name,
        "status": status,
    });
    if let Some(summary) = summary {
        payload["summary"] = serde_json::Value::String(summary.to_string());
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_call(id: &str, title: &str, raw_input: Value) -> Value {
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": id,
            "title": title,
            "kind": "other",
            "status": "pending",
            "rawInput": raw_input,
        })
    }

    fn tool_call_update(id: &str, status: &str) -> Value {
        json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "status": status,
        })
    }

    #[test]
    fn delegation_tool_call_emits_spawned() {
        let mut tracker = SubagentTracker::new();
        let payload = tracker
            .observe_update(&tool_call(
                "t1",
                "delegate_task",
                json!({"subagent": "research-worker", "prompt": "x"}),
            ))
            .expect("spawned payload");
        assert_eq!(payload["subagent_name"], "research-worker");
        assert_eq!(payload["status"], "spawned");
        assert!(payload.get("summary").is_none());
    }

    #[test]
    fn non_delegation_tools_are_ignored() {
        let mut tracker = SubagentTracker::new();
        assert!(tracker
            .observe_update(&tool_call("t1", "read_file", json!({"path": "/tmp"})))
            .is_none());
        // "taskmaster" contains "task" as a substring but is not the Task tool.
        assert!(tracker
            .observe_update(&tool_call("t2", "taskmaster9000", json!({})))
            .is_none());
    }

    #[test]
    fn full_lifecycle_spawned_running_complete() {
        let mut tracker = SubagentTracker::new();
        tracker
            .observe_update(&tool_call("t1", "Task", json!({"name": "scout"})))
            .expect("spawned");
        let running = tracker
            .observe_update(&tool_call_update("t1", "in_progress"))
            .expect("running payload");
        assert_eq!(running["status"], "running");
        // Repeated in_progress must not re-emit.
        assert!(tracker
            .observe_update(&tool_call_update("t1", "in_progress"))
            .is_none());
        let mut complete = tool_call_update("t1", "completed");
        complete["content"] = json!([
            {"type": "content", "content": {"type": "text", "text": "found 3 leads"}}
        ]);
        let done = tracker.observe_update(&complete).expect("complete payload");
        assert_eq!(done["status"], "complete");
        assert_eq!(done["subagent_name"], "scout");
        assert_eq!(done["summary"], "found 3 leads");
        // Terminal: further updates for the same id are untracked.
        assert!(tracker
            .observe_update(&tool_call_update("t1", "in_progress"))
            .is_none());
    }

    #[test]
    fn failed_update_emits_failed_with_summary() {
        let mut tracker = SubagentTracker::new();
        tracker
            .observe_update(&tool_call("t9", "spawn", json!({"agent": "worker"})))
            .expect("spawned");
        let mut failed = tool_call_update("t9", "failed");
        failed["content"] = json!([
            {"type": "content", "content": {"type": "text", "text": "boom"}}
        ]);
        let payload = tracker.observe_update(&failed).expect("failed payload");
        assert_eq!(payload["status"], "failed");
        assert_eq!(payload["summary"], "boom");
    }

    #[test]
    fn update_without_tracking_is_ignored() {
        let mut tracker = SubagentTracker::new();
        // An in_progress update for a non-delegation tool call we never saw.
        assert!(tracker
            .observe_update(&tool_call_update("other", "in_progress"))
            .is_none());
    }

    #[test]
    fn missing_name_falls_back_to_title() {
        let mut tracker = SubagentTracker::new();
        let payload = tracker
            .observe_update(&tool_call("t1", "delegate_task", json!({"prompt": "x"})))
            .expect("spawned payload");
        assert_eq!(payload["subagent_name"], "delegate_task");
    }

    #[test]
    fn long_summary_is_truncated() {
        let mut tracker = SubagentTracker::new();
        tracker
            .observe_update(&tool_call("t1", "Task", json!({"name": "n"})))
            .expect("spawned");
        let long_text = "x".repeat(SUMMARY_MAX_CHARS + 100);
        let mut complete = tool_call_update("t1", "completed");
        complete["content"] = json!([
            {"type": "content", "content": {"type": "text", "text": long_text}}
        ]);
        let done = tracker.observe_update(&complete).expect("complete payload");
        assert_eq!(
            done["summary"]
                .as_str()
                .map(str::chars)
                .map(Iterator::count),
            Some(SUMMARY_MAX_CHARS)
        );
    }
}
