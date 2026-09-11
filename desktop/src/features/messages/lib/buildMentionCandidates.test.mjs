import assert from "node:assert/strict";
import test from "node:test";

import { buildMentionCandidates } from "./buildMentionCandidates.ts";

const MEMBER_PUBKEY = "a".repeat(64);
const AGENT_PUBKEY = "b".repeat(64);
const ARCHIVED_PUBKEY = "c".repeat(64);
const SEARCHED_PUBKEY = "d".repeat(64);

function input(overrides = {}) {
  return {
    activeAgentPubkeys: new Set(),
    activePersonaById: new Map(),
    activePersonas: [],
    canSearchGlobalUsers: false,
    currentPubkey: null,
    isArchived: () => false,
    managedAgentDirectoryReady: true,
    managedAgentNamesByPubkey: new Map(),
    managedAgentPersonaIds: new Set(),
    managedAgentPersonaIdsByPubkey: new Map(),
    managedAgents: [],
    memberPubkeys: new Set(),
    members: [],
    mentionChannelId: null,
    mentionableAgentPubkeys: new Set(),
    personaNameByPubkey: new Map(),
    profiles: undefined,
    relayAgentDirectoryReady: true,
    relayAgentNamesByPubkey: new Map(),
    relayAgents: [],
    userSearchResults: [],
    ...overrides,
  };
}

test("a roster entry and its relay agent record coalesce into one candidate", () => {
  const candidates = buildMentionCandidates(
    input({
      // REG-27: liveness now comes ONLY from the injected set, so the caller
      // must supply it. Previously the relay row re-derived it from the
      // unexpiring directory status.
      activeAgentPubkeys: new Set([AGENT_PUBKEY]),
      members: [
        { pubkey: AGENT_PUBKEY, displayName: null, isAgent: true, role: "bot" },
      ],
      mentionableAgentPubkeys: new Set([AGENT_PUBKEY]),
      relayAgents: [
        {
          pubkey: AGENT_PUBKEY,
          name: "Scout",
          ownerPubkey: null,
          status: "online",
        },
      ],
    }),
  );

  assert.equal(candidates.length, 1);
  assert.equal(candidates[0].pubkey, AGENT_PUBKEY);
  // The roster contributes membership, the directory contributes the name.
  assert.equal(candidates[0].isMember, true);
  assert.equal(candidates[0].displayName, "Scout");
  assert.equal(candidates[0].isActiveAgent, true);
});

test("archived identities never become candidates", () => {
  const candidates = buildMentionCandidates(
    input({
      isArchived: (pubkey) => pubkey === ARCHIVED_PUBKEY,
      members: [
        { pubkey: MEMBER_PUBKEY, displayName: "Ada", isAgent: false },
        { pubkey: ARCHIVED_PUBKEY, displayName: "Gone", isAgent: false },
      ],
    }),
  );

  assert.deepEqual(
    candidates.map((candidate) => candidate.pubkey),
    [MEMBER_PUBKEY],
  );
});

test("an agent outside the mentionable set is hidden once its directory is ready", () => {
  const relayAgents = [
    { pubkey: AGENT_PUBKEY, name: "Scout", ownerPubkey: null, status: "away" },
  ];

  assert.deepEqual(buildMentionCandidates(input({ relayAgents })), []);
  assert.equal(
    buildMentionCandidates(
      input({ mentionableAgentPubkeys: new Set([AGENT_PUBKEY]), relayAgents }),
    ).length,
    1,
  );
});

test("active personas join unless a managed agent already carries them", () => {
  const activePersonas = [
    { id: "planner", displayName: "Planner", avatarUrl: null, isActive: true },
  ];

  const standalone = buildMentionCandidates(input({ activePersonas }));
  assert.equal(standalone.length, 1);
  assert.equal(standalone[0].kind, "persona");
  assert.equal(standalone[0].personaId, "planner");

  assert.deepEqual(
    buildMentionCandidates(
      input({ activePersonas, managedAgentPersonaIds: new Set(["planner"]) }),
    ),
    [],
  );
});

test("global search results join only while global search is enabled", () => {
  const userSearchResults = [
    {
      pubkey: SEARCHED_PUBKEY,
      displayName: "Dana",
      isAgent: false,
      nip05Handle: null,
      ownerPubkey: null,
    },
  ];

  assert.deepEqual(buildMentionCandidates(input({ userSearchResults })), []);

  const searched = buildMentionCandidates(
    input({ canSearchGlobalUsers: true, userSearchResults }),
  );
  assert.equal(searched.length, 1);
  assert.equal(searched[0].displayName, "Dana");
  assert.equal(searched[0].isGlobalSearchResult, true);
});

test("policy-only discovery stays selectable without claiming active presence", () => {
  const [candidate] = buildMentionCandidates(
    input({
      mentionableAgentPubkeys: new Set([AGENT_PUBKEY]),
      relayAgents: [
        {
          pubkey: AGENT_PUBKEY,
          name: "Scout",
          ownerPubkey: MEMBER_PUBKEY,
          status: "unknown",
        },
      ],
    }),
  );
  assert.equal(candidate.pubkey, AGENT_PUBKEY);
  assert.equal(candidate.isActiveAgent, false);
  assert.equal(candidate.ownerPubkey, MEMBER_PUBKEY);
});

for (const locallyManaged of [true, false]) {
  test(`roster candidate preserves exact local management: ${locallyManaged}`, () => {
    const [candidate] = buildMentionCandidates(
      input({
        members: [{ pubkey: AGENT_PUBKEY, displayName: "Scout", role: "bot" }],
        managedAgentNamesByPubkey: new Map(
          locallyManaged ? [[AGENT_PUBKEY, "Scout"]] : [],
        ),
        managedAgents: locallyManaged
          ? [{ pubkey: AGENT_PUBKEY, name: "Scout", status: "deployed" }]
          : [],
        mentionableAgentPubkeys: new Set([AGENT_PUBKEY]),
      }),
    );
    assert.equal(candidate.isMember, true);
    assert.equal(Boolean(candidate.isManagedAgent), locallyManaged);
  });
}

// REG-27: the relay-agent row must honour the injected set, not the directory.
test("relay-agent row honours activeAgentPubkeys, not the directory status", () => {
  const relayAgents = [
    {
      pubkey: AGENT_PUBKEY,
      name: "Scout",
      ownerPubkey: null,
      status: "online",
    },
  ];
  const base = {
    mentionableAgentPubkeys: new Set([AGENT_PUBKEY]),
    relayAgents,
  };

  // Directory says online but presence demoted it => set-absent => INACTIVE.
  const [ghost] = buildMentionCandidates(input(base));
  assert.equal(ghost.pubkey, AGENT_PUBKEY);
  assert.equal(ghost.isActiveAgent, false);

  // Directory says offline but presence says online => set-present => ACTIVE.
  const [live] = buildMentionCandidates(
    input({
      ...base,
      activeAgentPubkeys: new Set([AGENT_PUBKEY]),
      relayAgents: [{ ...relayAgents[0], status: "offline" }],
    }),
  );
  assert.equal(live.isActiveAgent, true);
});

test("E9 managed-agent classification is unchanged by the relay presence join", () => {
  for (const [status, expected] of [
    ["running", true],
    ["deployed", true],
    ["stopped", false],
  ]) {
    const [candidate] = buildMentionCandidates(
      input({
        managedAgentNamesByPubkey: new Map([[AGENT_PUBKEY, "Scout"]]),
        managedAgents: [{ pubkey: AGENT_PUBKEY, name: "Scout", status }],
        mentionableAgentPubkeys: new Set([AGENT_PUBKEY]),
      }),
    );
    assert.equal(candidate.isActiveAgent, expected, status);
  }
});
