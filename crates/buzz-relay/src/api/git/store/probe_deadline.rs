//! A total deadline around every request in the startup admission probe.

use super::{GitStore, ProbeConfig, ProbeFailure, ProbeReport, StoreError};

impl GitStore {
    /// Admit the backend only when all conformance phases finish within the budget.
    ///
    /// Dropping the inner future cancels the pending request futures, including
    /// the non-spawned racers in `join_all`. An unfinished racer is never treated
    /// as an observed transport drop or a successful admission.
    pub async fn run_conformance_probe(&self, cfg: ProbeConfig) -> Result<ProbeReport, StoreError> {
        let timeout = cfg.total_timeout;
        if timeout.is_zero() {
            return Err(ProbeFailure {
                phase: "config",
                round: 0,
                key: String::new(),
                reason: "total_timeout must be greater than zero".into(),
            }
            .into());
        }
        tokio::time::timeout(timeout, self.run_conformance_probe_inner(cfg))
            .await
            .map_err(|_| ProbeFailure {
                phase: "deadline",
                round: 0,
                key: String::new(),
                reason: format!(
                    "total probe deadline exceeded after {} ms; backend not admitted",
                    timeout.as_millis()
                ),
            })?
    }
}

#[cfg(test)]
mod tests;
