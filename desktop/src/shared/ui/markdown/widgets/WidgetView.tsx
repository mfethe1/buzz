import * as React from "react";

import { useSmoothCorners } from "@/shared/ui/smoothCorners";

import type { MetricWidget, TableWidget, Widget } from "./schema";

/**
 * Read-only widget renderers (AGENT-WIDGETS-001, PR-1).
 *
 * Every value below arrives from an agent and is rendered as text through JSX,
 * so React escapes it. Nothing here uses `dangerouslySetInnerHTML`, and no
 * value reaches an HTML parser.
 */

function WidgetFrame({
  caption,
  children,
}: {
  caption?: string;
  children: React.ReactNode;
}) {
  const frameRef = React.useRef<HTMLDivElement | null>(null);
  useSmoothCorners(frameRef);

  return (
    <div
      ref={frameRef}
      className="overflow-x-auto rounded-2xl border border-border/70"
      data-widget-block=""
    >
      {children}
      {caption && (
        <div className="border-t border-border/70 px-3 py-1.5 text-xs text-muted-foreground/70">
          {caption}
        </div>
      )}
    </div>
  );
}

function TableWidgetView({ widget }: { widget: TableWidget }) {
  return (
    <WidgetFrame caption={widget.caption}>
      <table className="w-full border-collapse text-left text-sm">
        <thead>
          <tr>
            {widget.columns.map((column, i) => (
              <th
                // biome-ignore lint/suspicious/noArrayIndexKey: agent data has no stable identity
                key={`${i}:${column}`}
                className="border-b border-border/70 px-3 py-1.5 font-semibold"
              >
                {column}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {widget.rows.map((row, r) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: agent data has no stable identity
            <tr key={`${r}:${row.join("\u0000")}`}>
              {row.map((cell, c) => (
                <td
                  // biome-ignore lint/suspicious/noArrayIndexKey: agent data has no stable identity
                  key={`${c}:${cell}`}
                  className="border-b border-border/40 px-3 py-1.5"
                >
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </WidgetFrame>
  );
}

function MetricWidgetView({ widget }: { widget: MetricWidget }) {
  return (
    <WidgetFrame caption={widget.caption}>
      <div className="flex flex-wrap gap-x-6 gap-y-3 px-3 py-2.5">
        {widget.metrics.map((metric, i) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: agent data has no stable identity
          <div key={`${i}:${metric.label}`} className="min-w-24">
            <div className="text-xs text-muted-foreground/70">
              {metric.label}
            </div>
            <div className="flex items-baseline gap-1">
              <span className="text-lg font-semibold tabular-nums">
                {metric.value}
              </span>
              {metric.unit && (
                <span className="text-xs text-muted-foreground/70">
                  {metric.unit}
                </span>
              )}
              {metric.delta && (
                <span className="text-xs tabular-nums text-muted-foreground">
                  {metric.delta}
                </span>
              )}
            </div>
          </div>
        ))}
      </div>
    </WidgetFrame>
  );
}

/** Render a validated widget. Unknown types are unreachable — `parseWidget` gates them. */
export function WidgetView({ widget }: { widget: Widget }) {
  switch (widget.type) {
    case "table":
      return <TableWidgetView widget={widget} />;
    case "metric":
      return <MetricWidgetView widget={widget} />;
  }
}
