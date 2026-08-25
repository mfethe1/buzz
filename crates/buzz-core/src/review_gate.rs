//! Blind review gate: verifies acceptance criteria without worker narrative.
//!
//! The verifier receives only the task/channel identity, pre-published
//! acceptance criteria, and evidence hashes — never the implementer's diff,
//! logs, or environment. Zero I/O; every check is a pure function.

use thiserror::Error;
use uuid::Uuid;

use crate::cml_event::{CmlTransition, ReducedCmlTask};

/// Errors raised by the blind review gate.
#[derive(Debug, Error)]
pub enum GateError {
    /// Gate input does not reference the task under review.
    #[error("gate input does not match the reduced task")]
    TaskMismatch,
    /// Transition is not a reviewer-cycle transition.
    #[error("invalid transition for the review gate: {0}")]
    InvalidTransition(String),
    /// A fourth rejection was attempted.
    #[error("round bound exceeded: the task must be blocked for human resolution")]
    RoundBoundExceeded,
}

/// Outcome of the blind verification pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateVerdict {
    /// Every criterion is verified and backed by evidence.
    Approved,
    /// One or more criteria failed; each maps to exactly one finding.
    Rejected(Vec<ReviewFinding>),
    /// Some verified criteria lack checkable evidence.
    InsufficientEvidence(Vec<String>),
}

/// A single failed acceptance criterion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFinding {
    /// Identifier of the failed criterion.
    pub criterion_id: String,
    /// Deterministic reason code; never free-form worker narrative.
    pub reason: &'static str,
}

/// The blind verifier contract: only machine-checkable inputs.
#[derive(Debug, Clone)]
pub struct ReviewGateInput {
    /// Task under review.
    pub task_id: Uuid,
    /// Channel hosting the task.
    pub channel_id: Uuid,
    /// Acceptance criteria as `(id, text, verified)` triples.
    pub expected_acceptance: Vec<(String, String, bool)>,
    /// Content hashes of submitted evidence artifacts.
    pub evidence_hashes: Vec<String>,
}

/// Verify the blind contract against a reduced task and produce a verdict.
///
/// Precedence is deterministic: missing evidence outranks findings, and no
/// criteria at all is always `InsufficientEvidence` — never `Approved`.
pub fn verify_blind(
    input: &ReviewGateInput,
    reduced: &ReducedCmlTask,
) -> Result<GateVerdict, GateError> {
    if input.task_id != reduced.task.id {
        return Err(GateError::TaskMismatch);
    }
    if input.expected_acceptance.is_empty() {
        return Ok(GateVerdict::InsufficientEvidence(vec![
            "no acceptance criteria supplied".into(),
        ]));
    }
    let has_evidence = !input.evidence_hashes.is_empty();
    let mut findings = Vec::new();
    let mut missing = Vec::new();
    for (id, _text, verified) in &input.expected_acceptance {
        match (*verified, has_evidence) {
            (true, true) => {}
            (true, false) => missing.push(id.clone()),
            (false, _) => findings.push(ReviewFinding {
                criterion_id: id.clone(),
                reason: "criterion not verified",
            }),
        }
    }
    if !missing.is_empty() {
        Ok(GateVerdict::InsufficientEvidence(missing))
    } else if findings.is_empty() {
        Ok(GateVerdict::Approved)
    } else {
        Ok(GateVerdict::Rejected(findings))
    }
}

/// Apply a reviewer-cycle transition to the round counter.
///
/// `reviewer.reject` increments by exactly one; a fourth rejection is
/// impossible — the caller must emit `worker.block` (or `planner.cancel`)
/// instead. Submit, fix-submit, and approve are round-neutral.
pub fn enforce_round_bound(current_round: u8, transition: CmlTransition) -> Result<u8, GateError> {
    match transition {
        CmlTransition::ReviewReject => {
            if current_round >= 3 {
                Err(GateError::RoundBoundExceeded)
            } else {
                Ok(current_round + 1)
            }
        }
        CmlTransition::Submit | CmlTransition::FixSubmit | CmlTransition::ReviewApprove => {
            Ok(current_round)
        }
        other => Err(GateError::InvalidTransition(other.as_str().into())),
    }
}
