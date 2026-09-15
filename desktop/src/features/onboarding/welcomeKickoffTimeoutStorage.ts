/**
 * Persistent latch for Welcome channels whose kickoff exhausted its window
 * (#50). Without it, every app restart replays the 90s "Setting up your
 * welcome team…" stage for a team that already proved it is not coming.
 *
 * Keyed by channel id in one app-level store; entries are pruned oldest-first
 * once the cap is exceeded. Same shape discipline as channelMutesStorage:
 * parse defensively, never throw on corrupt payloads.
 */
const STORAGE_KEY = "buzz-welcome-kickoff-timeouts.v1";
export const MAX_WELCOME_KICKOFF_TIMEOUT_ENTRIES = 50;

export type WelcomeKickoffTimeoutStore = {
  version: 1;
  /** channelId -> unix ms when the kickoff window expired. */
  channels: Record<string, number>;
};

export const DEFAULT_TIMEOUT_STORE: WelcomeKickoffTimeoutStore = Object.freeze({
  version: 1,
  channels: {},
});

export function parseTimeoutPayload(
  json: unknown,
): WelcomeKickoffTimeoutStore | null {
  if (typeof json !== "object" || json === null) return null;
  const obj = json as Record<string, unknown>;
  if (obj.version !== 1) return null;
  if (
    typeof obj.channels !== "object" ||
    obj.channels === null ||
    Array.isArray(obj.channels)
  ) {
    return null;
  }
  const channels: Record<string, number> = Object.fromEntries(
    Object.entries(obj.channels as Record<string, unknown>).filter(
      (entry): entry is [string, number] =>
        typeof entry[1] === "number" &&
        Number.isFinite(entry[1]) &&
        entry[1] >= 0,
    ),
  );
  return { version: 1, channels };
}

export function pruneTimeoutStore(
  store: WelcomeKickoffTimeoutStore,
  maxEntries = MAX_WELCOME_KICKOFF_TIMEOUT_ENTRIES,
): WelcomeKickoffTimeoutStore {
  const entries = Object.entries(store.channels);
  if (entries.length <= maxEntries) return store;
  const sorted = entries.sort((a, b) => b[1] - a[1]).slice(0, maxEntries);
  return { version: 1, channels: Object.fromEntries(sorted) };
}

export function readTimeoutStore(
  storage: Pick<Storage, "getItem">,
): WelcomeKickoffTimeoutStore {
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (raw === null) return DEFAULT_TIMEOUT_STORE;
    return parseTimeoutPayload(JSON.parse(raw)) ?? DEFAULT_TIMEOUT_STORE;
  } catch {
    return DEFAULT_TIMEOUT_STORE;
  }
}

export function isChannelKickoffTimedOut(
  storage: Pick<Storage, "getItem">,
  channelId: string | null,
): boolean {
  if (!channelId) return false;
  return channelId in readTimeoutStore(storage).channels;
}

export function latchChannelKickoffTimeout(
  storage: Pick<Storage, "getItem" | "setItem">,
  channelId: string,
  now: number,
): void {
  try {
    const store = readTimeoutStore(storage);
    if (channelId in store.channels) return;
    const next = pruneTimeoutStore({
      version: 1,
      channels: { ...store.channels, [channelId]: now },
    });
    storage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // Storage unavailable (private mode, quota) — the latch is best-effort;
    // the in-memory timeout still resolves the stage for this run.
  }
}
