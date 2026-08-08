//! Drive the budget at its real runtime surface and show the sawtooth.
//!
//! Decision D4 of wayfinder ticket #7 justifies a self-healing window by the
//! signature it leaves in the logs — "a runaway stops within minutes and
//! restarts only to stop again, producing a **sawtooth in the logs**, a
//! diagnosable signature rather than a silent drain."
//!
//! That claim is only true if something is actually emitted. This example
//! exists so it can be *observed* rather than asserted: it runs a simulated
//! two-agent runaway alongside an owner working in another channel, with a real
//! `tracing` subscriber attached, and prints what an operator would see.
//!
//! Run with:
//!   cargo run -p buzz-budget --example runaway

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use buzz_budget::{Supervisor, DEFAULT_BUDGET_USD, DEFAULT_WINDOW_SECS};

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_target(false)
        .without_time()
        .init();

    let runaway_channel = Uuid::from_u128(1);
    let human_channel = Uuid::from_u128(2);
    let t0: DateTime<Utc> =
        DateTime::from_timestamp(1_700_000_000, 0).expect("valid fixed timestamp");

    let mut sup = Supervisor::new(DEFAULT_BUDGET_USD, DEFAULT_WINDOW_SECS);

    println!("--- two agents talking to each other, nobody watching ---");
    let mut tripped = 0usize;
    for i in 0..30i64 {
        let at = t0 + Duration::seconds(i * 2);
        let (speaker, listener) = if i % 2 == 0 {
            ("otto", "eva")
        } else {
            ("eva", "otto")
        };
        sup.observe_message(runaway_channel, speaker, false, at);
        let v = sup.on_turn_completed(
            runaway_channel,
            listener,
            Some(0.60),
            at + Duration::seconds(1),
        );
        if v.is_exhausted() {
            tripped += 1;
        }
    }
    println!("=> exhausted verdicts in the first window: {tripped}");

    println!();
    println!("--- the same agents, one hour later: the window has slid ---");
    let later = t0 + Duration::seconds(DEFAULT_WINDOW_SECS + 60);
    sup.observe_message(runaway_channel, "otto", false, later);
    let v = sup.on_turn_completed(
        runaway_channel,
        "eva",
        Some(0.60),
        later + Duration::seconds(1),
    );
    println!("=> after the window slid: {v:?}");

    println!();
    println!("--- meanwhile, the owner working hard in another channel ---");
    let mut blocked = 0usize;
    for i in 0..40i64 {
        let at = t0 + Duration::seconds(i * 2);
        sup.observe_message(human_channel, "owner", true, at);
        let v = sup.on_turn_completed(human_channel, "eva", Some(2.50), at + Duration::seconds(1));
        if v.is_exhausted() {
            blocked += 1;
        }
    }
    println!("=> owner turns blocked (must be 0): {blocked}");
    println!(
        "=> owner-channel spend recorded (must be 0.00): {:.2}",
        sup.spent(human_channel, "eva", "owner", t0 + Duration::seconds(200))
    );
}
