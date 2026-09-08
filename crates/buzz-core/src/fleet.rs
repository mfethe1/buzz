//! Signed contracts for the fixed fleet repository-qualification operation.
//!
//! CML describes the plan; these types bind its execution to a relay admission.
//! A stored CML event by itself is never a fresh execution permit.

use nostr::Event;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{cml::CmlTask, cml_event::CmlEventError, kind::KIND_JOB_RESULT, CommunityId};

/// Versioned opt-in extension. Older fleet plans have no server admission.
pub const EXTENSION: &str = "org.buzz.fleet.v2";
/// Signed result/acknowledgement protocol; its `d` tag is the attempt, not task.
pub const RECEIPT_PROTOCOL: &str = "buzz-fleet-receipt";

/// The immutable authority and repository scope approved by the planner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FleetScope {
    /// Operator-configured host alias, never a command or address.
    pub target: String,
    /// Stable registered machine-home identity.
    pub machine_id: String,
    /// Operator-configured repository alias, never a filesystem path.
    pub repository: String,
    /// Only `qualify` is implemented.
    pub capability: String,
    /// Absolute deadline, at most one hour after the plan.
    pub expires_at: u64,
    /// Human task revision approved by this plan.
    pub task_revision: i32,
    /// Digest of the reviewed public scheduler/host policy.
    pub policy_digest: String,
}

impl FleetScope {
    /// Read and validate the opt-in scope without interpreting other extensions.
    pub fn from_task(task: &CmlTask) -> Result<Option<Self>, CmlEventError> {
        let Some(value) = task.extensions.get(EXTENSION) else {
            return Ok(None);
        };
        let scope: Self = serde_json::from_value(value.clone()).map_err(invalid)?;
        for (name, value) in [
            ("target", &scope.target),
            ("machine_id", &scope.machine_id),
            ("repository", &scope.repository),
        ] {
            if value.is_empty()
                || value.len() > 128
                || !value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            {
                return Err(invalid(format!("invalid fleet {name}")));
            }
        }
        if scope.capability != "qualify"
            || scope.task_revision < 0
            || !hex_id(&scope.policy_digest, 64)
        {
            return Err(invalid(
                "unsupported fleet capability, revision, or policy digest",
            ));
        }
        // Transitions retain this immutable deadline, so the plan-time upper
        // bound is additionally checked when its root is admitted.
        if scope.expires_at == 0 {
            return Err(invalid("fleet deadline required"));
        }
        Ok(Some(scope))
    }
}

/// Stable attempt identity, including the tenant rather than a display URL.
pub fn attempt_id(community: CommunityId, task: Uuid, plan_event_id: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"buzz-fleet-attempt-v2\0");
    hash.update(community.as_uuid().as_bytes());
    hash.update(task.as_bytes());
    hash.update(plan_event_id);
    format!("buzz-qualify-{}", hex::encode(hash.finalize()))
}

/// Observed fixed-operation outcome. `unknown` never acknowledges stopping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    /// Fixed probe finished successfully; independent review remains required.
    Success,
    /// Fixed probe returned a known error.
    Error,
    /// An accepted start has a known stopped outcome; any spawned probe exited.
    Cancelled,
    /// A local exclusive lock proved that execution never began.
    CancelledBeforeExecution,
    /// A prior start has no known terminal result; automatic replay is refused.
    Unknown,
}

/// Bounded output of the sole implemented operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Qualification {
    /// Canonical repository ID approved in CML.
    pub repository: String,
    /// Observed Git commit, not a worktree cleanliness assertion.
    pub head_sha: String,
    /// Number of entries in the repository index.
    pub tracked_files: u64,
    /// Executing Python version.
    pub python: String,
}

/// Worker-signed execution fact, distinct from a planner's cancellation intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FleetReceipt {
    /// Stable relay-generated attempt ID.
    pub attempt_id: String,
    /// Human/CML task UUID.
    pub task_id: Uuid,
    /// Approved signed plan event ID.
    pub plan_event_id: String,
    /// Exact server-admitted start, absent only for a proven pre-start stop.
    pub start_event_id: Option<String>,
    /// Registered execution machine.
    pub machine_id: String,
    /// Frozen public policy digest.
    pub policy_digest: String,
    /// Observed result, never review approval.
    pub status: ReceiptStatus,
    /// Probe output, present only on success.
    pub qualification: Option<Qualification>,
    /// Bounded error code, absent on success and pre-start cancellation.
    pub error: Option<String>,
    /// Frozen Unix time for deterministic event ID recovery.
    pub completed_at: u64,
}

impl FleetReceipt {
    /// Canonical signed representation shared by the relay and native CLI.
    pub fn to_canonical_json(&self) -> Result<String, CmlEventError> {
        serde_json::to_value(self)
            .map(crate::cml::sort_json)
            .and_then(|v| serde_json::to_string(&v))
            .map_err(invalid)
    }

    /// Validate the signed envelope after the ingest pipeline verified NIP-01.
    pub fn from_event_after_signature(event: &Event) -> Result<(Uuid, Self), CmlEventError> {
        let tag = |name: &str| -> Result<Vec<String>, CmlEventError> {
            let all: Vec<_> = event
                .tags
                .iter()
                .filter(|t| t.as_slice().first().map(String::as_str) == Some(name))
                .collect();
            if all.len() != 1 {
                return Err(invalid(format!("one {name} tag required")));
            }
            Ok(all[0].as_slice().to_vec())
        };
        if u32::from(event.kind.as_u16()) != KIND_JOB_RESULT
            || tag("protocol")? != ["protocol", RECEIPT_PROTOCOL, "1"]
        {
            return Err(invalid("invalid fleet receipt kind/protocol"));
        }
        let h = tag("h")?;
        let d = tag("d")?;
        if h.len() != 2 || d.len() != 2 || event.content.len() > 8192 {
            return Err(invalid("invalid fleet receipt envelope"));
        }
        let channel = h[1].parse::<Uuid>().map_err(invalid)?;
        let receipt: Self = serde_json::from_str(&event.content).map_err(invalid)?;
        if receipt.to_canonical_json()? != event.content
            || d[1] != receipt.attempt_id
            || !valid_attempt_id(&receipt.attempt_id)
        {
            return Err(invalid("noncanonical or mismatched fleet receipt"));
        }
        if !hex_id(&receipt.plan_event_id, 64)
            || !hex_id(&receipt.policy_digest, 64)
            || receipt.completed_at == 0
            || receipt.completed_at > event.created_at.as_secs()
        {
            return Err(invalid("invalid fleet receipt identity/time"));
        }
        let pre_start = receipt.status == ReceiptStatus::CancelledBeforeExecution;
        if receipt
            .start_event_id
            .as_ref()
            .is_some_and(|id| !hex_id(id, 64))
            || pre_start != receipt.start_event_id.is_none()
        {
            return Err(invalid("fleet receipt start binding required"));
        }
        if (receipt.status == ReceiptStatus::Success) != receipt.qualification.is_some()
            || receipt
                .error
                .as_ref()
                .is_some_and(|e| e.is_empty() || e.len() > 256 || e.chars().any(char::is_control))
        {
            return Err(invalid("invalid fleet result shape"));
        }
        if let Some(q) = &receipt.qualification {
            if (!hex_id(&q.head_sha, 40) && !hex_id(&q.head_sha, 64))
                || q.repository.is_empty()
                || q.repository.len() > 256
                || q.python.is_empty()
                || q.python.len() > 64
                || receipt.error.is_some()
            {
                return Err(invalid("invalid qualification output"));
            }
        }
        Ok((channel, receipt))
    }
}

/// Recognize only this versioned receipt protocol; unknown protocols still fail.
pub fn is_receipt(event: &Event) -> bool {
    event.tags.iter().any(|t| {
        t.as_slice().first().map(String::as_str) == Some("protocol")
            && t.as_slice().get(1).map(String::as_str) == Some(RECEIPT_PROTOCOL)
    })
}

/// Check the fixed attempt-ID wire shape without interpreting it as authority.
pub fn valid_attempt_id(value: &str) -> bool {
    value
        .strip_prefix("buzz-qualify-")
        .is_some_and(|v| hex_id(v, 64))
}

fn hex_id(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn invalid(error: impl std::fmt::Display) -> CmlEventError {
    CmlEventError::Invalid(error.to_string())
}

#[cfg(test)]
#[path = "fleet_tests.rs"]
mod tests;
