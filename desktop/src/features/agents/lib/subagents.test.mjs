import assert from "node:assert/strict";
import test from "node:test";

import {
  ACTIVE_SUBAGENT_STATUSES,
  activeSubagentCount,
  groupSubagentsByParent,
  subagentIdleLabel,
} from "./subagents.ts";

const PARENT_MACK = "m".repeat(64);
const PARENT_ROSIE = "r".repeat(64);

function subagent(overrides = {}) {
  return {
    id: "sub-1",
    name: "research-worker",
    parentPubkey: PARENT_MACK,
    status: "running",
    lastActiveAt: 1_000,
    ...overrides,
  };
}

test("subagents group under their parent pubkey, first-seen order", () => {
  const worker = subagent({ id: "w1", name: "worker" });
  const coder = subagent({ id: "c1", name: "coder" });
  const rosieChild = subagent({
    id: "r1",
    parentPubkey: PARENT_ROSIE,
  });

  const { byParent, orphans } = groupSubagentsByParent(
    [worker, rosieChild, coder],
    [PARENT_MACK, PARENT_ROSIE],
  );

  assert.deepEqual([...byParent.keys()], [PARENT_MACK, PARENT_ROSIE]);
  assert.deepEqual(
    byParent.get(PARENT_MACK)?.subagents.map((s) => s.id),
    ["w1", "c1"],
  );
  assert.equal(byParent.get(PARENT_ROSIE)?.subagents.length, 1);
  assert.equal(orphans.length, 0);
});

test("parent pubkeys are normalized for matching, on both sides", () => {
  const child = subagent({
    parentPubkey: ` ${PARENT_MACK.toUpperCase()} `,
  });

  const { byParent, orphans } = groupSubagentsByParent(
    [child],
    [` ${PARENT_MACK} `],
  );

  assert.equal(orphans.length, 0);
  assert.ok(byParent.get(PARENT_MACK), "normalized key indexes the group");
  assert.equal(
    byParent.get(PARENT_MACK)?.subagents[0].parentPubkey,
    PARENT_MACK,
    "records carry the normalized parent back out",
  );
});

test("children of an unknown parent surface as orphans, never dropped", () => {
  const orphan = subagent({ parentPubkey: "f".repeat(64) });

  const { byParent, orphans } = groupSubagentsByParent([orphan], [PARENT_MACK]);

  assert.equal(byParent.size, 0);
  assert.deepEqual(
    orphans.map((s) => s.id),
    ["sub-1"],
  );
});

test("active count is spawned + running, excluding complete and failed", () => {
  assert.deepEqual([...ACTIVE_SUBAGENT_STATUSES], ["spawned", "running"]);

  const count = activeSubagentCount([
    subagent({ status: "spawned" }),
    subagent({ status: "running" }),
    subagent({ status: "complete" }),
    subagent({ status: "failed" }),
  ]);
  assert.equal(count, 2);

  const grouped = groupSubagentsByParent(
    [
      subagent({ status: "spawned" }),
      subagent({ status: "running" }),
      subagent({ status: "complete" }),
    ],
    [PARENT_MACK],
  );
  assert.equal(grouped.byParent.get(PARENT_MACK)?.activeCount, 2);
});

test("idle label formats seconds, minutes, and hours, clamped at zero", () => {
  const at = subagent({ lastActiveAt: 10_000 });
  assert.equal(subagentIdleLabel(at, 10_500), "0s");
  assert.equal(subagentIdleLabel(at, 11_000), "1s");
  assert.equal(subagentIdleLabel(at, 70_000), "1m 0s");
  assert.equal(subagentIdleLabel(at, 3_660_000), "1h 0m 50s");
  // Clock skew or event reordering must never render negative idle time.
  assert.equal(subagentIdleLabel(at, 9_000), "0s");
});
