// #55 fleet audit: join of relay directory (owner-attested origin) with local
// managed-agent state (run location + activity).
import assert from "node:assert/strict";
import { test } from "node:test";

import { buildFleetAuditRows, formatFirstSeen, shortId } from "./fleetAudit.ts";

const relayAgent = (overrides = {}) => ({
  pubkey: "a".repeat(64),
  ownerPubkey: "b".repeat(64),
  name: "Canary",
  agentType: "agent",
  channels: [],
  channelIds: ["chan-1"],
  capabilities: [],
  status: "unknown",
  respondTo: null,
  respondToAllowlist: [],
  deviceId: "0123456789abcdef0123456789abcdef",
  deviceLabel: "winnie-desktop",
  firstSeen: 1_700_000_000,
  ...overrides,
});

const managedAgent = (overrides = {}) => ({
  pubkey: "a".repeat(64),
  name: "Canary",
  status: "running",
  lastStartedAt: "2026-09-14T20:00:00.000Z",
  backend: { type: "local" },
  ...overrides,
});

test("joins relay origin with local run state on matching pubkey", () => {
  const rows = buildFleetAuditRows(
    [relayAgent()],
    [managedAgent()],
    "b".repeat(64),
  );
  assert.equal(rows.length, 1);
  const row = rows[0];
  assert.equal(row.deviceLabel, "winnie-desktop");
  assert.equal(row.firstSeen, 1_700_000_000);
  assert.equal(row.ownedByViewer, true);
  assert.equal(row.runsHere, true);
  assert.equal(row.localStatus, "running");
  assert.equal(row.backend, "local");
  assert.equal(row.channelIds.length, 1);
});

test("relay-only agent appears with runsHere=false and owner compare is case-insensitive", () => {
  const rows = buildFleetAuditRows(
    [relayAgent({ ownerPubkey: "B".repeat(64) })],
    [],
    "b".repeat(64),
  );
  assert.equal(rows.length, 1);
  assert.equal(rows[0].runsHere, false);
  assert.equal(rows[0].localStatus, null);
  assert.equal(rows[0].ownedByViewer, true);
});

test("local-only agent appears with null origin and viewer as owner", () => {
  const rows = buildFleetAuditRows([], [managedAgent()], "b".repeat(64));
  assert.equal(rows.length, 1);
  const row = rows[0];
  assert.equal(row.runsHere, true);
  assert.equal(row.deviceLabel, null);
  assert.equal(row.firstSeen, null);
  assert.equal(row.ownerPubkey, "b".repeat(64));
});

test("local agents sort first, then by name", () => {
  const rows = buildFleetAuditRows(
    [
      relayAgent({ pubkey: "c".repeat(64), name: "Zeta remote" }),
      relayAgent({ pubkey: "d".repeat(64), name: "Beta remote" }),
    ],
    [managedAgent({ pubkey: "e".repeat(64), name: "Alpha local" })],
    null,
  );
  assert.deepEqual(
    rows.map((row) => row.name),
    ["Alpha local", "Beta remote", "Zeta remote"],
  );
});

test("viewer null means ownedByViewer false, never a crash", () => {
  const rows = buildFleetAuditRows([relayAgent()], [], null);
  assert.equal(rows[0].ownedByViewer, false);
});

test("provider backend surfaces its id in the run-location column", () => {
  const rows = buildFleetAuditRows(
    [relayAgent()],
    [
      managedAgent({
        backend: { type: "provider", id: "railway", config: {} },
      }),
    ],
    null,
  );
  assert.equal(rows[0].backend, "railway");
});

test("formatting helpers degrade gracefully", () => {
  assert.equal(formatFirstSeen(null), "—");
  assert.equal(formatFirstSeen(Number.NaN), "—");
  assert.match(formatFirstSeen(1_700_000_000), /\d{4}/);
  assert.equal(shortId(null), "—");
  assert.equal(shortId("abcdef123456").length, 9);
});
