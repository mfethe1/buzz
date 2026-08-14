//! Which credential a CLI-login harness will actually charge.
//!
//! Buzz already runs `claude auth status` / `codex login status` to decide
//! whether a runtime is authenticated, but it only reads the exit code. That is
//! not enough to answer the question a user actually cares about: *which*
//! credential is in play. `claude auth status` on a machine with a subscription
//! login and an `ANTHROPIC_API_KEY` in the environment reports:
//!
//! ```jsonc
//! { "loggedIn": true, "authMethod": "claude.ai", "apiKeySource": "ANTHROPIC_API_KEY",
//!   "email": null, "orgName": null, "subscriptionType": null }
//! ```
//!
//! and without that env var, on the same machine:
//!
//! ```jsonc
//! { "loggedIn": true, "authMethod": "claude.ai",
//!   "email": "someone@example.com", "orgName": "…", "subscriptionType": "max" }
//! ```
//!
//! Both exit `0` and both say `loggedIn: true`, so an exit-code probe calls
//! each of them "Ready" — while the first silently bills per token instead of
//! the plan the user logged in with. The discriminator is `apiKeySource`, and
//! note that the CLI blanks the subscription identity when a key is in play, so
//! the key must be checked first.
//!
//! This module turns that already-collected stdout into a typed
//! [`AuthCredential`] so the UI can show it. Parsing is deliberately
//! conservative: anything we cannot positively classify returns `None` and the
//! UI shows nothing rather than a confident wrong answer.

use serde::Deserialize;

use crate::managed_agents::AuthCredential;

/// The subset of `claude auth status` JSON we classify on.
///
/// Every field is optional: the CLI emits `null` for identity fields when an
/// API key is in play, and the shape may gain fields across versions. An
/// unparseable or unexpected body yields `None` from [`parse_claude`] rather
/// than a guess.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ClaudeAuthStatus {
    logged_in: bool,
    auth_method: Option<String>,
    /// Names the environment variable the CLI took a key from (e.g.
    /// `"ANTHROPIC_API_KEY"`). Absent when the stored login is being used.
    api_key_source: Option<String>,
    /// Plan tier (e.g. `"max"`, `"pro"`). Null whenever `api_key_source` is set.
    subscription_type: Option<String>,
    email: Option<String>,
    org_name: Option<String>,
}

/// Treat blank strings as absent — the CLI uses both `null` and `""` for
/// "no value" depending on the field.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|candidate| !candidate.trim().is_empty())
}

/// Classify `claude auth status` JSON output.
///
/// Returns `None` when the body does not parse, reports a logged-out CLI, or
/// describes an auth method we cannot map to a billing story.
pub(crate) fn parse_claude(stdout: &[u8]) -> Option<AuthCredential> {
    let status: ClaudeAuthStatus = serde_json::from_slice(stdout).ok()?;
    if !status.logged_in {
        return None;
    }

    // Checked first and deliberately: when a key is in play the CLI nulls out
    // `subscriptionType`/`email`, so a subscription-first check would read the
    // blanked fields and report "no plan" instead of "billing the API".
    if let Some(source) = non_empty(status.api_key_source) {
        return Some(AuthCredential::ApiKey {
            source: Some(source),
        });
    }

    let plan = non_empty(status.subscription_type);
    let account = non_empty(status.email).or_else(|| non_empty(status.org_name));

    match status.auth_method.as_deref() {
        // The consumer subscription login.
        Some("claude.ai") => Some(AuthCredential::Subscription { plan, account }),
        // A direct Anthropic Console login: still per-token billing, but there
        // is no env var to name as the source.
        Some("console") => Some(AuthCredential::ApiKey { source: None }),
        // A plan tier is itself proof of a subscription even if the method
        // string is one we do not recognize.
        _ if plan.is_some() => Some(AuthCredential::Subscription { plan, account }),
        _ => None,
    }
}

/// Classify `codex login status` output.
///
/// Codex prints prose rather than JSON (`"Logged in using ChatGPT"`), so this
/// matches on the phrases that distinguish plan billing from key billing and
/// gives up on anything else.
pub(crate) fn parse_codex(stdout: &[u8]) -> Option<AuthCredential> {
    let text = String::from_utf8_lossy(stdout).to_lowercase();
    if !text.contains("logged in") {
        return None;
    }
    // Key check first, mirroring the Claude ordering: a line naming both should
    // report the one that actually gets charged.
    if text.contains("api key") {
        return Some(AuthCredential::ApiKey { source: None });
    }
    if text.contains("chatgpt") {
        return Some(AuthCredential::Subscription {
            plan: Some("ChatGPT".to_string()),
            account: None,
        });
    }
    None
}

/// Dispatch to the right parser for a probe, keyed on the CLI being probed.
///
/// `probe_args[0]` is the binary name, matching the convention in
/// [`KnownAcpRuntime::auth_probe_args`](crate::managed_agents::KnownAcpRuntime).
/// Unknown CLIs return `None` — a harness we have no parser for shows no
/// credential rather than a wrong one.
pub(crate) fn parse_probe_stdout(probe_args: &[&str], stdout: &[u8]) -> Option<AuthCredential> {
    match probe_args.first()? {
        &"claude" => parse_claude(stdout),
        &"codex" => parse_codex(stdout),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The two bodies below are verbatim captures from `claude auth status` on
    // the same machine, differing only in whether ANTHROPIC_API_KEY was
    // exported. They are the regression anchor for this whole feature.

    const WITH_AMBIENT_KEY: &str = r#"{
      "loggedIn": true,
      "authMethod": "claude.ai",
      "apiProvider": "firstParty",
      "apiKeySource": "ANTHROPIC_API_KEY",
      "email": null,
      "orgId": null,
      "orgName": null,
      "subscriptionType": null
    }"#;

    const WITH_SUBSCRIPTION: &str = r#"{
      "loggedIn": true,
      "authMethod": "claude.ai",
      "apiProvider": "firstParty",
      "email": "someone@example.com",
      "orgId": "ae2f444c",
      "orgName": "someone's Organization",
      "subscriptionType": "max"
    }"#;

    #[test]
    fn ambient_key_is_reported_as_api_billing_naming_the_env_var() {
        assert_eq!(
            parse_claude(WITH_AMBIENT_KEY.as_bytes()),
            Some(AuthCredential::ApiKey {
                source: Some("ANTHROPIC_API_KEY".to_string()),
            })
        );
    }

    #[test]
    fn subscription_login_is_reported_with_plan_and_account() {
        assert_eq!(
            parse_claude(WITH_SUBSCRIPTION.as_bytes()),
            Some(AuthCredential::Subscription {
                plan: Some("max".to_string()),
                account: Some("someone@example.com".to_string()),
            })
        );
    }

    #[test]
    fn the_two_states_are_distinguishable_though_both_exit_zero() {
        // The whole point: an exit-code probe cannot tell these apart, and both
        // report loggedIn: true. If this assertion ever fails the badge is
        // lying about which account gets charged.
        assert_ne!(
            parse_claude(WITH_AMBIENT_KEY.as_bytes()),
            parse_claude(WITH_SUBSCRIPTION.as_bytes())
        );
    }

    #[test]
    fn api_key_wins_over_a_stale_subscription_field() {
        // Defensive: should a future CLI version report both, the key is what
        // actually gets charged, so it must win.
        let both = r#"{"loggedIn":true,"authMethod":"claude.ai",
            "apiKeySource":"ANTHROPIC_API_KEY","subscriptionType":"max",
            "email":"someone@example.com"}"#;
        assert_eq!(
            parse_claude(both.as_bytes()),
            Some(AuthCredential::ApiKey {
                source: Some("ANTHROPIC_API_KEY".to_string()),
            })
        );
    }

    #[test]
    fn console_login_reports_api_billing_without_an_env_var() {
        let console = r#"{"loggedIn":true,"authMethod":"console","email":"dev@example.com"}"#;
        assert_eq!(
            parse_claude(console.as_bytes()),
            Some(AuthCredential::ApiKey { source: None })
        );
    }

    #[test]
    fn logged_out_reports_no_credential() {
        let out = r#"{"loggedIn":false,"authMethod":null}"#;
        assert_eq!(parse_claude(out.as_bytes()), None);
    }

    #[test]
    fn blank_strings_count_as_absent() {
        let blank = r#"{"loggedIn":true,"authMethod":"claude.ai",
            "apiKeySource":"   ","subscriptionType":"","email":""}"#;
        assert_eq!(
            parse_claude(blank.as_bytes()),
            Some(AuthCredential::Subscription {
                plan: None,
                account: None,
            }),
            "a blank apiKeySource must not be reported as API billing"
        );
    }

    #[test]
    fn unparseable_output_reports_nothing_rather_than_guessing() {
        assert_eq!(parse_claude(b"not json at all"), None);
        assert_eq!(parse_claude(b""), None);
    }

    #[test]
    fn unrecognized_auth_method_without_a_plan_reports_nothing() {
        let odd = r#"{"loggedIn":true,"authMethod":"something-new"}"#;
        assert_eq!(parse_claude(odd.as_bytes()), None);
    }

    #[test]
    fn unrecognized_auth_method_with_a_plan_still_reports_the_subscription() {
        let odd = r#"{"loggedIn":true,"authMethod":"something-new","subscriptionType":"team"}"#;
        assert_eq!(
            parse_claude(odd.as_bytes()),
            Some(AuthCredential::Subscription {
                plan: Some("team".to_string()),
                account: None,
            })
        );
    }

    // ── codex ───────────────────────────────────────────────────────────────

    #[test]
    fn codex_chatgpt_plan_is_a_subscription() {
        // Verbatim capture from `codex login status`.
        assert_eq!(
            parse_codex(b"Logged in using ChatGPT\n"),
            Some(AuthCredential::Subscription {
                plan: Some("ChatGPT".to_string()),
                account: None,
            })
        );
    }

    #[test]
    fn codex_api_key_is_api_billing() {
        assert_eq!(
            parse_codex(b"Logged in using an API key\n"),
            Some(AuthCredential::ApiKey { source: None })
        );
    }

    #[test]
    fn codex_logged_out_reports_nothing() {
        assert_eq!(parse_codex(b"Not logged in\n"), None);
    }

    // ── dispatch ────────────────────────────────────────────────────────────

    #[test]
    fn dispatch_routes_by_probe_binary() {
        assert!(
            parse_probe_stdout(&["claude", "auth", "status"], WITH_SUBSCRIPTION.as_bytes())
                .is_some()
        );
        assert!(
            parse_probe_stdout(&["codex", "login", "status"], b"Logged in using ChatGPT").is_some()
        );
        // A harness with no parser must not borrow another CLI's semantics.
        assert!(parse_probe_stdout(&["goose"], WITH_SUBSCRIPTION.as_bytes()).is_none());
        assert!(parse_probe_stdout(&[], WITH_SUBSCRIPTION.as_bytes()).is_none());
    }
}
