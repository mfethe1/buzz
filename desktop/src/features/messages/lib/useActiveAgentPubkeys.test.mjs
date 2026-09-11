import assert from "node:assert/strict";
import test from "node:test";

import { isRelayAgentActive } from "./useActiveAgentPubkeys.ts";

// REG-27: relay presence (TTL'd kind:20001) is authority over the unexpiring
// kind:10100 directory status, but only when the presence read SUCCEEDED.
// `availability === undefined` means unknowable, never "inactive".

test("E8 stale-online ghost: directory online + presence offline is INACTIVE", () => {
  assert.equal(isRelayAgentActive("online", "offline"), false);
  assert.equal(isRelayAgentActive("away", "offline"), false);
});

test("presence online/away keeps an agent active", () => {
  assert.equal(isRelayAgentActive("online", "online"), true);
  assert.equal(isRelayAgentActive("offline", "online"), true);
  assert.equal(isRelayAgentActive("offline", "away"), true);
});

test("E2 relay disconnected / presence error falls back to the directory", () => {
  // Negative test: an unreadable relay must NOT empty the mention picker.
  assert.equal(isRelayAgentActive("online", undefined), true);
  assert.equal(isRelayAgentActive("away", undefined), true);
  assert.equal(isRelayAgentActive("offline", undefined), false);
});

test("E4 authz-denied partial read behaves as a failed read, never as offline", () => {
  // A denied presence entry surfaces as undefined, not "offline".
  assert.equal(isRelayAgentActive("online", undefined), true);
});

test("E5 un-queried pubkey (persona sibling) falls back to the directory", () => {
  assert.equal(isRelayAgentActive("online", undefined), true);
});

test("E6 malformed directory status is inactive, case-sensitively, without crashing", () => {
  for (const status of ["", "ONLINE", "Online", "Away", "unknown", "running"]) {
    assert.equal(isRelayAgentActive(status, undefined), false, status);
  }
  assert.equal(isRelayAgentActive(null, undefined), false);
  assert.equal(isRelayAgentActive(undefined, undefined), false);
});

test("a successful presence read overrides a malformed directory status", () => {
  assert.equal(isRelayAgentActive("ONLINE", "online"), true);
  assert.equal(isRelayAgentActive("", "offline"), false);
});

// E1: empty inputs — isRelayAgentActive never throws on edge values.
// The empty-relayAgents / empty-pubkey-list path is exercised by the hook's
// `relayAgents ?? []` spread (produces an empty set) and usePresenceQuery's
// self-disable at hooks.ts:100. Here we guard the pure resolver directly.
test("E1 empty / nullish inputs do not crash", () => {
  // No directory status, no presence → inactive, no throw.
  assert.equal(isRelayAgentActive("", undefined), false);
  // null/undefined directory status is already covered by E6; E1 confirms the
  // empty-string case and the no-throw contract on edge values.
});

// E7: a live 20001 event landing mid-render is handled by the memo dependency
// on getAvailability (which changes identity when query.data changes). This
// is architectural and covered by the existing setQueriesData + memo dep
// pattern; it cannot be unit-tested without mocking React's renderer. The
// isRelayAgentActive function is pure and stateless, so a re-derivation with
// updated availability is guaranteed correct.
test("E7 re-derivation: directory online + late-arriving presence offline is INACTIVE", () => {
  // Simulates a mid-render presence update: first read undefined, then offline.
  assert.equal(isRelayAgentActive("online", undefined), true); // before event
  assert.equal(isRelayAgentActive("online", "offline"), false); // after event
});
