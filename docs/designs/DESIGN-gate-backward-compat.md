---
status: Proposed
upstream: docs/prds/PRD-gate-transition-contract.md
problem: |
  Feature 1 preserved legacy gate behavior implicitly: when no `when` clause
  references `gates.*` fields, gates act as boolean pass/fail blockers. This
  implicit path is invisible to the compiler, so new templates can accidentally
  use legacy mode with no error or warning. The only known legacy template needs
  to keep working until it migrates.
decision: |
  A `legacy_gates: true` field in template frontmatter declares intentional
  legacy mode. The compiler errors for gates without `gates.*` routing unless
  this field is present, suppresses D4 warnings for declared-legacy templates,
  and emits a stderr warning from `koto init`. The engine excludes gate output
  from the resolver's evidence map for legacy states, matching R10 precisely.
rationale: |
  The frontmatter field was chosen over a CLI flag because the template
  self-documents its tech debt — reviewers see it in the file, not in CI
  scripts. The evidence exclusion (rather than injecting-but-ignoring) matches
  the PRD acceptance criterion and removes a misleading invariant. Both changes
  are independent and deletable when the last legacy template migrates.
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
It needs to keep working until migrated to structured routing, but it should
carry a visible marker that it's on the legacy path.

## Decision drivers

- Template authors must not accidentally opt into legacy mode. New templates
  using gates without `gates.*` routing should fail compilation by default.
- The legacy marker must be easy to locate and remove. When a template migrates
  to structured routing, the migration PR should consist of removing one
  frontmatter line and updating the transitions — nothing else.
- `koto init` must succeed for any template that would also pass `koto template
  compile`. Templates with legacy gates must declare `legacy_gates: true` to
  pass both commands.
- D4's unreferenced-field warning must not fire for templates that have
  explicitly declared legacy mode. The warning is for structured-mode templates
  where gate output is meant to drive routing but some fields are never checked.
- The engine's behavior for legacy states must match R10 precisely: gate output
  should not enter the resolver's evidence map for legacy states.
- The legacy code path must be self-contained and deletable. When the last
  legacy template migrates, removing the compat code should be a contained change.

## Considered options

### Decision 1: How legacy mode is declared and enforced

Template authors using gates without `gates.*` routing need a way to signal
that this is intentional. Without an explicit signal, the compiler can't
distinguish a legacy template from a new template that forgot to add `gates.*`
references. The choice of where this signal lives determines how visible and
removable it is, and how `koto init` vs `koto template compile` behave.

#### Chosen: Per-template frontmatter field `legacy_gates: true`

A boolean field in the template's YAML frontmatter:

```yaml
---
name: my-workflow
version: "1.0"
legacy_gates: true
initial_state: start
states:
  ...
---
```

When `legacy_gates: true` is present, the compiler accepts gates without
`gates.*` routing and suppresses D4 unreferenced-field warnings. When it's
absent and legacy behavior is detected (gate present, no `gates.*` references
in any transition for that state), the compiler emits an error naming the
state and gate.

`koto init` runs `compile()` on a cache miss, which calls `validate()`.
A template with gates but no `gates.*` routing and no `legacy_gates: true`
field will fail `koto init` with the D5 error — the same as `koto template
compile`. Templates that carry `legacy_gates: true` succeed on both init and
compile. After a successful init of a template with `legacy_gates: true`, init
emits a single `eprintln!` warning informing the caller that the template uses
legacy gate behavior and linking to migration guidance. No code path divergence
in the compile chain: `SourceFrontmatter` gets a new optional `legacy_gates`
field, `CompiledTemplate` carries it through, and `validate()` reads it.

Migration is a one-line change to the template frontmatter plus updating the
affected transitions to use `gates.*` routing. The migration PR is
self-contained and reviewable: removing `legacy_gates: true` is the commit
signal that a template has been fully migrated.

#### Alternatives considered

**CLI flag `--allow-legacy-gates` on `koto template compile`**: The flag is
external to the template, so the template doesn't self-document its compat
requirement. CI scripts carry the flag indefinitely, and there's no single
place to look to understand which templates are on the legacy path. Rejected
because it violates the co-location and self-documentation requirements.

**Per-state annotation**: Each state that uses legacy behavior could declare
`legacy_gates: true` at the state level. But migration is a whole-template
operation — you update all gate-bearing states at once — so state-level
granularity adds migration surface without benefit. Rejected.

**Separate compile subcommand `koto template compile-legacy`**: This would
give the compiler a clean signal at invocation time, but it adds a permanent
new CLI surface that's harder to remove than a frontmatter field. Rejected.

**Auto-detect with warning only (no error)**: Leave the compiler permissive
and rely on D4 warnings to surface legacy behavior. Rejected because it
allows new templates to accidentally opt into legacy mode silently, which is
the core problem this feature is meant to solve.

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

**Chosen: D1 frontmatter field + D2 evidence exclusion**

### Summary

A new `legacy_gates: true` field in a template's YAML frontmatter declares
that the template intentionally uses gates as pure pass/fail blockers rather
than structured routing sources. The compiler reads this field from
`SourceFrontmatter`, carries it through to `CompiledTemplate`, and uses it
at two points in `validate()`: to suppress the error that would otherwise fire
when gates are present without any `gates.*` when-clause references, and to
suppress D4's unreferenced-field warnings for the entire template. Templates
without this field that use legacy gate behavior fail compilation with a
message naming the state and gate.

`koto init` runs the same compile and validation pipeline as `koto template
compile`. A template with legacy gates must carry `legacy_gates: true` to
succeed on both commands. After initializing a template that carries
`legacy_gates: true`, init emits a non-fatal warning to stderr to surface the
legacy status to the caller.

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

The frontmatter field (D1) and the evidence exclusion (D2) are independent
changes to different layers — compiler and engine — that each address a
separate gap. D1 makes legacy mode explicit and auditable; D2 makes the engine
behavior match the spec. Neither requires the other, but they should land
together so the observable semantics are consistent: once a template declares
`legacy_gates: true`, both the compiler and the engine treat it consistently.

The frontmatter field was chosen over a CLI flag because the template
self-documents its tech debt. A reviewer scanning the template sees the field
and knows it's on the migration path. When the migration PR arrives, deleting
`legacy_gates: true` is a commit-level signal that the template has been
updated.

## Solution architecture

### Overview

Two isolated changes in two different layers. The compiler change adds
`legacy_gates` to the frontmatter schema and uses it to gate error emission
and D4 warnings. The engine change adds a condition to the evidence merge
step in the advance loop.

### Components

**`src/template/compile.rs` — SourceFrontmatter + compile()**

Add `legacy_gates: Option<bool>` to `SourceFrontmatter`. It serializes from
the YAML frontmatter with a `#[serde(default)]` — absent means `false`.
The `compile()` function reads it and populates the `CompiledTemplate`
constructor: `legacy_gates: fm.legacy_gates.unwrap_or(false)`. All three
sites must be updated together: the `SourceFrontmatter` field, the
`CompiledTemplate` field, and the `CompiledTemplate` struct literal in
`compile()`.

**`src/template/types.rs` — CompiledTemplate + validate()**

Add `legacy_gates: bool` to `CompiledTemplate` (default `false`).

In `validate()`, two changes:

1. **New D5 check (legacy gate error):** After the D2/D3 checks, scan each
   state. If any state has gates but no `gates.*` when-clause references AND
   `!self.legacy_gates`, emit an error:
   ```
   state "<name>": gate "<gate>" has no gates.* routing; declare legacy_gates: true
   in the template frontmatter to allow boolean pass/block behavior, or add a
   when clause referencing gates.<gate>.passed, gates.<gate>.error, ...
   ```
   This check runs before D4 so D4 is still unreachable when this error fires.

2. **D4 suppression:** The `validate_gate_reachability()` call site gains an
   early return when `self.legacy_gates` is true:
   ```rust
   if self.legacy_gates {
       return Ok(());
   }
   ```
   Note: for a legacy state, `validate_gate_reachability()` would vacuously
   return `Ok(())` anyway (no gate transitions exist). The early return is
   needed to suppress the `eprintln!` warning loop that iterates over gate
   schema fields — that loop would emit a warning per field for every gate in
   the template, which is noise for an intentionally-legacy template.

**`src/cli/mod.rs` — handle_init()**

After a successful init, check if `compiled.legacy_gates`. If true, emit:
```
warning: this template uses legacy gate behavior (legacy_gates: true).
Gates block state transitions but their output is not available for routing.
Migrate to gates.* when-clause references when ready.
```

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

`has_gates_routing` is already computed at lines 395–403 in the `if any_failed`
block. It needs to be hoisted to be available at the evidence merge step
(~line 437), which runs regardless of whether any gate failed.

### Key interfaces

**Frontmatter schema change:**
```yaml
---
name: my-workflow
version: "1.0"
legacy_gates: true     # optional; default false
initial_state: start
---
```

**Compiler error for undeclared legacy behavior:**
```
error: state "verify" gate "ci_check" has no gates.* routing
  declare legacy_gates: true in frontmatter for boolean pass/block behavior
  or add a when clause referencing gates.ci_check.passed, gates.ci_check.error, ...
```

**New compile-time warning for legacy templates:**
No warning at `koto template compile` time (the field is the explicit opt-in,
no further signal needed). Warning is emitted by `koto init` only.

### Data flow

```
SourceFrontmatter.legacy_gates
  │
  └─► CompiledTemplate.legacy_gates
        │
        ├─► validate() D5 check ─── error if gates without routing AND !legacy_gates
        │
        ├─► validate_gate_reachability() ── early return if legacy_gates
        │
        └─► handle_init() ── eprintln! warning if legacy_gates
        
advance loop
  │
  ├─► evaluate gates ──► gate_results (always)
  │                      gate_evidence_map (always)
  │
  ├─► has_gates_routing (hoisted to cover both GateBlocked path and evidence merge)
  │
  └─► evidence merge: insert "gates" only if has_gates_routing
```

## Implementation approach

### Phase 1: Frontmatter field and compiler enforcement

Add `legacy_gates: Option<bool>` to `SourceFrontmatter` in
`src/template/compile.rs`. Add `legacy_gates: bool` (default false) to
`CompiledTemplate` in `src/template/types.rs`. Add the D5 check in `validate()`
that errors for gates without `gates.*` routing when `!self.legacy_gates`.
Add the `validate_gate_reachability()` early return when `self.legacy_gates`.

Deliverables:
- `SourceFrontmatter` with `legacy_gates` field
- `CompiledTemplate` with `legacy_gates` field
- D5 compiler error with actionable message
- D4 warning suppression via early return
- Unit tests: template with `legacy_gates: true` compiles; template without it
  and with legacy gates fails with the D5 error message; template with neither
  gates nor `legacy_gates` compiles without change

### Phase 2: `koto init` warning

Add an `eprintln!` warning to `handle_init()` in `src/cli/mod.rs` when the
initialized template has `compiled.legacy_gates == true`.

Deliverables:
- `handle_init()` emits warning for legacy templates
- Integration test: `koto init` with a `legacy_gates: true` template exits 0
  and emits the warning to stderr

### Phase 3: Engine evidence exclusion

Hoist `has_gates_routing` computation in `src/engine/advance.rs` so it's
available at the evidence merge step regardless of whether `any_failed` is
true. Guard the `gate_evidence_map` insertion with `has_gates_routing`.

**Phase 3 must land in the same PR as Phase 1.** If Phase 3 ships without
Phase 1, the engine stops injecting gate evidence for legacy states before
the compiler provides a way for those templates to declare `legacy_gates: true`
— there would be no upgrade path for existing legacy templates.

Deliverables:
- `has_gates_routing` initialized to `false` before the gate block, then set
  inside it (ensures it's always in scope at the merge step)
- Evidence merge guarded by `has_gates_routing`
- Unit tests: legacy state does not expose `gates.*` keys in merged evidence;
  structured-mode state does

## Security considerations

This design adds a frontmatter field that the compiler reads and a conditional
in the engine. Neither introduces new attack surface.

The `legacy_gates: true` field is set by the template author, not by any
external input at runtime. A malicious template could set this field to bypass
the new D5 compiler error — but `koto template compile` is run by the template
author or in CI, not on untrusted input. The same trust boundary applies to all
other frontmatter fields.

The evidence exclusion change (Phase 3) reduces the data available in the
resolver's evidence map for legacy states. It can't be used to inject data
that wasn't there before; it only removes engine-produced data from the map.
No new data flows to external parties.

The `koto init` warning writes to stderr only and contains no user data.

## Consequences

### Positive

- New templates can't accidentally use legacy mode. The compiler error is clear
  and actionable.
- Every template using legacy mode is discoverable by searching for
  `legacy_gates: true` — one grep across the template corpus.
- The engine behavior for legacy states now matches the PRD acceptance criterion
  precisely.
- The D4 warning suppression eliminates false positives for intentionally-legacy
  templates, making the warning useful only where it belongs.
- The compat code path is bounded. Removing it when the last legacy template
  migrates is a small, contained change: delete the D5 check, the D4 early
  return, the init warning, and the evidence merge guard.

### Negative

- Templates currently using legacy gate behavior (any gates without `gates.*`
  routing, no `legacy_gates` field) will fail compilation after this lands.
  They must add `legacy_gates: true` to their frontmatter before the next
  `koto template compile` run.
- `has_gates_routing` is computed unconditionally rather than only inside the
  `if any_failed` block. Minor structural change to the advance loop.

### Mitigations

- The compiler error names the specific state and gate, making the fix obvious.
  Template authors can either add `legacy_gates: true` (one line, preserves
  behavior) or migrate to structured routing.
- Hoisting `has_gates_routing` is a small refactor with no behavioral change
  for the `if any_failed` path — the boolean is computed identically, just
  earlier.
