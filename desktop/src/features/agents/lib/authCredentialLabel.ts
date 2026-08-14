import type { AuthCredential } from "@/shared/api/types";

/**
 * Display model for the credential a CLI-login harness will be charged against.
 *
 * `tone` drives the visual treatment, and it is the whole point of this module:
 * an API key picked up from the environment is not an error — the harness runs
 * fine — but it is almost never what someone who logged into a subscription
 * intended, and nothing else in the UI would ever say so.
 */
export type AuthCredentialLabel = {
  tone: "ok" | "warning";
  /** Primary line, e.g. "Claude Max subscription". */
  title: string;
  /** Secondary line: where the credential came from, or who it belongs to. */
  detail: string | null;
  /**
   * Present only when the credential is an env-var API key shadowing a login.
   * Names the variable so the user can act on it without hunting.
   */
  envVar: string | null;
};

/** Vendor-facing name for the plan tiers the CLIs report as bare slugs. */
const PLAN_LABELS: Record<string, string> = {
  max: "Max",
  pro: "Pro",
  team: "Team",
  enterprise: "Enterprise",
  free: "Free",
};

function planLabel(plan: string): string {
  return PLAN_LABELS[plan.toLowerCase()] ?? plan;
}

/**
 * Build the credential line for a harness, or `null` when there is nothing
 * trustworthy to say.
 *
 * `vendor` ("Claude", "Codex") is optional and only shapes the copy. Omit it
 * where the surrounding UI already names the harness — a row headed "Claude
 * Code" should not then read "Claude Code Max subscription".
 */
export function authCredentialLabel(
  credential: AuthCredential | null | undefined,
  vendor?: string,
): AuthCredentialLabel | null {
  if (!credential) return null;

  if (credential.kind === "subscription") {
    const plan = credential.plan ? planLabel(credential.plan) : null;
    const prefix = vendor ? `${vendor} ` : "";
    return {
      tone: "ok",
      // "Claude Max subscription" when both are known, degrading to
      // "Max subscription" or a bare "Subscription" rather than inventing.
      title: plan
        ? `${prefix}${plan} subscription`
        : vendor
          ? `${vendor} subscription`
          : "Subscription",
      detail: credential.account ?? null,
      envVar: null,
    };
  }

  // API-key billing. When the CLI named the environment variable it took the
  // key from, say so plainly — that string is the thing the user has to change.
  return {
    tone: "warning",
    title: "API key billing",
    detail: credential.source
      ? `from ${credential.source} in your environment · billed per token`
      : "billed per token, not to a subscription",
    envVar: credential.source ?? null,
  };
}

/**
 * True when the harness is authenticated but an environment API key is
 * overriding a login the user completed. This is the state worth interrupting
 * for: the agent works, the bill lands somewhere unexpected.
 */
export function isShadowedByEnvKey(
  credential: AuthCredential | null | undefined,
): boolean {
  return credential?.kind === "api_key" && Boolean(credential.source);
}
