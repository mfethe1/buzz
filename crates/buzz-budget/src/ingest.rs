//! Turning a `kind:44200` event on the wire into something the ledger can charge.
//!
//! This is the only part of the crate that knows about Nostr. Everything else
//! works on plain values, which is what keeps [`crate::Ledger`] and
//! [`crate::TriggerLog`] testable without keys or events.
//!
//! The metric's content is NIP-44 v2 ciphertext addressed **agent key → owner
//! pubkey** (`crates/buzz-core/src/agent_turn_metric.rs:1-5`), so only the owner
//! can read it. That is why the budget is enforced owner-side: the relay
//! structurally cannot see `costUsd`.

use chrono::{DateTime, Utc};
use nostr::{Event, Keys};
use uuid::Uuid;

use buzz_core::agent_turn_metric::decrypt_agent_turn_metric;
use buzz_core::kind::KIND_AGENT_TURN_METRIC;

/// One completed turn, ready to be attributed and charged.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnCharge {
    /// Channel the turn served. Carried *inside* the encrypted payload, not as
    /// an `h` tag, so it is only readable by the owner.
    pub channel_id: Uuid,
    /// The agent that took the turn — the event's author.
    pub agent_pubkey: String,
    /// Spend for this turn. `None` when the harness did not report it; see
    /// ticket #7 OQ7.2, such a turn is charged zero and is effectively
    /// invisible to the budget.
    pub cost_usd: Option<f64>,
    /// End-of-turn timestamp from the payload.
    pub turn_end: DateTime<Utc>,
    /// `false` when the publisher could not observe the previous cumulative
    /// baseline, making `cost_usd` unreliable. Surfaced rather than swallowed —
    /// ticket #7 OQ7.1 asks how common this is in practice.
    pub delta_reliable: bool,
}

/// Why a `kind:44200` event could not be turned into a [`TurnCharge`].
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    /// The event is not `kind:44200` — expected when filtering a mixed stream.
    #[error("event kind {0} is not the agent turn metric kind ({KIND_AGENT_TURN_METRIC})")]
    WrongKind(u16),
    /// Decryption or NIP-AM validation failed. Also the case when the metric
    /// was addressed to a different owner.
    #[error("could not decrypt or validate the turn metric: {0}")]
    Payload(String),
    /// The payload omitted `channelId`, so the turn cannot be scoped.
    #[error("payload has no channelId")]
    MissingChannelId,
    /// `channelId` was present but is not a UUID.
    #[error("payload channelId {0:?} is not a UUID")]
    BadChannelId(String),
    /// `timestamp` was present but is not RFC 3339.
    #[error("payload timestamp {0:?} is not RFC 3339")]
    BadTimestamp(String),
    /// The self-reported turn time is too far from the signed `created_at` to
    /// be trusted. See [`MAX_TIMESTAMP_SKEW_SECS`].
    #[error(
        "payload timestamp {payload} is {skew_secs}s from the event's created_at {created_at} \
         (max {MAX_TIMESTAMP_SKEW_SECS}s)"
    )]
    TimestampSkew {
        /// The self-reported end-of-turn time from inside the payload.
        payload: String,
        /// The event's `created_at`, which is covered by the signature and
        /// checkable by the relay.
        created_at: String,
        /// Absolute difference, in seconds.
        skew_secs: i64,
    },
}

/// How far the payload's self-reported `timestamp` may sit from the event's
/// `created_at` before the metric is rejected.
///
/// This exists because the payload timestamp is **attacker-controlled by the
/// very agent being budgeted**: it is chosen by the agent, sealed inside its own
/// NIP-44 ciphertext, and `AgentTurnMetricPayload::validate` checks only the
/// numerics. Without this check, one metric dated far in the future moves the
/// ledger's eviction cutoff forward and wipes the pair's whole window — letting
/// a runaway agent zero its own budget at will.
///
/// `created_at` is not a perfect oracle (it is also chosen by the signer), but
/// it is covered by the signature and is the field a relay can and does police,
/// so tying the two together removes the free-form forgery.
pub const MAX_TIMESTAMP_SKEW_SECS: i64 = 300;

/// Decrypt a `kind:44200` event with the owner's keys and extract what the
/// budget needs.
///
/// Returns [`IngestError::WrongKind`] rather than panicking on unrelated
/// events, so a caller can pass a whole subscription stream through this.
pub fn charge_from_metric(owner_keys: &Keys, event: &Event) -> Result<TurnCharge, IngestError> {
    let kind = event.kind.as_u16();
    if u32::from(kind) != KIND_AGENT_TURN_METRIC {
        return Err(IngestError::WrongKind(kind));
    }

    let payload = decrypt_agent_turn_metric(owner_keys, event)
        .map_err(|e| IngestError::Payload(e.to_string()))?;

    let channel_raw = payload.channel_id.ok_or(IngestError::MissingChannelId)?;
    let channel_id = Uuid::parse_str(&channel_raw)
        .map_err(|_| IngestError::BadChannelId(channel_raw.clone()))?;

    let turn_end = DateTime::parse_from_rfc3339(&payload.timestamp)
        .map_err(|_| IngestError::BadTimestamp(payload.timestamp.clone()))?
        .with_timezone(&Utc);

    // The payload timestamp is chosen by the agent being budgeted. Tie it to
    // the signed `created_at` so it cannot be used to shift the ledger window.
    let created_at = DateTime::from_timestamp(event.created_at.as_secs() as i64, 0)
        .ok_or_else(|| IngestError::BadTimestamp(event.created_at.to_string()))?;
    let skew_secs = (turn_end - created_at).num_seconds().abs();
    if skew_secs > MAX_TIMESTAMP_SKEW_SECS {
        return Err(IngestError::TimestampSkew {
            payload: payload.timestamp.clone(),
            created_at: created_at.to_rfc3339(),
            skew_secs,
        });
    }

    Ok(TurnCharge {
        channel_id,
        agent_pubkey: event.pubkey.to_hex(),
        cost_usd: payload.turn.as_ref().and_then(|t| t.cost_usd),
        turn_end,
        delta_reliable: payload.delta_reliable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_core::agent_turn_metric::{
        encrypt_agent_turn_metric, AgentTurnMetricPayload, TokenCounts,
    };
    use nostr::{EventBuilder, Kind};

    const CHANNEL: &str = "12345678-1234-1234-1234-123456789abc";

    fn payload(cost: Option<f64>, channel: Option<&str>, ts: &str) -> AgentTurnMetricPayload {
        AgentTurnMetricPayload {
            harness: "goose".into(),
            model: None,
            channel_id: channel.map(|c| c.to_string()),
            session_id: None,
            turn_id: None,
            turn_seq: None,
            timestamp: ts.into(),
            turn: Some(TokenCounts {
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cost_usd: cost,
                cache_read_tokens: None,
                cache_write_tokens: None,
            }),
            cumulative: None,
            delta_reliable: true,
            stop_reason: None,
        }
    }

    /// Build a real signed kind:44200 event the way an agent would, with
    /// `created_at` consistent with the payload's own timestamp.
    fn metric_event(agent: &Keys, owner: &Keys, p: &AgentTurnMetricPayload) -> Event {
        let created = DateTime::parse_from_rfc3339(&p.timestamp)
            .map(|t| t.timestamp() as u64)
            .unwrap_or(1_700_000_000);
        metric_event_at(agent, owner, p, created)
    }

    /// Same, but with `created_at` chosen independently — used to exercise the
    /// skew check.
    fn metric_event_at(
        agent: &Keys,
        owner: &Keys,
        p: &AgentTurnMetricPayload,
        created_at: u64,
    ) -> Event {
        let content = encrypt_agent_turn_metric(agent, &owner.public_key(), p)
            .expect("payload should encrypt");
        EventBuilder::new(Kind::Custom(KIND_AGENT_TURN_METRIC as u16), content)
            .custom_created_at(nostr::Timestamp::from(created_at))
            .sign_with_keys(agent)
            .expect("event should sign")
    }

    #[test]
    fn a_real_metric_event_round_trips_into_a_charge() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let p = payload(Some(0.42), Some(CHANNEL), "2026-08-07T12:00:00Z");
        let ev = metric_event(&agent, &owner, &p);

        let charge = charge_from_metric(&owner, &ev).expect("should ingest");
        assert_eq!(charge.channel_id, Uuid::parse_str(CHANNEL).unwrap());
        assert_eq!(charge.agent_pubkey, agent.public_key().to_hex());
        assert_eq!(charge.cost_usd, Some(0.42));
        assert_eq!(charge.turn_end.to_rfc3339(), "2026-08-07T12:00:00+00:00");
        assert!(charge.delta_reliable);
    }

    /// A subscription stream carries other kinds; those must be rejected
    /// cleanly rather than panicking.
    #[test]
    fn an_unrelated_kind_is_rejected_without_decrypting() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let ev = EventBuilder::new(Kind::Custom(9), "hello")
            .sign_with_keys(&agent)
            .expect("event should sign");
        assert!(matches!(
            charge_from_metric(&owner, &ev),
            Err(IngestError::WrongKind(9))
        ));
    }

    /// The metric is addressed to one owner; anyone else must fail closed.
    #[test]
    fn a_different_owner_key_cannot_read_the_metric() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let stranger = Keys::generate();
        let ev = metric_event(
            &agent,
            &owner,
            &payload(Some(1.0), Some(CHANNEL), "2026-08-07T12:00:00Z"),
        );
        assert!(matches!(
            charge_from_metric(&stranger, &ev),
            Err(IngestError::Payload(_))
        ));
    }

    #[test]
    fn a_missing_channel_id_is_an_error_not_a_silent_default() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let ev = metric_event(
            &agent,
            &owner,
            &payload(Some(1.0), None, "2026-08-07T12:00:00Z"),
        );
        assert!(matches!(
            charge_from_metric(&owner, &ev),
            Err(IngestError::MissingChannelId)
        ));
    }

    #[test]
    fn a_non_uuid_channel_id_is_rejected() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let ev = metric_event(
            &agent,
            &owner,
            &payload(Some(1.0), Some("not-a-uuid"), "2026-08-07T12:00:00Z"),
        );
        assert!(matches!(
            charge_from_metric(&owner, &ev),
            Err(IngestError::BadChannelId(_))
        ));
    }

    #[test]
    fn a_bad_timestamp_is_rejected() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let ev = metric_event(
            &agent,
            &owner,
            &payload(Some(1.0), Some(CHANNEL), "yesterday"),
        );
        assert!(matches!(
            charge_from_metric(&owner, &ev),
            Err(IngestError::BadTimestamp(_))
        ));
    }

    /// Security regression: the payload timestamp is chosen by the agent being
    /// budgeted. A far-future one used to move the ledger's eviction cutoff
    /// forward and wipe the pair's whole window, letting a runaway zero its own
    /// budget. It must be rejected against the signed `created_at`.
    #[test]
    fn a_far_future_payload_timestamp_is_rejected() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        // created_at says 2023; the payload claims 2099.
        let ev = metric_event_at(
            &agent,
            &owner,
            &payload(Some(1.0), Some(CHANNEL), "2099-01-01T00:00:00Z"),
            1_700_000_000,
        );
        assert!(matches!(
            charge_from_metric(&owner, &ev),
            Err(IngestError::TimestampSkew { .. })
        ));
    }

    /// A far-past timestamp is equally forged, and equally rejected.
    #[test]
    fn a_far_past_payload_timestamp_is_rejected() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let ev = metric_event_at(
            &agent,
            &owner,
            &payload(Some(1.0), Some(CHANNEL), "1999-01-01T00:00:00Z"),
            1_700_000_000,
        );
        assert!(matches!(
            charge_from_metric(&owner, &ev),
            Err(IngestError::TimestampSkew { .. })
        ));
    }

    /// Ordinary clock jitter between the harness and the signer must not
    /// reject a legitimate metric.
    #[test]
    fn small_clock_skew_is_tolerated() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let p = payload(Some(0.5), Some(CHANNEL), "2023-11-14T22:13:20Z");
        let created = DateTime::parse_from_rfc3339(&p.timestamp)
            .unwrap()
            .timestamp() as u64;
        let ev = metric_event_at(&agent, &owner, &p, created + 60);
        assert!(charge_from_metric(&owner, &ev).is_ok());
    }

    /// OQ7.2 — a harness that reports no cost is ingestable, and invisible to
    /// the budget. Asserted so the hole cannot close silently unnoticed.
    #[test]
    fn a_metric_with_no_cost_ingests_as_none() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let ev = metric_event(
            &agent,
            &owner,
            &payload(None, Some(CHANNEL), "2026-08-07T12:00:00Z"),
        );
        let charge = charge_from_metric(&owner, &ev).expect("should ingest");
        assert_eq!(charge.cost_usd, None);
    }
}
