import { invokeTauri } from "@/shared/api/tauri";

/** Whether this build enforces owner-only managed-agent access. */
export const getAgentAccessOwnerOnly = () =>
  invokeTauri<boolean>("agent_access_owner_only");

/**
 * Whether this build skips provisioning the built-in Welcome Team during
 * onboarding. Baked at packaging time via `BUZZ_BUILD_SKIP_WELCOME_TEAM`
 * (mirrors `agent_access_owner_only`); `false` for OSS builds.
 */
export const getSkipWelcomeTeamProvisioning = () =>
  invokeTauri<boolean>("skip_welcome_team_provisioning");
