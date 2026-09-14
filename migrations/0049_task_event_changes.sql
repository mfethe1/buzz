-- Preserve structured before/after task changes without changing legacy rows.
-- Deploy readers that understand the new action strings before new writers.
ALTER TABLE task_events ADD COLUMN changes JSONB
    CHECK (changes IS NULL OR jsonb_typeof(changes) = 'object');

-- Transaction start can precede another writer's commit while waiting on a
-- task row lock. Timestamp the actual append so history remains chronological.
ALTER TABLE task_events ALTER COLUMN created_at SET DEFAULT clock_timestamp();
