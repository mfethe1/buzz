//! Sprig — all-in-one Buzz ACP harness, agent, and developer MCP.
//!
//! Sprig is the multicall binary the container image runs, so its exit code is
//! the one a supervisor (kubelet `restartPolicy`, systemd) observes. It must
//! therefore honor the harness clean-exit contract
//! (`docs/remote-agents.md` Known Defect 6): an intentional stop exits 0 so a
//! supervisor leaves it stopped; anything else exits nonzero.

use buzz_acp::exit::{Disposition, EXIT_FAILURE};

fn main() -> std::process::ExitCode {
    match dispatch() {
        Ok(disposition) => {
            std::process::ExitCode::from(u8::try_from(disposition.exit_code()).unwrap_or(1))
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::ExitCode::from(u8::try_from(EXIT_FAILURE).unwrap_or(1))
        }
    }
}

fn dispatch() -> Result<Disposition, String> {
    let argv0 = std::env::args().next().unwrap_or_default();
    let cmd = std::path::Path::new(&argv0)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match cmd.as_str() {
        // The harness classifies its own stop; pass that classification
        // through untouched so a clean stop stays exit 0.
        "buzz-acp" => buzz_acp::run().map_err(|e| e.to_string()),
        // The remaining personalities are one-shot tools: reaching the end of
        // a successful run is an intentional completion, not a failure.
        "buzz-agent" => buzz_agent::run()
            .map(|()| Disposition::IntentionalStop)
            .map_err(|e| e.to_string()),
        "sprig" => match std::env::args().nth(1).as_deref() {
            Some("-V") | Some("--version") => {
                println!("sprig {}", env!("CARGO_PKG_VERSION"));
                Ok(Disposition::IntentionalStop)
            }
            Some("-h") | Some("--help") | None => {
                print_usage();
                if std::env::args().len() <= 1 {
                    Err("error: invoke Sprig via a personality symlink".into())
                } else {
                    Ok(Disposition::IntentionalStop)
                }
            }
            Some(other) => {
                print_usage();
                Err(format!(
                    "error: unknown Sprig option or personality: {other}"
                ))
            }
        },
        // buzz-dev-mcp also handles its own multicall names: rg, tree,
        // buzz, git-credential-nostr, and git-sign-nostr.
        _ => buzz_dev_mcp::run()
            .map(|()| Disposition::IntentionalStop)
            .map_err(|e| e.to_string()),
    }
}

fn print_usage() {
    println!(
        "Sprig — all-in-one Buzz ACP harness, agent, and developer MCP\n\n\
Sprig is a multicall binary. Invoke it through one of the personality names:\n\n\
  buzz-acp       ACP harness\n  buzz-agent     ACP-compliant agent\n  buzz-dev-mcp   Developer MCP server\n\n\
Developer MCP helper names are also supported: rg, tree, buzz, git-credential-nostr, git-sign-nostr.\n\n\
Installers can create links with:\n  ln -s sprig buzz-acp\n  ln -s sprig buzz-agent\n  ln -s sprig buzz-dev-mcp"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_acp::exit::EXIT_CLEAN;

    #[test]
    fn a_clean_harness_stop_maps_to_exit_zero() {
        // The pin that matters for the image: sprig must not turn the
        // harness's intentional stop into a restartable failure.
        assert_eq!(Disposition::IntentionalStop.exit_code(), EXIT_CLEAN);
        assert!(!Disposition::IntentionalStop.supervisor_may_restart());
    }

    #[test]
    fn a_failure_maps_to_nonzero() {
        assert_eq!(Disposition::Failure.exit_code(), EXIT_FAILURE);
        assert_ne!(EXIT_FAILURE, EXIT_CLEAN);
    }
}
