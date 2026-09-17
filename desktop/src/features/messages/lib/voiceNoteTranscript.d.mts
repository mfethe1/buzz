/**
 * Type declarations for the pure resolver in `voiceNoteTranscript.mjs`.
 * Runtime lives in `.mjs` so the (TS-loader-less) `node:test` runner can
 * import it directly; this file gives TypeScript callers a typed view.
 */

export type TranscriptEvent = {
  id: string;
  kind: number;
  pubkey?: string;
  content?: string;
  created_at: number;
  tags?: string[][];
};

export type ResolvedTranscript = {
  /** Event id of the winning transcript. */
  id: string;
  created_at: number;
  pubkey: string;
  /** Trimmed, length-clamped transcript text. */
  text: string;
};

export const MAX_TRANSCRIPT_LENGTH: number;

/**
 * Resolve `kind:40009` transcript overlays, first-writer-wins per anchored
 * voice-note id. Transcripts whose `h` tag does not match the channel of the
 * note they claim to transcribe are dropped.
 */
export function resolveVoiceNoteTranscripts(
  events: readonly TranscriptEvent[] | undefined,
  voiceNoteChannel: (voiceNoteId: string) => string | undefined,
): Map<string, ResolvedTranscript>;
