-- One fixed qualification attempt per signed CML task. A started attempt is
-- never re-leased automatically; unknown outcomes require explicit inspection.
CREATE TABLE fleet_attempts (
    community_id UUID NOT NULL REFERENCES communities(id),
    id TEXT NOT NULL,
    task_id UUID NOT NULL,
    channel_id UUID NOT NULL,
    task_revision INTEGER NOT NULL CHECK (task_revision >= 0),
    plan_event_id BYTEA NOT NULL CHECK (octet_length(plan_event_id) = 32),
    plan_event JSONB NOT NULL,
    cml_head BYTEA NOT NULL CHECK (octet_length(cml_head) = 32),
    cml_event JSONB NOT NULL,
    planner_pubkey BYTEA NOT NULL CHECK (octet_length(planner_pubkey) = 32),
    worker_pubkey BYTEA NOT NULL CHECK (octet_length(worker_pubkey) = 32),
    machine_id TEXT NOT NULL,
    scope JSONB NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('planned','claimed','started','unknown','success','error','cancelled','cancelled_before_execution','expired')),
    claim_event_id BYTEA,
    start_event_id BYTEA,
    cancel_event_id BYTEA,
    receipt_event_id BYTEA,
    receipt JSONB,
    permission_grants JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, id),
    UNIQUE (community_id, task_id),
    UNIQUE (community_id, plan_event_id),
    FOREIGN KEY (community_id, task_id) REFERENCES tasks (community_id, id),
    FOREIGN KEY (community_id, channel_id) REFERENCES channels (community_id, id),
    CONSTRAINT fleet_attempt_state_requires_start CHECK (
        state IN ('planned','claimed','cancelled_before_execution','expired')
        OR num_nonnulls(start_event_id) = 1),
    CONSTRAINT fleet_attempt_terminal_requires_receipt CHECK (
        state NOT IN ('success','error','cancelled','cancelled_before_execution')
        OR num_nonnulls(receipt_event_id, receipt) = 2),
    CONSTRAINT fleet_attempt_cancel_ack_requires_intent CHECK (
        state NOT IN ('cancelled','cancelled_before_execution')
        OR num_nonnulls(cancel_event_id) = 1)
);
COMMENT ON TABLE fleet_attempts IS
    'Relay-authorized fixed qualification projection. A start is single use, not proof of running or completion. Signed events remain the audit source.';
SELECT attach_community_write_fence('fleet_attempts');
