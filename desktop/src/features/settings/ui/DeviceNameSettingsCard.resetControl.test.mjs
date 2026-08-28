/**
 * Mounted regressions for the REG-11 Reset control.
 *
 * The pure predicates behind it (`showsResetControl`, `saveIsDisabled`) are
 * covered in `deviceNameCardLogic.test.mjs`. What is NOT expressible there is
 * the *wiring*, and that is where this feature's real defect lived: the Reset
 * control derives its own visibility from the `device-identity` query, so
 * `onSuccess` must publish the identity the command returned
 * (`setQueryData`) rather than `invalidateQueries` and wait. With an
 * invalidate, React Query serves the previous data during the refetch
 * (`staleTime: Infinity`, `hooks.ts:368-374`), leaving a real name on screen
 * and a clickable Reset button after the reset already landed — a second
 * click issuing a redundant write, a second republish, and a second toast.
 *
 * Mutation proof: swap the `queryClient.setQueryData(...)` line in
 * `DeviceNameSettingsCard.tsx` back to
 * `void queryClient.invalidateQueries({ queryKey: deviceIdentityQueryKey })`
 * and test 2 goes RED on both assertions — `get_device_identity` is invoked a
 * second time and the Reset button is still in the DOM. Revert the
 * `resetPending` clause plumbed into `saveIsDisabled`/`disabled` and test 3
 * goes RED.
 */

import assert from "node:assert/strict";
import { after, before, beforeEach, describe, it } from "node:test";
import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});

Object.assign(globalThis, {
  HTMLElement: dom.window.HTMLElement,
  IS_REACT_ACT_ENVIRONMENT: true,
  MutationObserver: dom.window.MutationObserver,
  document: dom.window.document,
  localStorage: dom.window.localStorage,
  self: dom.window,
  window: dom.window,
});
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: dom.window.navigator,
});
dom.window.requestAnimationFrame = (callback) => setTimeout(callback, 0);
globalThis.requestAnimationFrame = dom.window.requestAnimationFrame;
dom.window.matchMedia ??= (query) => ({
  matches: false,
  media: query,
  onchange: null,
  addListener: () => {},
  removeListener: () => {},
  addEventListener: () => {},
  removeEventListener: () => {},
  dispatchEvent: () => false,
});
globalThis.matchMedia = dom.window.matchMedia;
for (const key of ["Event", "Node", "HTMLInputElement", "getComputedStyle"]) {
  if (!(key in globalThis) && dom.window[key] !== undefined) {
    globalThis[key] = dom.window[key];
  }
}

// ── Tauri IPC stub ────────────────────────────────────────────────────────────

const DEVICE_ID = "0123456789abcdef0123456789abcdef";
const OPAQUE = "device-01234567";

/** Per-command invocation counts, asserted directly by the refetch test. */
let calls;
/** Current on-disk label the stubbed backend reports. */
let backendLabel;
/** Optional gate held by `reset_device_label` so a pending state is observable. */
let resetGate;
/**
 * `get_device_identity` never resolves after the first call. A refetch is then
 * *observable* rather than merely counted: with an invalidate, the query stays
 * in flight, previous data keeps being served, and the stale Reset button is
 * still on screen — exactly the production window this test exists to close.
 */
function identityResponse() {
  calls.get_device_identity += 1;
  if (calls.get_device_identity > 1) return new Promise(() => {});
  return Promise.resolve({
    deviceId: DEVICE_ID,
    deviceLabel: backendLabel,
    createdAt: "2026-08-27T00:00:00Z",
  });
}

globalThis.__TAURI_INTERNALS__ = {
  invoke: (command) => {
    if (command === "get_device_identity") return identityResponse();
    if (command === "get_device_name_suggestion") {
      calls.get_device_name_suggestion += 1;
      return Promise.resolve(null);
    }
    if (command === "set_device_label") {
      calls.set_device_label += 1;
      backendLabel = "studio-mac";
      return Promise.resolve({
        deviceId: DEVICE_ID,
        deviceLabel: backendLabel,
        createdAt: "2026-08-27T00:00:00Z",
      });
    }
    if (command === "reset_device_label") {
      calls.reset_device_label += 1;
      backendLabel = OPAQUE;
      const settled = {
        deviceId: DEVICE_ID,
        deviceLabel: OPAQUE,
        createdAt: "2026-08-27T00:00:00Z",
      };
      return resetGate
        ? resetGate.promise.then(() => settled)
        : Promise.resolve(settled);
    }
    return Promise.reject(new Error(`unmocked: ${command}`));
  },
  transformCallback: () => 1,
};
dom.window.__TAURI_INTERNALS__ = globalThis.__TAURI_INTERNALS__;

// ── Deferred imports ──────────────────────────────────────────────────────────

let React,
  act,
  createRoot,
  QueryClient,
  QueryClientProvider,
  DeviceNameSettingsCard;

before(async () => {
  ({ default: React, act } = await import("react"));
  ({ createRoot } = await import("react-dom/client"));
  ({ QueryClient, QueryClientProvider } = await import(
    "@tanstack/react-query"
  ));
  ({ DeviceNameSettingsCard } = await import("./DeviceNameSettingsCard.tsx"));
});

beforeEach(() => {
  calls = {
    get_device_identity: 0,
    get_device_name_suggestion: 0,
    set_device_label: 0,
    reset_device_label: 0,
  };
  resetGate = null;
});

after(() => dom.window.close());

// ── Helpers ───────────────────────────────────────────────────────────────────

function deferred() {
  let resolve;
  const promise = new Promise((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

async function mountCard() {
  const queryClient = new QueryClient({
    defaultOptions: {
      // gcTime Infinity, not the 5-minute default: React Query schedules a real
      // `setTimeout(gcTime)` per query, and a finite one keeps the Node event
      // loop alive for five minutes after the assertions pass, so the file
      // times out instead of exiting. Infinity fails query-core's
      // `isValidTimeout`, so no timer is ever armed; `clear()` below still
      // drops the cache.
      queries: { gcTime: Number.POSITIVE_INFINITY, retry: false },
      mutations: { gcTime: Number.POSITIVE_INFINITY, retry: false },
    },
  });
  const container = dom.window.document.createElement("div");
  dom.window.document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(DeviceNameSettingsCard, null),
      ),
    );
  });
  await settle();
  return {
    container,
    unmount: async () => {
      await act(async () => {
        root.unmount();
      });
      container.remove();
      queryClient.clear();
    },
  };
}

/** Flush pending microtasks and timers inside `act`. */
async function settle() {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 10));
  });
}

const find = (container, testid) =>
  container.querySelector(`[data-testid="${testid}"]`);

async function click(element) {
  await act(async () => {
    element.dispatchEvent(
      new dom.window.MouseEvent("click", { bubbles: true, cancelable: true }),
    );
    await new Promise((r) => setTimeout(r, 0));
  });
}

/** Drive a React-controlled input the way a user would. */
async function type(input, value) {
  const setter = Object.getOwnPropertyDescriptor(
    dom.window.HTMLInputElement.prototype,
    "value",
  ).set;
  await act(async () => {
    setter.call(input, value);
    input.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
    await new Promise((r) => setTimeout(r, 0));
  });
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("DeviceNameSettingsCard Reset control — REG-11 mounted regressions", () => {
  it("withholds Reset from a device still on its anonymous default", async () => {
    backendLabel = OPAQUE;
    const { container, unmount } = await mountCard();

    assert.equal(
      find(container, "device-name-reset"),
      null,
      "a device at its default must not be offered a no-op reset",
    );
    assert.ok(find(container, "device-name-save"), "Save always renders");

    await unmount();
  });

  it("retires Reset on success without a refetch, closing the double-click window", async () => {
    backendLabel = "mfeth-win";
    const { container, unmount } = await mountCard();

    const reset = find(container, "device-name-reset");
    assert.ok(reset, "a named device must be offered the way back");
    assert.equal(calls.get_device_identity, 1);

    await click(reset);
    await settle();

    assert.equal(
      calls.reset_device_label,
      1,
      "one click must issue exactly one reset write",
    );
    assert.equal(
      calls.get_device_identity,
      1,
      "onSuccess must publish the returned identity, not invalidate and refetch",
    );
    assert.equal(
      find(container, "device-name-reset"),
      null,
      "Reset must be gone the moment the reset lands — a lingering button is a second write",
    );
    assert.equal(
      find(container, "device-name-input").value,
      OPAQUE,
      "the field must show the anonymous label the command returned",
    );

    await unmount();
  });

  it("offers Reset immediately after Save publishes a real name", async () => {
    backendLabel = OPAQUE;
    const { container, unmount } = await mountCard();

    await type(find(container, "device-name-input"), "studio-mac");
    await click(find(container, "device-name-save"));
    await settle();

    assert.equal(calls.set_device_label, 1);
    assert.equal(
      calls.get_device_identity,
      1,
      "Save success must publish the returned identity, not invalidate and refetch",
    );
    assert.equal(
      find(container, "device-name-save").disabled,
      true,
      "Save must retire after the saved label reaches the identity cache",
    );
    assert.ok(
      find(container, "device-name-reset"),
      "a just-published real name must expose the way back immediately",
    );

    await unmount();
  });

  it("disables Save and the field while a reset is in flight", async () => {
    backendLabel = "mfeth-win";
    resetGate = deferred();
    const { container, unmount } = await mountCard();

    // Make Save otherwise-enabled: a dirty, non-empty draft.
    await type(find(container, "device-name-input"), "studio-mac");
    assert.equal(
      find(container, "device-name-save").disabled,
      false,
      "precondition: a dirty draft enables Save",
    );

    await click(find(container, "device-name-reset"));

    assert.equal(
      find(container, "device-name-save").disabled,
      true,
      "both writes land on the same device.json and both republish — never concurrently",
    );
    assert.equal(find(container, "device-name-input").disabled, true);

    await act(async () => {
      resetGate.resolve();
      await new Promise((r) => setTimeout(r, 0));
    });
    await settle();

    assert.equal(find(container, "device-name-reset"), null);

    await unmount();
  });
});
