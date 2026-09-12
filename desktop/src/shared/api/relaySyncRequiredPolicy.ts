/**
 * Pure policy module for the relay's `BUZZ_SYNC_REQUIRED` gap frame.
 *
 * The relay emits `["BUZZ_SYNC_REQUIRED","<reason>"]` on the priority control
 * channel when it must drop a fan-out EVENT due to a full per-connection buffer.
 * The frame is **best-effort** (the relay itself can drop it when `ctrl_tx` is
 * full), so this module treats it as an *opportunistic accelerator* — never a
 * guaranteed gap notification. The reconnect-replay path remains the backstop.
 *
 * This module is pure: no I/O, no `window`, no session state. The session layer
 * owns the coalescing promise and the replay call; this module only normalises
 * the frame and provides the coalescing predicate.
 *
 * Mirrors the shipped mobile handler at `relay_session.dart:615-641`.
 */

/** Reason strings we recognise and will log. Anything else replays but logs as `<unknown>`. */
export const SYNC_REQUIRED_KNOWN_REASONS = new Set(["backpressure"]);

/**
 * Normalise the reason field from a `BUZZ_SYNC_REQUIRED` frame.
 *
 * The `reason` string is relay-supplied / attacker-influenced text. Mobile
 * allowlists it and **never renders** it. Unknown or non-string reasons are
 * normalised to `undefined`; callers log them as `"<unknown>"` and still
 * replay (the gap is real regardless of the label).
 */
export function normaliseSyncRequiredReason(
  reason: unknown,
): string | undefined {
  return typeof reason === "string" ? reason : undefined;
}

/**
 * Whether this reason should be included in debug logs.
 *
 * Only allowlisted reasons are logged by name; everything else is logged as
 * `"<unknown>"` to avoid echoing attacker-controllable text into log streams.
 */
export function isKnownSyncRequiredReason(reason: string | undefined): boolean {
  return reason !== undefined && SYNC_REQUIRED_KNOWN_REASONS.has(reason);
}

/**
 * Coalescing predicate: should the session start a new sync replay?
 *
 * Returns `false` when a replay is already in-flight (`inFlightReplay !== null`),
 * mirroring mobile's `_syncReplayScheduled` burst gate. The session clears the
 * slot in a `finally` on both settle paths, so the next burst in a new idle
 * window always proceeds.
 */
export function shouldStartSyncReplay(
  inFlightReplay: Promise<void> | null,
): boolean {
  return inFlightReplay === null;
}
