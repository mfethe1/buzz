/**
 * Fleet-build gate: `ensureWelcomeTeamUnlessSkipped` must not provision the
 * built-in Welcome Team when the build bakes `skip_welcome_team_provisioning`
 * as true, and must behave exactly like `ensureWelcomeTeam` otherwise.
 *
 * `welcomeGuide.ts` reaches the Tauri bridge via `@/shared/api/tauri`, so the
 * bridge is stubbed with `registerHooks` — no window/Tauri runtime needed.
 */

import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import test from "node:test";

const STUB_URL = "buzz-welcome-skip-stub:tauri";

// The stubbed module reads this module-global via globalThis so tests can
// flip the baked build flag between cases.
globalThis.__buzzTestSkipWelcomeTeam = false;

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier === "@/shared/api/tauri") {
      return { shortCircuit: true, url: STUB_URL };
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url === STUB_URL) {
      return {
        format: "module",
        shortCircuit: true,
        source: [
          "export async function invokeTauri(command, args) {",
          "  if (command === 'skip_welcome_team_provisioning') {",
          "    return globalThis.__buzzTestSkipWelcomeTeam;",
          "  }",
          "  throw new Error('unexpected command: ' + command + ' args=' + JSON.stringify(args));",
          "}",
          // welcomeGuide's transitive imports (via instanceInputForDefinition /
          // tauriAcpDiscovery) need these named exports to exist; they are
          // never called in the skip path, so inert functions suffice.
          "export function fromRawAcpRuntimeCatalogEntry() {",
          "  throw new Error('unexpected bridge call: fromRawAcpRuntimeCatalogEntry');",
          "}",
          "export async function addChannelMembers() {",
          "  throw new Error('unexpected bridge call: addChannelMembers');",
          "}",
          "export async function createManagedAgent() {",
          "  throw new Error('unexpected bridge call: createManagedAgent');",
          "}",
          "export async function getChannelMembers() {",
          "  throw new Error('unexpected bridge call: getChannelMembers');",
          "}",
          "export async function listManagedAgents() {",
          "  throw new Error('unexpected bridge call: listManagedAgents');",
          "}",
          "export async function updateManagedAgent() {",
          "  throw new Error('unexpected bridge call: updateManagedAgent');",
          "}",
        ].join("\n"),
      };
    }
    return nextLoad(url, context);
  },
});

const { ensureWelcomeTeamUnlessSkipped } = await import("./welcomeGuide.ts");

test("fleet build: provisioning is skipped and returns null", async () => {
  globalThis.__buzzTestSkipWelcomeTeam = true;
  // If provisioning were attempted it would invoke the bridge with channel
  // commands (listManagedAgents etc.) and this test would fail on the
  // "unexpected command" stub error.
  const result = await ensureWelcomeTeamUnlessSkipped("channel-1", null);
  assert.equal(result, null);
});

test("oss build: provisioning is not silently skipped", async () => {
  globalThis.__buzzTestSkipWelcomeTeam = false;
  // OSS path delegates to ensureWelcomeTeam, which attempts real provisioning
  // and fails fast in a stubbed environment (unexpected bridge command or a
  // missing persona). The contract is "no silent skip": never resolve null.
  let returnedNull = false;
  let rejected = false;
  try {
    const result = await ensureWelcomeTeamUnlessSkipped("channel-2", null);
    returnedNull = result === null;
  } catch {
    rejected = true;
  }
  assert.equal(returnedNull, false);
  assert.equal(rejected, true);
});
