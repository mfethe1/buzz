export const meta = {
  name: 'agent-mesh-iterate',
  description: 'One improvement iteration on the multi-machine agent mesh design: resolve open items, verify, judge, and report hygiene',
  whenToUse: 'Invoked each tick of the agent-mesh /loop. Pass args {focus, iteration, carry} where carry is the prior judge feedback.',
  phases: [
    { title: 'Select', detail: 'pick the highest-value unresolved item' },
    { title: 'Investigate', detail: 'trace it in the codebase, propose a precise change' },
    { title: 'Verify', detail: 'audit citations and adversarially check the proposal' },
    { title: 'Judge', detail: 'accept, or send back with specifics' },
    { title: 'Hygiene', detail: 'dead code, stale worktrees, leftover artifacts' },
  ],
}

const REPO = 'E:/Projects/buzz/.claude/worktrees/claude-oauth-env'
const DOC = `${REPO}/docs/agent-identity-sync.md`
const COMPANION = `${REPO}/docs/multi-machine-agent-coordination.md`

const focus = (args && args.focus) || 'the highest-value unresolved item in the document'
const iteration = (args && args.iteration) || 1
const carry = (args && args.carry) || null

const preamble = `Repo root (a git worktree — stay strictly inside it): ${REPO}
Primary document: ${DOC}
Companion document: ${COMPANION}

Buzz is a Nostr-based (NIP-29) chat/agent platform: Rust relay, Tauri 2 + React
desktop, Flutter mobile. Event kinds live in crates/buzz-core/src/kind.rs.

HARD RULE ON EVIDENCE: this project has already shipped one design document
containing FABRICATED file:line citations and a fabricated NIP, and a second
whose load-bearing claims were misreads of real code. Every citation you give
must be a file you personally opened at the line you cite. If you cannot verify
something, write "could not verify" — never approximate, never infer a line
number. A wrong citation is worse than no citation here.

DO NOT EDIT ANY FILES. You investigate and propose. The orchestrator applies
changes. This keeps the tree clean and avoids accumulating worktrees.`

// ── Select ────────────────────────────────────────────────────────────────
phase('Select')

const PLAN_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['item', 'rationale', 'subtasks', 'kind'],
  properties: {
    item: { type: 'string' },
    rationale: { type: 'string' },
    kind: { type: 'string', enum: ['verify-unverified', 'design-gap', 'code-change', 'test', 'decision'] },
    subtasks: {
      type: 'array',
      minItems: 2,
      maxItems: 3,
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['label', 'prompt'],
        properties: {
          label: { type: 'string' },
          prompt: { type: 'string' },
        },
      },
    },
  },
}

const selected = await agent(`${preamble}

TASK: Select the single highest-value unresolved item to advance this iteration.

Iteration: ${iteration}
Caller's focus hint: ${focus}
${carry ? `\nPrior judge sent work back with this feedback — prefer addressing it:\n${JSON.stringify(carry, null, 2)}` : ''}

Read the document. Its section 9 is a block of [unverified] claims taken from a
review and never personally traced; section 10 lists open questions; section 6
stages the work.

Prefer, in order:
1. Anything the prior judge sent back.
2. [unverified] claims that are cheap to settle by reading code — these convert
   directly into document accuracy, which is this project's known weak point.
3. Open questions answerable from the codebase (as opposed to product decisions
   only the owner can make).
4. Design gaps.

Do NOT select an item that requires a product/security decision from the owner —
flag those as kind:"decision" only if nothing else remains.

Split it into 2-3 independent subtasks, each with a self-contained prompt for an
investigator who has not read this conversation. Each prompt must name specific
files or symbols to start from.`,
  { label: 'select', phase: 'Select', schema: PLAN_SCHEMA })

log(`iteration ${iteration}: ${selected.item}`)

// ── Investigate → Verify → Judge, with one send-back round ────────────────
const FINDING_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['summary', 'verified', 'proposed_change', 'citations', 'confidence'],
  properties: {
    summary: { type: 'string' },
    verified: { type: 'boolean' },
    proposed_change: { type: 'string' },
    citations: { type: 'array', items: { type: 'string' } },
    confidence: { type: 'string', enum: ['low', 'medium', 'high'] },
  },
}

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['bad_citations', 'overreach', 'verdict'],
  properties: {
    bad_citations: { type: 'array', items: { type: 'string' } },
    overreach: { type: 'array', items: { type: 'string' } },
    verdict: { type: 'string', enum: ['clean', 'minor-fixes', 'unsound'] },
  },
}

const JUDGE_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['accept', 'headline', 'apply', 'send_back', 'next_focus'],
  properties: {
    accept: { type: 'boolean' },
    headline: { type: 'string' },
    apply: { type: 'array', items: { type: 'string' } },
    send_back: { type: 'array', items: { type: 'string' } },
    next_focus: { type: 'string' },
  },
}

async function round(feedback) {
  phase('Investigate')
  const findings = await parallel(selected.subtasks.map((s) => () =>
    agent(`${preamble}

WORK ITEM: ${selected.item}
SUBTASK: ${s.prompt}
${feedback ? `\nA judge rejected the previous attempt. Address this specifically:\n${feedback}` : ''}

Trace it in the code. Report what is actually true, and propose the precise
change to the document (quote the exact replacement prose) or to the code
(describe the edit and the file it lands in). Set verified=false if the claim
you investigated turns out to be wrong.`,
      { label: `dig:${s.label}`, phase: 'Investigate', schema: FINDING_SCHEMA })))

  const live = findings.filter(Boolean)

  phase('Verify')
  const checks = await parallel([
    () => agent(`${preamble}

TASK: Audit every citation in these findings by opening the file at the line.

${JSON.stringify(live, null, 2)}

Report any citation that does not show what it is claimed to show. Be exact
about what is actually at that line.`,
      { label: 'audit:citations', phase: 'Verify', schema: VERIFY_SCHEMA }),
    () => agent(`${preamble}

TASK: Adversarially check these findings for OVERREACH — conclusions wider than
the evidence supports.

${JSON.stringify(live, null, 2)}

Where does a proposal generalize from one code path to a system-wide claim?
Where does it assume a default, a deployment topology, or a config value it did
not verify? Where would it be false on a multi-pod relay, on mobile, or with the
relevant feature flag off?`,
      { label: 'audit:overreach', phase: 'Verify', schema: VERIFY_SCHEMA }),
  ])

  phase('Judge')
  const verdict = await agent(`${preamble}

TASK: You are the JUDGE for iteration ${iteration}. Decide whether this work is
good enough to apply to the repository.

WORK ITEM: ${selected.item}

FINDINGS:
${JSON.stringify(live, null, 2)}

VERIFICATION:
${JSON.stringify(checks.filter(Boolean), null, 2)}

Accept only if: every citation checks out, no finding overreaches its evidence,
and the proposed change is concrete enough to apply without further
interpretation.

If you accept, list in "apply" the exact changes the orchestrator should make —
each one specific enough to execute directly.
If you reject, list in "send_back" precisely what the investigators must redo.
Either way, set "next_focus" to the item the NEXT iteration should take up.

Do not accept work merely because it is plausible. This project's failure mode
is confident, well-written, unverified claims.`,
    { label: 'judge', phase: 'Judge', schema: JUDGE_SCHEMA, effort: 'high' })

  return { findings: live, checks: checks.filter(Boolean), verdict }
}

let result = await round(null)
if (!result.verdict.accept) {
  log(`judge sent work back: ${result.verdict.headline}`)
  result = await round(result.verdict.send_back.join('\n'))
}

// ── Hygiene ───────────────────────────────────────────────────────────────
phase('Hygiene')

const HYGIENE_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['dead_code', 'stale_artifacts', 'worktree_notes', 'action_required'],
  properties: {
    dead_code: { type: 'array', items: { type: 'string' } },
    stale_artifacts: { type: 'array', items: { type: 'string' } },
    worktree_notes: { type: 'array', items: { type: 'string' } },
    action_required: { type: 'boolean' },
  },
}

const hygiene = await agent(`${preamble}

TASK: Hygiene sweep for iteration ${iteration}.

Report ONLY things introduced by this design effort — do not audit the whole
repository, and do not propose touching other people's in-flight branches.

Check:
1. Dead code or unreferenced symbols added on branch design/tailnet-agent-mesh.
   Use: git diff --stat origin/main...HEAD  and inspect what it touched.
2. Stale artifacts in the working tree: leftover scratch files, generated
   output, .orig/.rej, editor backups, anything untracked that should not be.
   Use: git status --porcelain
3. Worktrees under .claude/worktrees/. Report which ones exist and whether each
   has unpushed commits or uncommitted changes. DO NOT propose deleting any that
   have unpushed work. This is a report, not an action.

Set action_required=true only if something introduced by THIS effort needs
cleanup.`,
  { label: 'hygiene', phase: 'Hygiene', schema: HYGIENE_SCHEMA })

return {
  iteration,
  item: selected.item,
  kind: selected.kind,
  accepted: result.verdict.accept,
  headline: result.verdict.headline,
  apply: result.verdict.apply,
  send_back: result.verdict.send_back,
  next_focus: result.verdict.next_focus,
  findings: result.findings,
  hygiene,
}
