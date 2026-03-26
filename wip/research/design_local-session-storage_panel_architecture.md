# Architecture Review: Session Persistence Document Set

Reviewer: architect-reviewer
Scope: PRD-session-persistence-storage.md, DESIGN-local-session-storage.md, ROADMAP-session-persistence.md
Source files examined: `src/cli/mod.rs`, `src/gate.rs`, `src/template/types.rs`, `src/engine/types.rs`, `src/discover.rs`, `src/template/compile.rs`, `src/engine/advance.rs`

---

## 1. PRD to ROADMAP to DESIGN hierarchy

The three-layer structure is coherent. The PRD defines requirements (R1-R15), the roadmap sequences four features against those requirements, and the DESIGN covers Feature 1 in detail.

**No contradictions found.** The layers reference each other correctly:
- DESIGN's `upstream` field points to the PRD
- ROADMAP references the DESIGN for Feature 1
- Feature boundaries are consistent across all three docs

**One gap:** The ROADMAP lists 4 features but the PRD has 15 requirements. There is no explicit mapping showing which requirements each feature satisfies. This makes it hard to verify coverage. The ROADMAP's feature descriptions imply full coverage, but it is implicit. Not blocking -- the feature descriptions are detailed enough to trace manually.

**Numbering mismatch:** The roadmap previously had 5 features (the DESIGN references "Feature 5" for cloud sync in Decision 1). The roadmap now has 4 features with cloud sync as Feature 4. The DESIGN's "Feature 5" references are stale. This should be corrected to avoid confusion during implementation.

---

## 2. Does Feature 1 deliver standalone value?

**Yes, with one caveat.**

After Feature 1 ships, a template author can:
1. Write `{{SESSION_DIR}}` in gate commands: `test -f {{SESSION_DIR}}/plan.md`
2. Write `{{SESSION_DIR}}` in directives: "Write your plan to `{{SESSION_DIR}}/plan.md`"
3. `koto next` substitutes the variable at runtime (Decision 7)
4. Skills call `koto session dir <name>` to discover the path
5. State files move to `~/.koto/sessions/<repo-id>/<name>/`

The end-to-end flow works without any other feature. The backend defaults to `LocalBackend` without a config system (Feature 2).

**The caveat: existing `variables` field is unused at runtime.**

The template type system already has a `variables` field (`CompiledTemplate.variables: BTreeMap<String, VariableDecl>`) and the `WorkflowInitialized` event has a `variables: HashMap<String, Value>` payload. But today:
- `koto init` writes `variables: HashMap::new()` (line 207 of `cli/mod.rs`) -- always empty
- No substitution of `{{TASK}}` or any user-declared variable happens anywhere in the codebase
- `{{TASK}}` tokens pass through raw in directive strings (confirmed by compile test at line 403 of `compile.rs`)

This means `{{SESSION_DIR}}` will be the first variable that actually gets substituted. The `substitute_vars` function in `src/cli/vars.rs` is the right place for this. But the design should explicitly note that template-declared variables (the `variables:` block in YAML) remain inert after Feature 1. A template author who writes `variables: { SESSION_DIR: { ... } }` might expect that declaration to interact with the built-in `SESSION_DIR`. The DESIGN should specify: `SESSION_DIR` is engine-provided and must NOT be declared in the template's `variables` block. If it is declared, the engine should either error or ignore the declaration (preferably error to prevent confusion).

---

## 3. Missing pieces that could block Feature 1

### 3a. SESSION_DIR is engine-provided, not user-provided

The DESIGN is clear on this (Decision 7): the vars map is built from `backend.session_dir(name)`, not from `--var` or template declarations. No `--var` flag is needed. This is correct.

### 3b. Gate command variable substitution location

The DESIGN says to substitute `{{SESSION_DIR}}` in gate commands "before passing to `evaluate_gates()`." Looking at the current code, `evaluate_gates` in `src/gate.rs` (line 39-58) receives `&BTreeMap<String, Gate>` and reads `gate.command` directly. The substitution needs to happen on the `Gate.command` strings before they reach this function.

Two call sites for gate evaluation exist:
1. `handle_next` at line 767-770 via `gate_closure`
2. `advance_until_stop` in `src/engine/advance.rs` via the `gate_fn` parameter

**Structural concern:** The `gate_closure` at line 767-770 passes gates directly to `evaluate_gates`. Since the closure is called from within `advance_until_stop` on every state that has gates, the substitution must happen inside the closure on every invocation -- not once before the loop. The DESIGN's wording ("replace `{{SESSION_DIR}}` in each `Gate.command` string before passing to `evaluate_gates()`") is correct in intent but ambiguous about timing. The implementation should clone the gates map, substitute in each command, then pass the modified map to `evaluate_gates`. This happens inside the closure, so it runs per-state.

The DESIGN should make this explicit to prevent an implementer from substituting once outside the loop.

### 3c. Directive substitution for advancement-loop states

Same timing question for directives, but this one is fine. The advancement loop can chain through multiple states before stopping. The final `NextResponse` includes the directive from the state where the loop stopped. Substitution happens after the loop exits, on the final response (lines 830-855 in `cli/mod.rs`). Only the final state's directive reaches the agent, so substituting once at the end is correct.

### 3d. No other blockers identified

- `sha256_hex` already exists in `src/cache.rs` -- confirmed
- `dirs` crate is the only new dependency -- reasonable
- Session ID validation regex is sound; starting-with-letter rejects path traversal
- The `run()` refactoring to thread `&dyn SessionBackend` touches every command handler but is mechanical

---

## 4. Roadmap sequencing

The 4-feature sequencing is correct:

| Feature | Dependencies | Rationale |
|---------|-------------|-----------|
| 1: Local storage + vars | None | Foundation trait, single backend, no config needed |
| 2: Config system | None | Needed by 3 and 4 for backend selection |
| 3: Git backend | 1, 2 | Thin trait impl + config for selection |
| 4: Cloud sync | 1, 2 | Most complex, S3 deps behind feature flag |

The parallel opportunity (1 and 2 simultaneously) is correctly identified.

One sequencing note: Feature 1 includes `koto session dir|list|cleanup` subcommands. After Feature 2 adds backend selection, `koto session list` will need to query the active backend. The Feature 1 implementation should note that `list` in Feature 1 only lists local sessions, and the backend abstraction already handles this (the trait's `list()` method is per-backend).

---

## 5. Scope alignment: DESIGN vs PRD

### In DESIGN but not in PRD

Nothing. Every decision in the DESIGN traces to a PRD requirement:
- SessionBackend trait -> R1, R2
- LocalBackend at ~/.koto/ -> R4
- Session directory structure -> R3
- CLI refactoring -> R2, R12
- `koto session dir|list|cleanup` -> R9
- `{{SESSION_DIR}}` substitution -> R12
- Session ID validation -> R1

### In PRD but deferred (correctly)

- R5 (cloud sync) -> Feature 4
- R6 (conflict detection) -> Feature 4
- R7 (git backend) -> Feature 3
- R8 (koto config) -> Feature 2
- R15 (cloud resilience) -> Feature 4

### Noted

- **R9 `koto session resolve --keep local|remote`**: Not in the DESIGN (correct -- it's a cloud-sync concern). Mentioned in Feature 4's roadmap description. No gap.

---

## 6. Architectural findings

### 6a. Template `variables` vs engine-provided `SESSION_DIR` interaction unspecified (Advisory)

Templates can declare variables in YAML front-matter (`variables: { TASK: ... }`). `SESSION_DIR` is engine-provided, not template-declared. The DESIGN should specify what happens if a template declares `SESSION_DIR` in its `variables` block. Without this, implementers will make an ad-hoc choice.

Recommended: error at runtime in `handle_next` when building the vars map, if the template declares a variable that collides with an engine-provided name. This keeps the template compiler unaware of session concepts (correct layering) while preventing silent override.

### 6b. Stale "Feature 5" references in DESIGN (Advisory)

Decision 1 says "Feature 5 will add them when the cloud backend ships." The roadmap numbers cloud sync as Feature 4. Fix before implementation starts.

### 6c. `workflow_state_path` refactoring scope (Advisory)

The DESIGN's "Minimal CLI disruption" driver says "one path-construction change." In practice, `workflow_state_path()` is called in `handle_init` (line 157), `handle_next` (line 428), `handle_cancel` (line 963), `Rewind` handler (line 246), and `Workflows` handler (line 323) -- five sites plus the `find_workflows_with_metadata` scanner. The refactoring is mechanical but touches every command handler. The DESIGN's Phase 2 description acknowledges this; the decision driver just undersells the count.

### 6d. Existing templates with hardcoded `wip/` paths will break (Blocking -- from prior review, retained)

After Feature 1, state files no longer live in the working tree. Templates with gate commands like `test -f wip/plan.md` will fail. The design should explicitly state whether template migration is in-scope for Feature 1 or handled separately. Given the PRD's "no backward compatibility constraint," migrating existing templates as part of Feature 1 is the right call, but it should be in the implementation plan.

### 6e. No parallel patterns introduced (Positive)

The `substitute_vars` utility in `src/cli/vars.rs` is the single substitution path. The DESIGN explicitly rejects alternatives (env vars for gates, compile-time substitution) that would create parallel mechanisms. The `HashMap<String, String>` design extends naturally to `--var` (issue #67) by adding user entries to the same map. The substitution lives at the CLI output boundary, keeping the engine unaware of session concepts. This is correct architectural layering.

---

## Summary

| Finding | Severity | Action |
|---------|----------|--------|
| Feature 1 delivers standalone end-to-end value | Positive | None |
| Existing templates with hardcoded `wip/` will break | Blocking | Add template migration to Feature 1 scope |
| Gate closure must substitute vars per-invocation (DESIGN wording ambiguous) | Advisory | Clarify in DESIGN Phase 2 |
| Template `variables` vs engine-provided `SESSION_DIR` collision unspecified | Advisory | Add collision policy to DESIGN Decision 7 |
| Stale "Feature 5" reference in Decision 1 | Advisory | Fix numbering |
| `workflow_state_path` refactoring touches 5+ sites, not "one change" | Advisory | Adjust DESIGN driver wording |
| PRD-ROADMAP-DESIGN hierarchy is coherent | Positive | None |
| Roadmap sequencing is correct | Positive | None |
| No scope in DESIGN outside PRD justification | Positive | None |
| `substitute_vars` is single substitution path, correct layering | Positive | None |
