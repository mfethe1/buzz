-- Reconcile schema details that pgschema does not preserve.
--
-- pgschema reconciles DDL, but it does not execute seed DML or preserve every
-- table storage parameter from schema/schema.sql. It also currently emits
-- partition children as standalone CREATE TABLE statements. Every pgschema
-- apply caller must run this idempotent script so fresh bootstraps converge on
-- the same live database contract as migration-managed databases.

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'events'::regclass
          AND inhrelid = 'events_p_past'::regclass
    ) THEN
        -- pgschema may copy parent triggers onto standalone children. Drop
        -- those copies before ATTACH; PostgreSQL recreates inherited parent
        -- triggers while attaching and rejects same-named child triggers.
        DROP TRIGGER IF EXISTS events_enqueue_push_match ON events_p_past;
        DROP TRIGGER IF EXISTS events_refresh_channel_ttl ON events_p_past;
        DROP TRIGGER IF EXISTS events_created_at_floor ON events_p_past;
        DROP TRIGGER IF EXISTS community_write_fence_events ON events_p_past;
        DROP TRIGGER IF EXISTS trg_events_guard_channel_roster_snapshot ON events_p_past;
        ALTER TABLE events ATTACH PARTITION events_p_past
            FOR VALUES FROM (MINVALUE) TO ('2026-01-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'events'::regclass
          AND inhrelid = 'events_p2026_01'::regclass
    ) THEN
        DROP TRIGGER IF EXISTS events_enqueue_push_match ON events_p2026_01;
        DROP TRIGGER IF EXISTS events_refresh_channel_ttl ON events_p2026_01;
        DROP TRIGGER IF EXISTS events_created_at_floor ON events_p2026_01;
        DROP TRIGGER IF EXISTS community_write_fence_events ON events_p2026_01;
        DROP TRIGGER IF EXISTS trg_events_guard_channel_roster_snapshot ON events_p2026_01;
        ALTER TABLE events ATTACH PARTITION events_p2026_01
            FOR VALUES FROM ('2026-01-01') TO ('2026-02-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'events'::regclass
          AND inhrelid = 'events_p2026_02'::regclass
    ) THEN
        DROP TRIGGER IF EXISTS events_enqueue_push_match ON events_p2026_02;
        DROP TRIGGER IF EXISTS events_refresh_channel_ttl ON events_p2026_02;
        DROP TRIGGER IF EXISTS events_created_at_floor ON events_p2026_02;
        DROP TRIGGER IF EXISTS community_write_fence_events ON events_p2026_02;
        DROP TRIGGER IF EXISTS trg_events_guard_channel_roster_snapshot ON events_p2026_02;
        ALTER TABLE events ATTACH PARTITION events_p2026_02
            FOR VALUES FROM ('2026-02-01') TO ('2026-03-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'events'::regclass
          AND inhrelid = 'events_p2026_03'::regclass
    ) THEN
        DROP TRIGGER IF EXISTS events_enqueue_push_match ON events_p2026_03;
        DROP TRIGGER IF EXISTS events_refresh_channel_ttl ON events_p2026_03;
        DROP TRIGGER IF EXISTS events_created_at_floor ON events_p2026_03;
        DROP TRIGGER IF EXISTS community_write_fence_events ON events_p2026_03;
        DROP TRIGGER IF EXISTS trg_events_guard_channel_roster_snapshot ON events_p2026_03;
        ALTER TABLE events ATTACH PARTITION events_p2026_03
            FOR VALUES FROM ('2026-03-01') TO ('2026-04-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'events'::regclass
          AND inhrelid = 'events_p2026_04'::regclass
    ) THEN
        DROP TRIGGER IF EXISTS events_enqueue_push_match ON events_p2026_04;
        DROP TRIGGER IF EXISTS events_refresh_channel_ttl ON events_p2026_04;
        DROP TRIGGER IF EXISTS events_created_at_floor ON events_p2026_04;
        DROP TRIGGER IF EXISTS community_write_fence_events ON events_p2026_04;
        DROP TRIGGER IF EXISTS trg_events_guard_channel_roster_snapshot ON events_p2026_04;
        ALTER TABLE events ATTACH PARTITION events_p2026_04
            FOR VALUES FROM ('2026-04-01') TO ('2026-05-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'events'::regclass
          AND inhrelid = 'events_p2026_05'::regclass
    ) THEN
        DROP TRIGGER IF EXISTS events_enqueue_push_match ON events_p2026_05;
        DROP TRIGGER IF EXISTS events_refresh_channel_ttl ON events_p2026_05;
        DROP TRIGGER IF EXISTS events_created_at_floor ON events_p2026_05;
        DROP TRIGGER IF EXISTS community_write_fence_events ON events_p2026_05;
        DROP TRIGGER IF EXISTS trg_events_guard_channel_roster_snapshot ON events_p2026_05;
        ALTER TABLE events ATTACH PARTITION events_p2026_05
            FOR VALUES FROM ('2026-05-01') TO ('2026-06-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'events'::regclass
          AND inhrelid = 'events_p2026_06'::regclass
    ) THEN
        DROP TRIGGER IF EXISTS events_enqueue_push_match ON events_p2026_06;
        DROP TRIGGER IF EXISTS events_refresh_channel_ttl ON events_p2026_06;
        DROP TRIGGER IF EXISTS events_created_at_floor ON events_p2026_06;
        DROP TRIGGER IF EXISTS community_write_fence_events ON events_p2026_06;
        DROP TRIGGER IF EXISTS trg_events_guard_channel_roster_snapshot ON events_p2026_06;
        ALTER TABLE events ATTACH PARTITION events_p2026_06
            FOR VALUES FROM ('2026-06-01') TO ('2026-07-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'events'::regclass
          AND inhrelid = 'events_p_future'::regclass
    ) THEN
        DROP TRIGGER IF EXISTS events_enqueue_push_match ON events_p_future;
        DROP TRIGGER IF EXISTS events_refresh_channel_ttl ON events_p_future;
        DROP TRIGGER IF EXISTS events_created_at_floor ON events_p_future;
        DROP TRIGGER IF EXISTS community_write_fence_events ON events_p_future;
        DROP TRIGGER IF EXISTS trg_events_guard_channel_roster_snapshot ON events_p_future;
        ALTER TABLE events ATTACH PARTITION events_p_future
            FOR VALUES FROM ('2026-07-01') TO (MAXVALUE);
    END IF;

    -- When pgschema creates partition children as standalone tables, it also
    -- preserves the parent's identity column on delivery_log children. PostgreSQL
    -- rejects attaching a child table that has its own identity column, so each
    -- delivery_log attach path drops that standalone identity first. Raw
    -- schema-created partitions are already attached, so these branches do not
    -- run against inherited partition columns.

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'delivery_log'::regclass
          AND inhrelid = 'delivery_log_p_past'::regclass
    ) THEN
        ALTER TABLE delivery_log_p_past ALTER COLUMN id DROP IDENTITY IF EXISTS;
        DROP TRIGGER IF EXISTS community_write_fence_delivery_log ON delivery_log_p_past;
        ALTER TABLE delivery_log ATTACH PARTITION delivery_log_p_past
            FOR VALUES FROM (MINVALUE) TO ('2026-03-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'delivery_log'::regclass
          AND inhrelid = 'delivery_log_p2026_03'::regclass
    ) THEN
        ALTER TABLE delivery_log_p2026_03 ALTER COLUMN id DROP IDENTITY IF EXISTS;
        DROP TRIGGER IF EXISTS community_write_fence_delivery_log ON delivery_log_p2026_03;
        ALTER TABLE delivery_log ATTACH PARTITION delivery_log_p2026_03
            FOR VALUES FROM ('2026-03-01') TO ('2026-04-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'delivery_log'::regclass
          AND inhrelid = 'delivery_log_p2026_04'::regclass
    ) THEN
        ALTER TABLE delivery_log_p2026_04 ALTER COLUMN id DROP IDENTITY IF EXISTS;
        DROP TRIGGER IF EXISTS community_write_fence_delivery_log ON delivery_log_p2026_04;
        ALTER TABLE delivery_log ATTACH PARTITION delivery_log_p2026_04
            FOR VALUES FROM ('2026-04-01') TO ('2026-05-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'delivery_log'::regclass
          AND inhrelid = 'delivery_log_p2026_05'::regclass
    ) THEN
        ALTER TABLE delivery_log_p2026_05 ALTER COLUMN id DROP IDENTITY IF EXISTS;
        DROP TRIGGER IF EXISTS community_write_fence_delivery_log ON delivery_log_p2026_05;
        ALTER TABLE delivery_log ATTACH PARTITION delivery_log_p2026_05
            FOR VALUES FROM ('2026-05-01') TO ('2026-06-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'delivery_log'::regclass
          AND inhrelid = 'delivery_log_p2026_06'::regclass
    ) THEN
        ALTER TABLE delivery_log_p2026_06 ALTER COLUMN id DROP IDENTITY IF EXISTS;
        DROP TRIGGER IF EXISTS community_write_fence_delivery_log ON delivery_log_p2026_06;
        ALTER TABLE delivery_log ATTACH PARTITION delivery_log_p2026_06
            FOR VALUES FROM ('2026-06-01') TO ('2026-07-01');
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_inherits
        WHERE inhparent = 'delivery_log'::regclass
          AND inhrelid = 'delivery_log_p_future'::regclass
    ) THEN
        ALTER TABLE delivery_log_p_future ALTER COLUMN id DROP IDENTITY IF EXISTS;
        DROP TRIGGER IF EXISTS community_write_fence_delivery_log ON delivery_log_p_future;
        ALTER TABLE delivery_log ATTACH PARTITION delivery_log_p_future
            FOR VALUES FROM ('2026-07-01') TO (MAXVALUE);
    END IF;
END $$;

-- pgschema reconciles DDL but does not apply seed DML or table storage
-- parameters from schema/schema.sql. Restore those parts of the desired-state
-- contract explicitly and fail the bootstrap if the live catalog disagrees.
ALTER TABLE replica_heartbeat SET (vacuum_truncate = false);

INSERT INTO replica_heartbeat (id) VALUES (1)
ON CONFLICT (id) DO NOTHING;

-- pgschema does not reproduce multi-column CHECK constraints, so it drops
-- chk_users_machine_fields_require_machine_id (it keeps the three
-- single-column machine CHECKs and the partial unique index). Without this,
-- a pgschema-bootstrapped database accepts a machine_label/machine_runtime
-- with no machine_id -- an unaddressable agent home -- while a
-- migration-managed database rejects it.
ALTER TABLE users DROP CONSTRAINT IF EXISTS chk_users_machine_fields_require_machine_id;
ALTER TABLE users
    ADD CONSTRAINT chk_users_machine_fields_require_machine_id
    CHECK (machine_id IS NOT NULL
           OR (machine_label IS NULL AND machine_runtime IS NULL));

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conrelid = 'users'::regclass
          AND conname = 'chk_users_machine_fields_require_machine_id'
    ) THEN
        RAISE EXCEPTION 'users must enforce machine-home field coherence after pgschema apply';
    END IF;
END $$;

-- Same pgschema multi-column CHECK gap on the task system (0046): a task could
-- be marked done with no done_at, or carry a done_at while not done.
ALTER TABLE tasks DROP CONSTRAINT IF EXISTS chk_tasks_done_at_matches_status;
ALTER TABLE tasks
    ADD CONSTRAINT chk_tasks_done_at_matches_status
    CHECK ((status = 'done') = (done_at IS NOT NULL));

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conrelid = 'tasks'::regclass
          AND conname = 'chk_tasks_done_at_matches_status'
    ) THEN
        RAISE EXCEPTION 'tasks must enforce done_at/status coherence after pgschema apply';
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_class AS relation
        JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
        WHERE namespace.nspname = current_schema()
          AND relation.relname = 'replica_heartbeat'
          AND COALESCE(
              relation.reloptions @> ARRAY['vacuum_truncate=false']::text[],
              false
          )
    ) THEN
        RAISE EXCEPTION 'replica_heartbeat must disable vacuum truncation after pgschema apply';
    END IF;

    IF (SELECT count(*) FROM replica_heartbeat WHERE id = 1) <> 1 THEN
        RAISE EXCEPTION 'replica_heartbeat must contain its singleton row after pgschema apply';
    END IF;
END $$;
