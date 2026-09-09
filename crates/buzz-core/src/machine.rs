//! Private machine enrollment and observations. These commands are never feed events.

use nostr::{Event, PublicKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Server freshness bound; observations are reports, never execution authority.
pub const OBSERVATION_TTL_SECS: i64 = 120;
/// Maximum age of a newly received observation.
pub const MAX_OBSERVATION_AGE_SECS: i64 = 30;
/// Maximum tolerated forward clock skew.
pub const MAX_CLOCK_SKEW_SECS: i64 = 5;

/// Runtime identity, independent of the machine's mutable presentation label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MachineRuntime {
    /// Hermes coordinator.
    Hermes,
    /// OpenClaw coordinator.
    Openclaw,
    /// Codex coordinator.
    Codex,
    /// Claude Code coordinator.
    ClaudeCode,
}

impl MachineRuntime {
    /// Stable storage value, also used for the existing agent type/home binding.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hermes => "hermes",
            Self::Openclaw => "openclaw",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }
}

/// Owner-signed registration. The outer signature binds the community and machine;
/// the NIP-OA proof must independently bind this owner to the coordinator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineEnrollment {
    /// Wire version (exactly 1).
    pub version: u32,
    /// Must equal the server-resolved tenant.
    pub community_id: Uuid,
    /// Opaque stable identifier, not a hostname or connection address.
    pub machine_id: Uuid,
    /// Coordinator's lowercase public key.
    pub coordinator_pubkey: String,
    /// Human-readable display label; no connection credentials or paths.
    pub label: String,
    /// Runtime serving this machine.
    pub runtime: MachineRuntime,
    /// Canonical NIP-OA auth tag, verified against the coordinator and this action.
    pub owner_auth: [String; 4],
    /// Short-lived coordinator consent bound to every enrollment field.
    pub coordinator_consent: Event,
}

/// Coordinator consent is embedded in owner enrollment only. Standalone consent
/// events are rejected by ingest, and never enter ordinary storage or fanout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineEnrollmentConsent {
    /// Consent format version (exactly 1).
    pub version: u32,
    /// Exact tenant audience.
    pub community_id: Uuid,
    /// Exact stable machine identity.
    pub machine_id: Uuid,
    /// Exact enrollment owner.
    pub owner_pubkey: String,
    /// Exact consenting coordinator.
    pub coordinator_pubkey: String,
    /// Exact approved presentation label.
    pub label: String,
    /// Exact approved runtime.
    pub runtime: MachineRuntime,
    /// Exact owner delegation accepted by the coordinator.
    pub owner_auth: [String; 4],
    /// Epoch-second expiry, at most 300 seconds after the consent signature.
    pub expires_at: i64,
}

/// Verify coordinator possession and exact enrollment consent at server time.
/// CPU-bound: async callers must run this in a blocking worker.
pub fn verify_enrollment_consent(
    enrollment: &MachineEnrollment,
    owner: &PublicKey,
    enrollment_created_at: u64,
    now: i64,
) -> Result<(), String> {
    crate::verify_event(&enrollment.coordinator_consent)
        .map_err(|_| "invalid coordinator consent signature")?;
    validate_enrollment_consent_after_signature(enrollment, owner, enrollment_created_at, now)
}

/// Check exact binding and current validity after signature verification. The
/// transaction repeats this check against its primary clock before persistence.
pub fn validate_enrollment_consent_after_signature(
    enrollment: &MachineEnrollment,
    owner: &PublicKey,
    enrollment_created_at: u64,
    now: i64,
) -> Result<(), String> {
    let event = &enrollment.coordinator_consent;
    if event.kind.as_u16() as u32 != crate::kind::KIND_MACHINE_ENROLLMENT_CONSENT
        || !event.tags.is_empty()
        || event.content.len() > 2048
        || event.pubkey.to_hex() != enrollment.coordinator_pubkey
    {
        return Err("invalid coordinator consent envelope".into());
    }
    let consent: MachineEnrollmentConsent =
        serde_json::from_str(&event.content).map_err(|_| "invalid coordinator consent")?;
    let signed = i64::try_from(event.created_at.as_secs()).map_err(|_| "invalid consent time")?;
    let outer = i64::try_from(enrollment_created_at).map_err(|_| "invalid enrollment time")?;
    if consent.version != 1
        || consent.community_id != enrollment.community_id
        || consent.machine_id != enrollment.machine_id
        || consent.owner_pubkey != owner.to_hex()
        || consent.coordinator_pubkey != enrollment.coordinator_pubkey
        || consent.label != enrollment.label
        || consent.runtime != enrollment.runtime
        || consent.owner_auth != enrollment.owner_auth
        || signed > now + MAX_CLOCK_SKEW_SECS
        || signed < now - 300
        || consent.expires_at <= now
        || consent.expires_at > signed.saturating_add(300)
        || outer < signed - MAX_CLOCK_SKEW_SECS
        || outer >= consent.expires_at
    {
        return Err("coordinator consent does not match live enrollment".into());
    }
    Ok(())
}

/// The coordinator's bounded report; this does not assert verified host health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    /// Coordinator reports it can receive work.
    Ready,
    /// Coordinator reports ongoing work.
    Busy,
    /// Coordinator reports it cannot currently receive work.
    Unavailable,
}

/// Coordinator-signed report bound to one immutable enrollment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineObservation {
    /// Wire version (exactly 1).
    pub version: u32,
    /// Must equal the server-resolved tenant.
    pub community_id: Uuid,
    /// Registered stable machine identifier.
    pub machine_id: Uuid,
    /// Exact enrollment event, preventing reports from crossing registrations.
    pub registration_event_id: String,
    /// Strictly increasing, JSON-safe positive sequence number.
    pub sequence: i64,
    /// Reported availability, not execution permission.
    pub state: MachineState,
}

/// Closed command set accepted only by the private machine store.
#[derive(Debug, Clone)]
pub enum MachineCommand {
    /// Establish immutable ownership and coordinator binding.
    Enroll(Box<MachineEnrollment>),
    /// Refresh the current observation after registration validation.
    Observe(MachineObservation),
}

fn hex32(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

impl MachineCommand {
    /// Parse the closed bounded wire shape after transport signature verification.
    /// No tags are accepted: routing, mentions and workflow tags have no role here.
    pub fn from_event_after_signature(event: &Event) -> Result<Self, String> {
        if !event.tags.is_empty() || event.content.len() > 4096 {
            return Err("machine commands require no tags and at most 4096 content bytes".into());
        }
        let command = match event.kind.as_u16() as u32 {
            crate::kind::KIND_MACHINE_ENROLLMENT => {
                let value: MachineEnrollment = serde_json::from_str(&event.content)
                    .map_err(|_| "invalid machine enrollment")?;
                if !hex32(&value.coordinator_pubkey)
                    || PublicKey::from_hex(&value.coordinator_pubkey).is_err()
                    || value.coordinator_pubkey == event.pubkey.to_hex()
                    || value.label.trim() != value.label
                    || value.label.is_empty()
                    || value.label.len() > 80
                    || value.label.chars().any(char::is_control)
                {
                    return Err("invalid coordinator or machine label".into());
                }
                Self::Enroll(Box::new(value))
            }
            crate::kind::KIND_MACHINE_OBSERVATION => {
                let value: MachineObservation = serde_json::from_str(&event.content)
                    .map_err(|_| "invalid machine observation")?;
                if !hex32(&value.registration_event_id)
                    || !(1..=9_007_199_254_740_991).contains(&value.sequence)
                {
                    return Err("invalid registration or observation sequence".into());
                }
                Self::Observe(value)
            }
            _ => return Err("not a machine command".into()),
        };
        let version = match &command {
            Self::Enroll(v) => v.version,
            Self::Observe(v) => v.version,
        };
        if version != 1 || command.machine_id().is_nil() || command.community_id().is_nil() {
            return Err("unsupported machine version or empty identity".into());
        }
        Ok(command)
    }

    /// Signed community audience.
    pub fn community_id(&self) -> Uuid {
        match self {
            Self::Enroll(v) => v.community_id,
            Self::Observe(v) => v.community_id,
        }
    }

    /// Stable registered machine.
    pub fn machine_id(&self) -> Uuid {
        match self {
            Self::Enroll(v) => v.machine_id,
            Self::Observe(v) => v.machine_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};
    use serde_json::json;
    #[test]
    fn machine_wire_rejects_unknown_duplicate_fields_tags_and_unbounded_sequences() {
        let keys = Keys::generate();
        let base = json!({"version":1,"community_id":Uuid::new_v4(),"machine_id":Uuid::new_v4(),"registration_event_id":"a".repeat(64),"sequence":1,"state":"ready"});
        let sign = |content: String| {
            EventBuilder::new(Kind::Custom(47211), content)
                .sign_with_keys(&keys)
                .unwrap()
        };
        assert!(MachineCommand::from_event_after_signature(&sign(base.to_string())).is_ok());
        for (key, value) in [
            ("version", json!(2)),
            ("sequence", json!(0)),
            ("sequence", json!(9007199254740992i64)),
            ("state", json!("running")),
            ("ssh_path", json!("secret")),
            ("registration_event_id", json!("A".repeat(64))),
        ] {
            let mut bad = base.clone();
            bad[key] = value;
            assert!(
                MachineCommand::from_event_after_signature(&sign(bad.to_string())).is_err(),
                "{key}"
            );
        }
        let duplicate = base.to_string().replacen("{", "{\"sequence\":2,", 1);
        assert!(MachineCommand::from_event_after_signature(&sign(duplicate)).is_err());
        let tagged = EventBuilder::new(Kind::Custom(47211), base.to_string())
            .tags([Tag::parse(["h", "private"]).unwrap()])
            .sign_with_keys(&keys)
            .unwrap();
        assert!(MachineCommand::from_event_after_signature(&tagged).is_err());
    }
}
