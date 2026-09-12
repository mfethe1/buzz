import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";

import { widgetFenceText } from "./fenceText.ts";
import { parseWidget, WIDGET_FENCE_LANGUAGE } from "./schema.ts";

// Mirrors the `pre` handler in markdown.tsx: classify the fence, render a
// widget when the payload validates, otherwise fall back to a code block.
function renderDoc(markdown) {
  return renderToStaticMarkup(
    React.createElement(
      ReactMarkdown,
      {
        components: {
          pre: ({ children }) => {
            let language = "";
            React.Children.forEach(children, (child) => {
              if (
                React.isValidElement(child) &&
                typeof child.props?.className === "string"
              ) {
                const m = child.props.className.match(/language-(\S+)/);
                language = m ? m[1] : "";
              }
            });
            if (language === WIDGET_FENCE_LANGUAGE) {
              const parsed = parseWidget(widgetFenceText(children));
              if (parsed.ok) {
                return React.createElement(
                  "div",
                  { "data-widget-block": "", "data-type": parsed.widget.type },
                  JSON.stringify(parsed.widget.rows ?? parsed.widget.metrics),
                );
              }
            }
            return React.createElement("pre", {}, children);
          },
        },
      },
      markdown,
    ),
  );
}

const fence = (body) =>
  `\u0060\u0060\u0060${WIDGET_FENCE_LANGUAGE}\n${body}\n\u0060\u0060\u0060`;

test("e2e: a valid table payload renders as a widget, not a code block", () => {
  const html = renderDoc(
    fence('{"v":1,"type":"table","columns":["PR"],"rows":[["#15"]]}'),
  );
  assert.match(html, /data-widget-block/);
  assert.match(html, /data-type="table"/);
  assert.doesNotMatch(html, /<pre>/);
});

test("e2e: an unknown widget type degrades to a readable code block", () => {
  const html = renderDoc(fence('{"v":1,"type":"kanban"}'));
  assert.match(html, /<pre>/);
  assert.doesNotMatch(html, /data-widget-block/);
  // The payload stays visible to the user rather than vanishing.
  assert.match(html, /kanban/);
});

test("e2e: malformed JSON degrades to a code block", () => {
  const html = renderDoc(fence("{not json"));
  assert.match(html, /<pre>/);
});

test("e2e: an ordinary code fence is untouched", () => {
  const html = renderDoc("```js\nconst a = 1;\n```");
  assert.match(html, /<pre>/);
  assert.doesNotMatch(html, /data-widget-block/);
});

test("e2e: hostile cell content is escaped in the rendered output", () => {
  const html = renderDoc(
    fence(
      '{"v":1,"type":"table","columns":["c"],"rows":[["<img src=x onerror=alert(1)>"]]}',
    ),
  );
  assert.match(html, /data-widget-block/);
  // No live tag reaches the DOM — it survives only as escaped text.
  assert.doesNotMatch(html, /<img/);
  assert.match(html, /&lt;img/);
});

test("e2e: a script payload inside a widget fence never becomes a tag", () => {
  const html = renderDoc(
    fence(
      '{"v":1,"type":"table","columns":["c"],"rows":[["<script>alert(1)</script>"]]}',
    ),
  );
  assert.doesNotMatch(html, /<script/);
});
