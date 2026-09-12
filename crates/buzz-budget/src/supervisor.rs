//! Composes attribution and accounting into the API a caller actually wants.
//!
//! [`Ledger`] knows how to charge a pair. [`TriggerLog`] knows who triggered a
//! turn. Neither is useful alone: a caller holding both has to remember to
//! attribute before charging, and to prune both on the same schedule. This type
//! owns that sequencing so a caller cannot get it wrong.
//!
//! ```no_run
//! use buzz_budget::{Supervisor, DEFAULT_BUDGET_USD, DEFAULT_WINDOW_SECS};
//! # use chrono::Utc; use uuid::Uuid;
//! let mut sup = Supervisor::new(DEFAULT_BUDGET_USD, DEFAULT_WINDOW_SECS);
//!
//! // Feed every kind:9 the supervisor sees.
//! sup.observe_message(Uuid::nil(), "peer-pubkey", false, Utc::now());
//!
//! // Feed every kind:44200 turn metric.
//! let verdict = sup.on_turn_completed(Uuid::nil(), "agent-pubkey", Some(0.25), Utc::now());
//! if verdict.is_exhausted() {
//!     // Enforcement is the caller's job — see the crate docs.
//! }
//! ```
//!
//! **Still does not enforce.** The verdict comes back; acting on it is the
//! caller's decision, for the reasons in the crate-level docs.

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use crate::{Ledger, TriggerLog, TurnCharge, Verdict};

/// Owns a [`Ledger`] and a [`TriggerLog`] and keeps them in step.
#[derive(Debug)]
pub struct Supervisor {
    ledger: Ledger,
    triggers: TriggerLog,
    window: Duration,
}

impl Supervisor {
    /// Create a supervisor. See [`crate::DEFAULT_BUDGET_USD`] and
    /// [`crate::DEFAULT_WINDOW_SECS`] for the values decision D3 settled on.
    pub fn new(budget_usd: f64, window_secs: i64) -> Self {
        Self {
            ledger: Ledger::new(budget_usd, window_secs),
            triggers: TriggerLog::new(),
            window: Duration::seconds(window_secs.max(1)),
        }
    }

    /// Record an inbound message so a later turn can be attributed to it.
    pub fn observe_message(
        &mut self,
        channel_id: Uuid,
        author: &str,
        is_owner: bool,
        at: DateTime<Utc>,
    ) {
        self.triggers.observe(channel_id, author, is_owner, at);
    }

    /// Attribute and charge one completed turn.
    ///
    /// A turn with no observable trigger — a heartbeat, or an observation gap —
    /// is **not charged**, and comes back as [`Verdict::Unbudgeted`]. See
    /// [`crate::origin`] for why that fails open rather than closed.
    ///
    /// Emits the trace events decision **D4** relies on: exhaustion at `warn`,
    /// ordinary charges at `debug`. D4 justifies a self-healing window by the
    /// **sawtooth it leaves in the logs** — "a diagnosable signature rather than
    /// a silent drain" — which only exists if something is emitted.
    pub fn on_turn_completed(
        &mut self,
        channel_id: Uuid,
        agent_pubkey: &str,
        cost_usd: Option<f64>,
        turn_end: DateTime<Utc>,
    ) -> Verdict {
        let Some(origin) = self
            .triggers
            .origin_for_turn(channel_id, agent_pubkey, turn_end)
        else {
            return Verdict::Unbudgeted;
        };

        let verdict = self
            .ledger
            .record(channel_id, agent_pubkey, &origin, cost_usd, turn_end);

        match &verdict {
            Verdict::Exhausted {
                spent_usd,
                budget_usd,
            } => tracing::warn!(
                %channel_id,
                agent = %agent_pubkey,
                spent_usd,
                budget_usd,
                "agent-pair budget exhausted; exchange should be stopped"
            ),
            Verdict::Allow { spent_usd } => tracing::debug!(
                %channel_id,
                agent = %agent_pubkey,
                spent_usd,
                "agent-triggered turn charged"
            ),
            Verdict::Unbudgeted => {}
        }
        verdict
    }

    /// Charge a turn decoded straight off the wire by
    /// [`crate::charge_from_metric`].
    ///
    /// Prefer this over [`Self::on_turn_completed`] when the input came from a
    /// `kind:44200` event: it is the only path that inspects
    /// [`TurnCharge::delta_reliable`], which the metric sets to `false` when the
    /// publisher lost its cumulative baseline and the per-turn cost is
    /// therefore untrustworthy.
    ///
    /// The charge is still applied — ticket #7 **OQ7.1** has not decided a
    /// fallback (its suggestion is "prefer `cumulative` where present", which
    /// this type does not yet carry). It is logged at `warn` so the question can
    /// be answered from real data instead of guessed at.
    pub fn on_turn_charge(&mut self, charge: &TurnCharge) -> Verdict {
        if !charge.delta_reliable {
            tracing::warn!(
                channel_id = %charge.channel_id,
                agent = %charge.agent_pubkey,
                cost_usd = ?charge.cost_usd,
                "turn metric reported delta_reliable=false; charging it anyway (see #7 OQ7.1)"
            );
        }
        self.on_turn_completed(
            charge.channel_id,
            &charge.agent_pubkey,
            charge.cost_usd,
            charge.turn_end,
        )
    }

    /// Spend for a pair across the current window.
    pub fn spent(&self, channel_id: Uuid, a: &str, b: &str, now: DateTime<Utc>) -> f64 {
        self.ledger.spent(channel_id, a, b, now)
    }

    /// Drop aged-out state from both halves. A long-running supervisor should
    /// call this periodically; nothing else evicts channels that go quiet.
    pub fn maintain(&mut self, now: DateTime<Utc>) {
        self.ledger.evict_expired(now);
        self.triggers.prune(now, self.window);
    }

    /// Pairs currently holding charges — for diagnostics.
    pub fn tracked_pairs(&self) -> usize {
        self.ledger.tracked_pairs()
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

    const EVA: &str = "eva";
    const OTTO: &str = "otto";
    const OWNER: &str = "owner";

    /// The whole point, end to end: two agents talking to each other with
    /// nobody watching eventually get cut off.
    #[test]
    fn a_two_agent_runaway_is_eventually_stopped() {
        let mut sup = Supervisor::new(5.0, 3600);
        let mut stopped_at = None;

        // Eva and Otto alternate, each turn costing $0.30.
        for i in 0..40i64 {
            let at = t0() + Duration::seconds(i * 2);
            let (speaker, listener) = if i % 2 == 0 { (OTTO, EVA) } else { (EVA, OTTO) };
            // The peer's message arrives, then the listener's turn completes.
            sup.observe_message(ch(), speaker, false, at);
            let v = sup.on_turn_completed(ch(), listener, Some(0.30), at + Duration::seconds(1));
            if v.is_exhausted() {
                stopped_at = Some(i);
                break;
            }
        }

        let i = stopped_at.expect("a runaway must eventually be stopped");
        // Every iteration charges exactly one turn, so the loop index is the
        // charged-turn count minus one. $5 / $0.30 = 16.67, so cumulative first
        // exceeds the budget on the 17th charged turn — index 16.
        assert_eq!(
            i, 16,
            "expected to trip on the 17th charged turn (index 16), tripped at index {i}"
        );
    }

    /// The failure this policy must never cause: an agent muted while its
    /// owner is actively working with it.
    #[test]
    fn an_intensive_human_session_is_never_cut_off() {
        let mut sup = Supervisor::new(5.0, 3600);
        for i in 0..200i64 {
            let at = t0() + Duration::seconds(i * 2);
            sup.observe_message(ch(), OWNER, true, at);
            let v = sup.on_turn_completed(ch(), EVA, Some(1.50), at + Duration::seconds(1));
            assert!(
                !v.is_exhausted(),
                "human-driven turn {i} must never be blocked"
            );
        }
        assert_eq!(
            sup.spent(ch(), EVA, OWNER, t0() + Duration::seconds(400)),
            0.0
        );
    }

    /// A human joining a runaway rescues it: the mixed batch attributes to the
    /// human, so that turn is free.
    #[test]
    fn a_human_joining_the_channel_makes_that_turn_free() {
        let mut sup = Supervisor::new(5.0, 3600);

        sup.observe_message(ch(), OTTO, false, t0());
        sup.on_turn_completed(ch(), EVA, Some(2.0), t0() + Duration::seconds(1));
        assert_eq!(sup.spent(ch(), EVA, OTTO, t0() + Duration::seconds(2)), 2.0);

        // Otto speaks again, but so does the owner, before Eva's next turn.
        sup.observe_message(ch(), OTTO, false, t0() + Duration::seconds(2));
        sup.observe_message(ch(), OWNER, true, t0() + Duration::seconds(3));
        sup.on_turn_completed(ch(), EVA, Some(9.0), t0() + Duration::seconds(4));

        assert_eq!(
            sup.spent(ch(), EVA, OTTO, t0() + Duration::seconds(5)),
            2.0,
            "a batch containing the owner must not be charged"
        );
    }

    #[test]
    fn a_runaway_recovers_after_the_window_slides() {
        let mut sup = Supervisor::new(5.0, 3600);
        sup.observe_message(ch(), OTTO, false, t0());
        let v = sup.on_turn_completed(ch(), EVA, Some(6.0), t0() + Duration::seconds(1));
        assert!(v.is_exhausted());

        let later = t0() + Duration::seconds(7200);
        sup.observe_message(ch(), OTTO, false, later);
        let v = sup.on_turn_completed(ch(), EVA, Some(0.10), later + Duration::seconds(1));
        assert!(!v.is_exhausted(), "must self-heal with no reset");
    }

    /// An observation gap fails open — documented, and asserted so a future
    /// change cannot flip it silently.
    #[test]
    fn an_unobserved_turn_is_not_charged() {
        let mut sup = Supervisor::new(5.0, 3600);
        // No observe_message call at all.
        for i in 0..50i64 {
            let v = sup.on_turn_completed(ch(), EVA, Some(10.0), t0() + Duration::seconds(i));
            assert!(!v.is_exhausted(), "unattributed turns must fail open");
        }
        assert_eq!(sup.tracked_pairs(), 0);
    }

    /// `on_turn_charge` must be equivalent to the plain path — the
    /// `delta_reliable` inspection is additive, not a different policy.
    #[test]
    fn charging_from_a_wire_decoded_turn_matches_the_plain_path() {
        use crate::TurnCharge;

        let mut sup = Supervisor::new(5.0, 3600);
        sup.observe_message(ch(), OTTO, false, t0());
        let v = sup.on_turn_charge(&TurnCharge {
            channel_id: ch(),
            agent_pubkey: EVA.into(),
            cost_usd: Some(6.0),
            turn_end: t0() + Duration::seconds(1),
            delta_reliable: true,
        });
        assert!(v.is_exhausted());
        assert_eq!(sup.spent(ch(), EVA, OTTO, t0() + Duration::seconds(2)), 6.0);
    }

    /// An unreliable delta is still charged — OQ7.1 is unresolved, so the
    /// behaviour must be explicit and tested rather than accidental.
    #[test]
    fn an_unreliable_delta_is_still_charged() {
        use crate::TurnCharge;

        let mut sup = Supervisor::new(5.0, 3600);
        sup.observe_message(ch(), OTTO, false, t0());
        sup.on_turn_charge(&TurnCharge {
            channel_id: ch(),
            agent_pubkey: EVA.into(),
            cost_usd: Some(2.5),
            turn_end: t0() + Duration::seconds(1),
            delta_reliable: false,
        });
        assert_eq!(sup.spent(ch(), EVA, OTTO, t0() + Duration::seconds(2)), 2.5);
    }

    #[test]
    fn maintain_clears_state_for_channels_that_went_quiet() {
        let mut sup = Supervisor::new(5.0, 3600);
        sup.observe_message(ch(), OTTO, false, t0());
        sup.on_turn_completed(ch(), EVA, Some(1.0), t0() + Duration::seconds(1));
        assert_eq!(sup.tracked_pairs(), 1);

        sup.maintain(t0() + Duration::seconds(7200));
        assert_eq!(sup.tracked_pairs(), 0);
    }

    /// Two separate agent pairs in one channel must not pool their budgets.
    #[test]
    fn separate_pairs_have_separate_budgets() {
        let mut sup = Supervisor::new(5.0, 3600);
        let third = "iris";

        sup.observe_message(ch(), OTTO, false, t0());
        let v = sup.on_turn_completed(ch(), EVA, Some(6.0), t0() + Duration::seconds(1));
        assert!(v.is_exhausted(), "eva/otto is over budget");

        sup.observe_message(ch(), third, false, t0() + Duration::seconds(2));
        let v = sup.on_turn_completed(ch(), EVA, Some(0.5), t0() + Duration::seconds(3));
        assert!(
            !v.is_exhausted(),
            "eva/iris must have its own budget; got {v:?}"
        );
    }
}
