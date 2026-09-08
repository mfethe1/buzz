-- Keep existing approval references; legacy rows have no safe continuation and
-- therefore cannot resume. Decisions and claims are bound to native event IDs.
ALTER TABLE workflow_approvals
    ADD COLUMN request_event_id BYTEA,
    ADD COLUMN decision_event_id BYTEA,
    ADD COLUMN request_message TEXT,
    ADD COLUMN continuation JSONB,
    ADD COLUMN resume_claimed_at TIMESTAMPTZ,
    ADD COLUMN resume_deadline_at TIMESTAMPTZ;

CREATE UNIQUE INDEX idx_workflow_approvals_decision_event
    ON workflow_approvals (community_id, decision_event_id)
    WHERE decision_event_id IS NOT NULL;
CREATE INDEX idx_workflow_approvals_recovery
    ON workflow_approvals (status, resume_deadline_at)
    WHERE continuation IS NOT NULL;

-- A legacy waiting run has no immutable continuation. Preserve its approval
-- history, but expose a terminal failure instead of silently stranding it or
-- executing the workflow's current (possibly changed) definition.
UPDATE workflow_runs SET status='failed', error_code='approval_continuation_unavailable',
    error_message='Legacy approval has no saved continuation; start a new run',
    completed_at=clock_timestamp()
WHERE status='waiting_approval' AND community_write_allowed(community_id);
UPDATE workflow_approvals a SET status='expired'
FROM workflow_runs r
WHERE (r.community_id,r.id)=(a.community_id,a.run_id)
    AND r.error_code='approval_continuation_unavailable' AND a.status='pending'
    AND community_write_allowed(a.community_id);
