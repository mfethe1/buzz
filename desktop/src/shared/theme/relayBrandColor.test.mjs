import assert from "node:assert/strict";
import test from "node:test";
import {
  BRAND_COLOR_CSS_VAR,
  applyBrandColor,
  applyRelayBrandColorFromInfo,
  fetchRelayBrandColor,
  parseBrandColor,
  relayInfoUrlFromRelayUrl,
} from "./relayBrandColor.ts";

function styleStub() {
  const props = new Map();
  return {
    props,
    style: {
      setProperty: (k, v) => props.set(k, v),
      removeProperty: (k) => props.delete(k),
    },
  };
}

test("accepts a well-formed #rrggbb brand color", () => {
  assert.equal(parseBrandColor({ buzz_brand_color: "#ff8800" }), "#ff8800");
  assert.equal(parseBrandColor({ buzz_brand_color: "#000000" }), "#000000");
  assert.equal(parseBrandColor({ buzz_brand_color: "#FFFFFF" }), "#FFFFFF");
});

test("absent or cleared brand color degrades to null, never throws", () => {
  assert.equal(parseBrandColor({}), null);
  assert.equal(parseBrandColor(null), null);
  assert.equal(parseBrandColor(undefined), null);
  assert.equal(parseBrandColor({ buzz_brand_color: "" }), null);
});

test("rejects every non-#rrggbb form the relay also rejects", () => {
  // Mirrors validate_brand_color in handlers/relay_admin.rs: no shorthand, no
  // alpha, no named colors, no functional notation.
  for (const bad of [
    "#fff",
    "#ff8800ff",
    "red",
    "rgb(255,136,0)",
    "ff8800",
    "#gggggg",
    "#ff 880",
    " #ff8800",
    "#ff8800 ",
  ]) {
    assert.equal(
      parseBrandColor({ buzz_brand_color: bad }),
      null,
      `must reject ${JSON.stringify(bad)}`,
    );
  }
});

test("rejects a CSS-injection payload rather than passing it through", () => {
  // The value lands in a CSS custom property. A server that is compromised,
  // downgraded, or simply older than the validation must not be able to inject.
  for (const hostile of [
    "#ff8800; background: url(https://evil.test/x)",
    "red; --other: 1",
    "}</style><script>alert(1)</script>",
    "expression(alert(1))",
  ]) {
    assert.equal(parseBrandColor({ buzz_brand_color: hostile }), null);
  }
});

test("rejects non-string types without throwing (malformed/hostile JSON)", () => {
  for (const bad of [123, true, null, {}, [], { toString: () => "#ff8800" }]) {
    assert.equal(parseBrandColor({ buzz_brand_color: bad }), null);
  }
});

test("applies the brand color as a composable custom property", () => {
  const root = styleStub();
  applyBrandColor(root, "#ff8800");
  assert.equal(root.props.get(BRAND_COLOR_CSS_VAR), "#ff8800");
});

test("clearing removes the property so a previous tenant's brand cannot leak", () => {
  const root = styleStub();
  applyBrandColor(root, "#ff8800");
  applyBrandColor(root, null);
  assert.equal(root.props.has(BRAND_COLOR_CSS_VAR), false);
});

test("switching communities replaces rather than accumulates", () => {
  const root = styleStub();
  applyBrandColor(root, "#ff8800");
  applyBrandColor(root, "#123abc");
  assert.equal(root.props.size, 1);
  assert.equal(root.props.get(BRAND_COLOR_CSS_VAR), "#123abc");
});

test("relayInfoUrlFromRelayUrl maps relay URLs to host-equivalent /info", () => {
  assert.equal(
    relayInfoUrlFromRelayUrl("wss://tenant.example/ws?x=1#frag")?.toString(),
    "https://tenant.example/info",
  );
  assert.equal(
    relayInfoUrlFromRelayUrl("ws://localhost:3000")?.toString(),
    "http://localhost:3000/info",
  );
  assert.equal(
    relayInfoUrlFromRelayUrl("https://tenant.example/custom")?.toString(),
    "https://tenant.example/info",
  );
  assert.equal(relayInfoUrlFromRelayUrl("file:///tmp/relay"), null);
  assert.equal(relayInfoUrlFromRelayUrl("not a url"), null);
});

test("fetchRelayBrandColor reads valid NIP-11 color with the expected Accept header", async () => {
  let seenUrl = null;
  let seenAccept = null;
  const color = await fetchRelayBrandColor("wss://tenant.example", {
    fetchImpl: async (url, init) => {
      seenUrl = url.toString();
      seenAccept = init.headers.Accept;
      return {
        ok: true,
        json: async () => ({ buzz_brand_color: "#ff8800" }),
      };
    },
  });

  assert.equal(color, "#ff8800");
  assert.equal(seenUrl, "https://tenant.example/info");
  assert.equal(seenAccept, "application/nostr+json");
});

test("fetchRelayBrandColor degrades to null for non-2xx, offline, and malformed JSON", async () => {
  assert.equal(
    await fetchRelayBrandColor("wss://tenant.example", {
      fetchImpl: async () => ({
        ok: false,
        json: async () => {
          throw new Error("json should not be read for non-2xx");
        },
      }),
    }),
    null,
  );

  assert.equal(
    await fetchRelayBrandColor("wss://tenant.example", {
      fetchImpl: async () => {
        throw new Error("offline");
      },
    }),
    null,
  );

  assert.equal(
    await fetchRelayBrandColor("wss://tenant.example", {
      fetchImpl: async () => ({
        ok: true,
        json: async () => {
          throw new Error("malformed json");
        },
      }),
    }),
    null,
  );
});

test("applyRelayBrandColorFromInfo clears first and ignores aborted stale responses", async () => {
  const root = styleStub();
  applyBrandColor(root, "#ff8800");

  let resolveJson;
  let notifyJsonStarted;
  const jsonStarted = new Promise((resolve) => {
    notifyJsonStarted = resolve;
  });
  const controller = new AbortController();
  const applyPromise = applyRelayBrandColorFromInfo(
    root,
    "wss://tenant.example",
    {
      signal: controller.signal,
      fetchImpl: async () => ({
        ok: true,
        json: () => {
          notifyJsonStarted();
          return new Promise((resolve) => {
            resolveJson = resolve;
          });
        },
      }),
    },
  );

  assert.equal(root.props.has(BRAND_COLOR_CSS_VAR), false);
  await jsonStarted;
  controller.abort();
  resolveJson({ buzz_brand_color: "#123abc" });
  assert.equal(await applyPromise, null);
  assert.equal(root.props.has(BRAND_COLOR_CSS_VAR), false);
});

test("applyRelayBrandColorFromInfo returns the applied color so callers can adopt it", async () => {
  const root = styleStub();
  assert.equal(
    await applyRelayBrandColorFromInfo(root, "wss://tenant.example", {
      fetchImpl: async () => ({
        ok: true,
        json: async () => ({ buzz_brand_color: "#123abc" }),
      }),
    }),
    "#123abc",
  );
  assert.equal(root.props.get(BRAND_COLOR_CSS_VAR), "#123abc");

  // A relay advertising no color must report null, not the previous value.
  assert.equal(
    await applyRelayBrandColorFromInfo(root, "wss://other.example", {
      fetchImpl: async () => ({ ok: true, json: async () => ({}) }),
    }),
    null,
  );
  assert.equal(root.props.has(BRAND_COLOR_CSS_VAR), false);
});

test("a non-2xx response is never parsed — the body of an error page is not a brand color", async () => {
  // Kills the mutant that drops `!response.ok`: a 500 whose body happens to
  // carry a well-formed color must still yield null, and json() must not run.
  let jsonCalls = 0;
  assert.equal(
    await fetchRelayBrandColor("wss://tenant.example", {
      fetchImpl: async () => ({
        ok: false,
        json: async () => {
          jsonCalls += 1;
          return { buzz_brand_color: "#ff8800" };
        },
      }),
    }),
    null,
  );
  assert.equal(jsonCalls, 0, "a non-2xx body must never be parsed");
});

test("each abort guard independently blocks a stale color — neither is redundant", async () => {
  // Two guards bracket the awaits in fetchRelayBrandColor. A mutant deleting
  // either one alone must fail, so each is exercised in isolation: abort is
  // observed at exactly one await point per case.
  const controller = new AbortController();

  // (1) aborted while the response is in flight, before `json()`.
  assert.equal(
    await fetchRelayBrandColor("wss://tenant.example", {
      signal: controller.signal,
      fetchImpl: async () => {
        controller.abort();
        return {
          ok: true,
          json: async () => ({ buzz_brand_color: "#ff8800" }),
        };
      },
    }),
    null,
    "abort observed before json() must not yield a color",
  );

  // (2) aborted only while `json()` is being parsed.
  const later = new AbortController();
  assert.equal(
    await fetchRelayBrandColor("wss://tenant.example", {
      signal: later.signal,
      fetchImpl: async () => ({
        ok: true,
        json: async () => {
          later.abort();
          return { buzz_brand_color: "#ff8800" };
        },
      }),
    }),
    null,
    "abort observed during json() must not yield a color",
  );
});

test("a null/absent relayUrl short-circuits without ever touching the network", async () => {
  // The signed-out / no-community path: nothing to brand, so no request.
  for (const relayUrl of [null, undefined, "", "   ", "not a url"]) {
    assert.equal(
      await fetchRelayBrandColor(relayUrl, {
        fetchImpl: () => {
          throw new Error(`fetch must not run for ${JSON.stringify(relayUrl)}`);
        },
      }),
      null,
    );
  }

  // And it still clears, so signing out drops the previous tenant's brand.
  const root = styleStub();
  applyBrandColor(root, "#ff8800");
  assert.equal(await applyRelayBrandColorFromInfo(root, null), null);
  assert.equal(root.props.has(BRAND_COLOR_CSS_VAR), false);
});
