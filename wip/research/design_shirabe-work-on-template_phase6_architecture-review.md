# Architecture Review: DESIGN-shirabe-work-on-template.md

Date: 2026-03-21
Reviewer: architecture-review agent
Scope: All six questions — implementability, missing interfaces, phase sequencing,
simpler alternatives, state machine consistency, and evidence routing correctness.

---

## 1. Is the architecture clear enough to implement?

**Finding: Phase 1 is implementable from the doc. Phases 2–3 have one gap.**

A developer can start Phase 1 without asking questions. The doc names the three
files to change (`src/engine/advance.rs`, `src/cli/next_types.rs`, `src/cli/mod.rs`),
states the behavioral change for each ("fall through to NeedsEvidence when a gate
fails and accepts is present"), and specifies what to add to the GateBlocked response
(`expects` field, `agent_actionable: true`). The test categories are listed.

The `--var` flag spec is adequate for the engine side. It says: add `--var KEY=VALUE`
(repeatable), parse to `HashMap<String, String>`, substitute `{{KEY}}` in directive
text and gate commands before compilation. That's enough to start.

**Gap: Phase 2 (template file) lacks template syntax.** The doc describes the 15
states in prose, but gives no example of the template YAML format. A developer writing
`work-on.md` needs to know the YAML front-matter structure, how `gates:` blocks are
written, how `accepts:` with enum fields are declared, and how conditional transitions
are expressed. The doc's "Key Interfaces" section shows koto CLI invocations but not
the template syntax. A developer would need to read the existing `hello-koto` template
or the template compiler source to infer the format.

**Recommendation:** Add a minimal state example in the doc — two or three lines of
YAML showing a state with a gate, an accepts block, and a conditional transition. This
unblocks Phase 2 without requiring access to other templates.

---

## 2. Are there missing components or interfaces?

**Finding: Three gaps between claims and definitions.**

### 2a. Template variable substitution scope is underspecified

The doc says `--var` substitutes `{{KEY}}` in "directive text and gate commands
before compilation." It does not specify:
- Whether substitution happens in `accepts` field descriptions (minor)
- Whether `{{ISSUE_NUMBER}}` in a gate command is substituted at compile time or at
  gate evaluation time

This matters for security: the doc's Security section correctly says "sanitization
must be applied during template compilation, not at gate evaluation time." But the
phrase "before compilation" in the decision section is ambiguous — it could mean
"before the YAML is parsed" (safe) or "after parsing, before the compiled JSON is
written" (also safe but different). The implementation should substitute in the raw
YAML source before parsing, so the compiled JSON never contains `{{VAR}}` tokens.

### 2b. `workflow_type` cross-state routing: no transition syntax shown for `setup`

The doc states that `setup`'s post-state routing uses `workflow_type` from `entry`
evidence. But it does not show the transition definition for `setup`. The `setup`
state description says "Routes to `staleness_check` (work-on) or `analysis`
(just-do-it) using `workflow_type` from `entry` evidence." This implies `setup` has
transitions like `when: {workflow_type: "work-on"}` → `staleness_check` and `when:
{workflow_type: "just-do-it"}` → `analysis`. The doc does not make this explicit.
This is fine for design doc purposes, but should be clear in the template itself.

### 2c. `done_blocked` transition sources are underspecified

The state list says `done_blocked` is "reachable from `analysis` (missing context),
`implementation` (blocked), and `ci_monitor` (unresolvable failure)." The state
diagram does not show these edges. The diagram shows `done_blocked` as a terminal off
`ci_monitor` but not off `analysis` or `implementation`. A developer implementing the
template would need to add those transitions.

The state diagram also shows `done_blocked` connected to `ci_monitor` with a branch
line, but `analysis` and `implementation` each have `done_blocked` as a possible
`plan_outcome: blocked_missing_context` / `implementation_status: blocked` target.
These are described in the state definitions but absent from the diagram. The diagram
should be updated or the state definitions should note "see state definition for
full transition list."

---

## 3. Are the implementation phases correctly sequenced?

**Finding: Sequencing is correct. One dependency clarification needed.**

Phase 1 (engine changes) → Phase 2 (template) → Phase 3 (shirabe integration) →
Phase 4 (docs) is the right order. The template can be written and `koto template
compile` run without Phase 1 being complete, since the compiler validates structure,
not runtime behavior. The doc acknowledges this ("can be written and compiled, but
the gate-with-evidence-fallback behavior won't activate until the advancement loop
is patched").

The `--var` flag is called a Phase 1 prerequisite in the decision text but listed
as part of Phase 1 deliverables — consistent.

**Clarification:** Phase 3 says the merged skill "copies the template to
`.koto/templates/work-on.md` (from the plugin directory) if it doesn't exist." This
step depends on `koto init` accepting `--template` with a path, which is already
implemented. No new dependency, but the copy-on-first-run behavior is a skill
responsibility, not a koto responsibility — the doc should note where in shirabe
this logic lives (in the skill instructions, not a new koto subcommand).

---

## 4. Are there simpler alternatives that were overlooked?

**Finding: The gate-with-evidence-fallback model is well-chosen. One simplification
is available for the `--var` implementation.**

The three alternatives considered (fine-grained evidence everywhere, coarse
checkpoints, pure auto-advancing) are correctly rejected. The gate-with-evidence-
fallback model keeps evidence at genuine decision points and is backward-compatible.

**A simpler `--var` implementation is available for Phase 1:** The doc proposes
substituting `{{KEY}}` during template compilation, storing the compiled JSON without
variable tokens. An alternative is to store the variable map in the
`workflow_initialized` event (the `variables` field already exists in
`EventPayload::WorkflowInitialized` — it is currently stored as `HashMap::new()`) and
substitute at gate evaluation time. This avoids modifying the template compiler and
uses an existing storage slot. The tradeoff is that gate commands in the compiled JSON
still contain `{{KEY}}` tokens, which is less clean.

The doc's chosen approach (substitute before compilation) is cleaner but requires
touching the compiler. The event-storage approach is simpler and uses an already-
defined field. Worth noting as an option if the compiler change proves difficult.

---

## 5. Does the state machine diagram match the state definitions?

**Finding: Two inconsistencies.**

### 5a. `done_blocked` edges missing from diagram

The diagram shows `done_blocked` as one of two branches off `ci_monitor`. The state
definitions describe `done_blocked` as also reachable from `analysis`
(`plan_outcome: blocked_missing_context`) and `implementation`
(`implementation_status: blocked`). These edges are absent from the diagram.

### 5b. `research` convergence path in diagram is ambiguous

The diagram shows `research` → `setup` and then `(converges to analysis)` in a note
below the just-do-it path. This notation is unclear: the reader has to infer that
`setup` → `analysis` is the just-do-it path and `setup` → `staleness_check` is
work-on. A labeled arrow from `setup` to `staleness_check` (work-on) and from
`setup` to `analysis` (just-do-it) would make the convergence point unambiguous.

State names are consistent between the diagram, state definitions, and prose. No
typos or renamed states were found.

---

## 6. Does `workflow_type` evidence routing actually work with koto's evidence model?

**Finding: It does NOT work as described. This is a critical correctness issue.**

The doc states: "The `workflow_type` evidence persists across the session via koto's
evidence merging model, so the `setup` state can route post-setup to `staleness_check`
(work-on) or `analysis` (just-do-it) based on the evidence submitted at `entry`."

The actual engine behavior contradicts this claim.

**Evidence is scoped to the current epoch.** In `src/engine/persistence.rs`,
`derive_evidence()` returns only `evidence_submitted` events that occur after the
most recent state-changing event (`transitioned`, `directed_transition`, or `rewound`)
whose `to` field matches the **current** state. Evidence submitted at `entry` is in
the `entry` epoch. When the workflow advances to `setup` (via `context_injection` for
work-on, or via `research` for just-do-it), the `entry` epoch ends. `derive_evidence`
called at `setup` finds the most recent state-changing event pointing to `setup` and
returns only events after that — which contains no `entry` evidence.

**In `src/engine/advance.rs`, this is reinforced by the auto-advance loop:**
```rust
// Fresh epoch: auto-advanced states have no evidence
current_evidence = BTreeMap::new();
```
After each auto-transition, evidence is cleared to `BTreeMap::new()`. So even if the
engine auto-advances through `context_injection` to `setup`, evidence from any prior
state is gone.

**The consequence:** When `koto next` is called at `setup`, `evidence` is derived
from the current epoch only. The `workflow_type` field submitted at `entry` is not
in scope. The transition resolver at `setup` evaluates conditions against empty
evidence, finds no match for `when: {workflow_type: "work-on"}` or
`when: {workflow_type: "just-do-it"}`, and returns `NeedsEvidence`. The agent is
blocked at `setup` asking for evidence that the template's `accepts` block may not
even declare.

**This breaks the core routing assumption of the design.** The two paths converge
at `setup`, and `setup`'s routing depends entirely on cross-state evidence that the
engine does not carry forward.

**How to fix it:** There are two viable options:

Option A — Re-submit `workflow_type` at `setup`. Add `workflow_type` to `setup`'s
`accepts` schema as a required field. The skill instructions submit it again when
reaching `setup`. This is explicit and requires no engine change, but adds a
redundant evidence submission.

Option B — Engine change: cross-epoch evidence projection. When the advance loop
loads evidence for a state, also scan the full event log for any
`evidence_submitted` event (from any prior epoch) containing fields that appear in
the current state's transition conditions. This would allow `setup` to see
`workflow_type` from `entry`. This is a more powerful engine change and creates
implicit coupling between states that is harder to reason about.

Option A is simpler and safer. The design doc's "Consequences / Negative" section
already lists this as a known fragility ("depends on evidence submitted at `entry`
persisting across states via koto's evidence merging model. If this model changes in
a future koto version, the routing breaks silently") — but the issue is that it
**already** doesn't work, not that it might break in a future version.

---

## Summary

1. **Phase 1 is implementable as written.** Phase 2 needs a YAML syntax example.

2. **`workflow_type` cross-state routing is broken.** Evidence submitted at `entry`
is not accessible at `setup`. This is the design's critical correctness gap and must
be resolved before Phase 2 can produce a working template. The fix is either adding
`workflow_type` to `setup`'s accepts schema (simple) or projecting cross-epoch
evidence in the engine (complex).

3. **State diagram missing edges.** `done_blocked` is reachable from `analysis` and
`implementation` but these edges are absent from the diagram. Low priority but should
be corrected before the doc is marked accepted.

4. **`--var` substitution timing is underspecified.** The doc says "before
compilation" but does not clarify whether this means pre-YAML-parse or post-parse.
Substitute in the raw YAML string before parsing to keep compiled JSON clean.
Alternatively, the existing `variables` field in `WorkflowInitialized` could store
vars for runtime substitution, avoiding a compiler change.

5. **The gate-with-evidence-fallback model and phase sequencing are sound.** The
implicit convention (co-presence of gates and accepts implies fallback) is clean and
backward-compatible. Phase ordering is correct.
