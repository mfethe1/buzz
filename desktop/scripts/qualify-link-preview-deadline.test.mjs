// Copied-process protocol controls. Native deadline behavior is qualified by
// link_preview_scheduler_tests.rs, not by this synthetic response fixture.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { once } from "node:events";
import http from "node:http";
import test from "node:test";
import { promisify } from "node:util";

const run = promisify(execFile);
const desktop = new URL("../", import.meta.url);
const cases = [
  ["reverse arrival", true],
  ["dispatch waits for readiness request", true],
  ["one request never arrives", false, /fixture readiness timed out/],
  ["readiness is false", false, /fixture did not confirm readiness/],
  [
    "response body is no longer live",
    false,
    /both slow response bodies must be live/,
  ],
  ["readiness is stale", false, /readiness must precede native settlement/],
  ["third request started early", false, /only the two slow paths/],
  ["readiness refused", false, /503 !== 200/],
  [
    "readiness response too large",
    false,
    /fixture response exceeds 8192 bytes/,
  ],
  ["acknowledgement refused", false, /fixture lost readiness/],
  ["readiness response never completes", false, /TimeoutError/],
];
for (const [scenario, passes, reason] of cases) {
  test(scenario, { timeout: 10_000 }, async () => {
    const paths = [];
    const pending = new Map();
    let acknowledgements = 0;
    let readinessRequests = 0;
    let dispatchReady;
    const dispatched = new Promise((resolve) => {
      dispatchReady = resolve;
    });
    const reply = (response, value) => {
      response.setHeader("content-type", "application/json");
      response.end(JSON.stringify(value));
    };
    const snapshot = () => ({
      ready: true,
      paths: [...paths],
      liveBodies: 2,
      completed: 0,
    });
    const server = http.createServer(async (request, response) => {
      let body = "";
      for await (const chunk of request) {
        body += chunk;
        assert.ok(body.length < 4096);
      }
      const { command, args } = JSON.parse(body);
      if (command === "fetch_link_preview_metadata") {
        const path = new URL(args.href).pathname;
        if (path === "/fast") {
          paths.push(path);
          reply(response, { ok: { title: "Fast preview" } });
          return;
        }
        pending.set(path, response);
        if (pending.size === 2) dispatchReady();
        if (scenario === "reverse arrival") {
          if (pending.size === 2) paths.push("/slow-two", "/slow-one");
        } else if (path === "/slow-one") {
          paths.push(path);
        }
        return;
      }
      if (command === "fixture_paths") {
        reply(response, { ok: paths }); // Supports the original red control.
        return;
      }
      if (command === "fixture_wait_ready") {
        readinessRequests += 1;
        await dispatched;
        if (scenario !== "reverse arrival") paths.push("/slow-two");
        if (scenario === "one request never arrives") {
          reply(response, { error: "fixture readiness timed out" });
        } else if (scenario === "readiness refused") {
          response.statusCode = 503;
          reply(response, { ok: snapshot() });
        } else if (scenario === "readiness response never completes") {
          response.writeHead(200, { "content-type": "application/json" });
          response.write('{"ok":');
        } else if (scenario === "readiness response too large") {
          reply(response, { ok: { ...snapshot(), padding: "x".repeat(9000) } });
        } else {
          const value = snapshot();
          if (scenario === "readiness is false") value.ready = false;
          if (scenario === "response body is no longer live")
            value.liveBodies = 1;
          if (scenario === "readiness is stale") value.completed = 1;
          if (scenario === "third request started early")
            value.paths.push("/fast");
          reply(response, { ok: value });
        }
        return;
      }
      if (command === "fixture_ready_ack") {
        acknowledgements += 1;
        if (scenario === "acknowledgement refused") {
          reply(response, { error: "fixture lost readiness" });
          return;
        }
        reply(response, { ok: snapshot() });
        for (const slow of pending.values()) {
          reply(slow, { error: "link preview operation timed out" });
        }
        return;
      }
      reply(response, { ok: null });
    });
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    try {
      const result = await run(
        process.execPath,
        [
          "--import",
          "./test-loader.mjs",
          "--experimental-strip-types",
          "./scripts/qualify-link-preview-deadline.mjs",
          `http://127.0.0.1:${server.address().port}/invoke`,
        ],
        { cwd: desktop, timeout: 6000, maxBuffer: 32 * 1024 },
      ).then(
        (output) => ({ ...output, code: 0 }),
        (error) => error,
      );
      assert.equal(result.code === 0, passes, result.stderr ?? result.message);
      if (passes) {
        assert.equal(readinessRequests, 1);
        assert.equal(acknowledgements, 1);
        assert.deepEqual(paths.slice(0, 2).sort(), ["/slow-one", "/slow-two"]);
        assert.equal(paths[2], "/fast");
        assert.match(result.stdout, /"nativeTimeouts":2/);
      } else {
        assert.equal(
          acknowledgements,
          scenario === "acknowledgement refused" ? 1 : 0,
          "invalid readiness was acknowledged",
        );
        assert.equal(
          result.code,
          1,
          "child must fail itself, not reach outer kill timeout",
        );
        assert.match(result.stderr, reason);
        assert.doesNotMatch(result.stdout ?? "", /"result":"PASS"/);
      }
    } finally {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    }
  });
}
