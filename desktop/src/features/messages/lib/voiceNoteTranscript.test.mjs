import assert from "node:assert/strict";
import test from "node:test";

// Imports the exact source the timeline renderer uses. No inlined copy → no
// drift risk between test expectations and production behaviour.
import {
  hasAudioAttachment,
  MAX_TRANSCRIPT_LENGTH,
  resolveVoiceNoteTranscripts,
} from "./voiceNoteTranscript.mjs";

const NOTE = "note-1";
const CHANNEL = "channel-a";
const channelOfNote = (id) => (id === NOTE ? CHANNEL : undefined);

const transcript = (overrides = {}) => ({
  id: "t1",
  kind: 40009,
  pubkey: "pk",
  content: "hello there",
  created_at: 100,
  tags: [
    ["e", NOTE, "", "mention"],
    ["h", CHANNEL],
  ],
  ...overrides,
});

test("resolves a transcript onto the voice note it anchors", () => {
  const got = resolveVoiceNoteTranscripts([transcript()], channelOfNote);
  assert.equal(got.size, 1);
  assert.deepEqual(got.get(NOTE), {
    id: "t1",
    created_at: 100,
    pubkey: "pk",
    text: "hello there",
  });
});

test("ignores non-transcript kinds in a mixed feed", () => {
  const got = resolveVoiceNoteTranscripts(
    [transcript({ kind: 9 }), transcript({ id: "t2" })],
    channelOfNote,
  );
  assert.equal(got.size, 1);
  assert.equal(got.get(NOTE).id, "t2");
});

test("empty and undefined input yield no transcripts", () => {
  assert.equal(resolveVoiceNoteTranscripts([], channelOfNote).size, 0);
  assert.equal(resolveVoiceNoteTranscripts(undefined, channelOfNote).size, 0);
});

// --- first-writer-wins ------------------------------------------------------

test("earliest created_at wins regardless of arrival order", () => {
  const late = transcript({ id: "late", created_at: 200, content: "second" });
  const early = transcript({ id: "early", created_at: 100, content: "first" });
  for (const order of [
    [late, early],
    [early, late],
  ]) {
    const got = resolveVoiceNoteTranscripts(order, channelOfNote);
    assert.equal(got.get(NOTE).text, "first");
  }
});

test("concurrent writers at the same timestamp converge on the smaller id", () => {
  const a = transcript({ id: "aaa", created_at: 100, content: "A" });
  const b = transcript({ id: "bbb", created_at: 100, content: "B" });
  assert.equal(
    resolveVoiceNoteTranscripts([a, b], channelOfNote).get(NOTE).text,
    "A",
  );
  assert.equal(
    resolveVoiceNoteTranscripts([b, a], channelOfNote).get(NOTE).text,
    "A",
  );
});

// --- authz ------------------------------------------------------------------

test("transcript signed in a different channel than the note is denied", () => {
  const foreign = transcript({
    tags: [
      ["e", NOTE, "", "mention"],
      ["h", "channel-elsewhere"],
    ],
  });
  assert.equal(resolveVoiceNoteTranscripts([foreign], channelOfNote).size, 0);
});

test("a denied transcript does not block a later legitimate one", () => {
  const foreign = transcript({
    id: "foreign",
    created_at: 1,
    tags: [
      ["e", NOTE, "", "mention"],
      ["h", "channel-elsewhere"],
    ],
  });
  const legit = transcript({ id: "legit", created_at: 50, content: "real" });
  const got = resolveVoiceNoteTranscripts([foreign, legit], channelOfNote);
  assert.equal(got.get(NOTE).text, "real");
});

test("transcript anchored to an unknown/unloaded note is dropped", () => {
  const dangling = transcript({
    tags: [
      ["e", "note-we-never-loaded", "", "mention"],
      ["h", CHANNEL],
    ],
  });
  assert.equal(resolveVoiceNoteTranscripts([dangling], channelOfNote).size, 0);
});

// --- malformed data ---------------------------------------------------------

test("missing, empty, or unmarked anchor tags are dropped", () => {
  const cases = [
    { tags: [["h", CHANNEL]] },
    {
      tags: [
        ["e", "", "", "mention"],
        ["h", CHANNEL],
      ],
    },
    {
      tags: [
        ["e", NOTE],
        ["h", CHANNEL],
      ],
    }, // no "mention" marker
    {
      tags: [
        ["e", NOTE, "", "reply"],
        ["h", CHANNEL],
      ],
    },
    { tags: undefined },
    { tags: [["e", NOTE, "", "mention"]] }, // no channel scope
  ];
  for (const override of cases) {
    assert.equal(
      resolveVoiceNoteTranscripts([transcript(override)], channelOfNote).size,
      0,
      `expected drop for ${JSON.stringify(override.tags)}`,
    );
  }
});

test("two disagreeing anchors fail closed rather than picking one", () => {
  const ambiguous = transcript({
    tags: [
      ["e", NOTE, "", "mention"],
      ["e", "note-2", "", "mention"],
      ["h", CHANNEL],
    ],
  });
  assert.equal(resolveVoiceNoteTranscripts([ambiguous], channelOfNote).size, 0);
});

test("a repeated identical anchor is not treated as a conflict", () => {
  const repeated = transcript({
    tags: [
      ["e", NOTE, "", "mention"],
      ["e", NOTE, "", "mention"],
      ["h", CHANNEL],
    ],
  });
  assert.equal(resolveVoiceNoteTranscripts([repeated], channelOfNote).size, 1);
});

test("malformed events (bad id, bad timestamp, junk tags) are skipped", () => {
  const events = [
    transcript({ id: "" }),
    transcript({ id: 7 }),
    transcript({ created_at: "100" }),
    transcript({ created_at: Number.NaN }),
    transcript({ tags: ["not-a-tag", null, 3] }),
    null,
    undefined,
  ];
  assert.equal(resolveVoiceNoteTranscripts(events, channelOfNote).size, 0);
});

test("a blank transcript is dropped so it cannot lock out a real one", () => {
  const blank = transcript({ id: "blank", created_at: 1, content: "   \n\t " });
  const real = transcript({
    id: "real",
    created_at: 500,
    content: "actual words",
  });
  const got = resolveVoiceNoteTranscripts([blank, real], channelOfNote);
  assert.equal(got.get(NOTE).text, "actual words");
});

test("missing content is dropped, not rendered as an empty transcript", () => {
  assert.equal(
    resolveVoiceNoteTranscripts(
      [transcript({ content: undefined })],
      channelOfNote,
    ).size,
    0,
  );
});

test("oversized transcripts are clamped instead of rejected", () => {
  const long = transcript({ content: "x".repeat(MAX_TRANSCRIPT_LENGTH + 500) });
  const got = resolveVoiceNoteTranscripts([long], channelOfNote);
  assert.equal(got.get(NOTE).text.length, MAX_TRANSCRIPT_LENGTH);
});

test("hasAudioAttachment tolerates absent and malformed tags", () => {
  assert.equal(hasAudioAttachment(undefined), false);
  assert.equal(hasAudioAttachment([]), false);
  assert.equal(hasAudioAttachment("not-an-array"), false);
  assert.equal(hasAudioAttachment([["imeta"]]), false);
  assert.equal(
    hasAudioAttachment([
      ["imeta", "url"],
      ["h", "chan"],
    ]),
    false,
  );
  assert.equal(hasAudioAttachment([[null, undefined]]), false);
});

test("hasAudioAttachment recognises an audio imeta attachment", () => {
  assert.equal(
    hasAudioAttachment([
      ["h", "chan"],
      [
        "imeta",
        "url https://blossom.example/voice-note-1.wav",
        "m audio/wav",
        `x ${"c".repeat(64)}`,
      ],
    ]),
    true,
  );
});
