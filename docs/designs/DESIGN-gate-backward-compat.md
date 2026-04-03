---
status: Proposed
upstream: docs/prds/PRD-gate-transition-contract.md
problem: |
  Feature 1 preserved legacy gate behavior implicitly: when no `when` clause
  references `gates.*` fields, gates act as boolean pass/fail blockers. This
  path is invisible to the compiler. Template authors who accidentally omit
  `gates.*` references get the old behavior silently, and the only known legacy
  template needs to keep working without source changes until it migrates.
decision: |
  `koto template compile` errors on legacy gate behavior unless
  `--allow-legacy-gates` is passed. `koto init` (implicit compile on cache miss)
  warns but proceeds — agents starting a workflow can't change the template.
  The flag is explicitly transitory: it is removed from koto once the last
  legacy template migrates. The engine excludes gate output from the resolver's
  evidence map for legacy states, matching R10 precisely.
rationale: |
  The compile/init distinction maps to the real actors: template authors use
  `koto template compile` and are responsible for correctness; agents use
  `koto init` and can't change the template. The flag on `koto template compile`
  is the migration debt signal — its presence in a CI script means a template
  still owes migration work. The flag's removal from koto is the migration
  completion signal. No template source changes are required, which preserves
  existing installs and in-progress workflows.
---

# DESIGN: Gate backward compatibility

## Status

Proposed

## Upstream design reference

This is Feature 4 of the gate-transition contract. Feature 1 design:
[DESIGN-structured-gate-output](current/DESIGN-structured-gate-output.md).
Feature 2 design: [DESIGN-gate-override-mechanism](current/DESIGN-gate-override-mechanism.md).
Feature 3 design: [DESIGN-gate-contract-compiler-validation](current/DESIGN-gate-contract-compiler-validation.md).

## Context and problem statement

Features 1–3 of the gate-transition contract introduced structured gate output,
the override mechanism, and compiler validation. Feature 1 preserved the old
gate behavior implicitly: when no `when` clause in a state references `gates.*`
fields, the advance loop falls back to boolean pass/block behavior — the gate
runs, and if it fails the state blocks; routing happens entirely through agent
evidence submitted via `accepts` blocks. This fallback path was designed in but
never formalized.

The problem is that this implicit path is invisible to the compiler. Template
authors who accidentally write gates without `gates.*` routing get the old
behavior silently — no warning, no error, nothing in the template that signals
their gates aren't feeding into transition logic. The compiler validation added
in Feature 3 warns about unreferenced gate fields, but that warning fires for
intentionally-legacy templates too, which creates noise that discourages
migration.

Two concrete issues remain unresolved after Feature 3:

First, the compiler has no distinction between "intentionally using legacy mode"
and "accidentally omitting `gates.*` references." Both look the same. The PRD
acceptance criterion for R10 says legacy states should produce "no structured
output" — but the advance loop currently injects gate output into the resolver
evidence for all states regardless.

Second, the only known template using legacy gate behavior uses gates as pure
pass/fail blockers and routes entirely on agent evidence via `accepts` blocks.
It needs to keep working until migrated to structured routing, and it must do
so without source changes — agents starting workflows from this template can't
patch the template themselves.

## Decision drivers

- Template authors must not accidentally opt into legacy mode. New templates
  using gates without `gates.*` routing should fail `koto template compile` by
  default.
- Existing legacy templates must work with `koto init` without any source
  changes. Agents starting a workflow from a legacy template can't modify it.
- The migration debt signal must be visible and removable. A flag in a CI script
  is an explicit acknowledgment of pending migration work; removing it is the
  completion signal.
- The compat flag is explicitly transitory. Once the last legacy template
  migrates, the flag is removed from koto entirely.
- D4's unreferenced-field warning must not fire during `koto init`. That warning
  is aimed at template authors, not at agents running workflows.
- The engine's behavior for legacy states must match R10 precisely: gate output
  should not enter the resolver's evidence map for legacy states.
- The legacy code path must be self-contained and deletable. When the flag is
  removed, deleting the compat code should be a contained change.

## Considered options

### Decision 1: How legacy mode is declared and enforced

Template authors using gates without `gates.*` routing need a way to signal
that this is intentional. Without an explicit signal, the compiler can't
distinguish a legacy template from a new template that forgot to add `gates.*`
references. The core tension is between two actors with different capabilities:
template authors can change templates; agents starting workflows cannot.

#### Chosen: `--allow-legacy-gates` flag on `koto template compile`; `koto init` warns and proceeds

`koto template compile` errors when it detects a state with gates but no
`gates.*` when-clause references, unless `--allow-legacy-gates` is passed.
With the flag, it suppresses the error and D4 unreferenced-field warnings,
and proceeds.

`koto init` (which compiles on a cache miss) runs in permissive mode
unconditionally: legacy gate behavior emits a warning to stderr and proceeds.
D4 warnings are also suppressed in permissive mode — they are aimed at template
authors, not at agents.

The flag is explicitly transitory. It is added to koto as a temporary
accommodation for the known legacy template, and is removed from koto once
that template migrates to structured routing. A comment in the source at the
flag's definition tracks this intent.

Template CI scripts that pass `--allow-legacy-gates` carry the migration debt
visibly: the flag in a Makefile or CI YAML is a searchable, reviewable signal
that the template still owes migration work. When the template migrates, the
flag is removed from both the CI script and from koto.

No template source changes are required. Existing installs and in-progress
workflows are unaffected — `koto next` never recompiles.

#### Alternatives considered

**Per-template frontmatter field `legacy_gates: true`**: The field is co-located
with the template and self-documents its migration status. However, it requires
changing the template source before `koto init` works. The only known legacy
template lives in a separate repo (shirabe) whose migration timeline is
independent of this koto change landing. Requiring a coordinated frontmatter
change in shirabe before koto ships is a synchronization risk. Rejected because
it requires template source changes to preserve existing behavior.

**Per-state annotation**: Each state that uses legacy behavior could declare
`legacy_gates: true` at the state level. Migration is a whole-template
operation — you update all gate-bearing states at once — so state-level
granularity adds migration surface without benefit. Rejected.

**Separate compile subcommand `koto template compile-legacy`**: This would give
the compiler a clean signal at invocation time, but it adds a permanent new CLI
surface. The explicit goal is that this code path is removed after migration,
and a subcommand is harder to remove than a flag. Rejected.

**Auto-detect with warning only, no error**: Leave `koto template compile`
permissive and rely on D4 warnings to surface legacy behavior. Rejected because
it allows new templates to accidentally use legacy mode silently, which is the
core problem this feature is meant to solve.

---

### Decision 2: Evidence injection for legacy states

The advance loop builds a merged evidence map — agent evidence plus gate
output under the `gates.*` namespace — and passes it to `resolve_transition`.
For legacy states (no `gates.*` when-clause references), gate output is
currently injected into this map but never matched by any transition condition.
The PRD R10 acceptance criterion says these states should "produce no
structured output (legacy boolean behavior)."

The question is whether to enforce this literally — skip the evidence
injection for legacy states — or to accept the current "injected but ignored"
behavior as functionally equivalent.

#### Chosen: Exclude gate output from the merged evidence for legacy states

For states where `has_gates_routing` is false (computed at lines 395–403 of
`src/engine/advance.rs`), skip the `gate_evidence_map` insertion into the
merged evidence before calling `resolve_transition`. Gate output is still
computed by the evaluators and is still available for `GateEvaluated` events
and for the `GateBlocked` stop reason's `failed_gates` field — those use
`gate_results` directly, not the merged evidence map.

The implementation guard reuses the already-computed `has_gates_routing`
boolean to condition the merge:

```rust
if !gate_evidence_map.is_empty() && has_gates_routing {
    merged.insert(
        "gates".to_string(),
        serde_json::Value::Object(gate_evidence_map),
    );
}
```

This satisfies R10 precisely: the resolver never sees gate output for legacy
states. It also removes a misleading invariant from the code — currently the
advance loop injects data it knows will never be matched.

#### Alternatives considered

**Keep current behavior (inject but ignore)**: Functionally identical for all
observable behaviors, since no legacy `when` clause references `gates.*`. The
`GateBlocked` stop reason, `GateEvaluated` events, and `blocking_conditions`
response all use `gate_results` directly and are unaffected. Rejected because
it contradicts the PRD R10 acceptance criterion and leaves a misleading
invariant in the codebase with no offsetting benefit.

## Decision outcome

**Chosen: D1 context-sensitive validation flag + D2 evidence exclusion**

### Summary

`koto template compile` is the template-author context: it runs `validate()` in
strict mode and errors when it detects a state with gates but no `gates.*`
routing. Template authors pass `--allow-legacy-gates` to suppress this error
while their template is on the migration path. The flag also suppresses D4
unreferenced-field warnings for the duration of its use.

`koto init` is the agent context: it runs `validate()` in permissive mode
unconditionally. Legacy gate behavior emits a warning to stderr and proceeds.
D4 warnings are suppressed. Agents starting a workflow from a legacy template
are not blocked.

The flag `--allow-legacy-gates` is transitory by design. A comment at its
definition in the CLI source names the expected removal condition. When the last
legacy template migrates, the flag is deleted from koto — along with the strict/
permissive branching in `validate()`, the D4 suppression, and the evidence
merge guard.

In the engine, the advance loop's evidence merge step is guarded by
`has_gates_routing`. For states with no `gates.*` when-clause references,
gate output is computed (for events and blocking_conditions) but not inserted
into the merged evidence map passed to `resolve_transition`. This matches
R10's acceptance criterion precisely.

Templates that carry `accepts` blocks alongside gates continue to work. In
legacy mode, the advance loop falls through to transition resolution when a
gate fails and an `accepts` block is present — the resolver uses agent evidence
only (no `gates.*` in the merged map for legacy states). The `--with-data`
workaround pattern (`{"status": "override"}`) remains plain evidence submission
unrelated to the new override mechanism.

### Rationale

The compile/init distinction maps directly to the real actors. Template authors
run `koto template compile` in CI and are responsible for the template's
correctness — they get an error and a clear path (either migrate or pass the
flag). Agents run `koto init` to start a workflow and have no ability to change
the template source — they get a warning and proceed.

The frontmatter field alternative was rejected because it requires template
source changes before existing behavior is preserved. The only known legacy
template lives in a separate repo with its own migration timeline. Requiring
a frontmatter change before this koto change lands creates a cross-repo
synchronization dependency. The CLI flag avoids this: koto ships the flag,
the legacy template continues to work at init time without changes, and the
template author adds `--allow-legacy-gates` to their CI at their own pace.

The flag's transitory nature is intentional and explicit. It is not a permanent
feature; it is a migration scaffold with a named exit condition.

## Solution architecture

### Overview

Two isolated changes in two different layers. The compiler change adds a
`strict` parameter to `validate()` and wires it to the CLI context — strict
for `koto template compile`, permissive for `koto init`. The engine change adds
a condition to the evidence merge step in the advance loop.

### Components

**`src/cli/mod.rs` — handle_template_compile() + handle_init()**

`handle_template_compile()` gains an `--allow-legacy-gates` flag. It passes
`strict = !allow_legacy_gates` when calling `validate()`. A source comment at
the flag definition names the removal condition:

```rust
// TODO: remove --allow-legacy-gates once the shirabe work-on template
// migrates to gates.* routing. See issue #119.
```

`handle_init()` calls `validate()` with `strict = false` unconditionally. On a
cache miss the full compile + validate pipeline runs in permissive mode.

**`src/template/types.rs` — validate()**

`validate()` gains a `strict: bool` parameter.

1. **D5 check (legacy gate detection):** After the D2/D3 checks, scan each
   state. If any state has gates but no `gates.*` when-clause references:
   - `strict = true`: emit an error:
     ```
     error: state "verify" gate "ci_check" has no gates.* routing
       add a when clause referencing gates.ci_check.passed, gates.ci_check.error, ...
       or use --allow-legacy-gates to permit boolean pass/block behavior
     ```
   - `strict = false`: emit a warning to stderr and continue:
     ```
     warning: state "verify" gate "ci_check" has no gates.* routing (legacy behavior)
     ```

2. **D4 suppression in permissive mode:** The `validate_gate_reachability()`
   call site gains an early return when `!strict`:
   ```rust
   if !strict {
       return Ok(());
   }
   ```
   Note: for a legacy state, `validate_gate_reachability()` would vacuously
   return `Ok(())` anyway (no gate transitions exist). The early return is
   needed to suppress the `eprintln!` warning loop that iterates over gate
   schema fields — that loop would emit a warning per field for every gate in
   the template, which is noise for an agent.

No changes to `SourceFrontmatter` or `CompiledTemplate` — the strict/permissive
distinction is entirely in the call context, not in the compiled artifact.

**`src/template/compile.rs` — compile()**

`compile()` calls `validate()`. It gains a `strict: bool` parameter and passes
it through. Callers (`handle_template_compile`, `handle_init`, tests) set it
explicitly.

**`src/engine/advance.rs` — advance loop**

Guard the gate evidence merge with `has_gates_routing`:

```rust
// Only inject gate output into evidence for states that use gates.* routing.
// Legacy states (no gates.* when-clause references) get boolean pass/block
// behavior only; gate output does not enter the resolver evidence map (R10).
if !gate_evidence_map.is_empty() && has_gates_routing {
    merged.insert(
        "gates".to_string(),
        serde_json::Value::Object(gate_evidence_map),
    );
}
```

`has_gates_routing` is currently computed inside the `if any_failed` block at
lines 395–403. It must be hoisted above that block and initialized to `false`
before the gate evaluation loop, so it is always in scope at the evidence merge
step (~line 437).

### Key interfaces

**`koto template compile` with legacy gates (no flag):**
```
error: state "verify" gate "ci_check" has no gates.* routing
  add a when clause referencing gates.ci_check.passed, gates.ci_check.error, ...
  or use --allow-legacy-gates to permit boolean pass/block behavior
```

**`koto template compile --allow-legacy-gates` with legacy gates:**
No output for legacy behavior. Template compiles successfully.

**`koto init` with legacy gates:**
```
warning: state "verify" gate "ci_check" has no gates.* routing (legacy behavior)
```
Init proceeds. Workflow starts normally.

### Data flow

```
koto template compile
  │
  ├─► --allow-legacy-gates? ──► strict = false ──► validate(strict=false) ── warn + proceed
  │
  └─► (no flag) ──────────────► strict = true  ──► validate(strict=true)  ── D5 error

koto init
  │
  └─► strict = false (always) ──► validate(strict=false) ── warn + proceed

validate(strict)
  │
  ├─► D2/D3 checks (always strict)
  │
  ├─► D5 check: gates without gates.* routing
  │     strict=true  → Err(...)
  │     strict=false → eprintln! warning, continue
  │
  └─► validate_gate_reachability()
        strict=false → early return (suppress D4 eprintln! loop)
        strict=true  → run D4 checks

advance loop
  │
  ├─► evaluate gates ──► gate_results (always)
  │                      gate_evidence_map (always)
  │
  ├─► has_gates_routing (hoisted, initialized false)
  │
  └─► evidence merge: insert "gates" only if has_gates_routing
```

## Implementation approach

### Phase 1: Context-sensitive validation

Add `strict: bool` to `validate()` and `compile()`. Add the D5 check — error
in strict mode, warning in permissive mode. Add the D4 early return in
permissive mode. Add `--allow-legacy-gates` to `handle_template_compile()` with
a removal comment. Make `handle_init()` call `compile()` with `strict = false`.

**Phase 1 and Phase 2 must land in the same PR.** If Phase 2 ships without
Phase 1, the engine stops injecting gate evidence for legacy states before
`koto init` is updated to permissive mode — agents would hit the D5 error with
no workaround.

Deliverables:
- `validate(strict: bool)` and `compile(strict: bool)`
- D5 error (strict) and warning (permissive) with actionable messages
- D4 early return in permissive mode
- `--allow-legacy-gates` flag on `koto template compile` with removal comment
- `handle_init()` uses `strict = false`
- Unit tests: strict mode errors on legacy gates; permissive mode warns and
  compiles; template with no gates compiles in both modes
- Integration test: `koto init` with a legacy-gate template exits 0 and emits
  the warning to stderr; `koto template compile` without flag exits nonzero;
  with `--allow-legacy-gates` exits 0

### Phase 2: Engine evidence exclusion

Hoist `has_gates_routing` computation in `src/engine/advance.rs` so it's
available at the evidence merge step regardless of whether `any_failed` is
true. Guard the `gate_evidence_map` insertion with `has_gates_routing`.

Deliverables:
- `has_gates_routing` initialized to `false` before the gate block, then set
  inside it (ensures it's always in scope at the merge step)
- Evidence merge guarded by `has_gates_routing`
- Unit tests: legacy state does not expose `gates.*` keys in merged evidence;
  structured-mode state does

## Security considerations

This design adds a CLI flag that conditions compiler behavior and a conditional
in the engine. Neither introduces new attack surface.

The `--allow-legacy-gates` flag is supplied by the template author or CI
operator, not by any external input at runtime. A CI script that passes the
flag is making an explicit, reviewable acknowledgment that the template uses
legacy behavior. The flag cannot be supplied by an agent running `koto init`
— init is always permissive regardless.

The evidence exclusion change (Phase 2) reduces the data available in the
resolver's evidence map for legacy states. It can't be used to inject data
that wasn't there before; it only removes engine-produced data from the map.
No new data flows to external parties.

The `koto init` warning writes to stderr only and contains no user data.
The warning text is derived from the compiled template's state and gate names,
which are set by the template author at compile time.

## Consequences

### Positive

- Existing legacy templates work with `koto init` without source changes.
  In-progress workflows and cached compiled templates are unaffected.
- New templates can't accidentally use legacy mode — `koto template compile`
  errors without the flag.
- The flag in a CI script is a searchable, reviewable migration debt signal.
  One grep across CI configs finds all templates on the legacy path.
- The engine behavior for legacy states now matches the PRD R10 acceptance
  criterion precisely.
- The D4 warning suppression in permissive mode eliminates noise for agents,
  making D4 useful only where it belongs (template author validation).
- The compat code path is bounded and named. Removing it when the flag is
  dropped is a contained change: delete the flag definition, the `strict`
  parameter from `validate()` and `compile()`, the D4 early return, and the
  evidence merge guard.

### Negative

- The flag is on the CLI, not the template. A template doesn't self-document
  its legacy status — you have to check the CI script to know it's on the
  legacy path.
- `has_gates_routing` is computed unconditionally rather than only inside the
  `if any_failed` block. Minor structural change to the advance loop.

### Mitigations

- The scope of legacy templates is known and small (one template in shirabe).
  The discoverability concern is minimal at this scale.
- Hoisting `has_gates_routing` is a small refactor with no behavioral change
  for the `if any_failed` path — the boolean is computed identically, just
  earlier.

## Future work: removing backward compatibility

When the last legacy template migrates to structured `gates.*` routing, the
compat code introduced by this feature should be removed. This is intentionally
not planned here — the trigger is shirabe's migration, not a koto milestone.

The removal is a contained, mechanical change across four sites:

**`src/cli/mod.rs`**
- Remove the `--allow-legacy-gates` flag and its `TODO` comment from
  `handle_template_compile()`
- Remove the `strict = false` argument from the `compile()` call in
  `handle_init()` (or make strict the only mode and drop the parameter)

**`src/template/types.rs`**
- Remove the `strict: bool` parameter from `validate()`
- Remove the D5 check (the legacy gate detection block)
- Remove the D4 early return in `validate_gate_reachability()` (the
  `if !strict { return Ok(()); }` guard)

**`src/template/compile.rs`**
- Remove the `strict: bool` parameter from `compile()`

**`src/engine/advance.rs`**
- Remove the `has_gates_routing` guard on the evidence merge. Gate output
  is injected for all states once legacy mode no longer exists.
- `has_gates_routing` may still be needed for the `GateBlocked` early return
  path — evaluate whether it can be removed entirely at that point.

After removal, `validate()` and `compile()` have no concept of permissive mode.
`koto init` and `koto template compile` run the same validation. The engine
always injects gate evidence into the resolver map when gates are present.

The removal PR should include a note in the commit message referencing this
design and the migration PR in shirabe that triggered it.
