/**
 * Per-community brand color, as advertised in the relay's NIP-11 document.
 *
 * The relay serves `buzz_brand_color` as a host-scoped scalar (see
 * `crates/buzz-relay/src/nip11.rs::workspace_brand_color_for_host`): the value
 * belongs to the community bound to the requesting host and can never be
 * another tenant's. It is readable *pre-auth*, which is the whole point — the
 * brand must be legible on the login/connect screen, before any session exists.
 *
 * Trust boundary: the relay validates the value as a `#rrggbb` triplet at the
 * kind:9033 write path, but a client must never rely on a server-side check for
 * a value it is about to inject into CSS. `parseBrandColor` re-validates with
 * the identical rule, and every consumer goes through it.
 */

/** A validated `#rrggbb` brand color. */
export type BrandColor = string;

const BRAND_COLOR_PATTERN = /^#[0-9a-fA-F]{6}$/;

/** The NIP-11 fields this module consumes. Structurally typed: the document
 * carries many more keys, and unknown keys are ignored rather than rejected. */
export type RelayBrandInfo = {
  buzz_brand_color?: unknown;
};

type BrandColorRoot = {
  style: {
    setProperty(k: string, v: string): void;
    removeProperty(k: string): void;
  };
};

type RelayBrandResponse = {
  ok: boolean;
  json(): Promise<unknown>;
};

type RelayBrandFetch = (
  input: URL,
  init: { headers: { Accept: string }; signal?: AbortSignal },
) => Promise<RelayBrandResponse>;

type RelayBrandFetchOptions = {
  fetchImpl?: RelayBrandFetch;
  signal?: AbortSignal;
};

/**
 * Returns the relay's advertised brand color, or `null` when absent, cleared,
 * or not a well-formed `#rrggbb` literal.
 *
 * Mirrors the relay's `validate_brand_color` exactly — no named colors, no
 * `rgb()`, no alpha, no 3-digit shorthand. Anything else degrades to `null`
 * (fall back to the stock theme) rather than throwing: a malformed brand color
 * must never prevent a user from reaching a usable, correctly-themed client.
 * This is the same validate-or-None rule the agent directory applies to
 * device labels.
 */
export function parseBrandColor(
  info: RelayBrandInfo | null | undefined,
): BrandColor | null {
  const raw = info?.buzz_brand_color;
  if (typeof raw !== "string") return null;
  if (!BRAND_COLOR_PATTERN.test(raw)) return null;
  return raw;
}

/** The CSS custom property the brand color is exposed as. */
export const BRAND_COLOR_CSS_VAR = "--buzz-brand-color";

/**
 * Converts a community relay URL to its host-equivalent NIP-11 `/info` URL.
 *
 * Communities are stored as ws(s) relay URLs in the desktop state. NIP-11 is
 * served over HTTP(S) on the same host, pre-auth. Query strings/fragments from
 * the relay URL are intentionally discarded; `/info` is a fixed document.
 */
export function relayInfoUrlFromRelayUrl(
  relayUrl: string | null | undefined,
): URL | null {
  if (!relayUrl) return null;

  try {
    const infoUrl = new URL(relayUrl);
    if (infoUrl.protocol === "wss:") {
      infoUrl.protocol = "https:";
    } else if (infoUrl.protocol === "ws:") {
      infoUrl.protocol = "http:";
    } else if (infoUrl.protocol !== "http:" && infoUrl.protocol !== "https:") {
      return null;
    }
    infoUrl.pathname = "/info";
    infoUrl.search = "";
    infoUrl.hash = "";
    return infoUrl;
  } catch {
    return null;
  }
}

/**
 * Applies (or clears) the brand color on a root element as a CSS custom
 * property.
 *
 * Deliberately a *custom property* rather than a direct style write: it
 * composes under the existing theme and identity-variant layers instead of
 * fighting them, so a relay with no brand color renders byte-identically to
 * today. Clearing on `null` is load-bearing — switching from a branded
 * community to an unbranded one must not leave the previous tenant's color
 * behind.
 */
export function applyBrandColor(
  root: BrandColorRoot,
  color: BrandColor | null,
): void {
  if (color === null) {
    root.style.removeProperty(BRAND_COLOR_CSS_VAR);
    return;
  }
  root.style.setProperty(BRAND_COLOR_CSS_VAR, color);
}

export async function fetchRelayBrandColor(
  relayUrl: string | null | undefined,
  { fetchImpl = globalThis.fetch, signal }: RelayBrandFetchOptions = {},
): Promise<BrandColor | null> {
  const infoUrl = relayInfoUrlFromRelayUrl(relayUrl);
  if (!infoUrl || !fetchImpl) return null;

  try {
    const response = await fetchImpl(infoUrl, {
      headers: { Accept: "application/nostr+json" },
      signal,
    });
    if (signal?.aborted || !response.ok) return null;

    const info = await response.json();
    if (signal?.aborted) return null;
    return parseBrandColor(info && typeof info === "object" ? info : null);
  } catch {
    return null;
  }
}

export async function applyRelayBrandColorFromInfo(
  root: BrandColorRoot,
  relayUrl: string | null | undefined,
  options: RelayBrandFetchOptions = {},
): Promise<void> {
  // Clear first, synchronously, so a community transition never carries the
  // previous tenant's color while the new relay's `/info` request is pending.
  applyBrandColor(root, null);
  const color = await fetchRelayBrandColor(relayUrl, options);
  if (!options.signal?.aborted) {
    applyBrandColor(root, color);
  }
}
