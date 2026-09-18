/**
 * Resolve `kind:40009` voice-note transcript overlays onto the voice notes they
 * describe.
 *
 * A voice note is a signed event: its `imeta alt` cannot be amended after the
 * fact, and the desktop edit path deliberately drops `alt` on reload
 * (`imetaMediaMarkdown.ts`), so a transcript can never live on the note itself.
 * It arrives instead as a separate signed event anchored by
 * `["e", <voice-note-id>, "", "mention"]` and scoped by `["h", <channel>]`.
 *
 * Because signed events cannot be amended, a second transcript for the same
 * note is a competing claim, not a correction: resolution is FIRST-WRITER-WINS
 * by `created_at`, ties broken by the lexicographically smallest event id so
 * every client converges on the same transcript regardless of arrival order.
 *
 * Runtime lives in `.mjs` so the `node:test` runner imports the same source
 * production uses; `voiceNoteTranscript.d.mts` types it for callers.
 */

import { KIND_VOICE_NOTE_TRANSCRIPT } from "../../../shared/constants/kinds.ts";
import { parseImetaTags } from "../../../shared/ui/markdown/parseImeta.ts";
import { isAudioAttachment } from "./audioAttachment.ts";

/**
 * Defensive read-side ceiling on rendered transcript text. There is no
 * first-party publisher yet, so this cannot be described as "matching" one: a
 * transcript is an event signed by an arbitrary transcriber, and the renderer
 * is responsible for bounding what an unbounded `content` can do to a row.
 */
export const MAX_TRANSCRIPT_LENGTH = 8000;

const ANCHOR_MARKER = "mention";

/**
 * True when an event carries at least one audio/voice-note NIP-92 attachment.
 *
 * Anchor eligibility, not decoration: a transcript is only meaningful for audio,
 * and without this any channel member could sign a `kind:40009` anchored to
 * somebody else's plain-text message and have their own text rendered inside
 * that author's row. Reuses `isAudioAttachment` so "what counts as audio" has a
 * single definition shared with the player.
 */
export function hasAudioAttachment(tags) {
  if (!Array.isArray(tags)) return false;
  for (const entry of parseImetaTags(tags).values()) {
    if (isAudioAttachment(entry)) return true;
  }
  return false;
}

/**
 * The anchor is the first `e` tag carrying the `mention` marker. A transcript
 * with several `e` tags (a client that also threads it as a reply) still has
 * exactly one transcript-of target; anything else is malformed and dropped
 * rather than guessed at.
 */
function anchorId(tags) {
  if (!Array.isArray(tags)) return undefined;
  let found;
  for (const tag of tags) {
    if (!Array.isArray(tag)) continue;
    if (tag[0] !== "e" || tag[3] !== ANCHOR_MARKER) continue;
    const id = typeof tag[1] === "string" ? tag[1] : "";
    if (!id) return undefined;
    // Two disagreeing anchors: we cannot tell which note is meant. Fail closed.
    if (found !== undefined && found !== id) return undefined;
    found = id;
  }
  return found;
}

function channelOf(tags) {
  if (!Array.isArray(tags)) return undefined;
  for (const tag of tags) {
    if (
      Array.isArray(tag) &&
      tag[0] === "h" &&
      typeof tag[1] === "string" &&
      tag[1]
    ) {
      return tag[1];
    }
  }
  return undefined;
}

/** Earlier wins; identical timestamps are broken by event id, never by order. */
function winsOver(candidate, incumbent) {
  if (candidate.created_at !== incumbent.created_at) {
    return candidate.created_at < incumbent.created_at;
  }
  return candidate.id < incumbent.id;
}

/**
 * @param events transcript events (callers may pass a mixed feed; non-40009
 *   kinds are ignored so this can be dropped straight onto a timeline slice).
 * @param voiceNoteChannel resolves a voice-note id to the channel it was posted
 *   in. A transcript is only accepted when its own `h` tag matches, so an event
 *   signed in another channel cannot inject text under a note it cannot read.
 *   Unknown ids are dropped: we do not attach transcripts to notes we have not
 *   loaded.
 */
export function resolveVoiceNoteTranscripts(events, voiceNoteChannel) {
  const winners = new Map();
  if (!Array.isArray(events)) return winners;

  for (const event of events) {
    if (event?.kind !== KIND_VOICE_NOTE_TRANSCRIPT) continue;
    if (typeof event.id !== "string" || !event.id) continue;
    if (
      typeof event.created_at !== "number" ||
      !Number.isFinite(event.created_at)
    )
      continue;

    const noteId = anchorId(event.tags);
    if (!noteId) continue;

    const expected = voiceNoteChannel(noteId);
    if (!expected || channelOf(event.tags) !== expected) continue;

    const text = typeof event.content === "string" ? event.content.trim() : "";
    // A blank transcript is indistinguishable from "not transcribed yet" to a
    // reader, but it would permanently lock out a later real one under
    // first-writer-wins. Drop it instead of enshrining silence.
    if (!text) continue;

    const candidate = {
      id: event.id,
      created_at: event.created_at,
      pubkey: typeof event.pubkey === "string" ? event.pubkey : "",
      text: text.slice(0, MAX_TRANSCRIPT_LENGTH),
    };

    const incumbent = winners.get(noteId);
    if (!incumbent || winsOver(candidate, incumbent)) {
      winners.set(noteId, candidate);
    }
  }

  return winners;
}
