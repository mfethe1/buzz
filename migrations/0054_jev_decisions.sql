-- Jev routing: decision audit, labels, classify queue, per-channel policy and
-- owner-authored agent profiles (plan J1, §2.2-§2.6, §6.4).
--
-- Every table is tenant-scoped: community_id NOT NULL leads every key, and each
-- carries the community write fence. Numbered 0054: 0050 is reserved by
-- feat/HW-017 and 0053 by REG-8.

SET LOCAL lock_timeout = '5s';

-- One row per Jev judgment (§6.4). The UNIQUE key makes re-delivery a no-op
-- (ON CONFLICT DO NOTHING); a new question set or model gets a new row. Every
-- non-route outcome carries its §2.5 reason code; nothing is dropped.
CREATE TABLE jev_decisions (
    community_id         UUID        NOT NULL REFERENCES communities(id),
    id                   UUID        NOT NULL DEFAULT gen_random_uuid(),
    subject_kind         TEXT        NOT NULL CHECK (subject_kind IN ('message', 'task', 'watch')),
    subject_id           BYTEA       NOT NULL,
    channel_id           UUID,
    question_set_version TEXT        NOT NULL,
    model_version        TEXT        NOT NULL,
    mode                 TEXT        NOT NULL CHECK (mode IN ('shadow', 'suggest', 'auto')),
    outcome              TEXT        NOT NULL CHECK (outcome IN ('route', 'abstain', 'deferred')),
    reason_code          TEXT        CHECK (reason_code IN ('Model', 'BelowFloor', 'ForcedInjection',
                                         'ErrorFailClosed', 'NotMember', 'ShapeAnomaly', 'Deferred')),
    top_option           TEXT,
    top_mass             REAL,
    second_mass          REAL,
    injection_score      REAL,
    masses               JSONB       NOT NULL DEFAULT '{}'::jsonb,
    floor_at_decision    REAL        NOT NULL,
    error_class          TEXT,
    roster_version       TEXT,
    state_digest         BYTEA       NOT NULL,
    request_hash         BYTEA       NOT NULL,
    response             JSONB,
    latency_ms           INTEGER,
    input_tokens         INTEGER,
    output_tokens        INTEGER,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, id),
    UNIQUE (community_id, subject_kind, subject_id, question_set_version, model_version),
    CONSTRAINT chk_jev_decisions_reason CHECK ((outcome = 'route') = (reason_code IS NULL))
);

CREATE INDEX idx_jev_decisions_channel_created
    ON jev_decisions (community_id, channel_id, created_at DESC);

-- Labels. One current row per (decision, actor): a new label supersedes the
-- old one, which is kept as history. Agent labels are advisory (§6.1).
CREATE TABLE jev_decision_feedback (
    community_id   UUID        NOT NULL,
    id             UUID        NOT NULL DEFAULT gen_random_uuid(),
    decision_id    UUID        NOT NULL,
    actor_pubkey   BYTEA       NOT NULL,
    labeller_kind  TEXT        NOT NULL CHECK (labeller_kind IN ('human', 'agent')),
    verdict        TEXT        NOT NULL CHECK (verdict IN ('correct', 'wrong')),
    label          TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    superseded_at  TIMESTAMPTZ,
    PRIMARY KEY (community_id, id),
    FOREIGN KEY (community_id, decision_id) REFERENCES jev_decisions(community_id, id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX idx_jev_decision_feedback_current
    ON jev_decision_feedback (community_id, decision_id, actor_pubkey)
    WHERE superseded_at IS NULL;

-- Work queue drained by a leader-locked worker (FOR UPDATE SKIP LOCKED + lease).
CREATE TABLE jev_classify_queue (
    community_id    UUID        NOT NULL REFERENCES communities(id),
    subject_kind    TEXT        NOT NULL CHECK (subject_kind IN ('message', 'task', 'watch')),
    subject_id      BYTEA       NOT NULL,
    channel_id      UUID,
    state           TEXT        NOT NULL DEFAULT 'pending'
                                CHECK (state IN ('pending', 'claimed', 'done', 'error')),
    attempts        INTEGER     NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    claim_id        UUID,
    lease_until     TIMESTAMPTZ,
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, subject_kind, subject_id)
);

CREATE INDEX idx_jev_classify_queue_due
    ON jev_classify_queue (community_id, next_attempt_at)
    WHERE state IN ('pending', 'claimed');

-- Per-channel routing mode. No row means 'off', so existing channels are
-- untouched until an admin opts in. 'auto' names the human who enabled it.
CREATE TABLE channel_routing_policy (
    community_id  UUID        NOT NULL,
    channel_id    UUID        NOT NULL,
    mode          TEXT        NOT NULL DEFAULT 'off'
                              CHECK (mode IN ('off', 'shadow', 'suggest', 'auto')),
    floor         REAL        NOT NULL DEFAULT 0.60 CHECK (floor >= 0 AND floor <= 1),
    owner_pubkey  BYTEA,
    updated_by    BYTEA       NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, channel_id),
    FOREIGN KEY (community_id, channel_id) REFERENCES channels(community_id, id) ON DELETE CASCADE,
    CONSTRAINT chk_channel_routing_policy_auto_owner CHECK (mode <> 'auto' OR owner_pubkey IS NOT NULL)
);

-- Owner-authored capability descriptions for the roster. Never sourced from
-- the agent's self-published profile (an injection surface, §6.1).
CREATE TABLE agent_routing_profiles (
    community_id  UUID        NOT NULL REFERENCES communities(id),
    agent_pubkey  BYTEA       NOT NULL,
    description   TEXT        NOT NULL,
    enabled       BOOLEAN     NOT NULL DEFAULT TRUE,
    authored_by   BYTEA       NOT NULL,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (community_id, agent_pubkey)
);

SELECT attach_community_write_fence('jev_decisions');
SELECT attach_community_write_fence('jev_decision_feedback');
SELECT attach_community_write_fence('jev_classify_queue');
SELECT attach_community_write_fence('channel_routing_policy');
SELECT attach_community_write_fence('agent_routing_profiles');
