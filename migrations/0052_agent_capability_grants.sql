-- Per-machine capability grants: default deny.
--
-- PR-3 gave agents a machine home. This is the authorization half: which agent
-- may perform which capability against which target. Nothing is implicit —
-- absence of a row is denial, and revocation is a tombstone (revoked_at), not a
-- DELETE, so the audit story survives the grant being taken away.
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
    'Tombstone. Non-NULL means revoked; the row is retained so the audit trail survives.';

SELECT attach_community_write_fence('agent_capability_grants');
