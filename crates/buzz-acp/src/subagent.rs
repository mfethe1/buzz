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

/// Maximum finished-but-undelivered subagent results retained per client.
///
/// Bounded, drop-oldest: a delegation result is a convenience notification,
/// not durable state. An agent that somehow finishes more than this many
/// delegations without any turn draining them keeps the newest.
pub(crate) const MAX_PENDING_DELIVERIES: usize = 16;

/// Where a delegation was spawned from, so its result can be routed back to
/// the thread whose author is waiting for it instead of leaking into whatever
/// turn happens to run next (typically the channel-less idle heartbeat, whose
/// prompt instructs the agent to post nothing).
///
/// Deliberately plain strings: this module correlates ACP tool calls and must
/// not take a dependency on the queue's session types.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DelegationOrigin {
    /// Channel UUID (hyphenated) the spawning turn belonged to.
    pub channel_id: String,
    /// NIP-10 root event id (hex) of the spawning turn's thread, if threaded.
    pub root_event_id: Option<String>,
    /// NIP-10 parent event id (hex) of the spawning turn's thread, if threaded.
    pub parent_event_id: Option<String>,
}

/// A delegation that reached a terminal status and has not been delivered yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletedSubagent {
    /// ACP `toolCallId` that produced this result. Unique per delegation, so it
    /// is the identity used to suppress re-emitted terminal frames — two
    /// concurrent delegations to the same subagent are value-identical but are
    /// two distinct results, and both are owed to the waiting author.
    pub tool_call_id: String,
    /// Display name of the subagent.
    pub name: String,
    /// Terminal status: `complete` or `failed`.
    pub status: &'static str,
    /// Summary text extracted from the terminal tool-call update, if any.
    pub summary: Option<String>,
    /// Thread the spawning turn came from. `None` when the delegation was
    /// spawned by a turn with no channel (e.g. a heartbeat) — such results
    /// have no waiting thread and are not routed anywhere.
    pub origin: Option<DelegationOrigin>,
}

/// A delegated subagent being tracked across tool-call updates.
#[derive(Debug, Clone)]
struct TrackedSubagent {
    /// Display name parsed from the delegation tool call, or the tool title.
    name: String,
    /// Last lifecycle status emitted for this subagent.
    status: &'static str,
    /// Thread that spawned this delegation, captured at spawn time. The turn
    /// that *observes* completion is frequently not the turn that spawned it,
    /// which is exactly the bug this field exists to fix.
    origin: Option<DelegationOrigin>,
}

/// Per-client correlation of delegation tool calls to subagent lifecycle.
#[derive(Debug, Default)]
pub(crate) struct SubagentTracker {
    /// `toolCallId` → tracked subagent. Entries are retired into
    /// [`completed`](Self::completed) on terminal statuses
    /// (`complete`/`failed`).
    tasks: std::collections::HashMap<String, TrackedSubagent>,
    /// Terminal results awaiting delivery, oldest first. Capped at
    /// [`MAX_PENDING_DELIVERIES`] (drop-oldest).
    completed: std::collections::VecDeque<CompletedSubagent>,
    /// Origin stamped onto delegations spawned from here on. Set by the pool
    /// at turn start; `None` for channel-less turns.
    current_origin: Option<DelegationOrigin>,
}

impl SubagentTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Stamp subsequent spawns with the thread of the turn now running.
    pub(crate) fn set_origin(&mut self, origin: Option<DelegationOrigin>) {
        self.current_origin = origin;
    }

    /// Drain finished-but-undelivered delegation results, oldest first.
    pub(crate) fn take_completed(&mut self) -> Vec<CompletedSubagent> {
        self.completed.drain(..).collect()
    }

    /// Retire a terminal delegation into the pending-delivery buffer.
    ///
    /// Duplicate guard keys on `tool_call_id`, not on the whole value: a
    /// re-emitted terminal frame for the same call must not deliver twice,
    /// while two *different* calls that happen to look identical (same
    /// subagent, same thread, both summary-less) are two real results and must
    /// both be delivered.
    fn push_completed(&mut self, entry: CompletedSubagent) {
        if self
            .completed
            .iter()
            .any(|e| e.tool_call_id == entry.tool_call_id)
        {
            return;
        }
        if self.completed.len() >= MAX_PENDING_DELIVERIES {
            self.completed.pop_front();
        }
        self.completed.push_back(entry);
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
                origin: self.current_origin.clone(),
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
                let retired = self.tasks.remove(tool_call_id)?;
                let summary = extract_summary(update);
                self.push_completed(CompletedSubagent {
                    tool_call_id: tool_call_id.to_string(),
                    name: name.clone(),
                    status: "complete",
                    summary: summary.clone(),
                    origin: retired.origin,
                });
                Some(lifecycle_payload(&name, "complete", summary.as_deref()))
            }
            "failed" => {
                let retired = self.tasks.remove(tool_call_id)?;
                let summary = extract_summary(update);
                self.push_completed(CompletedSubagent {
                    tool_call_id: tool_call_id.to_string(),
                    name: name.clone(),
                    status: "failed",
                    summary: summary.clone(),
                    origin: retired.origin,
                });
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

    fn origin(channel: &str) -> DelegationOrigin {
        DelegationOrigin {
            channel_id: channel.to_string(),
            root_event_id: Some("root".into()),
            parent_event_id: Some("parent".into()),
        }
    }

    fn spawn(tracker: &mut SubagentTracker, id: &str, name: &str) {
        tracker.observe_update(&tool_call(
            id,
            "delegate_task",
            json!({"subagent": name, "prompt": "x"}),
        ));
    }

    /// The core bug: the turn that observes completion is not the turn that
    /// spawned it, so the result must carry the ORIGINATING thread.
    #[test]
    fn completed_delegation_retains_spawn_time_origin_not_current_origin() {
        let mut tracker = SubagentTracker::new();
        tracker.set_origin(Some(origin("chan-a")));
        spawn(&mut tracker, "t1", "worker");
        // A later, unrelated turn (e.g. the channel-less heartbeat) is running
        // when the completion frame arrives.
        tracker.set_origin(None);
        tracker.observe_update(&tool_call_update("t1", "completed"));

        let done = tracker.take_completed();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].status, "complete");
        assert_eq!(done[0].origin, Some(origin("chan-a")));
    }

    #[test]
    fn failed_delegation_is_queued_for_delivery() {
        let mut tracker = SubagentTracker::new();
        tracker.set_origin(Some(origin("chan-a")));
        spawn(&mut tracker, "t1", "worker");
        tracker.observe_update(&tool_call_update("t1", "failed"));

        let done = tracker.take_completed();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].status, "failed");
    }

    /// Channel-less spawn (heartbeat): nothing is waiting, so no origin.
    #[test]
    fn delegation_spawned_without_origin_has_no_delivery_target() {
        let mut tracker = SubagentTracker::new();
        tracker.set_origin(None);
        spawn(&mut tracker, "t1", "worker");
        tracker.observe_update(&tool_call_update("t1", "completed"));

        let done = tracker.take_completed();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].origin, None);
    }

    #[test]
    fn take_completed_drains_so_results_are_delivered_once() {
        let mut tracker = SubagentTracker::new();
        tracker.set_origin(Some(origin("chan-a")));
        spawn(&mut tracker, "t1", "worker");
        tracker.observe_update(&tool_call_update("t1", "completed"));

        assert_eq!(tracker.take_completed().len(), 1);
        assert!(tracker.take_completed().is_empty());
    }

    /// A re-emitted terminal frame for an already-retired call must not produce
    /// a second delivery of the same report.
    #[test]
    fn repeated_terminal_update_does_not_duplicate_delivery() {
        let mut tracker = SubagentTracker::new();
        tracker.set_origin(Some(origin("chan-a")));
        spawn(&mut tracker, "t1", "worker");
        tracker.observe_update(&tool_call_update("t1", "completed"));
        // Second terminal frame: the task is already retired, so it is ignored.
        assert!(tracker
            .observe_update(&tool_call_update("t1", "completed"))
            .is_none());

        assert_eq!(tracker.take_completed().len(), 1);
    }

    /// Unknown tool-call ids (never spawned, or already retired) are inert.
    #[test]
    fn terminal_update_for_untracked_call_is_ignored() {
        let mut tracker = SubagentTracker::new();
        assert!(tracker
            .observe_update(&tool_call_update("ghost", "completed"))
            .is_none());
        assert!(tracker.take_completed().is_empty());
    }

    /// Two delegations to the SAME subagent, spawned concurrently from the same
    /// thread, both finishing with no summary, are value-identical as pending
    /// entries — but they are two real results and both are owed to the author.
    #[test]
    fn concurrent_identical_delegations_both_deliver() {
        let mut tracker = SubagentTracker::new();
        tracker.set_origin(Some(origin("chan-a")));
        spawn(&mut tracker, "t1", "worker");
        spawn(&mut tracker, "t2", "worker");
        tracker.observe_update(&tool_call_update("t1", "completed"));
        tracker.observe_update(&tool_call_update("t2", "completed"));

        assert_eq!(tracker.take_completed().len(), 2);
    }

    /// Malformed terminal frame: `toolCallId` present but the status field is
    /// absent/garbage. Must not retire the task nor queue a delivery.
    #[test]
    fn malformed_terminal_update_neither_retires_nor_delivers() {
        let mut tracker = SubagentTracker::new();
        tracker.set_origin(Some(origin("chan-a")));
        spawn(&mut tracker, "t1", "worker");

        assert!(tracker
            .observe_update(&json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "t1",
                "status": "not-a-real-status",
            }))
            .is_none());
        assert!(tracker.take_completed().is_empty());

        // The task survived, so the real completion still delivers.
        tracker.observe_update(&tool_call_update("t1", "completed"));
        assert_eq!(tracker.take_completed().len(), 1);
    }

    /// Offline/timeout shape: a delegation that never reaches a terminal frame
    /// stays pending forever rather than being invented as a result.
    #[test]
    fn delegation_that_never_finishes_delivers_nothing() {
        let mut tracker = SubagentTracker::new();
        tracker.set_origin(Some(origin("chan-a")));
        spawn(&mut tracker, "t1", "worker");
        tracker.observe_update(&tool_call_update("t1", "in_progress"));

        assert!(tracker.take_completed().is_empty());
    }

    /// Routing denial: results are stamped with the thread that spawned them,
    /// so a result from another channel is never attributed to this one.
    #[test]
    fn results_from_different_threads_keep_their_own_origins() {
        let mut tracker = SubagentTracker::new();
        tracker.set_origin(Some(origin("chan-a")));
        spawn(&mut tracker, "t1", "worker");
        tracker.set_origin(Some(origin("chan-b")));
        spawn(&mut tracker, "t2", "worker");
        tracker.observe_update(&tool_call_update("t1", "completed"));
        tracker.observe_update(&tool_call_update("t2", "completed"));

        let done = tracker.take_completed();
        assert_eq!(done.len(), 2);
        assert_eq!(done[0].origin, Some(origin("chan-a")));
        assert_eq!(done[1].origin, Some(origin("chan-b")));
    }

    #[test]
    fn pending_deliveries_are_bounded_drop_oldest() {
        let mut tracker = SubagentTracker::new();
        tracker.set_origin(Some(origin("chan-a")));
        let total = MAX_PENDING_DELIVERIES + 3;
        for i in 0..total {
            let id = format!("t{i}");
            spawn(&mut tracker, &id, &format!("worker-{i}"));
            tracker.observe_update(&tool_call_update(&id, "completed"));
        }

        let done = tracker.take_completed();
        assert_eq!(done.len(), MAX_PENDING_DELIVERIES);
        // Oldest dropped, newest kept.
        assert_eq!(
            done[0].name,
            format!("worker-{}", total - MAX_PENDING_DELIVERIES)
        );
        assert_eq!(done[done.len() - 1].name, format!("worker-{}", total - 1));
    }
}
