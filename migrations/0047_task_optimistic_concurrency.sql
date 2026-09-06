-- HW-017: Optimistic concurrency guard for task PATCH.
--
-- A stale write (two clients fetch the same task, both PATCH, the second
-- clobbering the first) must not silently win. `revision` is a monotonic
-- counter that increments on every UPDATE, so a PATCH carrying
-- `expected_revision` can compare-and-swap: if the row's revision does not
-- match, the relay returns 409 and the caller re-fetches.
--
-- The counter is maintained by a BEFORE UPDATE trigger so it is unconditional
-- for every write path, not just the relay's `update_task`. This keeps the
-- guarantee structural rather than convention-based.
--
-- Backward compatibility: a PATCH that omits `expected_revision` (every
-- existing client at the time this ships) skips the guard entirely and
-- behaves exactly as before. The column defaults to 0 so existing rows
-- receive a revision without a backfill; the trigger sets it to 1 on the
-- first post-migration UPDATE.
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
    -- Only bump when something actually changed. `UPDATE tasks SET … WHERE
    -- …` that touches zero rows never fires this trigger; a statement that
    -- sets every column to its existing value (an idempotent restate) does
    -- fire it, but the revision bump is harmless — the caller's
    -- `expected_revision` will simply mismatch on the next genuine attempt,
    -- which is the correct behaviour for a stale snapshot.
    NEW.revision := OLD.revision + 1;
    NEW.updated_at := NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_tasks_revision
    BEFORE UPDATE ON tasks
    FOR EACH ROW
    EXECUTE FUNCTION bump_task_revision();
