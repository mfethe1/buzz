import assert from "node:assert/strict";
import test from "node:test";

import {
  agentDisplayGroupKey,
  groupAgentsForDisplay,
} from "./agentIdentity.ts";

const PARENT_A = "a".repeat(64);
const PARENT_B = "b".repeat(64);

test("the display group key nests by parent pubkey FIRST, persona second", () => {
  // Same persona, same name, different parents → different groups: a
  // subagent always lands under its parent, never on a sibling card of the
  // persona it happens to share (SPEC-nested-subagents B1).
  const childOfA = {
    pubkey: "c".repeat(64),
    name: "research-worker",
    personaId: "builtin:fizz",
    parentPubkey: PARENT_A,
  };
  const childOfB = {
    pubkey: "d".repeat(64),
    name: "research-worker",
    personaId: "builtin:fizz",
    parentPubkey: PARENT_B,
  };

  assert.notEqual(
    agentDisplayGroupKey(childOfA),
    agentDisplayGroupKey(childOfB),
  );
});

test("the persona segment is the tiebreaker AFTER parent, per the spec order", () => {
  // "parent-pubkey FIRST, persona second": the parent segment never merges
  // agents with different parents (test above), and within one parent the
  // persona still disambiguates — same parent + same name but different
  // personas stay separate groups, exactly as they did before nesting.
  const childOfPersonaA = {
    pubkey: "1".repeat(64),
    name: "worker",
    personaId: "builtin:aaa",
    parentPubkey: PARENT_A,
  };
  const childOfPersonaB = {
    pubkey: "2".repeat(64),
    name: "worker",
    personaId: "builtin:zzz",
    parentPubkey: PARENT_A,
  };
  const samePersonaSibling = {
    pubkey: "3".repeat(64),
    name: "Worker",
    personaId: "builtin:aaa",
    parentPubkey: PARENT_A,
  };

  assert.notEqual(
    agentDisplayGroupKey(childOfPersonaA),
    agentDisplayGroupKey(childOfPersonaB),
    "persona still splits groups within one parent",
  );
  assert.equal(
    agentDisplayGroupKey(childOfPersonaA),
    agentDisplayGroupKey(samePersonaSibling),
    "same parent + persona + folded name still share one group",
  );
});

test("the parent segment is normalized, so case or padding cannot split a nest", () => {
  const lower = {
    pubkey: "1".repeat(64),
    name: "worker",
    personaId: "builtin:fizz",
    parentPubkey: PARENT_A,
  };
  const upperPadded = {
    pubkey: "2".repeat(64),
    name: "worker",
    personaId: "builtin:fizz",
    parentPubkey: ` ${PARENT_A.toUpperCase()} `,
  };

  assert.equal(agentDisplayGroupKey(lower), agentDisplayGroupKey(upperPadded));
});

test("agents without a parent keep the established grouping semantics", () => {
  // The key change must be purely additive for top-level agents: absent
  // parentPubkey behaves exactly as before this field existed.
  const withParentField = {
    pubkey: "1".repeat(64),
    name: "Fizz",
    personaId: "builtin:fizz",
  };
  const withoutParentField = {
    pubkey: "2".repeat(64),
    name: "fizz",
    personaId: "builtin:fizz",
    parentPubkey: null,
  };
  const undefinedParent = {
    pubkey: "3".repeat(64),
    name: " FIZZ ",
    personaId: "builtin:fizz",
    parentPubkey: undefined,
  };

  assert.equal(
    agentDisplayGroupKey(withParentField),
    agentDisplayGroupKey(withoutParentField),
  );
  assert.equal(
    agentDisplayGroupKey(withParentField),
    agentDisplayGroupKey(undefinedParent),
  );
});

test("a subagent never merges onto a parentless card, and never the reverse", () => {
  const parent = {
    pubkey: PARENT_A,
    name: "research",
    personaId: "builtin:fizz",
  };
  const child = {
    pubkey: "e".repeat(64),
    name: "research",
    personaId: "builtin:fizz",
    parentPubkey: PARENT_A,
  };

  assert.notEqual(agentDisplayGroupKey(parent), agentDisplayGroupKey(child));

  const groups = groupAgentsForDisplay([parent, child]);
  assert.equal(groups.length, 2, "parent and child render distinct cards");
  assert.deepEqual(
    groups.map((group) => group.name),
    ["research", "research"],
  );
});

test("the parent segment cannot be forged by a name or persona containing a separator", () => {
  // Length-prefixed segments: no shifted boundary can make one agent's key
  // equal another's (same invariant the persona segment already carries).
  assert.notEqual(
    agentDisplayGroupKey({
      parentPubkey: PARENT_A,
      personaId: "p",
      name: "x|parent:0:|persona:0:",
    }),
    agentDisplayGroupKey({
      parentPubkey: `${PARENT_A}|parent:0:|persona:0:|name:x`,
      personaId: "p2",
      name: "",
    }),
  );
});
