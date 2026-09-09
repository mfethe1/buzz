// Invoked by the isolated native Rust transport fixture, never a live relay.
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
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
async function invoke(command, args) {
  if (command === "fetch_link_preview_metadata") invoked.push(args.href);
  const response = await fetch(endpoint, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ command, args }),
  });
  assert.equal(response.status, 200);
  const result = await response.json();
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
  await delay(100);
  assert.deepEqual(invoked, urls.slice(0, 2));
  assert.deepEqual(await invoke("fixture_paths", {}), [
    "/slow-one",
    "/slow-two",
  ]);
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
      automaticReplay: false,
    }),
  );
} finally {
  clearTimeout(deadline);
  resetLinkPreviewMetadataCache();
  dom.window.close();
}
