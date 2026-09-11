import assert from "node:assert/strict";
import test from "node:test";

import {
  SYNC_REQUIRED_KNOWN_REASONS,
  normaliseSyncRequiredReason,
  isKnownSyncRequiredReason,
  shouldStartSyncReplay,
} from "./relaySyncRequiredPolicy.ts";

// ── normaliseSyncRequiredReason ────────────────────────────────────────────

test("normaliseSyncRequiredReason: returns string reasons as-is", () => {
  assert.equal(normaliseSyncRequiredReason("backpressure"), "backpressure");
  assert.equal(normaliseSyncRequiredReason("evil-cache"), "evil-cache");
});

test("normaliseSyncRequiredReason: returns undefined for non-string values", () => {
  assert.equal(normaliseSyncRequiredReason(undefined), undefined);
  assert.equal(normaliseSyncRequiredReason(null), undefined);
  assert.equal(normaliseSyncRequiredReason(42), undefined);
  assert.equal(normaliseSyncRequiredReason({}), undefined);
  assert.equal(normaliseSyncRequiredReason([]), undefined);
});

// ── isKnownSyncRequiredReason ───────────────────────────────────────────────

test("isKnownSyncRequiredReason: backpressure is known", () => {
  assert.equal(isKnownSyncRequiredReason("backpressure"), true);
});

test("isKnownSyncRequiredReason: unknown reasons are NOT known", () => {
  assert.equal(isKnownSyncRequiredReason("evil-cache"), false);
  assert.equal(isKnownSyncRequiredReason("unexpected-error"), false);
});

test("isKnownSyncRequiredReason: undefined is NOT known", () => {
  assert.equal(isKnownSyncRequiredReason(undefined), false);
});

test("SYNC_REQUIRED_KNOWN_REASONS: contains exactly backpressure", () => {
  assert.ok(SYNC_REQUIRED_KNOWN_REASONS.has("backpressure"));
  assert.equal(SYNC_REQUIRED_KNOWN_REASONS.size, 1);
});

// ── shouldStartSyncReplay (burst coalescing) ───────────────────────────────

test("shouldStartSyncReplay: returns true when no replay is in-flight", () => {
  assert.equal(shouldStartSyncReplay(null), true);
});

test("shouldStartSyncReplay: returns false when a replay is already in-flight", () => {
  // A settled-but-not-cleared promise should still block — the session's finally
  // block is responsible for clearing the slot, not this predicate.
  const inFlight = Promise.resolve();
  assert.equal(shouldStartSyncReplay(inFlight), false);
});

test("shouldStartSyncReplay: returns false when a pending replay is in-flight", () => {
  // A never-resolving promise simulates a long-running replay.
  let _resolve;
  const inFlight = new Promise((resolve) => {
    _resolve = resolve;
  });
  assert.equal(shouldStartSyncReplay(inFlight), false);
});

test("shouldStartSyncReplay: burst coalescing — second frame while first in-flight is suppressed", () => {
  // Simulate the session's coalescing lifecycle:
  //   1. Frame 1 arrives → slot null → start replay, store promise
  //   2. Frame 2 arrives → slot non-null → suppressed
  //   3. Replay settles → finally clears slot
  //   4. Frame 3 arrives → slot null → start replay
  let slot = null;

  // Frame 1
  assert.equal(shouldStartSyncReplay(slot), true);
  slot = Promise.resolve();
  // Frame 2 (burst — should be coalesced)
  assert.equal(shouldStartSyncReplay(slot), false);
  // Replay settles, finally clears
  slot = null;
  // Frame 3 (new burst window)
  assert.equal(shouldStartSyncReplay(slot), true);
});

// ── Reason-never-rendered contract ──────────────────────────────────────────

test("normaliseSyncRequiredReason + isKnownSyncRequiredReason: unknown reason still replays (gap is real regardless)", () => {
  // The policy contract is: unknown reasons are normalised but the session
  // still replays. The predicate does not gate on the reason at all.
  const reason = normaliseSyncRequiredReason(42);
  assert.equal(reason, undefined);
  assert.equal(isKnownSyncRequiredReason(reason), false);
  // Replay decision is independent of reason:
  assert.equal(shouldStartSyncReplay(null), true);
});

test("normaliseSyncRequiredReason + isKnownSyncRequiredReason: missing reason still replays", () => {
  const reason = normaliseSyncRequiredReason(undefined);
  assert.equal(reason, undefined);
  assert.equal(isKnownSyncRequiredReason(reason), false);
  assert.equal(shouldStartSyncReplay(null), true);
});
