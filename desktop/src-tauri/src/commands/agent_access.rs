/// Return whether this build enforces owner-only managed-agent access.
#[tauri::command]
pub fn agent_access_owner_only() -> bool {
    crate::managed_agents::owner_only_access_build()
}

/// Return whether this build skips provisioning the built-in Welcome Team
/// during onboarding. Mirrors `agent_access_owner_only`: a presence-only
/// build marker (`BUZZ_BUILD_SKIP_WELCOME_TEAM` at packaging time) so fleet
/// installs can start with a clean agent inventory. OSS default is `false`.
#[tauri::command]
pub fn skip_welcome_team_provisioning() -> bool {
    option_env!("BUZZ_DESKTOP_BUILD_SKIP_WELCOME_TEAM").is_some()
}

/// Tiny executable-facing probe for release packaging smoke tests. Keeping the
/// probe in the product crate makes it impossible for buzz-releases to validate
/// a copied flag interpretation that has drifted from Desktop's command.
#[doc(hidden)]
pub fn print_agent_access_owner_only_probe_if_requested() -> bool {
    if std::env::args().any(|arg| arg == "--print-agent-access-owner-only") {
        println!("{}", agent_access_owner_only());
        true
    } else if std::env::args().any(|arg| arg == "--print-skip-welcome-team") {
        println!("{}", skip_welcome_team_provisioning());
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires BUZZ_TEST_EXPECTED_AGENT_ACCESS_OWNER_ONLY"]
    fn compiled_policy_matches_expected() {
        let expected = std::env::var("BUZZ_TEST_EXPECTED_AGENT_ACCESS_OWNER_ONLY")
            .expect("BUZZ_TEST_EXPECTED_AGENT_ACCESS_OWNER_ONLY must be set")
            .parse::<bool>()
            .expect("BUZZ_TEST_EXPECTED_AGENT_ACCESS_OWNER_ONLY must be true or false");
        assert_eq!(super::agent_access_owner_only(), expected);
    }

    #[test]
    #[ignore = "requires BUZZ_TEST_EXPECTED_SKIP_WELCOME_TEAM"]
    fn compiled_skip_welcome_team_matches_expected() {
        let expected = std::env::var("BUZZ_TEST_EXPECTED_SKIP_WELCOME_TEAM")
            .expect("BUZZ_TEST_EXPECTED_SKIP_WELCOME_TEAM must be set")
            .parse::<bool>()
            .expect("BUZZ_TEST_EXPECTED_SKIP_WELCOME_TEAM must be true or false");
        assert_eq!(super::skip_welcome_team_provisioning(), expected);
    }
}
