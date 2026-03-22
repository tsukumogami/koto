# Architecture Review: DESIGN-shirabe-work-on-template.md

Date: 2026-03-22
Reviewer: architecture-review agent
Scope: Solution Architecture and Implementation Approach sections — implementability,
missing components/interfaces, phase sequencing, simpler alternatives.

---

## 1. Is the architecture clear enough to implement?

**Finding: Yes, with one gap in Phase 2.**

Phase 1 (engine changes) is fully specified. The doc names the exact files to change
(`src/engine/advance.rs`, `src/cli/next_types.rs`, `src/cli/mod.rs`), describes the
behavioral change for each (fall through to `NeedsEvidence` when gate fails and
`accepts` block is present), and calls out the security constraint (sanitize `--var`
values at `koto init` time, reject shell metacharacters). Source code inspection
confirms the current code at all three change points — `advance_until_stop` hard-stops
at `GateBlocked` when any gate fails (line 233-238 of `advance.rs`), `GateBlocked`
variant in `next_types.rs` has no `expects` field, and `variables: HashMap::new()` is
hardcoded in the init handler (line 207 of `cli/mod.rs`). The design accurately
describes the current state.

Phase 3 (shirabe skill integration) is sufficiently specified for implementation.

**Gap: Phase 2 lacks template syntax.** The doc specifies all 17 states in prose but
gives no example of the YAML front-matter format. A developer writing `work-on.md`
needs to know how `gates:`, `accepts:` with enum fields, and conditional transitions
with `when:` blocks are expressed. The doc's "Key Interfaces" section shows CLI
invocations but not template syntax. The developer must read `hello-koto.md` or the
template compiler source to infer the format. A 3–5 line YAML snippet showing one
gate-with-fallback state would close this gap without growing the doc materially.

---

## 2. Are there missing components or interfaces?

**Finding: Three interface details need clarification.**

### 2a. `--var` substitution timing is underspecified

The doc says `{{KEY}}` in gate commands is substituted "at gate evaluation time by
reading from the stored variables map." This is the correct approach (variables stored
in `WorkflowInitialized`, substituted at runtime, compiled template stays
variable-agnostic). However, the spec does not state whether `{{KEY}}` in directive
text is also substituted at runtime, or only in gate commands. If directives are not
substituted, states whose directives reference `{{ISSUE_NUMBER}}` for the agent's
benefit will show the literal token until `--var` ships. This should be made explicit.

### 2b. `koto rewind` accepts only one step back, not a named state

The doc says `done_blocked`'s directive instructs agents to run
`koto rewind <originating-state>`. The actual `koto rewind` implementation (CLI
`mod.rs` lines 244–318) accepts only a workflow `name` — not a target state. It always
rewinds exactly one step to the previous state-changing event. There is no
`<originating-state>` argument. A workflow blocked in `done_blocked` after two or more
transitions from the origin state cannot reach the origin in one rewind; it requires
repeated `koto rewind` calls. The directive text must be corrected to reflect the
actual CLI, and the recovery instructions should explain the "repeat rewind" procedure.
This is a concrete correctness gap in the Phase 2 deliverable.

### 2c. Free-form `analysis` gate uses shell glob expansion

The `analysis` state's free-form gate is
`ls wip/task_*_plan.md 2>/dev/null | grep -q .`. This relies on shell glob expansion
in the gate runner. If the gate runner executes commands via `sh -c` (which koto does),
this works. But if the free-form workflow name slug contains special shell characters
(e.g., spaces or glob metacharacters), `task_*_plan.md` could match unintended files.
The workflow name is validated at `koto init` time against `^[a-zA-Z0-9][a-zA-Z0-9-]*$`,
which excludes problematic characters — so this is safe given the validation constraint.
The gate and the validation constraint should cross-reference each other in the template
header comment so the dependency is visible to future template authors.

---

## 3. Are the implementation phases correctly sequenced?

**Finding: Sequencing is correct. One clarification needed.**

Phase 1 (engine) → Phase 2 (template) → Phase 3 (shirabe skill) → Phase 4 (docs)
is the right order. Phase 2 can begin before Phase 1 completes because `koto template
compile` validates structure, not runtime behavior. Gate-with-evidence-fallback states
will compile successfully; the fallback just won't activate until Phase 1 is deployed.
The doc acknowledges this explicitly.

The `--var` flag is both a Phase 1 deliverable and described as a prerequisite for
issue-specific gates. That's self-consistent: Phase 2 writes gate commands with
`{{ISSUE_NUMBER}}` tokens that fall through to evidence fallback until Phase 1 ships,
which is the documented degraded-but-functional path.

**Clarification:** Phase 3 includes "Session stop hook: extend the existing koto Stop
hook." This implies an existing stop hook in shirabe. The doc does not identify which
file contains this hook. Phase 3 should name the file (likely in
`plugins/koto-skills/hooks.json` or equivalent) so the implementer doesn't need to
discover it by searching shirabe's codebase.

---

## 4. Are there simpler alternatives we overlooked?

**Finding: The split topology is the right call. One simplification is available for
`--var`.**

The split topology (two separate setup states, `setup_issue_backed` and
`setup_free_form`) directly solves the epoch-scoped evidence problem that would have
broken the previous single-setup design. Source code confirms that evidence is cleared
to `BTreeMap::new()` after each auto-transition (advance.rs line 267), so cross-state
evidence does not carry forward. The split topology avoids this entirely by routing
unconditionally from each setup state — no evidence routing required. This is the
correct design given the engine's actual behavior.

**Simpler `--var` implementation:** The doc proposes substituting `{{KEY}}` at gate
evaluation time from the stored variables map (already-defined `variables` field in
`WorkflowInitialized`). This is already the simpler path — it avoids modifying the
template compiler and uses an existing storage slot. The doc calls this out correctly.
No further simplification is available here.

**`TEST_COMMAND` variable:** The doc's Consequences section mentions a `TEST_COMMAND`
template variable as planned mitigation for the language-specific test gate. This could
also be expressed as a Phase 1 deliverable: default `TEST_COMMAND=go test ./...` in the
`koto init` call so the template works without requiring callers to pass it explicitly.
This keeps the template language-agnostic without forcing every caller to specify it.

---

## Summary

1. **The split topology correctly handles koto's epoch-scoped evidence model.** Two
   unconditional setup states eliminate the cross-state evidence dependency that would
   have broken a single-setup design. The core routing architecture is sound.

2. **`koto rewind` has no named-state argument.** The current CLI rewinds exactly one
   step. The `done_blocked` directive must be corrected — agents need repeated calls
   or a manual workaround, not `koto rewind <originating-state>`. Verify and fix
   before Phase 2.

3. **Phase 2 needs a YAML syntax example.** All 17 states are specified in prose but
   the template file format is not shown. One short example of a gate-with-fallback
   state unblocks the template author without requiring them to read other files.

4. **`--var` substitution scope needs explicit statement.** Specify whether directive
   text (not just gate commands) gets `{{KEY}}` substituted at runtime. If not, states
   with `{{ISSUE_NUMBER}}` in directives will show literal tokens until `--var` ships.

5. **Phase sequencing and gate-with-evidence-fallback model are correct.** Phases are
   ordered with the right prerequisites. The co-presence convention (gates + accepts =
   fallback enabled) is clean, backward-compatible, and matches the engine's current
   structure.
