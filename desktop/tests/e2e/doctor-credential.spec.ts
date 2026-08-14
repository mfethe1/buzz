import { expect, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";
import { waitForAnimations } from "../helpers/animations";
import { openSettings } from "../helpers/settings";

const SHOTS = "test-results/screenshots-credential";

/**
 * Both fixtures below are `availability: "available"` with
 * `auth_status: { status: "logged_in" }` — deliberately identical, because that
 * is the real-world trap: `claude auth status` exits 0 and reports logged-in
 * whether the CLI is using the user's subscription or an ANTHROPIC_API_KEY it
 * found in the environment. Everything the Doctor row rendered before
 * `auth_credential` existed is the same across these two entries, so if the
 * credential line regresses, these tests capture byte-identical rows.
 */
const CLAUDE_ON_SUBSCRIPTION = {
  id: "claude",
  label: "Claude Code",
  avatar_url: "",
  availability: "available",
  command: "claude-agent-acp",
  binary_path: "/usr/local/bin/claude-agent-acp",
  default_args: [],
  mcp_command: null,
  install_hint: "",
  install_instructions_url:
    "https://github.com/agentclientprotocol/claude-agent-acp",
  can_auto_install: true,
  underlying_cli_path: "/usr/local/bin/claude",
  node_required: false,
  auth_status: { status: "logged_in" },
  auth_credential: {
    kind: "subscription",
    plan: "max",
    account: "someone@example.com",
  },
};

const CLAUDE_SHADOWED_BY_ENV_KEY = {
  ...CLAUDE_ON_SUBSCRIPTION,
  auth_credential: { kind: "api_key", source: "ANTHROPIC_API_KEY" },
};

test.describe("Doctor credential line", () => {
  test.use({ viewport: { width: 1280, height: 720 } });

  test("subscription login names the plan and account", async ({ page }) => {
    await installMockBridge(page, {
      acpRuntimesCatalog: [CLAUDE_ON_SUBSCRIPTION],
    });
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await openSettings(page, "agents");

    const row = page.getByTestId("doctor-runtime-claude");
    await expect(row).toBeVisible({ timeout: 10_000 });

    const credential = page.getByTestId("doctor-runtime-credential-claude");
    await expect(credential).toBeVisible();
    await expect(credential).toContainText("Max subscription");
    await expect(credential).toContainText("someone@example.com");
    // The row still reads Ready — the credential line adds information, it does
    // not downgrade a working harness.
    await expect(page.getByTestId("doctor-runtime-ready-claude")).toBeVisible();

    await waitForAnimations(page);
    await row.screenshot({ path: `${SHOTS}/01-subscription.png` });
  });

  test("ambient API key warns and names the variable", async ({ page }) => {
    await installMockBridge(page, {
      acpRuntimesCatalog: [CLAUDE_SHADOWED_BY_ENV_KEY],
    });
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await openSettings(page, "agents");

    const row = page.getByTestId("doctor-runtime-claude");
    await expect(row).toBeVisible({ timeout: 10_000 });

    const credential = page.getByTestId("doctor-runtime-credential-claude");
    await expect(credential).toBeVisible();
    await expect(credential).toContainText("API key billing");
    // The exact variable name is the actionable part — without it the user has
    // to go hunting through shell profiles and Windows user env vars.
    await expect(credential).toContainText("ANTHROPIC_API_KEY");
    await expect(credential).toContainText("billed per token");
    // Still Ready: the harness runs fine, which is precisely why this needed a
    // surface of its own rather than a readiness failure.
    await expect(page.getByTestId("doctor-runtime-ready-claude")).toBeVisible();

    await waitForAnimations(page);
    await row.screenshot({ path: `${SHOTS}/02-api-key.png` });
  });

  test("a runtime with no reported credential stays silent", async ({
    page,
  }) => {
    // Better to show nothing than to guess wrong about someone's bill.
    const { auth_credential: _omitted, ...noCredential } =
      CLAUDE_ON_SUBSCRIPTION;
    await installMockBridge(page, { acpRuntimesCatalog: [noCredential] });
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await openSettings(page, "agents");

    await expect(page.getByTestId("doctor-runtime-claude")).toBeVisible({
      timeout: 10_000,
    });
    await expect(
      page.getByTestId("doctor-runtime-credential-claude"),
    ).toHaveCount(0);
  });
});
