/// Control-plane tables that survive the community data purge.
pub const CONTROL_PLANE_TABLES: &[&str] = &[
    "community_deletion_approvals",
    "community_deletion_checkpoints",
    "community_deletion_executor_heartbeats",
    "community_deletion_requests",
    "community_serving_write_leases",
];

/// Expected community-scoped tables purged by V1.
///
/// Catalog inventory compares the live database against this exact set before
/// approval and again before PostgreSQL purge. A new tenant table therefore
/// blocks deletion until this manifest is intentionally updated.
pub const EXPECTED_SCOPED_TABLES: &[&str] = &[
    "agent_capability_events",
    "agent_capability_grants",
    "api_tokens",
    "archived_identities",
    "audit_log",
    "channel_members",
    "channels",
    "community_bans",
    "delivery_log",
    "event_mentions",
    "events",
    "git_repo_names",
    "join_policy_acceptances",
    "moderation_actions",
    "moderation_reports",
    "parameterized_event_watermarks",
    "pubkey_allowlist",
    "push_leases",
    "push_match_queue",
    "push_wake_outbox",
    "reactions",
    "relay_invites",
    "relay_members",
    "scheduled_workflow_fires",
    "subscriptions",
    "task_events",
    "tasks",
    "thread_metadata",
    "users",
    "workflow_approvals",
    "workflow_runs",
    "workflows",
];

/// Foreign-key-safe child-before-parent order for the PostgreSQL purge.
pub const PURGE_SCOPED_TABLES: &[&str] = &[
    "agent_capability_events",
    "agent_capability_grants",
    "workflow_approvals",
    "scheduled_workflow_fires",
    "workflow_runs",
    "push_wake_outbox",
    "join_policy_acceptances",
    "moderation_reports",
    "subscriptions",
    // task_events → tasks (FK, cascading) and tasks → channels/users, so both
    // must precede `channels` and `users` below.
    "task_events",
    "tasks",
    "api_tokens",
    "channel_members",
    "thread_metadata",
    "moderation_actions",
    "workflows",
    "event_mentions",
    "reactions",
    "push_match_queue",
    "push_leases",
    "relay_invites",
    "delivery_log",
    "events",
    "parameterized_event_watermarks",
    "git_repo_names",
    "archived_identities",
    "audit_log",
    "community_bans",
    "pubkey_allowlist",
    "relay_members",
    "users",
    "channels",
];
