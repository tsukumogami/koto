# Lead: What should `requires_confirmation` mean, and what would confirm-before-execute cost?

## Findings

### 1. What the design intended

Issue #71's acceptance criteria state the intent in plain language:

> Template schema supports marking an action as requiring confirmation (irreversible
> flag), **preventing auto-execution**

That's confirm-before-execute, stated as an AC, not a stretch reading. The design doc
(`docs/designs/current/DESIGN-default-action-execution.md`) frames it the same way in
the security section, line 442-446:

> **Reversibility constraint.** The `requires_confirmation` flag prevents irreversible
> actions from auto-executing. The engine enforces this at the loop level — when the
> flag is set, the loop stops and returns to the caller.

"Prevents... from auto-executing" and "the loop stops" both read as pre-execution
gating. But the design's own architecture section contradicts this two pages earlier.
Decision 1's numbered execution order (lines 110-122) puts action execution and the
confirmation stop in the *same* step:

```
6. Action execution: call action closure, append DefaultActionExecuted event
   - If requires_confirmation: stop loop with ActionRequiresConfirmation
```

The closure runs the command, the event is appended, *then* the confirmation stop
fires — all inside step 6. The `ActionResult` enum defined at lines 291-299 makes this
concrete: `RequiresConfirmation { exit_code, stdout, stderr }` carries the executed
command's output as its payload. You cannot construct that variant without having
already run the command. The design's prose (security section) asserts prevention;
the design's own interface definition (architecture section) implements confirm-after.
These are irreconcilable within the same document, and nothing in the doc flags the
tension — it reads as unnoticed, not resolved.

The parent design, `DESIGN-shirabe-work-on-template.md`, never actually pairs
`requires_confirmation: true` with an irreversible action. Its reversibility table
(lines 541-548) lists the five states that get `default_action`, and all five are
marked reversible (file overwrite, branch deletion, read-only, read-only, read-only).
The one irreversible state in the template, `pr_creation` (line 688: "judgment
(irreversible)"), is *not* in that table — it's a judgment state where the agent
calls `gh pr create` directly through its own tool layer and submits evidence, not a
`default_action` state at all. So the parent design's actual state classification
never exercises `requires_confirmation` on anything irreversible. The engine capability
was built speculatively, for a use case the shipped template doesn't use. Issue #71
names "PR creation, posting comments" as the motivating irreversible actions, but no
current template routes either through `default_action`.

No PR discussion caught this: PR #75 (which merged this design) has zero review
comments and one reviewer-less author-merge. The test suite encodes confirm-after as
the expected, tested behavior (`tests/next_response_baseline.rs:166-206`,
`tests/integration_test.rs:3983-4011`, `src/engine/advance.rs:2878-2936`) without
comment on whether that matches the AC. The advance.rs unit test is the clearest
tell: it stubs `ActionResult::RequiresConfirmation { stdout: "PR #42 created", .. }`
(`src/engine/advance.rs:2903`) — the test author wrote a PR-creation scenario and had
the stub report the PR as already created, seemingly without registering that this
contradicts "preventing auto-execution."

### 2. What the code does

Tracing the full path, execution happens unconditionally and confirmation is decided
only afterward:

- `src/template/types.rs:205` — `requires_confirmation: bool` on `ActionDecl`, a plain
  flag with no engine-visible semantics of its own.
- `src/template/compile.rs:243` — copied verbatim from the source template into the
  compiled `ActionDecl`; no compile-time interpretation.
- `src/engine/advance.rs:283-311` (step 5 in the loop, called out in the design as step
  6) — the engine calls `execute_action(&state, action, has_evidence)` and pattern
  matches on the result. It does not consult `action.requires_confirmation` itself;
  the flag's effect is entirely inside the closure the CLI supplies. The engine only
  reacts to which `ActionResult` variant comes back.
- `src/cli/mod.rs:3970-3974` — the closure checks `has_evidence` first and returns
  `ActionResult::Skipped` without running anything if override evidence exists (this
  is the override path, unaffected by `requires_confirmation`).
- `src/cli/mod.rs:3985-4020` — variable substitution, working-dir resolution, then
  unconditional execution: `crate::action::run_shell_command(&command, &wd, 30)` (or
  `execute_with_polling` for polling actions). This runs regardless of
  `action.requires_confirmation`. The `DefaultActionExecuted` event is appended right
  after (`src/cli/mod.rs:4030-4036`), also unconditionally.
- `src/cli/mod.rs:4038-4048` — only *now* does the closure branch on
  `action.requires_confirmation`: `true` wraps the already-obtained output in
  `ActionResult::RequiresConfirmation`; `false` wraps it in `ActionResult::Executed`.
  Both branches carry identical `exit_code`/`stdout`/`stderr` — the flag changes
  nothing about what ran, only how the result is labeled.
- `src/engine/advance.rs:297-311` — on `RequiresConfirmation`, the engine returns
  `StopReason::ActionRequiresConfirmation { state, exit_code, stdout, stderr }`
  without advancing.
- `src/cli/next_types.rs:104-111` and the `confirm` action-string mapping
  (`src/cli/next_types.rs:627`) surface this as `NextResponse::ActionRequiresConfirmation`
  with `action: "confirm"` and `action_output` holding the command's stdout/stderr/exit
  code, per `tests/next_response_baseline.rs:186-206`.

The command has already run and its event has already landed in the persisted log
before `koto next` returns anything to the caller. There is no engine-level or
CLI-level checkpoint before `run_shell_command` fires. "Confirmation" in the shipped
system means: the caller is shown the result and asked to acknowledge it via a normal
`koto next` call on the same state (any subsequent call re-enters the state, and
since there's no override evidence yet, re-runs the action again — see the discussion
of idempotency below).

### 3. Is confirm-after-execute defensible?

**The case for it.** For a genuinely reversible action, confirming the *result* is a
coherent, cheaper design. The agent gets to inspect what happened before the
workflow treats it as settled — useful when the gate that would normally verify the
outcome isn't expressive enough to catch every failure mode a human would recognize
(a script that exits 0 but prints a warning worth escalating). This is arguably a
richer checkpoint than the existing "gate passes → auto-advance" path, since it hands
the agent raw stdout/stderr instead of a boolean. If the schema's mental model is
"reversible actions get a lightweight after-the-fact human/agent sanity check before
advancing," calling that `requires_confirmation` isn't crazy — it's just named
wrong, because "confirmation" strongly implies gating an action that hasn't happened.

**The case against.** For an irreversible action — the only case the design and issue
actually motivate this feature with — a post-hoc checkpoint is not a checkpoint at
all. By the time anyone is asked to confirm, the side effect is external and durable
(a PR exists, a comment is posted, a resource is provisioned). "Confirm" implies
consent that can still be withheld; here consent can only be acknowledged after the
fact, which is a different speech act entirely. The security section's own framing
("prevents irreversible actions from auto-executing") describes a guarantee the code
doesn't provide, and issue #71's AC ("preventing auto-execution") is unambiguous
about which guarantee was wanted.

**Does the distinction map onto reversibility, and could the schema carry both
modes?** Yes, cleanly. The two readings aren't in tension with each other — they're
the correct policy for different reversibility classes:

- Reversible action → confirm-after-execute (or no confirmation at all, gate-driven)
  is fine; the cost of a wrong default is bounded and undoable.
- Irreversible action → only confirm-before-execute satisfies the stated safety
  goal; confirm-after is a no-op safety-wise.

The schema could carry both without a breaking change: keep `requires_confirmation`
meaning what it currently does (a post-execution acknowledgment gate for reversible
actions with rich output), and add a distinct field — e.g. `confirm_before: bool` or,
better, fold this into a `reversibility: reversible | irreversible` enum on
`ActionDecl` that the engine interprets directly rather than leaving interpretation
entirely to authors' judgment. `reversibility: irreversible` would mean "the engine
must not call `run_shell_command` before an explicit approval exists"; anything else
behaves as it does today. This also fixes the doc-vs-code contradiction by making the
irreversible path a distinct code path instead of a relabeling of the same path.

### 4. What confirm-before-execute would cost

This is a real feature, not a flag flip. Concretely, inside `advance_until_stop`
(`src/engine/advance.rs`):

- **New stop reason.** `StopReason::ActionPending { state: String, command: String,
  working_dir: String }` (or similar) — returned *before* calling `execute_action`,
  carrying the substituted command text so the caller can review exactly what would
  run, not a template placeholder.
- **New response field/action value.** A `NextResponse::ActionPendingApproval`
  variant, `action: "confirm_pending"` (distinct from today's `"confirm"`, which
  means "look at what happened"), with an `action_pending` object: `{ state,
  command, working_dir }`. This is a new wire contract that agents/skills must learn
  to handle, on top of the existing `"confirm"` value — two different meanings would
  otherwise collide under one action string.
- **A new event type.** `EventPayload::ActionPending { state, command }` appended
  when the engine halts for approval, so the pending state survives process restarts
  and is visible in `koto status`/the audit log. This mirrors `DefaultActionExecuted`
  (`src/engine/types.rs`) but records intent, not outcome.
- **What the agent submits to approve.** Not a bare `{"approve": true}` — that's
  forgeable/replayable across states and across calls, since `koto next` is
  idempotent-ish and re-entrant. The approval payload needs to bind to the *exact*
  pending command, not just the state name (a state could re-enter with a different
  substituted command across resumes if variables changed). Concretely:
  `koto next <session> --with-data '{"status": "approved", "command_hash":
  "<sha256 of substituted command>"}'`. The engine computes the hash of the
  actually-pending command and rejects the submission if the hash doesn't match
  (`NextError` variant, e.g. `StaleApproval`) — this is the anchor against a stale or
  substituted command being approved blind, and against an agent that cached an old
  directive text approving a since-changed command.
- **Where the hash is compared.** The pending command (and its hash) must be
  persisted in the event log at the moment of the stop (in `ActionPending`), and
  `handle_next` must re-derive current variables, re-substitute, and re-hash the
  *current* command before comparing against the persisted hash — not trust the
  hash the agent echoes back — otherwise the agent could fabricate an approval for
  any command by computing its own hash of anything and asserting it matches.
- **Resuming.** On the next `koto next` call with a matching approval, the engine
  needs a new entry point that skips straight to "run the already-approved command"
  rather than re-evaluating from step 1 of the loop — or, more simply, the approval
  becomes evidence-shaped data that `current_evidence` carries into a second pass
  through the loop, and the action closure checks for an `approved_command_hash` in
  evidence matching the just-substituted command before calling
  `run_shell_command`. This second design is less invasive: no new stop-loop
  re-entry logic, just a third branch in the existing has_evidence check at
  `src/cli/mod.rs:3970-3974` (today that check only distinguishes "evidence present
  → skip" from "no evidence → run"; it would need to distinguish "override evidence
  → skip", "matching approval evidence → run", "no or mismatched evidence for a
  pending irreversible action → halt with ActionPending").
- **Interaction with the existing evidence/override path.** Today, submitting *any*
  evidence in the same call skips the action entirely (`has_evidence` →
  `ActionResult::Skipped`, `src/cli/mod.rs:3970-3974`,
  `src/engine/advance.rs:294-296`). Confirm-before-execute needs a *narrower* check:
  evidence that is specifically an approval for *this* command must trigger
  execution, while evidence that's an override (a different `status` value, e.g.
  `"override"`) must still skip execution as today. That means the evidence schema
  for these states needs a discriminating field (`status: approved | override |
  blocked`), and the closure's dispatch logic gets a third branch instead of the
  current binary has-evidence check. This is the single largest behavioral change:
  the override-skips-everything invariant that today makes override paths safe
  without idempotent commands no longer holds uniformly — approval evidence must
  *cause* execution, which is the opposite polarity from every other evidence type
  in the system today.
- **Rewind while an action is pending.** `koto rewind` (`src/cli/mod.rs:1985`)
  currently clears evidence and event delivery markers for arrival-detection
  purposes (`src/engine/persistence.rs:1652` — "rewind clears prior evidence"). A
  pending `ActionPending` event needs the same treatment: rewinding out of a state
  with a pending irreversible action must invalidate that pending approval (delete
  or supersede the `ActionPending` event, or have the engine treat it as stale once
  a later rewind event exists in the log) so that a stale command-hash approval
  can't be replayed against a re-entered state after the workflow moved on and came
  back with different variables.
- **Cancel.** There's no existing "reject" evidence shape; today an agent that
  doesn't want an action to run either submits override evidence before the state is
  entered (preventing execution) or does nothing (which, under the current
  after-the-fact model, doesn't matter because it already ran). Under
  confirm-before, a rejection needs its own status value (`status: "rejected"`)
  routed to a recovery/failure transition, distinct from `blocked` (which today
  means "the action ran and failed" per the three-path model in
  `DESIGN-shirabe-work-on-template.md:526-533`) — rejecting a not-yet-run action is
  a different event from a failed one.

None of this is free, and most of it (the hash binding, the third evidence branch,
rewind invalidation) is exactly the kind of schema/engine surface the design
explicitly tried to avoid adding (`DESIGN-default-action-execution.md` decision
drivers: "Minimal engine API change: avoid restructuring advance_until_stop's core
loop"). Confirm-before-execute is not a bug fix to the existing mechanism; it's a
second mechanism that happens to reuse `ActionDecl`.

### 5. Is halting even the right shape?

Four alternatives, weighed against what shirabe already does:

- **Agent runs the command itself through its own tool layer; koto verifies via a
  gate.** This is the *status quo* for the one irreversible action that exists today
  (`pr_creation`) — the agent calls `gh pr create` directly and submits evidence; koto
  never touches the command. The scope doc for this exploration confirms this
  pattern works today with zero koto changes ("the three-path model works today with
  no koto changes — an action, a gate that independently verifies the outcome... Verified
  running", `wip/explore_koto-command-authority_scope.md:26-28`). What
  confirm-before-execute would add over this: (a) a canonical, auditable record of
  the *exact* command intended, hashed and logged before execution, rather than
  trusting the agent's own tool-call log; (b) a single choke point if multiple
  templates want the same policy, instead of every template author remembering to
  keep irreversible commands out of `default_action`. What it costs in exchange: all
  of section 4. Given that the pattern already works and is already used for the one
  irreversible action in the current template, the marginal benefit is mostly about
  defense-in-depth (stopping a template author from *accidentally* putting an
  irreversible command in `default_action` with `requires_confirmation: true` and
  believing it's gated), not about unlocking new capability.
- **Dry-run/echo mode.** `ActionDecl` gains a boolean or the substituted command is
  echoed without a `dry_run` concept in the shell — this doesn't generalize; most
  irreversible commands (`gh pr create`, `gh pr comment`) have no dry-run flag koto
  can rely on being present, and inventing one per command defeats the "commands are
  static strings" model the design deliberately keeps (`DESIGN-default-action-execution.md`
  security section: "commands are static strings... same threat model as gates").
  Not viable as a general mechanism.
- **A declared-reversibility field that changes engine behavior.** This is the
  `reversibility: reversible | irreversible` enum sketched in section 3. It's the
  cleanest fit: it makes the engine's behavior depend on a claim about the action's
  nature rather than a flag whose meaning has to be inferred, and it makes
  `requires_confirmation` (or its replacement) do exactly one uncontested thing per
  value. It still needs the section-4 machinery for the `irreversible` case, but it
  stops the current situation where a boolean is asked to mean two contradictory
  things depending on which part of the doc you read.
- **Leaving confirmation to the skill's prose.** I.e., don't build engine
  enforcement at all; document in the template's directive that the agent should
  pause and ask the user before an irreversible step, the way `pr_creation`'s
  judgment-state directive presumably already does. This is weaker than a gate — it
  relies on the agent reading and following prose — but it's honest about what it
  is, costs nothing, and matches the current working pattern for `pr_creation`.

**Recommendation on shape:** don't build confirm-before-execute as a generic
engine primitive right now. The one case that needs it (irreversible actions) is
already served, today, by keeping those actions out of `default_action` entirely
and letting the agent run them through its own tool layer with a koto gate
verifying the outcome — which is exactly what `pr_creation` already does. Section 4's
cost (new stop reason, new event type, hash-bound approval, a rewind-invalidation
path, and a rewrite of the override-evidence polarity) buys defense-in-depth against
a template author misusing `default_action`, not new capability. That's better spent
as a compile-time or authoring-time guard (see recommendation below) than as new
runtime machinery.

### 6. Who would actually confirm?

In shirabe's workflows, `koto next` is called by an agent in a loop, not by a human
directly. The scope doc frames this precisely: consent for *running commands at all*
was ruled to live at workflow invocation — "invoking a koto-backed workflow
authorizes every command that workflow bakes in"
(`wip/explore_koto-command-authority_scope.md:9-11`). Nothing in the current
`requires_confirmation` mechanism routes the confirmation to a human; the JSON goes
back to whatever process called `koto next`, which in shirabe's case is the same
agent that just triggered the action. An agent confirming its own already-executed
`gh pr create` isn't a safety control in any meaningful sense — it's the same actor
acknowledging its own output, with no new party in the loop who could have said no.

This matters for both readings. If `requires_confirmation` means "let the agent
inspect output before advancing" (the defensible reading for reversible actions),
agent-as-confirmer is fine — that's exactly the audience it should reach. But if it's
meant as the safety boundary for *irreversible* actions (what issue #71 and the design's
security section actually claim), agent-as-confirmer defeats the purpose: the same
non-human actor that would have run the command anyway is also the one being asked
to bless it, after it already ran. A control an agent grants itself, post-hoc, is not
a control on the agent. If irreversible-action gating is ever built for real, the
approval needs to reach a human — which `koto next`'s current CLI-request/CLI-response
shape doesn't provide any channel for (there's no notion of "pause and wait for an
out-of-band human signal" anywhere in the engine; every `koto next` call is
synchronous request/response with the calling process). Building that channel is a
larger, separate design problem than anything in section 4.

## Implications

- The current behavior is not merely mis-named — it fails the acceptance criteria
  that were written for it ("preventing auto-execution", issue #71). That's a defect
  against the issue's own stated bar, not just a naming quibble, even though the
  *design document's* architecture section independently and correctly specifies
  confirm-after semantics (so the code matches the architecture spec while
  contradicting the security-section prose and the issue AC in the same repo).
- Because the only irreversible template state (`pr_creation`) never actually uses
  `default_action` + `requires_confirmation`, the defect is latent, not currently
  exploitable through any shipped template. It would become live the moment any
  future template author reaches for `requires_confirmation` believing the security
  section's promise.
- The smallest change that makes the system honest without new engine machinery:
  rename the field/response to reflect what it does (`confirm_result` /
  `action: "review"` instead of `requires_confirmation` / `"confirm"`), strike the
  "prevents... from auto-executing" language from the design doc's security section,
  and add an authoring-time rule (in `template-format.md` and/or compile-time
  validation in `src/template/compile.rs`) that irreversible actions must not use
  `default_action` at all — keep them as judgment states with agent-executed
  commands and a verifying gate, the pattern `pr_creation` already uses. This costs
  a rename, a doc edit, and a lint, versus the full confirm-before-execute build in
  section 4.

## Surprises

- The design doc contradicts itself in two adjacent sections: the security section's
  prose promises prevention, the architecture section's own interface (`ActionResult::
  RequiresConfirmation` carrying executed output) implements confirm-after. This isn't
  ambiguity that crept in during implementation — it's baked into the design doc
  itself, and PR #75 shipped it with zero review comments.
- The one committed unit test that models the motivating scenario
  (`src/engine/advance.rs:2878-2936`, action named `"create-pr"`, stub stdout
  `"PR #42 created"`) encodes the executed-before-confirmed behavior as the
  *expected, passing* result, without any comment flagging that this is the opposite
  of "irreversible actions require confirmation" in the everyday sense of the phrase.
- The parent template design never actually wires `requires_confirmation: true` to
  an irreversible action in the one place that would matter (`pr_creation`) — the
  capability was built for a case the template author (same person, `dangazineu`,
  across issue/design/PR/template) didn't end up using, which is why the defect
  survived to today undetected: there's no live template exercising it.

## Open Questions

- Should `reversibility` become a first-class template field (as sketched in section
  3/5) that the compiler can lint against `default_action` presence, or is prose
  guidance ("don't put irreversible commands in default_action") sufficient given
  there's exactly one author of templates today?
- If confirm-before-execute is ever built for real, who defines "a human is in the
  loop" — is that a shirabe-level concern (the skill pauses and waits for user input
  before calling `koto next` with an approval) rather than an engine-level one? That
  would sidestep section 4's engine cost entirely by keeping the gate in the caller,
  matching the "agent runs it itself, koto verifies" pattern already in use.
- Does `context_assignments` interact with the approval-hash idea — e.g. could an
  approval payload's `command_hash` reuse infrastructure already being explored for
  execution anchoring in this same investigation (per the scope doc's "Execution
  anchoring becomes the primary remaining guard" note)?

## Summary
`requires_confirmation` currently means "run the command, then let the caller see
what happened" — `src/cli/mod.rs:3985-4048` executes unconditionally and only
afterward branches on the flag to pick `ActionResult::Executed` vs.
`RequiresConfirmation`, both carrying identical already-obtained output. This
directly fails issue #71's acceptance criterion ("preventing auto-execution") and
contradicts the design doc's own security-section prose, even though it matches the
design doc's own architecture-section interface — a self-contradiction that shipped
through PR #75 with zero review comments and one test (`advance.rs:2878-2936`) that
encodes a `create-pr` action reporting `"PR #42 created"` as the confirmation output.
True confirm-before-execute is buildable (new stop reason, hash-bound approval
evidence, rewind invalidation, a third branch in the override-evidence check) but
costly and, for the one irreversible state that exists (`pr_creation`), already
unnecessary — that state gets the agent to run `gh pr create` itself with a koto gate
verifying the result, which is genuinely defensible. The smallest honest fix is a
rename plus an authoring rule keeping `default_action` off irreversible commands,
not a new engine primitive.
