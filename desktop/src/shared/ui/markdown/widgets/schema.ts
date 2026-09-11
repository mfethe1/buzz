/**
 * Agent-authored widget payloads (AGENT-WIDGETS-001).
 *
 * Agents publish structured data; the client owns the rendering. Nothing here
 * ever produces HTML — a payload is parsed, validated, and handed to a
 * client-shipped React component, or it is rejected and the caller falls back
 * to rendering the fence as an ordinary code block.
 *
 * The widget type lives *inside* the JSON rather than in the fence info string
 * because `extractLanguage` keeps only the first token of the info string, so
 * ```` ```buzz-widget kanban ```` would arrive as `language-buzz-widget` with
 * `kanban` silently dropped.
 */

/** Fence info string that marks a payload as a widget. */
export const WIDGET_FENCE_LANGUAGE = "buzz-widget";

/**
 * Upper bound on a payload, in UTF-16 code units. A hostile agent must not be
 * able to wedge the renderer with a multi-megabyte fence; oversized payloads
 * degrade to a code block, which is virtualized and cheap.
 */
export const MAX_WIDGET_PAYLOAD_CHARS = 32 * 1024;

/** Schema version understood by this client. */
export const WIDGET_SCHEMA_VERSION = 1;

export type TableWidget = {
  v: 1;
  type: "table";
  columns: string[];
  rows: string[][];
  caption?: string;
};

export type MetricEntry = {
  label: string;
  value: string;
  /** Optional signed change, rendered with directional emphasis. */
  delta?: string;
  unit?: string;
};

export type MetricWidget = {
  v: 1;
  type: "metric";
  metrics: MetricEntry[];
  caption?: string;
};

export type Widget = TableWidget | MetricWidget;

/** Widget types this client is willing to render. Client-side allowlist. */
export const WIDGET_TYPES = ["table", "metric"] as const;

export type WidgetType = (typeof WIDGET_TYPES)[number];

export type WidgetParseResult =
  | { ok: true; widget: Widget }
  | { ok: false; reason: string };

const isRecord = (x: unknown): x is Record<string, unknown> =>
  typeof x === "object" && x !== null && !Array.isArray(x);

const isStringArray = (x: unknown): x is string[] =>
  Array.isArray(x) && x.every((s) => typeof s === "string");

const fail = (reason: string): WidgetParseResult => ({ ok: false, reason });

function parseTable(o: Record<string, unknown>): WidgetParseResult {
  if (!isStringArray(o.columns)) return fail("table.columns must be string[]");
  if (o.columns.length === 0) return fail("table.columns must be non-empty");
  if (!Array.isArray(o.rows) || !o.rows.every(isStringArray)) {
    return fail("table.rows must be string[][]");
  }
  const width = o.columns.length;
  if (!o.rows.every((r) => r.length === width)) {
    return fail("table.rows must all match the column count");
  }
  if (o.caption !== undefined && typeof o.caption !== "string") {
    return fail("table.caption must be a string");
  }
  return { ok: true, widget: o as unknown as TableWidget };
}

function parseMetric(o: Record<string, unknown>): WidgetParseResult {
  if (!Array.isArray(o.metrics) || o.metrics.length === 0) {
    return fail("metric.metrics must be a non-empty array");
  }
  const optional = (v: unknown) => v === undefined || typeof v === "string";
  const valid = o.metrics.every(
    (m) =>
      isRecord(m) &&
      typeof m.label === "string" &&
      typeof m.value === "string" &&
      optional(m.delta) &&
      optional(m.unit),
  );
  if (!valid) return fail("metric.metrics entries need string label + value");
  if (o.caption !== undefined && typeof o.caption !== "string") {
    return fail("metric.caption must be a string");
  }
  return { ok: true, widget: o as unknown as MetricWidget };
}

const PARSERS: Record<
  WidgetType,
  (o: Record<string, unknown>) => WidgetParseResult
> = { table: parseTable, metric: parseMetric };

/**
 * Parse and validate a `buzz-widget` fence body.
 *
 * Never throws and never returns partially-validated data: callers render the
 * widget on `ok`, and fall back to a plain code block on failure.
 */
export function parseWidget(raw: string): WidgetParseResult {
  if (raw.length > MAX_WIDGET_PAYLOAD_CHARS) return fail("payload too large");

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return fail("payload is not valid JSON");
  }
  if (!isRecord(parsed)) return fail("payload must be a JSON object");
  if (parsed.v !== WIDGET_SCHEMA_VERSION) {
    return fail(`unsupported schema version: ${String(parsed.v)}`);
  }
  if (typeof parsed.type !== "string") return fail("payload.type is required");

  const parser = PARSERS[parsed.type as WidgetType];
  if (!parser) return fail(`unknown widget type: ${parsed.type}`);
  return parser(parsed);
}
