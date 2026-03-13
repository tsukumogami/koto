# Phase 3 Research: Template Format Migration

## Questions Investigated

- What does the current template YAML/TOML format look like for states, transitions, and gates?
- How does `pkg/template/` parse and compile templates into `pkg/engine/` types?
- What is the format version field? Where is it stored? How is it validated?
- What would the new `TransitionDecl` format look like in YAML?
- What's the full pipeline from template file → compiled machine state → engine types?
- Are there example template files in the repo? What do they look like?

## Findings

### Source format (`pkg/template/compile/compile.go`)

YAML frontmatter + markdown body. `sourceFrontmatter` parsed by go-yaml v3. States declared
as a map with:
- `transitions: [list of strings]` (line 48) — the field that must change
- `gates: map[string]sourceGateDecl` (line 50) — state-level gates, currently attached to
  individual states, not transitions

### Compiled format (`pkg/template/compiled.go`)

JSON with `CompiledTemplate` struct:
- `FormatVersion: 1` (line 14) — the versioning field
- `States: map[string]StateDecl` where `StateDecl` has:
  - `Transitions: []string` (line 35) — simple string list
  - `Gates: map[string]engine.GateDecl` (line 37) — state-level gate map

Format version validated at line 49 in `compiled.go` — rejects anything other than version 1.

### Compilation pipeline

```
Source file
  → compile.Compile()           (YAML parse → CompiledTemplate struct)
  → CompiledTemplate (JSON)     (cached in ~/.koto/cache/ keyed by SHA-256 of source)
  → template.ParseJSON()
  → template.ToTemplate()
  → engine.Machine with gates on MachineState
```

### Cache system (`pkg/cache/cache.go`)

Stores compiled JSON keyed by SHA-256 hash of source file in `~/.koto/cache/` or
`$KOTO_HOME/cache/`. Cache invalidation is implicit: new source hash = new cache entry.

### Test fixtures

`scenario9Source` in `compile_test.go` (lines 14-76) shows the current array-based
transitions format. These tests must be updated for the new format.

### DESIGN-koto-template-format.md

An existing design doc explicitly defers "Transition-level gates" to Phase 2, noting this
is out of scope for Phase 1. This confirms the feature is acknowledged and the format was
intentionally designed to evolve.

## Implications for Design

**Breaking change is well-contained.** The format version field already exists as a single
enforcement point. Version bump from 1 → 2, with validation rejecting v1 templates at
`compiled.go:49` or providing a migration error message.

**YAML parsing change**: `sourceStateDecl.Transitions` changes from `[]string` to
`[]sourceTransitionDecl{Target string, Gates map[string]sourceGateDecl}`. The go-yaml v3
library handles both shapes cleanly.

**Compiled JSON change**: `StateDecl.Transitions []string` → `[]TransitionDecl{Target string, Gates map[string]engine.GateDecl}`. The `engine.GateDecl` type is already the right shape — no changes to the gate type itself.

**New `TransitionDecl` type location**: Can live in `pkg/template/compiled.go` (alongside
`StateDecl`) or in `pkg/engine/types.go` (used by both). Given it carries `engine.GateDecl`,
the cleaner location is `pkg/engine/types.go` with `pkg/template` importing it.

**Cache compatibility**: No special handling needed. Old compiled templates have a different
SHA-256 source hash only if the source changes; but the format version check at parse time
will reject old compiled JSON if someone has a v1 compiled cache entry. Cache entries are
cheap to invalidate (delete `~/.koto/cache/`).

**Example template new format** (illustrative):
```yaml
states:
  gather_info:
    transitions:
      - target: analyze
        gates:
          has_data:
            type: field_not_empty
            field: input_file
      - target: skip_to_output
    gates:
      # shared gates (evaluated before any transition attempt)
      workflow_initialized:
        type: field_not_empty
        field: workflow_id
```

## Surprises

1. Transition-level gates were already anticipated in the existing template format design doc
   (`DESIGN-koto-template-format.md`) — deferred to Phase 2. This change is expected and
   planned for in the overall architecture.
2. The cache system uses content-addressed storage (SHA-256), so format version changes don't
   require explicit cache invalidation — a changed source file gets a new hash.
3. The compilation pipeline is clean and well-separated; the breaking change is isolated to
   the `sourceStateDecl` parsing and `StateDecl` compiled struct.

## Summary

The template compilation pipeline (source YAML → compiled JSON → engine types) is cleanly
separated and well-suited for the `TransitionDecl` change. The format version field (currently
1) provides the enforcement point for the breaking schema change; a bump to version 2 with
clear error messaging handles migration. The change to `Transitions: []string` → `[]TransitionDecl`
touches three layers (source parser, compiled struct, engine type import) but the blast radius
is contained and the go-yaml library handles both shapes without additional infrastructure.
