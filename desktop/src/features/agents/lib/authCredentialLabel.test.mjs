import assert from "node:assert/strict";
import test from "node:test";

import {
  authCredentialLabel,
  isShadowedByEnvKey,
} from "./authCredentialLabel.ts";

// The two credentials below are what the Rust parser produces from the two
// verbatim `claude auth status` bodies captured on one machine, differing only
// in whether ANTHROPIC_API_KEY was exported. Both exit 0 and both report
// logged-in, so this label is the only place the difference becomes visible.
const AMBIENT_KEY = { kind: "api_key", source: "ANTHROPIC_API_KEY" };
const SUBSCRIPTION = {
  kind: "subscription",
  plan: "max",
  account: "someone@example.com",
};

test("subscription reads as the plan the user signed in with", () => {
  const label = authCredentialLabel(SUBSCRIPTION, "Claude");
  assert.equal(label.tone, "ok");
  assert.equal(label.title, "Claude Max subscription");
  assert.equal(label.detail, "someone@example.com");
  assert.equal(label.envVar, null);
});

test("ambient env key warns and names the variable to change", () => {
  const label = authCredentialLabel(AMBIENT_KEY, "Claude");
  assert.equal(label.tone, "warning");
  assert.equal(label.title, "API key billing");
  assert.match(label.detail, /ANTHROPIC_API_KEY/);
  assert.match(label.detail, /billed per token/);
  // The UI needs the bare variable name for its "remove it" affordance, not
  // just the prose sentence.
  assert.equal(label.envVar, "ANTHROPIC_API_KEY");
});

test("the two states never render the same", () => {
  // Regression anchor for the whole feature: if these ever collapse to the
  // same label the badge is hiding exactly what it exists to reveal.
  assert.notDeepEqual(
    authCredentialLabel(AMBIENT_KEY, "Claude"),
    authCredentialLabel(SUBSCRIPTION, "Claude"),
  );
});

test("plan slugs are given their vendor-facing names", () => {
  assert.equal(
    authCredentialLabel({ kind: "subscription", plan: "pro" }, "Claude").title,
    "Claude Pro subscription",
  );
  // An unknown tier passes through verbatim rather than being dropped.
  assert.equal(
    authCredentialLabel({ kind: "subscription", plan: "ultra" }, "Claude")
      .title,
    "Claude ultra subscription",
  );
});

test("a plan-less subscription stays honest rather than inventing a tier", () => {
  const label = authCredentialLabel({ kind: "subscription" }, "Codex");
  assert.equal(label.title, "Codex subscription");
  assert.equal(label.detail, null);
});

test("the vendor prefix is dropped where the UI already names the harness", () => {
  // A Doctor row headed "Claude Code" must not read "Claude Code Max
  // subscription".
  assert.equal(authCredentialLabel(SUBSCRIPTION).title, "Max subscription");
  assert.equal(
    authCredentialLabel({ kind: "subscription" }).title,
    "Subscription",
  );
});

test("api billing with no named source still warns", () => {
  // A direct Console login: per-token billing, but no env var to point at.
  const label = authCredentialLabel({ kind: "api_key" }, "Claude");
  assert.equal(label.tone, "warning");
  assert.equal(label.envVar, null);
  assert.match(label.detail, /not to a subscription/);
});

test("no credential renders nothing at all", () => {
  // Better silent than confidently wrong about someone's bill.
  assert.equal(authCredentialLabel(null, "Claude"), null);
  assert.equal(authCredentialLabel(undefined, "Claude"), null);
});

test("isShadowedByEnvKey fires only for an env-sourced key", () => {
  assert.equal(isShadowedByEnvKey(AMBIENT_KEY), true);
  assert.equal(isShadowedByEnvKey(SUBSCRIPTION), false);
  // Console login — API billing, but nothing in the environment to remove, so
  // the "we found a key in your environment" nudge must not fire.
  assert.equal(isShadowedByEnvKey({ kind: "api_key" }), false);
  assert.equal(isShadowedByEnvKey(null), false);
});
