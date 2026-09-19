/**
 * ThemeProvider's community-brand-color wiring.
 *
 * The pure logic lives in `relayBrandColor.test.mjs`; this file covers the
 * React layer it hangs off — the `relayUrl` effect, the abort cleanup that
 * makes community switching safe, and the precedence rule that lets a
 * community brand override the user's personal accent swatch. Mutating any of
 * those three left every other suite green before this file existed.
 */

import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";

import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});

before(() => {
  Object.assign(globalThis, {
    document: dom.window.document,
    HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
    window: dom.window,
  });
  dom.window.matchMedia = () => ({
    matches: false,
    addEventListener() {},
    removeEventListener() {},
  });
});

afterEach(async () => {
  const { cleanup } = await import("@testing-library/react");
  cleanup();
  document.documentElement.removeAttribute("style");
  delete globalThis.fetch;
});

after(() => dom.window.close());

const selectedAccent = () =>
  document.documentElement.style.getPropertyValue("--buzz-selected-accent");
const primary = () =>
  document.documentElement.style.getPropertyValue("--primary");
const brand = () =>
  document.documentElement.style.getPropertyValue("--buzz-brand-color");

/** `applyAccentColor` publishes HSL triplets, so assert on the converted form. */
async function hslOf(hex) {
  const { hexToHsl } = await import("@/shared/theme/adaptive-theme");
  return hexToHsl(hex);
}

test("a community's brand color is fetched from its relay and drives the live accent", async () => {
  globalThis.fetch = async () => ({
    ok: true,
    json: async () => ({ buzz_brand_color: "#ff8800" }),
  });

  const { createElement } = await import("react");
  const { act, render, screen } = await import("@testing-library/react");
  const { ThemeProvider } = await import("@/shared/theme/ThemeProvider");

  await act(async () => {
    render(
      createElement(
        ThemeProvider,
        { defaultTheme: "buzz", relayUrl: "wss://tenant.example" },
        createElement("p", null, "branded"),
      ),
    );
  });

  assert.ok(screen.getByText("branded"));
  assert.equal(brand(), "#ff8800", "brand custom property must be published");
  // The precedence rule: without this the color is inert decoration.
  const expected = await hslOf("#ff8800");
  assert.equal(
    selectedAccent(),
    expected,
    "the accent token must be driven by the brand color",
  );
  assert.equal(primary(), expected, "--primary must follow the brand color");
});

test("a relay advertising no brand color leaves the personal accent intact", async () => {
  globalThis.fetch = async () => ({ ok: true, json: async () => ({}) });

  const { createElement } = await import("react");
  const { act, render } = await import("@testing-library/react");
  const { ThemeProvider } = await import("@/shared/theme/ThemeProvider");

  await act(async () => {
    render(
      createElement(
        ThemeProvider,
        { defaultTheme: "buzz", relayUrl: "wss://plain.example" },
        createElement("p", null, "unbranded"),
      ),
    );
  });

  assert.equal(brand(), "", "no brand color must leave the property unset");
});

test("switching communities aborts the outgoing fetch so a stale brand cannot win", async () => {
  // The race this guards: community A's slow /info resolving *after* the user
  // has already switched to B. Without the abort cleanup, A's color lands on
  // B's workspace.
  const seen = [];
  let releaseFirst;
  const firstReleased = new Promise((resolve) => {
    releaseFirst = resolve;
  });

  globalThis.fetch = async (url, init) => {
    const href = String(url);
    seen.push({ href, signal: init?.signal });
    if (href.includes("slow.example")) {
      await firstReleased;
      if (init?.signal?.aborted) {
        throw new dom.window.DOMException("aborted", "AbortError");
      }
      return { ok: true, json: async () => ({ buzz_brand_color: "#aa0000" }) };
    }
    return { ok: true, json: async () => ({ buzz_brand_color: "#00bb00" }) };
  };

  const { createElement } = await import("react");
  const { act, render } = await import("@testing-library/react");
  const { ThemeProvider } = await import("@/shared/theme/ThemeProvider");

  const element = (relayUrl) =>
    createElement(
      ThemeProvider,
      { defaultTheme: "buzz", relayUrl },
      createElement("p", null, "switching"),
    );

  let rerender;
  await act(async () => {
    ({ rerender } = render(element("wss://slow.example")));
  });
  await act(async () => {
    rerender(element("wss://fast.example"));
  });

  // Unblocking A *after* the switch is what makes this a race at all.
  releaseFirst();
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

  assert.deepEqual(
    seen.map((c) => c.href),
    ["https://slow.example/info", "https://fast.example/info"],
    "both communities must be fetched, in order",
  );
  assert.equal(
    seen[0].signal?.aborted,
    true,
    "the superseded community's request must have been aborted",
  );
  assert.equal(
    brand(),
    "#00bb00",
    "the community we switched TO must own the brand color",
  );
});
