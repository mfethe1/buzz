-- Owner-private control journal and current display projection. Neither table
-- supplies execution grants; existing fleet admission remains authoritative.
CREATE TABLE machines (
    community_id UUID NOT NULL REFERENCES communities(id),
    machine_id UUID NOT NULL,
    owner_pubkey BYTEA NOT NULL CHECK (octet_length(owner_pubkey) = 32),
    coordinator_pubkey BYTEA NOT NULL CHECK (octet_length(coordinator_pubkey) = 32),
    registration_event_id BYTEA NOT NULL CHECK (octet_length(registration_event_id) = 32),
    label TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 80),
    runtime TEXT NOT NULL CHECK (runtime IN ('hermes','openclaw','codex','claude-code')),
    observation_event_id BYTEA,
    observation_sequence BIGINT NOT NULL DEFAULT 0 CHECK (observation_sequence BETWEEN 0 AND 9007199254740991),
    observed_state TEXT CHECK (observed_state IN ('ready','busy','unavailable')),
    observed_at TIMESTAMPTZ,
    received_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    enrolled_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, machine_id),
    UNIQUE (community_id, coordinator_pubkey),
    UNIQUE (community_id, registration_event_id),
    FOREIGN KEY (community_id, owner_pubkey) REFERENCES users(community_id, pubkey),
    FOREIGN KEY (community_id, coordinator_pubkey) REFERENCES users(community_id, pubkey),
    CONSTRAINT machine_distinct_owner CHECK (owner_pubkey <> coordinator_pubkey),
    CONSTRAINT machine_observation_complete CHECK (
        (observation_sequence = 0 AND num_nonnulls(observation_event_id, observed_state, observed_at, received_at, expires_at) = 0)
        OR (observation_sequence > 0 AND num_nonnulls(observation_event_id, observed_state, observed_at, received_at, expires_at) = 5)),
    CONSTRAINT machine_observation_expiry CHECK (
        expires_at <= observed_at + interval '120 seconds' AND expires_at <= received_at + interval '120 seconds')
);
CREATE INDEX machines_owner_page ON machines(community_id, owner_pubkey, machine_id);
CREATE TABLE machine_control_events (
    community_id UUID NOT NULL REFERENCES communities(id),
    event_id BYTEA NOT NULL CHECK (octet_length(event_id) = 32),
    machine_id UUID NOT NULL,
    owner_pubkey BYTEA NOT NULL CHECK (octet_length(owner_pubkey) = 32),
    kind INTEGER NOT NULL CHECK (kind IN (47210,47211)),
    signed_event JSONB NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, event_id),
    FOREIGN KEY (community_id, machine_id) REFERENCES machines(community_id, machine_id)
);
COMMENT ON TABLE machine_control_events IS 'Private signed machine journal; excluded from generic event reads, search and fanout.';
SELECT attach_community_write_fence('machines');
SELECT attach_community_write_fence('machine_control_events');
