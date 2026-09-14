/**
 * React hook: report whether this build skips provisioning the built-in
 * Welcome Team during onboarding.
 *
 * The value is baked at build time, so it cannot change while the app runs.
 * The query key is stable and the result never goes stale, which keeps one
 * fetch per QueryClient lifetime and gives every caller the same answer.
 */
import { useQuery } from "@tanstack/react-query";

import { getSkipWelcomeTeamProvisioning } from "@/shared/api/tauriAgentAccess";

export const skipWelcomeTeamProvisioningQueryKey = [
  "skip-welcome-team-provisioning",
] as const;

export function useSkipWelcomeTeamProvisioningQuery(options?: {
  enabled?: boolean;
}) {
  return useQuery({
    queryKey: skipWelcomeTeamProvisioningQueryKey,
    queryFn: () => getSkipWelcomeTeamProvisioning(),
    enabled: options?.enabled ?? true,
    staleTime: Infinity,
    refetchInterval: false,
    retry: false,
  });
}
