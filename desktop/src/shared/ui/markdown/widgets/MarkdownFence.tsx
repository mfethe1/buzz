import * as React from "react";

import { MarkdownCodeBlock } from "../CodeBlock";
import { widgetFenceText } from "./fenceText";
import { parseWidget, WIDGET_FENCE_LANGUAGE } from "./schema";
import { WidgetView } from "./WidgetView";

/**
 * Render one fenced block: a widget when the fence declares the widget
 * language and its payload parses, otherwise the ordinary code block.
 *
 * Invalid payloads fall through to a plain code block on purpose: a malformed
 * or unknown widget must stay readable, never blank.
 *
 * This lives beside the widget code rather than inside `markdown.tsx` so the
 * fence-dispatch rule has one home, and the widget feature does not grow an
 * already-oversized module (desktop file-size ratchet).
 */
export function MarkdownFence({
  language,
  children,
}: {
  language?: string;
  children?: React.ReactNode;
}) {
  if (language === WIDGET_FENCE_LANGUAGE) {
    const parsed = parseWidget(widgetFenceText(children));
    if (parsed.ok) return <WidgetView widget={parsed.widget} />;
  }
  return <MarkdownCodeBlock language={language}>{children}</MarkdownCodeBlock>;
}
