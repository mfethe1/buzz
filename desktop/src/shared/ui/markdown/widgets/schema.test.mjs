import assert from "node:assert/strict";
import { test } from "node:test";

import {
  MAX_WIDGET_PAYLOAD_CHARS,
  parseWidget,
  WIDGET_FENCE_LANGUAGE,
} from "./schema.ts";

const table = (over = {}) =>
  JSON.stringify({
    v: 1,
    type: "table",
    columns: ["a"],
    rows: [["1"]],
    ...over,
  });

// ── acceptance ──────────────────────────────────────────────────────────

test("parseWidget: accepts a well-formed table", () => {
  const r = parseWidget(table());
  assert.equal(r.ok, true);
  assert.equal(r.widget.type, "table");
});

test("parseWidget: accepts a well-formed metric", () => {
  const r = parseWidget(
    JSON.stringify({
      v: 1,
      type: "metric",
      metrics: [{ label: "Open PRs", value: "2", delta: "+1" }],
    }),
  );
  assert.equal(r.ok, true);
  assert.equal(r.widget.metrics[0].label, "Open PRs");
});

// ── rejection: every failure must degrade, never throw ───────────────────

test("parseWidget: rejects malformed JSON without throwing", () => {
  const r = parseWidget("{not json");
  assert.equal(r.ok, false);
  assert.match(r.reason, /valid JSON/);
});

test("parseWidget: rejects an unknown widget type (allowlist holds)", () => {
  const r = parseWidget(JSON.stringify({ v: 1, type: "kanban" }));
  assert.equal(r.ok, false);
  assert.match(r.reason, /unknown widget type/);
});

test("parseWidget: rejects a future schema version", () => {
  const r = parseWidget(table({ v: 2 }));
  assert.equal(r.ok, false);
  assert.match(r.reason, /unsupported schema version/);
});

test("parseWidget: rejects a missing type", () => {
  const r = parseWidget(JSON.stringify({ v: 1 }));
  assert.equal(r.ok, false);
});

test("parseWidget: rejects ragged rows", () => {
  const r = parseWidget(table({ columns: ["a", "b"], rows: [["1"]] }));
  assert.equal(r.ok, false);
  assert.match(r.reason, /column count/);
});

test("parseWidget: rejects non-string cells", () => {
  const r = parseWidget(table({ rows: [[1]] }));
  assert.equal(r.ok, false);
});

test("parseWidget: rejects a JSON array payload", () => {
  const r = parseWidget("[]");
  assert.equal(r.ok, false);
});

test("parseWidget: rejects an oversized payload before parsing", () => {
  const r = parseWidget("x".repeat(MAX_WIDGET_PAYLOAD_CHARS + 1));
  assert.equal(r.ok, false);
  assert.match(r.reason, /too large/);
});

// ── security: hostile content survives as inert data ─────────────────────

test("parseWidget: hostile strings are preserved as data, not executed", () => {
  const xss = "<img src=x onerror=alert(1)>";
  const r = parseWidget(table({ rows: [[xss]] }));
  assert.equal(r.ok, true);
  // Preserved verbatim as a string — React escapes it at render time.
  assert.equal(r.widget.rows[0][0], xss);
});

test("parseWidget: prototype pollution keys do not reach the prototype", () => {
  const r = parseWidget(
    '{"v":1,"type":"table","columns":["a"],"rows":[["1"]],"__proto__":{"polluted":true}}',
  );
  assert.equal(r.ok, true);
  assert.equal({}.polluted, undefined);
});

test("fence language constant is stable", () => {
  assert.equal(WIDGET_FENCE_LANGUAGE, "buzz-widget");
});
