//! Durable approval continuation orchestration; signed decisions remain in relay ingest.

use std::{collections::HashMap, sync::Arc};

use buzz_core::CommunityId;
use buzz_db::workflow::{
    approval::{ApprovalContinuation, ApprovalWait},
    hash_approval_token, WorkflowStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    executor::{self, TriggerContext},
    ActionDef, WorkflowDef, WorkflowEngine, WorkflowError,
};

const RESUME_BUDGET_SECS: i64 = 300;

async fn before_resume_deadline<F: std::future::Future>(
    deadline: tokio::time::Instant,
    work: F,
) -> Result<F::Output, ()> {
    // Tokio's timeout may poll a ready inner future before its timer. A late
    // worker must not poll admission or dispatch even once.
    if tokio::time::Instant::now() >= deadline {
        return Err(());
    }
    tokio::time::timeout_at(deadline, work)
        .await
        .map_err(|_| ())
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    definition: WorkflowDef,
    definition_hash: String,
    owner_pubkey: String,
    channel_id: Uuid,
    trigger: TriggerContext,
    outputs: HashMap<String, Value>,
    next_step: usize,
}

impl WorkflowEngine {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn persist_approval_wait(
        &self,
        community: CommunityId,
        run_id: Uuid,
        def: &WorkflowDef,
        trigger: &TriggerContext,
        resolved: &ActionDef,
        token: &str,
        step_index: usize,
        outputs: &HashMap<String, Value>,
        trace: &[Value],
    ) -> Result<(), WorkflowError> {
        let ActionDef::RequestApproval {
            from,
            message,
            timeout,
        } = resolved
        else {
            return Err(WorkflowError::InvalidDefinition(
                "suspended step is not an approval".into(),
            ));
        };
        // Approver identity is authority, never substituted from trigger text.
        let Some(crate::schema::Step {
            action:
                ActionDef::RequestApproval {
                    from: authored_from,
                    ..
                },
            ..
        }) = def.steps.get(step_index)
        else {
            return Err(WorkflowError::InvalidDefinition(
                "approval step missing".into(),
            ));
        };
        let spec = from.trim().to_lowercase();
        if authored_from.trim().to_lowercase() != spec
            || (spec != "any" && !crate::schema::is_lowercase_hex_pubkey(&spec))
        {
            return Err(WorkflowError::InvalidDefinition(
                "approval requires an authored exact pubkey or any current channel member".into(),
            ));
        }
        let timeout_secs = executor::parse_duration_secs(timeout.as_deref().unwrap_or("24h"))?;
        if !(1..=604800).contains(&timeout_secs) || message.trim().is_empty() {
            return Err(WorkflowError::InvalidDefinition(
                "approval requires a message and a timeout between 1 second and 7 days".into(),
            ));
        }
        let run = self.db.get_workflow_run(community, run_id).await?;
        let workflow = self.db.get_workflow(community, run.workflow_id).await?;
        let current_def: WorkflowDef = serde_json::from_value(workflow.definition.clone())
            .map_err(|e| WorkflowError::InvalidDefinition(e.to_string()))?;
        if serde_json::to_value(&current_def).ok() != serde_json::to_value(def).ok()
            || !workflow.enabled
            || workflow.status != WorkflowStatus::Active
        {
            return Err(WorkflowError::Unauthorized(
                "workflow changed before approval suspension".into(),
            ));
        }
        let channel_id = workflow
            .channel_id
            .ok_or_else(|| WorkflowError::Unauthorized("workflow has no channel".into()))?;
        self.check_owner_authority(community, channel_id, &workflow.owner_pubkey, def)
            .await?;
        let snapshot = Snapshot {
            definition: def.clone(),
            definition_hash: hex::encode(&workflow.definition_hash),
            owner_pubkey: hex::encode(&workflow.owner_pubkey),
            channel_id,
            trigger: trigger.clone(),
            outputs: outputs.clone(),
            next_step: step_index + 1,
        };
        let reference = hash_approval_token(token);
        let mut full_trace = run
            .execution_trace
            .as_array()
            .cloned()
            .ok_or_else(|| WorkflowError::Database("run trace is not an array".into()))?;
        full_trace.extend_from_slice(trace);
        full_trace.push(serde_json::json!({"step_id":def.steps[step_index].id,"status":"waiting_approval","approval_ref":hex::encode(&reference)}));
        let wait = ApprovalWait {
            community_id: community,
            workflow_id: run.workflow_id,
            run_id,
            channel_id,
            reference,
            step_id: def.steps[step_index].id.clone(),
            step_index: step_index as i32,
            approver_spec: spec,
            message: message.clone(),
            timeout_secs: timeout_secs as i64,
            continuation: serde_json::to_value(snapshot)
                .map_err(|e| WorkflowError::Database(e.to_string()))?,
            trace: Value::Array(full_trace),
        };
        self.action_sink()?
            .request_approval(wait)
            .await
            .map_err(WorkflowError::from)
    }

    /// Recover at most 100 candidates per tick. Claims are durable and single-use;
    /// capacity is acquired before claiming so a busy pod leaves ready work safe.
    pub async fn recover_approvals(self: &Arc<Self>) -> Result<(), WorkflowError> {
        for (community, reference) in self.db.workflow_approval_candidates().await? {
            let Ok(permit) = Arc::clone(&self.run_semaphore).try_acquire_owned() else {
                break;
            };
            let deadline = tokio::time::Instant::now()
                + std::time::Duration::from_secs(RESUME_BUDGET_SECS as u64);
            let Some(claim) = self
                .db
                .claim_workflow_approval(community, &reference, RESUME_BUDGET_SECS)
                .await?
            else {
                continue;
            };
            let engine = Arc::clone(self);
            tokio::spawn(async move {
                let _permit = permit;
                engine.resume_claim(claim, deadline).await;
            });
        }
        Ok(())
    }

    async fn prepare_resume(
        &self,
        claim: &ApprovalContinuation,
    ) -> Result<Snapshot, WorkflowError> {
        let mut snapshot: Snapshot =
            serde_json::from_value(claim.snapshot.clone()).map_err(|e| {
                WorkflowError::InvalidDefinition(format!("invalid approval continuation: {e}"))
            })?;
        let workflow = self
            .db
            .get_workflow(claim.community_id, claim.workflow_id)
            .await?;
        if snapshot.next_step != claim.next_step as usize
            || !workflow.enabled
            || workflow.status != WorkflowStatus::Active
            || workflow.channel_id != Some(snapshot.channel_id)
            || hex::encode(&workflow.owner_pubkey) != snapshot.owner_pubkey
            || hex::encode(&workflow.definition_hash) != snapshot.definition_hash
        {
            return Err(WorkflowError::Unauthorized(
                "approved workflow version or lifecycle changed".into(),
            ));
        }
        self.check_owner_authority(
            claim.community_id,
            snapshot.channel_id,
            &workflow.owner_pubkey,
            &snapshot.definition,
        )
        .await?;
        if self
            .db
            .get_member_role(
                claim.community_id,
                snapshot.channel_id,
                &claim.approver_pubkey,
            )
            .await?
            .is_none()
        {
            return Err(WorkflowError::Unauthorized(
                "approval signer is no longer a channel member".into(),
            ));
        }
        let approval_step = snapshot
            .definition
            .steps
            .get(snapshot.next_step.saturating_sub(1))
            .ok_or_else(|| {
                WorkflowError::InvalidDefinition("saved approval step missing".into())
            })?;
        snapshot.outputs.insert(approval_step.id.clone(),serde_json::json!({"approved":true,"decision_event_id":hex::encode(&claim.decision_event_id)}));
        Ok(snapshot)
    }

    async fn resume_claim(&self, claim: ApprovalContinuation, deadline: tokio::time::Instant) {
        // The deadline starts before the database claim, covering admission,
        // spawn scheduling and execution. A late worker cannot dispatch.
        let execution = before_resume_deadline(deadline, async {
            let snapshot = self.prepare_resume(&claim).await.map_err(|error| {
                (
                    error,
                    crate::error::PartialProgress {
                        step_index: claim.next_step as usize,
                        trace: vec![],
                    },
                )
            })?;
            executor::execute_steps(
                self,
                claim.community_id,
                claim.run_id,
                &snapshot.definition,
                &snapshot.trigger,
                snapshot.next_step,
                Some(snapshot.outputs),
            )
            .await
        })
        .await;
        // The claim reserves another 30 seconds for finalization. If storage is
        // still unavailable, durable recovery records unknown without replaying.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            match execution {
                Ok(result) => self.finalize_run(
                    claim.community_id,
                    claim.run_id,
                    result,
                    claim.trace.as_array().cloned(),
                ).await,
                Err(_) => {
                    // A later saved approval wait is protected from this old
                    // finalizer; an already-issued external effect is unknown.
                    let _ = self.db.update_workflow_run(
                        claim.community_id,
                        claim.run_id,
                        buzz_db::workflow::RunStatus::Failed,
                        claim.next_step,
                        &claim.trace,
                        Some(buzz_db::workflow::WorkflowRunFailure {
                            code: "approval_resume_outcome_unknown",
                            message: "Continuation timed out; effects may have occurred and will not be replayed",
                        }),
                    ).await;
                }
            }
        }).await;
    }
}

#[cfg(test)]
mod postgres_tests {
    use super::*;
    use buzz_db::workflow::RunStatus;
    use serde_json::json;
    use std::time::Duration;

    async fn claimed_fixture() -> (buzz_db::Db, ApprovalContinuation) {
        let db = buzz_db::Db::new(&buzz_db::DbConfig {
            database_url: std::env::var("BUZZ_TEST_DATABASE_URL").expect("isolated database"),
            max_connections: 1,
            min_connections: 1,
            acquire_timeout_secs: 5,
            ..Default::default()
        })
        .await
        .expect("real Postgres pool");
        let owner = nostr::Keys::generate().public_key().to_bytes();
        let host = format!("resume-deadline-{}.example", Uuid::new_v4());
        let community = db
            .ensure_configured_community(&host)
            .await
            .expect("community")
            .id;
        db.ensure_user(community, &owner).await.expect("owner");
        let channel = db
            .create_channel(
                community,
                "deadline",
                buzz_core::channel::ChannelType::Stream,
                buzz_core::channel::ChannelVisibility::Open,
                None,
                &owner,
                None,
            )
            .await
            .expect("channel")
            .id;
        let definition: WorkflowDef = serde_json::from_value(json!({
            "name":"deadline", "trigger":{"on":"webhook"}, "enabled":true,
            "steps":[{"id":"review","action":"request_approval","from":"any","message":"approve"},
                {"id":"after","action":"delay","duration":"0s"}],
        }))
        .expect("definition");
        let definition_hash = vec![42; 32];
        let workflow = db
            .create_workflow(
                community,
                Some(channel),
                &owner,
                "deadline",
                &serde_json::to_string(&definition).expect("definition JSON"),
                &definition_hash,
            )
            .await
            .expect("workflow");
        let run = db
            .create_workflow_run(community, workflow, None, None)
            .await
            .expect("run");
        db.update_workflow_run(community, run, RunStatus::Running, 1, &json!([]), None)
            .await
            .expect("admitted run");
        let claim = ApprovalContinuation {
            community_id: community,
            reference: vec![1; 32],
            run_id: run,
            workflow_id: workflow,
            snapshot: serde_json::to_value(Snapshot {
                definition,
                definition_hash: hex::encode(definition_hash),
                owner_pubkey: hex::encode(owner),
                channel_id: channel,
                trigger: TriggerContext::default(),
                outputs: HashMap::new(),
                next_step: 1,
            })
            .expect("snapshot"),
            trace: json!([]),
            next_step: 1,
            approver_pubkey: owner.to_vec(),
            decision_event_id: vec![2; 32],
        };
        (db, claim)
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn workflow_approval_resume_deadline_includes_blocked_admission() {
        let (db, claim) = claimed_fixture().await;
        let community = claim.community_id;
        let run = claim.run_id;
        // Occupy the sole real database connection while resume tries to read
        // its workflow. Releasing it after the budget must not admit steps.
        let held = db
            .begin_event_write_transaction()
            .await
            .expect("occupy pool");
        let engine = Arc::new(WorkflowEngine::new(
            db.clone(),
            crate::WorkflowConfig::default(),
        ));
        let worker = tokio::spawn(async move {
            engine
                .resume_claim(
                    claim,
                    tokio::time::Instant::now() + Duration::from_millis(25),
                )
                .await;
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        held.rollback().await.expect("release connection");
        tokio::time::timeout(Duration::from_secs(3), worker)
            .await
            .expect("bounded resume")
            .expect("worker");
        let outcome = db
            .get_workflow_run(community, run)
            .await
            .expect("durable outcome");
        assert_eq!(outcome.status, RunStatus::Failed);
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("approval_resume_outcome_unknown")
        );
        assert_eq!(
            outcome.current_step, 1,
            "no post-approval step was admitted"
        );
        assert_eq!(outcome.execution_trace, json!([]));
    }

    #[tokio::test]
    async fn workflow_approval_elapsed_deadline_never_polls_ready_work() {
        let polled = std::sync::atomic::AtomicBool::new(false);
        let result = before_resume_deadline(tokio::time::Instant::now(), async {
            polled.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .await;
        assert!(result.is_err());
        assert!(
            !polled.load(std::sync::atomic::Ordering::SeqCst),
            "expired worker polled work"
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn workflow_approval_spawn_delay_cannot_restart_resume_budget() {
        let (db, claim) = claimed_fixture().await;
        let community = claim.community_id;
        let run = claim.run_id;
        let engine = Arc::new(WorkflowEngine::new(
            db.clone(),
            crate::WorkflowConfig::default(),
        ));
        let deadline = tokio::time::Instant::now() + Duration::from_millis(25);
        let (release, delayed) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            delayed.await.expect("delayed worker released");
            engine.resume_claim(claim, deadline).await;
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        release.send(()).expect("release worker past deadline");
        tokio::time::timeout(Duration::from_secs(3), worker)
            .await
            .expect("bounded late worker")
            .expect("worker");
        let outcome = db
            .get_workflow_run(community, run)
            .await
            .expect("durable outcome");
        assert_eq!(outcome.status, RunStatus::Failed);
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("approval_resume_outcome_unknown")
        );
        assert_eq!(outcome.current_step, 1);
        assert_eq!(
            outcome.execution_trace,
            json!([]),
            "late worker must not execute any step"
        );
    }
}
