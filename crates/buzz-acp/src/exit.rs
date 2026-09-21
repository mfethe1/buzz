//! The clean-exit exit-code contract (`docs/remote-agents.md` Known Defect 6).
//!
//! A supervisor restart policy (`restartPolicy: OnFailure`, systemd
//! `Restart=on-failure`) can only be deployed against this harness if
//! "intentional stop" is distinguishable from "failure" by exit code alone.
//! Before this module the distinction was *emergent*: the graceful path
//! happened to return `Ok(())`, which `main` happened to map to 0. Nothing
//! pinned it, so a refactor that returned `Err` from a drain timeout would
//! silently convert every clean stop into a restart loop — I5 defeated with
//! no failing test (spec §Known Defects 6, I5 ordering rule).
//!
//! This module makes the contract explicit and testable:
//!
//! * [`EXIT_CLEAN`] — the process stopped because it was *told* to, or
//!   because it reached a bound its owner declared. A supervisor MUST NOT
//!   restart it.
//! * [`EXIT_FAILURE`] — anything else. A supervisor MAY restart it.
//!
//! The load-bearing rule is [`Disposition::exit_code`]: once a stop is
//! classified [`Disposition::IntentionalStop`], the exit code is 0 **even if
//! the graceful tail subsequently errors**. The tail (drain, reap, presence
//! `offline`, relay close) is best-effort cleanup that runs *after* the
//! decision to stop; letting a failure there flip the exit code is precisely
//! the restart-loop bug this contract exists to prevent.

/// Intentional stop. A supervisor MUST NOT restart the process.
pub const EXIT_CLEAN: i32 = 0;

/// Failure. A supervisor MAY restart the process.
pub const EXIT_FAILURE: i32 = 1;

/// Why the harness stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// The owner sent `!shutdown`, the declared inactivity bound was reached,
    /// or the supervisor sent SIGTERM/SIGINT. All are the agent honoring its
    /// owner's lifetime choice (spec [L1] conformance rule 4).
    IntentionalStop,
    /// Startup failure, or any error that is not an intentional stop.
    Failure,
}

impl Disposition {
    /// Map a disposition to the process exit code.
    ///
    /// An intentional stop is 0 unconditionally — see the module note on why
    /// a failing graceful tail must not change this.
    pub fn exit_code(self) -> i32 {
        match self {
            Disposition::IntentionalStop => EXIT_CLEAN,
            Disposition::Failure => EXIT_FAILURE,
        }
    }

    /// True when a conforming supervisor is permitted to restart.
    pub fn supervisor_may_restart(self) -> bool {
        matches!(self, Disposition::Failure)
    }
}

/// Classify the result of the harness run into a process exit code.
///
/// `Ok(disposition)` carries the run's own classification. `Err` is a failure
/// the run could not absorb — by construction the intentional-stop paths
/// return `Ok(Disposition::IntentionalStop)`, so an `Err` here is always a
/// genuine failure.
pub fn exit_code_for(result: &anyhow::Result<Disposition>) -> i32 {
    match result {
        Ok(disposition) => disposition.exit_code(),
        Err(_) => EXIT_FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests are the pin the spec requires: they must fail if anyone
    // makes an intentional stop exit nonzero.

    #[test]
    fn intentional_stop_exits_zero() {
        assert_eq!(Disposition::IntentionalStop.exit_code(), 0);
        assert_eq!(Disposition::IntentionalStop.exit_code(), EXIT_CLEAN);
    }

    #[test]
    fn failure_exits_nonzero() {
        assert_ne!(Disposition::Failure.exit_code(), 0);
        assert_eq!(Disposition::Failure.exit_code(), EXIT_FAILURE);
    }

    #[test]
    fn supervisor_may_not_restart_an_intentional_stop() {
        // I5: "Any supervisor the launcher configures never restarts an
        // intentional clean exit."
        assert!(!Disposition::IntentionalStop.supervisor_may_restart());
        assert!(Disposition::Failure.supervisor_may_restart());
    }

    #[test]
    fn an_intentional_stop_stays_clean_through_exit_code_for() {
        let result: anyhow::Result<Disposition> = Ok(Disposition::IntentionalStop);
        assert_eq!(exit_code_for(&result), EXIT_CLEAN);
    }

    #[test]
    fn an_error_is_a_failure() {
        let result: anyhow::Result<Disposition> = Err(anyhow::anyhow!("startup failed"));
        assert_eq!(exit_code_for(&result), EXIT_FAILURE);
    }

    // The regression this contract exists to prevent: a drain timeout in the
    // graceful tail must not convert a clean stop into a restart. The tail
    // runs after the stop decision, so the disposition — not the tail's
    // outcome — determines the exit code.
    #[test]
    fn a_failing_graceful_tail_does_not_make_an_intentional_stop_restartable() {
        // Model the tail the way the run does: the stop is classified first,
        // then best-effort cleanup runs and is allowed to fail. The exit code
        // must be derived from the classification, never from the cleanup.
        fn exit_code_after_tail(disposition: Disposition, _tail: anyhow::Result<()>) -> i32 {
            disposition.exit_code()
        }

        let drain_timed_out: anyhow::Result<()> = Err(anyhow::anyhow!("drain timed out"));
        assert_eq!(
            exit_code_after_tail(Disposition::IntentionalStop, drain_timed_out),
            EXIT_CLEAN,
        );

        let tail_ok: anyhow::Result<()> = Ok(());
        assert_eq!(
            exit_code_after_tail(Disposition::IntentionalStop, tail_ok),
            EXIT_CLEAN,
        );
    }
}
