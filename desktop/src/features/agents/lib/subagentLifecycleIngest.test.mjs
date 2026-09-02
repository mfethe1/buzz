/**
 * Tests for pure subagent lifecycle ingestion (SPEC-nested-subagents).
 * Alias-free module → plain `node --test` runs it, matching repo convention
 * (see lib/subagents.test.mjs).
 */

import assert from "node:assert/strict";
import test from "node:test";

import {
  foldSubagentLifecycle,
  parseSubagentLifecyclePayload,
} from "./subagentLifecycleIngest.ts";

const PARENT = "p".repeat(64);
const T0 = Date.parse("2026-09-02T18:00:00.000Z");
const T1 = Date.parse("2026-09-02T18:00:01.000Z");

function record(overrides = {}) {
  return {
    id: `${PARENT}:w`,
    name: "w",
    parentPubkey: PARENT,
    status: "running",
    lastActiveAt: T0,
    ...overrides,
  };
}

test("parse accepts the wire payload and rejects malformed", () => {
  assert.deepEqual(
    parseSubagentLifecyclePayload({
      subagent_name: "research-worker",
      status: "spawned",
    }),
    { subagent_name: "research-worker", status: "spawned", summary: undefined },
  );

  for (const bad of [
    null,
    "x",
    {},
    { subagent_name: "", status: "running" },
    { subagent_name: "w" },
    { subagent_name: "w", status: "bogus" },
    { status: "running" },
    { subagent_name: 7, status: "running" },
  ]) {
    assert.equal(
      parseSubagentLifecyclePayload(bad),
      null,
      String(JSON.stringify(bad)),
    );
  }
});

test("fold adds a new subagent and replaces on same-name respawn", () => {
  const first = foldSubagentLifecycle(
    [],
    PARENT,
    {
      subagent_name: "w",
      status: "spawned",
    },
    T0,
  );
  assert.ok(first);
  assert.equal(first.length, 1);
  assert.equal(first[0].status, "spawned");

  const second = foldSubagentLifecycle(
    first,
    PARENT,
    {
      subagent_name: "w",
      status: "running",
    },
    T1,
  );
  assert.ok(second);
  assert.equal(second.length, 1);
  assert.equal(second[0].status, "running");
  assert.equal(second[0].lastActiveAt, T1);
});

test("fold returns null on no-op duplicates", () => {
  const list = [record()];
  assert.equal(
    foldSubagentLifecycle(
      list,
      PARENT,
      {
        subagent_name: "w",
        status: "running",
      },
      T0,
    ),
    null,
  );
  // Older-or-equal timestamp with same status is still a no-op.
  assert.equal(
    foldSubagentLifecycle(
      list,
      PARENT,
      {
        subagent_name: "w",
        status: "running",
      },
      T0 - 5_000,
    ),
    null,
  );
});

test("fold keeps distinct subagents as separate rows", () => {
  const a = foldSubagentLifecycle(
    [],
    PARENT,
    { subagent_name: "a", status: "running" },
    T0,
  );
  const b = foldSubagentLifecycle(
    a ?? [],
    PARENT,
    { subagent_name: "b", status: "running" },
    T0,
  );
  assert.ok(b);
  assert.equal(b.length, 2);
  assert.deepEqual(b.map((s) => s.name).sort(), ["a", "b"]);
});

test("summary updates and terminal statuses flow through", () => {
  const list = [record()];
  const done = foldSubagentLifecycle(
    list,
    PARENT,
    {
      subagent_name: "w",
      status: "complete",
      summary: "shipped",
    },
    T1,
  );
  assert.ok(done);
  assert.equal(done[0].status, "complete");
  assert.equal(done[0].summary, "shipped");
});
