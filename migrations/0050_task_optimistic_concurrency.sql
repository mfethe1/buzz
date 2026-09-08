-- HW-017: Optimistic concurrency guard for task PATCH.
--
-- A stale write (two clients fetch the same task, both PATCH, the second
-- clobbering the first) must not silently win. `revision` is a monotonic
-- counter that increments when persisted task fields change, so a PATCH carrying
-- `expected_revision` can compare-and-swap: if the row's revision does not
-- match, the relay returns 409 and the caller re-fetches.
--
-- The counter is maintained by a BEFORE UPDATE trigger so it applies to every
-- write path, not just the relay's `update_task`. This keeps the guarantee
-- structural rather than convention-based. The trigger bumps only when the
-- row's payload actually changed, so an idempotent restate cannot manufacture
-- a spurious conflict for other readers (see the function body).
--
-- Backward compatibility: a PATCH that omits `expected_revision` (every
-- existing client at the time this ships) skips the guard entirely and
-- behaves exactly as before. The column defaults to 0 so existing rows
-- receive a revision without a backfill; the trigger sets it to 1 on the
-- first post-migration UPDATE that changes something.
--
-- Additive only: ALTER TABLE ... ADD COLUMN does not rewrite history that
-- brownfield relays have already applied, so 0001's checksum is untouched.
SET LOCAL lock_timeout = '5s';

ALTER TABLE tasks ADD COLUMN revision INT NOT NULL DEFAULT 0;

-- A monotonic counter the relay can compare-and-swap against. The trigger
-- fires on every UPDATE, so the guarantee is structural — not a convention
-- the relay could forget.
CREATE OR REPLACE FUNCTION bump_task_revision()
RETURNS TRIGGER AS $$
BEGIN
    -- Bump ONLY when the row's payload actually changed.
    --
    -- A statement that restates every column at its existing value (a client
    -- retry, or a PATCH that sets status to the status it already holds) still
    -- fires a BEFORE UPDATE trigger. Bumping there would be a correctness bug,
    -- not a harmless extra: it would invalidate every other client's
    -- `expected_revision` for a write that changed nothing, so an idempotent
    -- retry would manufacture spurious 409s. The existing task-event logic
    -- already suppresses same-value events (`status_change_action` returns
    -- None on an unchanged status); revision must agree with that guarantee.
    --
    -- The two derived columns are first normalised to their OLD values so the
    -- whole-row comparison sees only caller-supplied payload. Comparing the
    -- entire row rather than an enumerated column list means a future column
    -- added to `tasks` is covered automatically — an explicit list would
    -- silently stop guarding whatever someone forgot to add to it.
    NEW.revision := OLD.revision;
    NEW.updated_at := OLD.updated_at;
    IF NEW IS DISTINCT FROM OLD THEN
        NEW.revision := OLD.revision + 1;
        NEW.updated_at := clock_timestamp();
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_tasks_revision
    BEFORE UPDATE ON tasks
    FOR EACH ROW
    EXECUTE FUNCTION bump_task_revision();
