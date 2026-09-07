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
