-- Per-machine capability grants: default deny.
--
-- PR-3 gave agents a machine home. This is the authorization half: which agent
-- may perform which capability against which target. Nothing is implicit —
-- absence of a row is denial, and revocation is a tombstone (revoked_at), not a
-- DELETE. The separate append-only history retains each grant/revoke transition.
--
-- Tenant-scoped like every non-operator table: community_id NOT NULL, and it
-- leads the primary key and the unique index (migration lint enforces both).

SET LOCAL lock_timeout = '5s';

CREATE TABLE agent_capability_grants (
    community_id  UUID        NOT NULL REFERENCES communities(id),
    agent_pubkey  BYTEA       NOT NULL,
    capability    VARCHAR(64) NOT NULL,
    target        TEXT        NOT NULL,
    granted_by    BYTEA       NOT NULL,
    granted_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at    TIMESTAMPTZ,
    revoked_by    BYTEA,
    PRIMARY KEY (community_id, agent_pubkey, capability, target)
);

-- A revoked grant keeps its row; re-granting reuses it (see store::grant).
CREATE INDEX idx_agent_capability_grants_active
    ON agent_capability_grants (community_id, agent_pubkey, capability)
    WHERE revoked_at IS NULL;

COMMENT ON TABLE agent_capability_grants IS
    'Per-machine capability grants. Default deny: no row (or revoked_at set) means denied.';
COMMENT ON COLUMN agent_capability_grants.target IS
    'Target machine_id (users.machine_id from PR-3), or "*" for any machine in the community.';
COMMENT ON COLUMN agent_capability_grants.revoked_at IS
    'Current tombstone. Immutable grant/revoke history is in agent_capability_events.';

SELECT attach_community_write_fence('agent_capability_grants');

-- The projection can be re-granted, but its overwritten actors/timestamps must
-- survive. An AFTER trigger appends the OLD/NEW images in the same statement
-- transaction. Row locking serializes concurrent upserts, including first grant.
CREATE TABLE agent_capability_events (
    community_id UUID NOT NULL REFERENCES communities(id),
    id BIGINT GENERATED ALWAYS AS IDENTITY,
    agent_pubkey BYTEA NOT NULL,
    capability VARCHAR(64) NOT NULL,
    target TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('grant', 'revoke')),
    actor_pubkey BYTEA NOT NULL,
    before_state JSONB,
    after_state JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, id)
);
CREATE INDEX idx_agent_capability_events_grant
    ON agent_capability_events (community_id, agent_pubkey, capability, target, id);
COMMENT ON TABLE agent_capability_events IS
    'Append-only grant/revoke facts; removed only by the fenced whole-community purge. Not a signed event or execution authorization receipt.';
SELECT attach_community_write_fence('agent_capability_events');

CREATE FUNCTION record_agent_capability_change() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND NEW IS NOT DISTINCT FROM OLD THEN
        RETURN NULL;
    END IF;
    INSERT INTO agent_capability_events
        (community_id, agent_pubkey, capability, target, action, actor_pubkey,
         before_state, after_state)
    VALUES
        (NEW.community_id, NEW.agent_pubkey, NEW.capability, NEW.target,
         CASE WHEN NEW.revoked_at IS NULL THEN 'grant' ELSE 'revoke' END,
         CASE WHEN NEW.revoked_at IS NULL THEN NEW.granted_by ELSE NEW.revoked_by END,
         CASE WHEN TG_OP = 'INSERT' THEN NULL ELSE to_jsonb(OLD) END,
         to_jsonb(NEW));
    RETURN NULL;
END
$$;
CREATE TRIGGER agent_capability_change_history
    AFTER INSERT OR UPDATE ON agent_capability_grants
    FOR EACH ROW EXECUTE FUNCTION record_agent_capability_change();

-- A mutable projection never grants authority to rewrite its history. The one
-- deletion exception is the existing generation-bound whole-community executor;
-- the universal write fence independently verifies that same proof.
CREATE FUNCTION protect_agent_capability_history() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' AND EXISTS (
        SELECT 1 FROM communities
        WHERE id = OLD.community_id AND deletion_state IN ('fenced', 'tombstone')
          AND current_setting('buzz.deletion_executor_community', true) = id::TEXT
          AND current_setting('buzz.deletion_fence_generation', true) = deletion_fence_generation::TEXT
    ) THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'capability history is append-only'
        USING ERRCODE = 'object_not_in_prerequisite_state';
END
$$;
CREATE TRIGGER agent_capability_history_immutable
    BEFORE UPDATE OR DELETE ON agent_capability_events
    FOR EACH ROW EXECUTE FUNCTION protect_agent_capability_history();
