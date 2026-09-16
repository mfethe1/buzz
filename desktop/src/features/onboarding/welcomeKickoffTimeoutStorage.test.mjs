/**
 * #50: persistent kickoff-timeout latch. Pure module over an injected Storage
 * so it runs under node:test with a Map-backed fake — no DOM, no Tauri.
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_TIMEOUT_STORE,
  MAX_WELCOME_KICKOFF_TIMEOUT_ENTRIES,
  isChannelKickoffTimedOut,
  latchChannelKickoffTimeout,
  parseTimeoutPayload,
  pruneTimeoutStore,
  readTimeoutStore,
} from "./welcomeKickoffTimeoutStorage.ts";

function fakeStorage() {
  const map = new Map();
  return {
    getItem: (k) => (map.has(k) ? map.get(k) : null),
    setItem: (k, v) => {
      map.set(k, String(v));
    },
    _map: map,
  };
}

test("parse rejects malformed payloads", () => {
  assert.equal(parseTimeoutPayload(null), null);
  assert.equal(parseTimeoutPayload("nope"), null);
  assert.equal(parseTimeoutPayload({ version: 2, channels: {} }), null);
  assert.equal(parseTimeoutPayload({ version: 1, channels: [] }), null);
  assert.equal(parseTimeoutPayload({ version: 1 }), null);
});

test("parse drops non-finite timestamps but keeps valid ones", () => {
  const parsed = parseTimeoutPayload({
    version: 1,
    channels: { a: 100, b: "x", c: -1, d: NaN },
  });
  assert.deepEqual(parsed, { version: 1, channels: { a: 100 } });
});

test("read returns the default store on empty or corrupt storage", () => {
  assert.equal(readTimeoutStore(fakeStorage()), DEFAULT_TIMEOUT_STORE);
  const corrupt = fakeStorage();
  corrupt.setItem("buzz-welcome-kickoff-timeouts.v1", "{not json");
  assert.equal(readTimeoutStore(corrupt), DEFAULT_TIMEOUT_STORE);
});

test("isChannelKickoffTimedOut is false for unknown channels", () => {
  assert.equal(isChannelKickoffTimedOut(fakeStorage(), null), false);
  assert.equal(isChannelKickoffTimedOut(fakeStorage(), "ch-1"), false);
});

test("latch records the channel and is then reported timed out", () => {
  const storage = fakeStorage();
  latchChannelKickoffTimeout(storage, "ch-1", 1000);
  assert.equal(isChannelKickoffTimedOut(storage, "ch-1"), true);
  assert.equal(isChannelKickoffTimedOut(storage, "ch-2"), false);
});

test("latch is idempotent — re-latching keeps the first timestamp", () => {
  const storage = fakeStorage();
  latchChannelKickoffTimeout(storage, "ch-1", 1000);
  latchChannelKickoffTimeout(storage, "ch-1", 5000);
  const store = readTimeoutStore(storage);
  assert.equal(store.channels["ch-1"], 1000);
});

test("prune keeps the newest entries up to the cap", () => {
  const channels = {};
  for (let i = 0; i < MAX_WELCOME_KICKOFF_TIMEOUT_ENTRIES + 10; i++) {
    channels[`ch-${i}`] = i;
  }
  const pruned = pruneTimeoutStore({ version: 1, channels });
  assert.equal(
    Object.keys(pruned.channels).length,
    MAX_WELCOME_KICKOFF_TIMEOUT_ENTRIES,
  );
  // Highest (newest) timestamps survive.
  assert.ok(`ch-${MAX_WELCOME_KICKOFF_TIMEOUT_ENTRIES + 9}` in pruned.channels);
  assert.ok(!("ch-0" in pruned.channels));
});

test("prune is a no-op under the cap (stable reference)", () => {
  const store = { version: 1, channels: { a: 1 } };
  assert.equal(pruneTimeoutStore(store), store);
});

test("latch enforces the cap by pruning oldest", () => {
  const storage = fakeStorage();
  for (let i = 0; i < MAX_WELCOME_KICKOFF_TIMEOUT_ENTRIES + 5; i++) {
    latchChannelKickoffTimeout(storage, `ch-${i}`, i);
  }
  const store = readTimeoutStore(storage);
  assert.equal(
    Object.keys(store.channels).length,
    MAX_WELCOME_KICKOFF_TIMEOUT_ENTRIES,
  );
});
