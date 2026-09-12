import * as React from "react";

/**
 * Recover the raw text of a fenced code block from its rendered children.
 *
 * `react-markdown` hands the `pre` handler a `<code>` element whose children
 * are the fence body, split into an arbitrary number of string nodes (syntax
 * plugins may fragment it further). Widgets need the exact original text to
 * parse as JSON, so this walks the tree and concatenates every string leaf.
 *
 * Non-string leaves are ignored rather than coerced: a fence containing real
 * elements is not a valid payload, and `JSON.parse` will reject the remainder.
 */
export function widgetFenceText(children: React.ReactNode): string {
  let out = "";
  const walk = (node: React.ReactNode): void => {
    if (typeof node === "string") {
      out += node;
      return;
    }
    if (typeof node === "number") {
      out += String(node);
      return;
    }
    if (Array.isArray(node)) {
      node.forEach(walk);
      return;
    }
    if (React.isValidElement<{ children?: React.ReactNode }>(node)) {
      walk(node.props?.children);
    }
  };
  walk(children);
  return out;
}
