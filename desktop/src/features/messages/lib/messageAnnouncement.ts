import { isValidElement, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import remarkSpoilers from "@/shared/lib/remarkSpoilers";

function visibleText(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(visibleText).join("");
  if (!isValidElement<{ children?: ReactNode; alt?: string }>(node)) return "";
  if (node.type === "spoiler") return "Hidden spoiler";
  if (node.type === "img") return node.props.alt ?? "";
  const text = visibleText(node.props.children);
  return typeof node.type === "string" &&
    ["p", "br", "li", "th", "td", "tr", "pre"].includes(node.type)
    ? `${text} `
    : text;
}

/** Announce the parsed visible text, never concealed content or link targets. */
export function messageAnnouncement(body: string): string {
  // Use the same Markdown and spoiler parser as the message renderer. A regex
  // would disagree on code, formatted links, block spoilers, and GFM tables.
  const rendered = ReactMarkdown({
    children: body,
    remarkPlugins: [remarkGfm, remarkSpoilers],
  });
  return visibleText(rendered).replace(/\s+/g, " ").trim();
}
