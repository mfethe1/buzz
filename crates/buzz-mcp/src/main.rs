//! `buzz-mcp` binary entrypoint — stdio MCP server. Configure via the client's
//! MCP config with env `BUZZ_RELAY_URL` + `BUZZ_PRIVATE_KEY` (same vars as the
//! buzz CLI). Example: `buzz-mcp` spawned by Claude Code / Codex / Hermes.

#![cfg_attr(not(windows), forbid(unsafe_code))]

fn main() {
    // Errors go to stderr — stdout IS the MCP stdio protocol channel.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    if let Err(e) = runtime.block_on(buzz_mcp::serve()) {
        eprintln!("buzz-mcp: {e}");
        std::process::exit(1);
    }
}
