-- Machine homes: the machine an agent actually runs on, as first-class relay data.
--
-- Modeling choice (the open item PR-3 was asked to settle at review): these are
-- columns on `users`, NOT a new `agent_machine_homes` table. 0046 states the rule
-- this follows -- "a task creator/assignee is a `users` row, never a separate
-- agent table. Agents in Buzz *are* users" -- and warns that a dedicated id
-- "would invent a second identity space that nothing else in the schema uses."
-- A side table keyed by `(community_id, pubkey)` would hold exactly one row per
-- agent and would be joined on every read, so it buys no cardinality the user
-- row cannot express, while adding precisely the second identity space 0046
-- rejects. `users.agent_owner_pubkey` (NIP-OA) already set this precedent:
-- agent-shaped facts live on the agent's own user row.
--
-- `machine_id` is the stable host identity (the desktop's device id); the label
-- is human-facing and renameable. `machine_runtime` is unconstrained TEXT for
-- the reason 0031 gives for `workflow_runs.error_code` and 0046 repeats for
-- `tasks.source`: a new runtime (openclaw, hermes, claude-code, codex) must be
-- addable across a rolling upgrade without a schema migration.
--
-- Every constraint leads with `community_id`, as the migration lint
-- (`scoped_primary_key_unique_and_foreign_key_constraints_lead_with_community_id`)
-- requires, so one community's machine registration is invisible to another.
SET LOCAL lock_timeout = '5s';

ALTER TABLE users
    ADD COLUMN machine_id      VARCHAR(255),
    ADD COLUMN machine_label   VARCHAR(255),
    ADD COLUMN machine_runtime TEXT;

-- One home agent per machine, per community. This is the "one-home-per-machine"
-- invariant the agent-homes program is built on: two agents claiming the same
-- host is the exact ambiguity that makes a task assignee meaningless. Enforced
-- as a partial unique index so the (overwhelming) majority of users, who carry
-- no machine_id at all, are entirely unconstrained.
CREATE UNIQUE INDEX idx_users_one_home_per_machine
    ON users (community_id, machine_id)
    WHERE machine_id IS NOT NULL;

-- A machine home is meaningless without the machine it names, and a bare label
-- or runtime with no `machine_id` is unaddressable -- it could never be resolved
-- to a host. Rejecting that at the database keeps a half-registered home
-- unrepresentable rather than merely discouraged.
ALTER TABLE users
    ADD CONSTRAINT chk_users_machine_fields_require_machine_id
        CHECK (machine_id IS NOT NULL
               OR (machine_label IS NULL AND machine_runtime IS NULL));

-- Blank/whitespace ids and labels are the other way a home becomes
-- unaddressable, and TEXT columns accept them silently.
ALTER TABLE users
    ADD CONSTRAINT chk_users_machine_id_not_blank
        CHECK (machine_id IS NULL OR length(btrim(machine_id)) > 0),
    ADD CONSTRAINT chk_users_machine_label_not_blank
        CHECK (machine_label IS NULL OR length(btrim(machine_label)) > 0),
    ADD CONSTRAINT chk_users_machine_runtime_not_blank
        CHECK (machine_runtime IS NULL OR length(btrim(machine_runtime)) > 0);

-- `users` already carries the universal community write fence from 0001; adding
-- columns does not detach it, so no re-attach is needed here.
