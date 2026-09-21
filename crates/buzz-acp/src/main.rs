//! `buzz-acp` entry point.
//!
//! The exit code is the harness's half of the supervisor contract
//! (`docs/remote-agents.md` Known Defect 6): an intentional stop exits 0 so a
//! `restartPolicy: OnFailure` / `Restart=on-failure` supervisor leaves it
//! stopped, and anything else exits nonzero so the supervisor may restart it.
//!
//! `main` deliberately does NOT return `anyhow::Result`: that maps every error
//! to exit code 1 *and* prints a `Debug` representation, which would leave the
//! clean-exit code undefended and the failure rendering unowned.

fn main() -> std::process::ExitCode {
    let result = buzz_acp::run();
    if let Err(error) = &result {
        eprintln!("buzz-acp: {error:#}");
    }
    let code = buzz_acp::exit::exit_code_for(&result);
    std::process::ExitCode::from(u8::try_from(code).unwrap_or(1))
}
