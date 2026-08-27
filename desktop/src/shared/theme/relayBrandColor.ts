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
  root: {
    style: {
      setProperty(k: string, v: string): void;
      removeProperty(k: string): void;
    };
  },
  color: BrandColor | null,
): void {
  if (color === null) {
    root.style.removeProperty(BRAND_COLOR_CSS_VAR);
    return;
  }
  root.style.setProperty(BRAND_COLOR_CSS_VAR, color);
}
