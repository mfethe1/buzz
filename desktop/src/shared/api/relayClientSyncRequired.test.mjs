// Session-level hardening tests for the `BUZZ_SYNC_REQUIRED` gap-frame consumer
// (REG-28). `relaySyncRequiredPolicy.test.mjs` covers the *pure* policy
// contract; this file drives the real `RelayClient` dispatcher so the session
// behaviours that policy purity cannot express are covered:
//
//   - the disconnected guard (`wsId === null`)
//   - burst coalescing across concurrent frames (one replay per burst)
//   - swallow-not-rethrow: a failing replay must NOT tear down a healthy,
//     authenticated socket the way the reconnect call site deliberately does
//   - malformed / hostile `reason` payloads
//   - the reason string is never echoed onto the wire
//
// Harness pattern is lifted from `relayClientPublishRejection.test.mjs`, which
// is the repo's existing precedent for exercising `RelayClient` without a real
// socket.
import assert from "node:assert/strict";
import test from "node:test";

const fakeNow = 0;
const pendingTimers = new Map();
let nextTimerId = 1;
const deliveredFrames = [];
let sendTransport = async () => {};

globalThis.window = {
  setTimeout: (fn, ms) => {
    const id = nextTimerId++;
    pendingTimers.set(id, { fn, fireAt: fakeNow + ms });
    return id;
  },
  clearTimeout: (id) => pendingTimers.delete(id),
  __TAURI_INTERNALS__: {
    invoke: async (command, args) => {
      if (command === "plugin:websocket|send") {
        deliveredFrames.push(args);
        return sendTransport(args);
      }
    },
  },
};
Date.now = () => fakeNow;

const { RelayClient } = await import("./relayClientSession.ts");
const { resetRateLimitGate } = await import("./relayRateLimitGate.ts");

function reset() {
  resetRateLimitGate();
  pendingTimers.clear();
  nextTimerId = 1;
  deliveredFrames.length = 0;
  sendTransport = async () => {};
}

/** A client with a live socket and one live subscription to replay. */
function connectedClient({ withSubscription = true } = {}) {
  const client = new RelayClient();
  client.wsId = 7;
  if (withSubscription) {
    client.subscriptions.set("sub-1", {
      mode: "live",
      filter: { kinds: [9] },
      onEvent: () => {},
      resolveReady: () => {},
    });
  }
  return client;
}

/** Feeds a raw relay frame through the real inbound dispatch path. */
function deliver(client, frame) {
  return client.handleWsMessage(
    { type: "Text", data: JSON.stringify(frame) },
    client.connectionGeneration,
  );
}

function reqFrames() {
  return deliveredFrames.filter(
    ({ message }) => JSON.parse(message.data)[0] === "REQ",
  );
}

/** Lets queued microtasks/promise chains settle. */
async function flush(turns = 25) {
  for (let i = 0; i < turns; i++) await Promise.resolve();
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

// ── Happy path ──────────────────────────────────────────────────────────────

test("BUZZ_SYNC_REQUIRED on a live socket replays live subscriptions", async () => {
  reset();
  const client = connectedClient();

  await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
  await flush();

  assert.equal(
    reqFrames().length,
    1,
    "the gap frame must re-REQ the live subscription — otherwise the dropped " +
      "fan-out event stays missing until an arbitrary later reconnect",
  );
});

// ── Edge case: offline / disconnected ───────────────────────────────────────

test("EDGE offline: a frame with no socket (wsId === null) starts no replay", async () => {
  reset();
  const client = connectedClient();
  client.wsId = null;

  await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
  await flush();

  assert.equal(reqFrames().length, 0, "no REQ may be sent without a socket");
  assert.equal(
    client.syncReplayScheduled,
    null,
    "the coalescing slot must not be armed by a frame we refused to act on",
  );
});

// ── Edge case: empty input (no live subscriptions) ──────────────────────────

test("EDGE empty: a frame with zero live subscriptions replays nothing and does not throw", async () => {
  reset();
  const client = connectedClient({ withSubscription: false });

  await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
  await flush();

  assert.equal(reqFrames().length, 0);
  assert.equal(
    client.syncReplayScheduled,
    null,
    "the slot must be released after an empty replay settles",
  );
});

test("EDGE empty: a frame with NO reason element at all still replays", async () => {
  reset();
  const client = connectedClient();

  await deliver(client, ["BUZZ_SYNC_REQUIRED"]);
  await flush();

  assert.equal(
    reqFrames().length,
    1,
    "the gap is real regardless of whether the relay labelled it",
  );
});

// ── Edge case: concurrent writers / burst coalescing ───────────────────────

test("EDGE concurrency: a burst of frames during an in-flight replay yields exactly one replay", async () => {
  reset();
  const gate = deferred();
  sendTransport = () => gate.promise;
  const client = connectedClient();

  await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
  await flush();
  assert.equal(reqFrames().length, 1, "first frame starts the replay");

  // Three more frames arrive while the first replay is still awaiting its send.
  await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
  await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
  await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
  await flush();

  assert.equal(
    reqFrames().length,
    1,
    "a backpressure burst must coalesce — fanning one REQ storm per dropped " +
      "event is how a struggling relay gets hammered harder",
  );

  gate.resolve();
  await flush();
  assert.equal(
    client.syncReplayScheduled,
    null,
    "the slot must be cleared once the replay settles",
  );
});

test("EDGE concurrency: a new burst AFTER the previous replay settles does replay again", async () => {
  reset();
  const client = connectedClient();

  await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
  await flush();
  assert.equal(reqFrames().length, 1);

  await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
  await flush();
  assert.equal(
    reqFrames().length,
    2,
    "coalescing must be per-burst, not a permanent latch",
  );
});

// ── Edge case: replay failure / timeout (swallow-not-rethrow) ──────────────

test("EDGE failure: a rejecting replay does NOT tear down the healthy socket", async () => {
  reset();
  sendTransport = async () => {
    throw new Error("relay send failed");
  };
  const client = connectedClient();
  const generationBefore = client.connectionGeneration;

  await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
  await flush();

  // `replayLiveSubscriptions()` (the private wrapper) calls resetConnection()
  // and rethrows on failure — correct for the reconnect call site, fatal here.
  // The handler's `.catch(() => {})` is what keeps an authenticated session
  // alive when an opportunistic accelerator fails.
  assert.equal(
    client.wsId,
    7,
    "a failed best-effort replay must not drop a working connection",
  );
  assert.equal(
    client.connectionGeneration,
    generationBefore,
    "resetConnection() bumps the generation — it must not have been reached " +
      "in a way that invalidates the live session",
  );
});

test("EDGE failure: a rejecting replay releases the coalescing slot (no permanent wedge)", async () => {
  reset();
  sendTransport = async () => {
    throw new Error("relay send failed");
  };
  const client = connectedClient();

  await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
  await flush();

  assert.equal(
    client.syncReplayScheduled,
    null,
    "clearing only on the success path would wedge the consumer permanently " +
      "after the first transient failure",
  );
});

test("EDGE failure: a rejecting replay produces no unhandled rejection", async () => {
  reset();
  sendTransport = async () => {
    throw new Error("relay send failed");
  };
  const unhandled = [];
  const onUnhandled = (reason) => unhandled.push(reason);
  process.on("unhandledRejection", onUnhandled);

  try {
    const client = connectedClient();
    await deliver(client, ["BUZZ_SYNC_REQUIRED", "backpressure"]);
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.deepEqual(unhandled, []);
  } finally {
    process.off("unhandledRejection", onUnhandled);
  }
});

// ── Edge case: malformed / hostile payloads ────────────────────────────────

test("EDGE malformed: non-string reasons replay without throwing", async () => {
  for (const bad of [42, null, {}, [], true]) {
    reset();
    const client = connectedClient();

    await deliver(client, ["BUZZ_SYNC_REQUIRED", bad]);
    await flush();

    assert.equal(
      reqFrames().length,
      1,
      `reason ${JSON.stringify(bad)} must still trigger replay`,
    );
  }
});

test("EDGE malformed: an unknown reason string still replays", async () => {
  reset();
  const client = connectedClient();

  await deliver(client, ["BUZZ_SYNC_REQUIRED", "totally-made-up-reason"]);
  await flush();

  assert.equal(reqFrames().length, 1);
});

test("EDGE hostile: relay-supplied reason text is never echoed onto the wire", async () => {
  reset();
  const client = connectedClient();
  const hostile = "<img src=x onerror=alert(1)>";

  await deliver(client, ["BUZZ_SYNC_REQUIRED", hostile]);
  await flush();

  const wire = JSON.stringify(deliveredFrames);
  assert.equal(
    wire.includes("onerror"),
    false,
    "the reason is attacker-influenced text: it is logged (allowlisted) but " +
      "must never be rendered or reflected back to the relay",
  );
});

// ── Negative tests: the branch must not over-trigger ───────────────────────

test("NEGATIVE: an unrelated NOTICE frame does not trigger a sync replay", async () => {
  reset();
  const client = connectedClient();

  await deliver(client, ["NOTICE", "something happened"]);
  await flush();

  assert.equal(reqFrames().length, 0);
  assert.equal(client.syncReplayScheduled, null);
});

test("NEGATIVE: a lookalike frame type does not trigger a sync replay", async () => {
  reset();
  const client = connectedClient();

  await deliver(client, ["BUZZ_SYNC_REQUIRED_NOT", "backpressure"]);
  await deliver(client, ["buzz_sync_required", "backpressure"]);
  await flush();

  assert.equal(
    reqFrames().length,
    0,
    "frame-type matching must be exact — no prefix or case-insensitive match",
  );
});

test("NEGATIVE: an EOSE frame does not trigger a sync replay", async () => {
  reset();
  const client = connectedClient();

  await deliver(client, ["EOSE", "sub-1"]);
  await flush();

  assert.equal(reqFrames().length, 0);
});
