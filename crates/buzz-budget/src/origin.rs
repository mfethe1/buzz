//! Attributing a completed turn to whatever triggered it.
//!
//! Decision **D5** of wayfinder ticket #7 says only agent-triggered turns
//! consume budget. The kind 44200 turn metric does not carry that information:
//! [`AgentTurnMetricPayload`] has `harness`, `model`, `channel_id`,
//! `session_id`, `turn_id`, `turn_seq`, `timestamp`, `turn`, `cumulative`,
//! `delta_reliable` and `stop_reason` — and no field naming the author that
//! caused the turn.
//!
//! This module recovers it without changing the wire format, using a property
//! the harness already guarantees: **turns are serialised per channel.**
//! `buzz-acp`'s queue keeps one in-flight prompt per channel and drains *all*
//! pending events for that channel into a single batch
//! (`crates/buzz-acp/src/queue.rs:1-7`). So the messages observed in a channel
//! since that channel's previous turn are exactly the batch that triggered the
//! next one.
//!
//! # Attribution rule
//!
//! Given the un-consumed messages in a channel up to the turn's end time,
//! ignoring the agent's own:
//!
//! - **any message from the owner → [`TurnOrigin::Human`]** (unbudgeted).
//!   A batch mixing human and agent messages counts as human because D5 exists
//!   to protect the case where a person is present and watching. Charging that
//!   turn risks muting an agent mid-conversation with its owner, which is the
//!   failure the policy is meant to avoid. Erring the other way only means a
//!   runaway costs one extra turn before it trips.
//! - **otherwise, the most recent agent message → [`TurnOrigin::Agent`]**.
//! - **nothing observed → `None`.** Heartbeat turns and turns whose trigger was
//!   missed are *not charged*. See the caveat below.
//!
//! # Caveat — the boundary is end-of-turn, not start-of-turn
//!
//! The metric's `timestamp` is *end-of-turn*
//! (`crates/buzz-core/src/agent_turn_metric.rs:107`) and the payload carries no
//! start time. So a message that arrives **while a turn is running** falls
//! inside that turn's window, even though it actually triggered the *next* one.
//!
//! The consequence is concrete: one owner message landing mid-turn attributes
//! that turn to `Human` (free), and the turn it really triggered then finds
//! nothing new and returns `None` (also free). **One human message can free two
//! turns.** Per-agent cursors do not fix this — the boundary is wrong, not the
//! bookkeeping.
//!
//! It errs toward under-charging, which is the same direction as every other
//! approximation here, so it is safe rather than merely tolerable. The durable
//! fix is a turn-start (or trigger) field on NIP-AM — a second argument for
//! ticket #7 OQ7.5.
//!
//! # Caveat — this is best-effort, and it fails open
//!
//! A supervisor that starts mid-conversation, drops a relay subscription, or
//! cannot read a private channel will observe no trigger and return `None`,
//! which is not charged. **An observation gap therefore disables the budget
//! rather than tripping it.** That is the right default for an accident-shaped
//! threat model, but it means coverage of the message stream is a correctness
//! requirement, not a nice-to-have.
//!
//! The durable fix is a `trigger` field on NIP-AM — the spec already says
//! *"Consumers MUST ignore unknown fields (forward compatibility)"*
//! (`crates/buzz-core/src/agent_turn_metric.rs:86`), so adding one is
//! backward-compatible. That needs a `buzz-acp` change; see ticket #7 OQ7.5.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use crate::TurnOrigin;

/// One observed inbound message in a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    at: DateTime<Utc>,
    author: String,
    is_owner: bool,
}

/// Records inbound messages per channel so a completed turn can be attributed.
///
/// Feed it every kind:9 the supervisor sees, then ask [`Self::origin_for_turn`]
/// when a kind 44200 metric arrives.
#[derive(Debug, Default)]
pub struct TriggerLog {
    seen: HashMap<Uuid, Vec<Observed>>,
    /// Per-(channel, agent) high-water mark of what that agent has already
    /// been attributed. Exclusive lower bound on the next lookup.
    cursors: HashMap<(Uuid, String), DateTime<Utc>>,
}

impl TriggerLog {
    /// Create an empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an inbound message. `is_owner` distinguishes the human from a
    /// sibling agent — `buzz-acp` already computes exactly this distinction in
    /// `is_owner_or_sibling` (`crates/buzz-acp/src/lib.rs:192-216`).
    pub fn observe(&mut self, channel_id: Uuid, author: &str, is_owner: bool, at: DateTime<Utc>) {
        self.seen.entry(channel_id).or_default().push(Observed {
            at,
            author: author.to_string(),
            is_owner,
        });
    }

    /// Attribute a turn that ended at `turn_end`.
    ///
    /// Consumption is tracked **per (channel, agent)**, not per channel. Each
    /// agent carries its own cursor, so two agents sharing a channel each see
    /// the batch that triggered them. Draining per channel — as this did
    /// originally — meant whichever metric the supervisor happened to process
    /// first ate the other agent's trigger, leaving the second turn attributed
    /// to `None` and therefore uncharged. Relay reordering alone was enough to
    /// silently disable the budget.
    ///
    /// Returns `None` when nothing was observed — a heartbeat turn, or an
    /// observation gap. `None` is not charged.
    pub fn origin_for_turn(
        &mut self,
        channel_id: Uuid,
        agent_pubkey: &str,
        turn_end: DateTime<Utc>,
    ) -> Option<TurnOrigin> {
        let entries = self.seen.get(&channel_id)?;
        let cursor = self
            .cursors
            .get(&(channel_id, agent_pubkey.to_string()))
            .copied();

        // Candidates are the messages this agent has not already accounted for,
        // up to the end of the turn. `cursor` is exclusive so a message cannot
        // trigger the same agent twice.
        let triggers: Vec<&Observed> = entries
            .iter()
            .filter(|o| o.at <= turn_end)
            .filter(|o| cursor.is_none_or(|c| o.at > c))
            .filter(|o| o.author != agent_pubkey)
            .collect();

        // Advance this agent's cursor whether or not anything was found, so a
        // turn that saw nothing does not re-examine the same window forever.
        self.cursors
            .insert((channel_id, agent_pubkey.to_string()), turn_end);

        if triggers.is_empty() {
            return None;
        }
        // A mixed batch counts as human — see the module docs for why.
        if triggers.iter().any(|o| o.is_owner) {
            return Some(TurnOrigin::Human);
        }
        triggers
            .iter()
            .max_by_key(|o| o.at)
            .map(|o| TurnOrigin::Agent(o.author.clone()))
    }

    /// Drop observations older than `max_age`, so a channel that goes quiet
    /// with un-consumed messages cannot grow without bound.
    pub fn prune(&mut self, now: DateTime<Utc>, max_age: Duration) {
        let cutoff = now - max_age;
        self.seen.retain(|_, v| {
            v.retain(|o| o.at > cutoff);
            !v.is_empty()
        });
        // Cursors for channels with nothing left to attribute are dead weight.
        self.cursors
            .retain(|(ch, _), at| *at > cutoff && self.seen.contains_key(ch));
    }

    /// Number of channels currently holding un-consumed observations.
    pub fn tracked_channels(&self) -> usize {
        self.seen.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch() -> Uuid {
        Uuid::nil()
    }
    fn t0() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
    }

    const AGENT: &str = "agent";
    const PEER: &str = "peer";
    const OWNER: &str = "owner";

    #[test]
    fn a_turn_triggered_by_another_agent_is_attributed_to_that_agent() {
        let mut log = TriggerLog::new();
        log.observe(ch(), PEER, false, t0());
        assert_eq!(
            log.origin_for_turn(ch(), AGENT, t0() + Duration::seconds(5)),
            Some(TurnOrigin::Agent(PEER.into()))
        );
    }

    #[test]
    fn a_turn_triggered_by_the_owner_is_human() {
        let mut log = TriggerLog::new();
        log.observe(ch(), OWNER, true, t0());
        assert_eq!(
            log.origin_for_turn(ch(), AGENT, t0() + Duration::seconds(5)),
            Some(TurnOrigin::Human)
        );
    }

    /// The safety-critical case: a batch containing the human must not be
    /// charged, or an agent can be muted mid-conversation with its owner.
    #[test]
    fn a_mixed_batch_counts_as_human() {
        let mut log = TriggerLog::new();
        log.observe(ch(), PEER, false, t0());
        log.observe(ch(), OWNER, true, t0() + Duration::seconds(1));
        assert_eq!(
            log.origin_for_turn(ch(), AGENT, t0() + Duration::seconds(5)),
            Some(TurnOrigin::Human)
        );
    }

    #[test]
    fn the_agents_own_messages_do_not_trigger_it() {
        let mut log = TriggerLog::new();
        log.observe(ch(), AGENT, false, t0());
        assert_eq!(
            log.origin_for_turn(ch(), AGENT, t0() + Duration::seconds(5)),
            None
        );
    }

    #[test]
    fn nothing_observed_is_unattributed_and_therefore_uncharged() {
        let mut log = TriggerLog::new();
        assert_eq!(log.origin_for_turn(ch(), AGENT, t0()), None);
    }

    /// Per-channel serialisation is what makes this correlator sound; a turn
    /// must not consume messages that arrived after it ended.
    #[test]
    fn messages_after_the_turn_end_belong_to_the_next_turn() {
        let mut log = TriggerLog::new();
        log.observe(ch(), PEER, false, t0());
        log.observe(ch(), OWNER, true, t0() + Duration::seconds(10));

        assert_eq!(
            log.origin_for_turn(ch(), AGENT, t0() + Duration::seconds(5)),
            Some(TurnOrigin::Agent(PEER.into()))
        );
        assert_eq!(
            log.origin_for_turn(ch(), AGENT, t0() + Duration::seconds(20)),
            Some(TurnOrigin::Human)
        );
    }

    #[test]
    fn a_batch_is_consumed_so_it_cannot_be_counted_twice() {
        let mut log = TriggerLog::new();
        log.observe(ch(), PEER, false, t0());
        assert!(log
            .origin_for_turn(ch(), AGENT, t0() + Duration::seconds(5))
            .is_some());
        assert_eq!(
            log.origin_for_turn(ch(), AGENT, t0() + Duration::seconds(6)),
            None,
            "the same message must not trigger two turns"
        );
    }

    /// Regression: consumption used to be per-channel, so with two agents in
    /// one channel whichever metric arrived first ate the other's trigger and
    /// the second turn went uncharged. Relay reordering alone disabled the
    /// budget.
    #[test]
    fn two_agents_in_one_channel_each_see_their_own_trigger() {
        let mut log = TriggerLog::new();
        // Iris mentions both agents in one message.
        log.observe(ch(), "iris", false, t0());

        assert_eq!(
            log.origin_for_turn(ch(), AGENT, t0() + Duration::seconds(5)),
            Some(TurnOrigin::Agent("iris".into()))
        );
        assert_eq!(
            log.origin_for_turn(ch(), "otto", t0() + Duration::seconds(6)),
            Some(TurnOrigin::Agent("iris".into())),
            "the second agent must still see the trigger that woke it"
        );
    }

    /// Each agent's cursor advances independently.
    #[test]
    fn one_agents_cursor_does_not_advance_another() {
        let mut log = TriggerLog::new();
        log.observe(ch(), PEER, false, t0());
        assert!(log
            .origin_for_turn(ch(), AGENT, t0() + Duration::seconds(5))
            .is_some());
        // AGENT is now caught up, but a later message must reach it again.
        log.observe(ch(), PEER, false, t0() + Duration::seconds(10));
        assert_eq!(
            log.origin_for_turn(ch(), AGENT, t0() + Duration::seconds(15)),
            Some(TurnOrigin::Agent(PEER.into()))
        );
    }

    #[test]
    fn channels_are_independent() {
        let mut log = TriggerLog::new();
        let other = Uuid::from_u128(1);
        log.observe(ch(), PEER, false, t0());
        assert_eq!(log.origin_for_turn(other, AGENT, t0()), None);
        assert!(log.origin_for_turn(ch(), AGENT, t0()).is_some());
    }

    #[test]
    fn prune_drops_stale_unconsumed_observations() {
        let mut log = TriggerLog::new();
        log.observe(ch(), PEER, false, t0());
        assert_eq!(log.tracked_channels(), 1);
        log.prune(t0() + Duration::seconds(7200), Duration::seconds(3600));
        assert_eq!(log.tracked_channels(), 0);
    }

    #[test]
    fn the_most_recent_agent_wins_when_several_peers_spoke() {
        let mut log = TriggerLog::new();
        log.observe(ch(), "early", false, t0());
        log.observe(ch(), "late", false, t0() + Duration::seconds(2));
        assert_eq!(
            log.origin_for_turn(ch(), AGENT, t0() + Duration::seconds(5)),
            Some(TurnOrigin::Agent("late".into()))
        );
    }
}
