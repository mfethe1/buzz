//! End-to-end proof of the harness clean-exit contract
//! (`docs/remote-agents.md` Known Defect 6, invariant I5).
//!
//! The unit tests in `buzz_acp::exit` pin the mapping from disposition to
//! code. They cannot prove the *process* honors it: `main` could ignore the
//! disposition, and the previous `fn main() -> anyhow::Result<()>` mapped
//! every outcome to 1. These tests spawn the real binary and read the real
//! exit status, which is what a kubelet or systemd actually observes.
//!
//! Without a pinned contract, `restartPolicy: OnFailure` cannot be deployed:
//! a supervisor that cannot tell "told to stop" from "crashed" either
//! restarts a deliberately stopped agent forever, or declines to restart one
//! that genuinely died.
//!
//! These tests drive only real, supported behavior — no test-only branch is
//! compiled into the shipped binary to make them pass.
//!
//! Coverage note: the SIGTERM path through the long-running harness loop
//! needs a full ACP agent stub to reach, so it is covered by the disposition
//! unit tests in `buzz_acp::exit` rather than here. What these tests pin is
//! the part that regressed before: that `main` maps a run's outcome to a
//! code at all, and that a failure returning through `run()` is nonzero and
//! distinct from a clean completion.

use std::process::Command;

/// A run that completes intentionally must exit 0 so a supervisor leaves the
/// process stopped (I5).
#[test]
fn an_intentional_completion_exits_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_buzz-acp"))
        .arg("--help")
        .output()
        .expect("run buzz-acp --help");

    assert_eq!(
        output.status.code(),
        Some(0),
        "an intentional completion MUST exit 0 so a supervisor does not \
         restart it (docs/remote-agents.md Known Defect 6, I5); stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A failure returned through `run()` must exit nonzero, or `OnFailure`
/// cannot restart genuine crashes. `models` against a nonexistent agent
/// binary fails inside the run and returns `Err` through `main`'s mapping —
/// unlike an argument-parse error, which clap exits on directly.
#[test]
fn a_runtime_failure_exits_nonzero_through_our_mapping() {
    let output = Command::new(env!("CARGO_BIN_EXE_buzz-acp"))
        .args([
            "models",
            "--agent-command",
            "/nonexistent-agent-binary-for-exit-code-test",
        ])
        .output()
        .expect("run buzz-acp models");

    assert_eq!(
        output.status.code(),
        Some(1),
        "a failure returning through run() MUST exit 1 so a supervisor may \
         restart it; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The two outcomes must not collapse onto the same code — that collapse is
/// exactly what made `OnFailure` undeployable.
#[test]
fn clean_and_failed_exits_are_distinguishable() {
    let clean = Command::new(env!("CARGO_BIN_EXE_buzz-acp"))
        .arg("--help")
        .output()
        .expect("run buzz-acp --help");
    let failed = Command::new(env!("CARGO_BIN_EXE_buzz-acp"))
        .args([
            "models",
            "--agent-command",
            "/nonexistent-agent-binary-for-exit-code-test",
        ])
        .output()
        .expect("run buzz-acp models");

    assert_eq!(clean.status.code(), Some(0));
    assert_ne!(
        clean.status.code(),
        failed.status.code(),
        "a supervisor distinguishes stop from crash by exit code alone; if \
         these match, restartPolicy: OnFailure cannot be deployed"
    );
}
