//! Sliding-window cost accounting for agent-to-agent exchanges.
//!
//! Implements the accounting half of the loop-protection policy decided in
//! wayfinder ticket #7 (`mfethe1/agent-mesh`):
//!
//! - **D2** — budget measured `cost_usd`, not message count. Buzz already emits
//!   `cost_usd` per completed turn on kind 44200 (NIP-AM).
//! - **D3** — $5 per (channel, agent-pair) per rolling hour.
//! - **D4** — self-healing: the window slides, no human reset.
//! - **D5** — only agent-triggered turns count. Human-triggered turns are
//!   unbudgeted, because the human is present and can stop it themselves.
//!
//! **This crate deliberately does not enforce.** [`Ledger::record`] returns a
//! [`Verdict`]; acting on `Verdict::Exhausted` is the caller's job. That seam
//! exists because the enforcement mechanism is still an open question — Buzz's
//! owner commands (`!shutdown`, `!cancel`, `!rotate`) cannot mute a single peer,
//! so enforcement needs either a new owner command or a supervisor with
//! process-level control. Keeping accounting pure means that decision can land
//! later without touching this code.
//!
//! Why cost rather than message count: a productive exchange of 200 short
//! messages costs pennies and passes; 30 huge-context turns burn the budget and
//! stop. Message count would penalise the first and permit the second.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

/// Default budget from decision D3.
pub const DEFAULT_BUDGET_USD: f64 = 5.0;

/// Default rolling window from decision D3.
pub const DEFAULT_WINDOW_SECS: i64 = 3600;

/// What caused an agent to take a turn.
///
/// Per **D5** only [`TurnOrigin::Agent`] turns consume budget. A turn the human
/// asked for is never charged: the human is watching, and cutting them off is
/// the failure this policy exists to avoid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOrigin {
    /// Triggered by the owner (a human). Never charged.
    Human,
    /// Triggered by another agent, identified by its pubkey hex.
    Agent(String),
}

/// The unordered pair of agents an exchange runs between, scoped to a channel.
///
/// Unordered because a runaway is a property of the *pair*, not of a direction:
/// A→B and B→A are the same exchange and must share one budget.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PairKey {
    pub channel_id: Uuid,
    lo: String,
    hi: String,
}

impl PairKey {
    /// Build a key from two agent pubkeys in either order.
    pub fn new(channel_id: Uuid, a: &str, b: &str) -> Self {
        let (lo, hi) = if a <= b {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        };
        Self { channel_id, lo, hi }
    }

    /// The two pubkeys, lexicographically ordered.
    pub fn members(&self) -> (&str, &str) {
        (&self.lo, &self.hi)
    }
}

/// The result of recording a turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Within budget. `spent` is the pair's total across the current window,
    /// inclusive of the turn just recorded.
    Allow { spent_usd: f64 },
    /// Budget exceeded for this pair in this channel. The caller decides what
    /// to do about it; this crate does not act.
    Exhausted { spent_usd: f64, budget_usd: f64 },
}

impl Verdict {
    /// True when the exchange should be stopped.
    pub fn is_exhausted(&self) -> bool {
        matches!(self, Verdict::Exhausted { .. })
    }
}

#[derive(Debug, Clone, Copy)]
struct Charge {
    at: DateTime<Utc>,
    cost_usd: f64,
}

/// Sliding-window cost ledger.
///
/// Holds one window of charges per [`PairKey`]. Charges older than the window
/// are dropped whenever that pair is touched, which is what makes the budget
/// self-healing (**D4**) with no reset step.
#[derive(Debug)]
pub struct Ledger {
    budget_usd: f64,
    window: Duration,
    charges: HashMap<PairKey, Vec<Charge>>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new(DEFAULT_BUDGET_USD, DEFAULT_WINDOW_SECS)
    }
}

impl Ledger {
    /// Create a ledger. `window_secs` must be positive; non-positive values are
    /// clamped to 1 second so a misconfiguration cannot disable the window and
    /// silently accumulate forever.
    pub fn new(budget_usd: f64, window_secs: i64) -> Self {
        Self {
            budget_usd,
            window: Duration::seconds(window_secs.max(1)),
            charges: HashMap::new(),
        }
    }

    /// Record one completed turn and return whether the exchange may continue.
    ///
    /// `cost_usd` comes from the kind 44200 turn metric. A `None` cost — a
    /// harness that does not report spend — is charged as zero and logged by the
    /// caller's concern, not silently treated as over budget; see the crate docs
    /// and ticket #7 OQ7.2.
    pub fn record(
        &mut self,
        channel_id: Uuid,
        agent_pubkey: &str,
        origin: &TurnOrigin,
        cost_usd: Option<f64>,
        at: DateTime<Utc>,
    ) -> Verdict {
        // D5: human-triggered turns are never charged and never blocked.
        let peer = match origin {
            TurnOrigin::Human => return Verdict::Allow { spent_usd: 0.0 },
            TurnOrigin::Agent(peer) => peer,
        };

        let key = PairKey::new(channel_id, agent_pubkey, peer);
        let cutoff = at - self.window;
        let entry = self.charges.entry(key).or_default();
        entry.retain(|c| c.at > cutoff);
        entry.push(Charge {
            at,
            cost_usd: cost_usd.unwrap_or(0.0).max(0.0),
        });

        let spent: f64 = entry.iter().map(|c| c.cost_usd).sum();
        if spent > self.budget_usd {
            Verdict::Exhausted {
                spent_usd: spent,
                budget_usd: self.budget_usd,
            }
        } else {
            Verdict::Allow { spent_usd: spent }
        }
    }

    /// Spend for a pair across the current window, without recording anything.
    pub fn spent(&self, channel_id: Uuid, a: &str, b: &str, now: DateTime<Utc>) -> f64 {
        let key = PairKey::new(channel_id, a, b);
        let cutoff = now - self.window;
        self.charges
            .get(&key)
            .map(|cs| {
                cs.iter()
                    .filter(|c| c.at > cutoff)
                    .map(|c| c.cost_usd)
                    .sum()
            })
            .unwrap_or(0.0)
    }

    /// Number of pairs currently holding charges. Charges are only evicted when
    /// their pair is touched, so this counts pairs seen, not pairs active.
    pub fn tracked_pairs(&self) -> usize {
        self.charges.len()
    }

    /// Drop every pair whose charges have all aged out. Callers running a
    /// long-lived supervisor should call this periodically; without it, a pair
    /// that stops talking retains an empty-but-allocated entry.
    pub fn evict_expired(&mut self, now: DateTime<Utc>) {
        let cutoff = now - self.window;
        self.charges.retain(|_, cs| {
            cs.retain(|c| c.at > cutoff);
            !cs.is_empty()
        });
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

    const A: &str = "aaaa";
    const B: &str = "bbbb";

    /// The runaway this whole policy exists to stop: two agents exchanging
    /// turns with nobody watching. Written first, per the map's Notes.
    #[test]
    fn runaway_agent_exchange_is_stopped_once_the_budget_is_spent() {
        let mut led = Ledger::new(5.0, 3600);
        let origin = TurnOrigin::Agent(B.into());

        // Ten turns at $0.40 = $4.00. Still under.
        for i in 0..10 {
            let v = led.record(ch(), A, &origin, Some(0.40), t0() + Duration::seconds(i));
            assert!(!v.is_exhausted(), "turn {i} should be allowed");
        }

        // Three more crosses $5.00.
        let mut stopped = false;
        for i in 10..14 {
            if led
                .record(ch(), A, &origin, Some(0.40), t0() + Duration::seconds(i))
                .is_exhausted()
            {
                stopped = true;
                break;
            }
        }
        assert!(
            stopped,
            "runaway must be stopped once the budget is exceeded"
        );
    }

    /// D5 — the case that would make this policy worse than nothing.
    #[test]
    fn human_triggered_turns_are_never_charged_or_blocked() {
        let mut led = Ledger::new(5.0, 3600);
        for i in 0..100 {
            let v = led.record(
                ch(),
                A,
                &TurnOrigin::Human,
                Some(1.0),
                t0() + Duration::seconds(i),
            );
            assert!(!v.is_exhausted(), "human turn {i} must never be blocked");
        }
        assert_eq!(led.spent(ch(), A, B, t0() + Duration::seconds(100)), 0.0);
    }

    /// D4 — self-healing, no human reset.
    #[test]
    fn budget_recovers_once_charges_slide_out_of_the_window() {
        let mut led = Ledger::new(5.0, 3600);
        let origin = TurnOrigin::Agent(B.into());

        let v = led.record(ch(), A, &origin, Some(6.0), t0());
        assert!(v.is_exhausted(), "one $6 turn exceeds a $5 budget");

        // Two hours later the charge has aged out.
        let later = t0() + Duration::seconds(7200);
        assert_eq!(led.spent(ch(), A, B, later), 0.0);
        let v = led.record(ch(), A, &origin, Some(0.10), later);
        assert!(!v.is_exhausted(), "budget must self-heal without a reset");
    }

    /// A runaway is a property of the pair, not a direction.
    #[test]
    fn the_pair_budget_is_shared_regardless_of_direction() {
        let mut led = Ledger::new(5.0, 3600);

        led.record(ch(), A, &TurnOrigin::Agent(B.into()), Some(3.0), t0());
        // B replying to A must draw on the same budget, not a fresh one.
        let v = led.record(
            ch(),
            B,
            &TurnOrigin::Agent(A.into()),
            Some(3.0),
            t0() + Duration::seconds(1),
        );
        assert!(
            v.is_exhausted(),
            "A→B and B→A must share one budget; got {v:?}"
        );
    }

    #[test]
    fn budgets_are_scoped_per_channel() {
        let mut led = Ledger::new(5.0, 3600);
        let other = Uuid::from_u128(1);
        let origin = TurnOrigin::Agent(B.into());

        assert!(led.record(ch(), A, &origin, Some(6.0), t0()).is_exhausted());
        // The same pair in a different channel is unaffected.
        assert!(!led
            .record(other, A, &origin, Some(0.5), t0())
            .is_exhausted());
    }

    /// OQ7.2 — a harness that reports no cost must not crash or be charged
    /// arbitrarily. It is charged zero, which is why #7 flags per-harness
    /// coverage as something to establish before relying on this.
    #[test]
    fn a_turn_with_no_reported_cost_is_charged_zero() {
        let mut led = Ledger::new(5.0, 3600);
        let origin = TurnOrigin::Agent(B.into());
        for i in 0..50 {
            assert!(!led
                .record(ch(), A, &origin, None, t0() + Duration::seconds(i))
                .is_exhausted());
        }
        assert_eq!(led.spent(ch(), A, B, t0() + Duration::seconds(50)), 0.0);
    }

    #[test]
    fn negative_costs_cannot_refund_the_budget() {
        let mut led = Ledger::new(5.0, 3600);
        let origin = TurnOrigin::Agent(B.into());
        led.record(ch(), A, &origin, Some(4.9), t0());
        led.record(ch(), A, &origin, Some(-100.0), t0() + Duration::seconds(1));
        let v = led.record(ch(), A, &origin, Some(0.2), t0() + Duration::seconds(2));
        assert!(
            v.is_exhausted(),
            "a negative cost must not buy back budget; got {v:?}"
        );
    }

    #[test]
    fn pair_key_is_order_independent() {
        assert_eq!(PairKey::new(ch(), A, B), PairKey::new(ch(), B, A));
        assert_eq!(PairKey::new(ch(), B, A).members(), (A, B));
    }

    #[test]
    fn evict_expired_drops_pairs_that_have_gone_quiet() {
        let mut led = Ledger::new(5.0, 3600);
        led.record(ch(), A, &TurnOrigin::Agent(B.into()), Some(1.0), t0());
        assert_eq!(led.tracked_pairs(), 1);
        led.evict_expired(t0() + Duration::seconds(7200));
        assert_eq!(led.tracked_pairs(), 0);
    }

    #[test]
    fn a_non_positive_window_is_clamped_rather_than_disabling_the_budget() {
        let led = Ledger::new(5.0, 0);
        assert_eq!(led.window, Duration::seconds(1));
    }
}
