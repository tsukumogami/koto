# Phase 3 Research: Template Event Schema Declarations

## Questions Investigated

1. What does the current template YAML format look like?
2. What does the compilation pipeline look like?
3. What is the format version field and how is it used?
4. What should the new YAML syntax look like for evidence field schemas?
5. What should `when` conditions on transitions look like?
6. How should integration declarations be expressed?
7. What existing template files exist in the repo?
8. What does "mutual exclusivity" mean concretely, and how can the compiler detect it?
9. What is the right compilation output for event schemas?

## Findings

### Current template format (`pkg/template/compile/compile.go`)

Source format: YAML frontmatter + markdown body. The YAML declares state names; the
markdown `## <heading>` sections provide directive text for each state. The compiler
resolves headings by matching them against the declared state list — heading collisions
are not an issue because only declared state names are matched.

Current compiled state structure (`pkg/template/compiled.go`):
```go
type StateDecl struct {
    Directive   string
    Transitions []string            // simple string list of target state names
    Terminal    bool
    Gates       map[string]GateDecl // state-level gates
}
```

Gate types supported: `field_not_empty`, `field_equals`, `command`. Gates are state-level
only — evaluated before any outgoing transition.

### Compilation pipeline

```
Source YAML + markdown
  → compile.Compile()      (parse YAML frontmatter, extract directives from markdown)
  → CompiledTemplate JSON  (format_version, states map, variables)
  → cached by SHA-256 of source file
  → template.ParseJSON()   → CompiledTemplate struct
  → template.ToTemplate()  → engine.Machine
```

Format version is a top-level `format_version: int` field in the compiled JSON. The
parser rejects unsupported versions at load time (`compiled.go:49-50`). Currently at
version 1.

### New YAML syntax for event schema declarations

The compiled `StateDecl` needs a new `EventSchema` field carrying per-state evidence
requirements and per-transition `when` conditions. The source YAML must express:

```yaml
states:
  analyze_results:
    transitions:
      # Per-transition conditions replace the flat string list
      - target: deploy
        when:
          decision: proceed       # evidence field value that triggers this transition
      - target: escalate_review
        when:
          decision: escalate
    # Evidence field declarations (the event schema for evidence_submitted events)
    accepts:
      decision:
        type: enum
        values: [proceed, escalate]
        required: true
      rationale:
        type: string
        required: true
    # Existing state-level gates remain (for condition integrations)
    gates:
      tests_passed:
        type: command
        command: ./check-ci.sh
    # Processing integration declaration (from PRD R7)
    integration: delegate_review   # name maps to user config; template stays portable
```

States with no `accepts` block and no `when` conditions on transitions are auto-advanced
through (conditions are koto-verifiable gates only).

### Integration declarations

Per PRD R7 (processing integrations): declared as a tag on the state (`integration: <name>`),
with routing from tag name to actual CLI staying in user config. This keeps templates
portable — a template that tags a state as `integration: deep_review` works in any
environment; only the config binding changes.

The compiled `StateDecl` gains an `Integration string` field (empty = no processing
integration).

### Mutual exclusivity detection (PRD R18)

Two transitions from the same state are mutually exclusive if no single evidence submission
can satisfy both sets of `when` conditions simultaneously. For simple enum conditions:
if transition A requires `decision == "proceed"` and transition B requires
`decision == "escalate"`, they are mutually exclusive by value disjointness.

For the compiler to detect violations statically, it must check: for every pair of outgoing
transitions with `when` conditions, can a single evidence map satisfy both? For simple
equality conditions on the same field (`decision == X` vs. `decision == Y`), this is a
string equality check. For conditions on different fields, mutual exclusivity is not
guaranteed and the compiler should warn or require explicit annotation.

This is a template compile-time validation, not a runtime concern. Invalid templates are
rejected with a clear error identifying the conflicting transitions.

### Compilation output for event schemas

The compiled JSON gains a new `EventSchema` structure per state:

```json
{
  "format_version": 2,
  "states": {
    "analyze_results": {
      "directive": "...",
      "terminal": false,
      "integration": "delegate_review",
      "gates": { "tests_passed": { "type": "command", "command": "./check-ci.sh" } },
      "accepts": {
        "decision": { "type": "enum", "values": ["proceed", "escalate"], "required": true },
        "rationale": { "type": "string", "required": true }
      },
      "transitions": [
        { "target": "deploy", "when": { "decision": "proceed" } },
        { "target": "escalate_review", "when": { "decision": "escalate" } }
      ]
    }
  }
}
```

Transitions change from `[]string` to `[]TransitionDecl{Target, When}`. The `accepts`
block is the event schema for `evidence_submitted` events in this state.

## Implications for Design

- **Format version bumps to 2**: source YAML and compiled JSON both change; old format
  rejected at compile time with a migration message
- **`accepts` + `transitions[].when` are the two new YAML blocks**: `accepts` declares
  what the event payload must contain; `when` conditions on transitions declare which
  transition fires based on payload values
- **Existing `gates` remain**: they're the mechanism for condition integrations (command
  gates, field checks); they coexist with `accepts`/`when` — gates are koto-verified,
  `accepts` is agent-submitted
- **`integration` field is a string tag**: single value per state; routing lives in config
- **Mutual exclusivity is a compiler validation**: simple cases (same field, disjoint
  values) are auto-detected; complex cases (multi-field conditions) require the template
  author to annotate

## Surprises

1. The compilation pipeline already uses SHA-256 content hashing for the cache — this also
   serves as a tamper-detection mechanism, since any template modification produces a
   different hash and invalidates the cache
2. The markdown body is already parsed by matching `## headings` against the declared state
   list, not by position — this means the new `accepts` and `when` fields live entirely
   in YAML frontmatter and don't affect the markdown parsing at all
3. No example template files exist in the repo outside of test fixtures — the format design
   has no existing user-facing documentation to update, which simplifies the migration story

## Summary

The current compiled template stores transitions as a flat `[]string` and gates as a
state-level map. The new format adds two YAML blocks: `accepts` (per-state evidence field
schema) and `when` conditions on transitions (per-transition routing conditions). These
coexist with existing `gates`. The `integration` field is a string tag routing to user
config. Mutual exclusivity of per-transition conditions is a compile-time validation for
simple same-field cases. Format version bumps to 2; existing templates must be rewritten
(no backward compatibility with `transitions: []string`).
