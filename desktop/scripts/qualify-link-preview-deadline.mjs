// Invoked by the isolated native Rust transport fixture, never a live relay.
import assert from "node:assert/strict";
import { JSDOM } from "jsdom";

const endpoint = new URL(process.argv[2]);
assert.equal(endpoint.hostname, "127.0.0.1");
const dom = new JSDOM("<!doctype html><html><body></body></html>");
Object.assign(globalThis, {
  window: dom.window,
  document: dom.window.document,
});
const invoked = [];
const settled = [];
let deadline;
const requests = new AbortController();
async function boundedJson(response) {
  const reader = response.body.getReader();
  let bytes = 0;
  const chunks = [];
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      assert.ok(bytes <= 8192, "fixture response exceeds 8192 bytes");
      chunks.push(value);
    }
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } finally {
    await reader.cancel();
    reader.releaseLock();
  }
}
function assertReady(snapshot) {
  assert.equal(snapshot.ready, true, "fixture did not confirm readiness");
  assert.equal(
    snapshot.liveBodies,
    2,
    "both slow response bodies must be live",
  );
  assert.equal(
    snapshot.completed,
    0,
    "readiness must precede native settlement",
  );
  assert.deepEqual(
    [...snapshot.paths].sort(),
    ["/slow-one", "/slow-two"],
    "only the two slow paths may have reached the transport",
  );
}
async function invoke(command, args) {
  if (command === "fetch_link_preview_metadata") invoked.push(args.href);
  const response = await fetch(endpoint, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ command, args }),
    signal: AbortSignal.any([requests.signal, AbortSignal.timeout(3000)]),
  });
  assert.equal(response.status, 200);
  const result = await boundedJson(response);
  if (command === "fetch_link_preview_metadata") {
    settled.push({ href: args.href, error: result.error ?? null });
  }
  if (result.error) throw new Error(result.error);
  return result.ok;
}
dom.window.__TAURI_INTERNALS__ = { invoke };
const { loadLinkPreviewMetadata, resetLinkPreviewMetadataCache } = await import(
  "../src/shared/lib/useResolvedLinkPreviews.ts"
);
try {
  const urls = ["slow-one", "slow-two", "fast"].map(
    (path) => `https://scheduler-deadline.example/${path}`,
  );
  const started = performance.now();
  const loads = urls.map(loadLinkPreviewMetadata);
  // The fixture observes real HTTP bodies and native completions. Neither
  // invocation order nor a sleep establishes that both slots are occupied.
  const readiness = await invoke("fixture_wait_ready", {});
  assertReady(readiness);
  assert.deepEqual(invoked, urls.slice(0, 2));
  assert.equal(
    settled.length,
    0,
    "third URL must be queued before any settlement",
  );
  assertReady(await invoke("fixture_ready_ack", {}));
  const values = await Promise.race([
    Promise.all(loads.map((load) => load.promise)),
    new Promise((_, reject) => {
      deadline = setTimeout(
        () =>
          reject(new Error("native previews retained both scheduler slots")),
        2000,
      );
    }),
  ]);
  assert.deepEqual(values.slice(0, 2), [null, null]);
  assert.equal(values[2]?.title, "Fast preview");
  assert.deepEqual(invoked, urls);
  assert.equal(
    settled.filter((row) => row.error === "link preview operation timed out")
      .length,
    2,
  );
  assert.equal(settled.length, 3);
  console.log(
    JSON.stringify({
      result: "PASS",
      nativeTimeouts: 2,
      fastTitle: values[2].title,
      elapsedMs: Math.round(performance.now() - started),
      schedulerSlots: 2,
      readinessPaths: readiness.paths,
      readinessLiveBodies: readiness.liveBodies,
      readinessCompleted: readiness.completed,
      automaticReplay: false,
    }),
  );
} finally {
  clearTimeout(deadline);
  requests.abort();
  resetLinkPreviewMetadataCache();
  dom.window.close();
}
