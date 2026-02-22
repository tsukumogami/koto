---
status: Proposed
problem: |
  koto templates need to serve two audiences: the engine (which needs deterministic,
  unambiguous state machine definitions) and humans (who need to author and understand
  workflow definitions naturally). The v1 design attempted a single format for both,
  creating cascading complexity: heading collision, declared-state matching, dual
  transition sources, format wars. These are symptoms of conflating two concerns.
decision: |
  Separate the template into a canonical JSON format (what the engine reads) and a
  markdown authoring format (what humans write). JSON is deterministic, schema-
  validatable, and uses Go's stdlib. Markdown renders on GitHub and naturally embeds
  rich directive content. A deterministic compiler converts markdown to JSON. An
  optional LLM-assisted linter helps humans fix authoring errors before compilation,
  but sits outside the parse path. The engine only ever reads JSON.
rationale: |
  The dual-format approach eliminates the heading collision problem entirely (JSON
  has no headings), removes the YAML-vs-TOML debate (JSON for machines, markdown for
  humans), and lets each format do what it's good at. The compilation step is
  deterministic -- same input always produces same output. LLM assistance is confined
  to the authoring tooling layer, never in the engine's parse path. JSON Schema
  provides editor support and validation without custom tooling.
---

# DESIGN: koto Template Format Specification

## Status

**Proposed**

## Context and Problem Statement

koto templates define workflow state machines that guide AI agents through multi-step tasks. A template contains two types of content:

**Structure**: States, transitions, evidence gates, variables, and metadata. This is the state machine definition -- it must be parsed deterministically with zero ambiguity. The engine depends on this being correct.

**Content**: Directive text for each state -- the instructions an AI agent reads when it enters a state. This is rich markdown with tables, code blocks, headings, and examples. It needs to be authored by humans and rendered readably.

The v1 design tried to serve both needs with a single file format (TOML/YAML frontmatter + markdown body). This created a cascade of problems:

- **Heading collision**: `## headings` in directive content conflicted with `## headings` marking state boundaries, requiring "declared-state matching" rules
- **Dual transition sources**: Transitions declared in the header AND `**Transitions**:` lines in the body, with precedence rules for conflicts
- **Format debates**: TOML vs YAML vs extended markdown -- each had trade-offs because no single format serves both deterministic parsing and human authoring well
- **Parsing fragility**: Code blocks containing state-name headings, whitespace sensitivity, case sensitivity rules -- all artifacts of using a content format for structure

These problems share a root cause: **conflating the machine-readable format with the human authoring format**. They don't need to be the same.

### The Insight

Systems like Terraform (HCL/JSON), Protocol Buffers (.proto/binary), and Markdoc (markdown/AST) separate the human authoring format from the machine-readable format, with a deterministic compilation step between them. This separation lets each format do what it's good at:

- The **canonical format** is optimized for machines: deterministic, schema-validatable, unambiguous
- The **authoring format** is optimized for humans: readable, writable, renderable
- The **compiler** bridges them deterministically: same input always produces same output
- **Validation tooling** (including optional LLM assistance) helps humans write correct input before compilation

### Scope

**In scope:**
- Canonical (machine-readable) format specification
- Markdown authoring format specification
- Deterministic compilation from markdown to canonical format
- Evidence gate declaration syntax (both formats)
- Variable declaration syntax (both formats)
- Template search path and resolution
- Validation contract (parse-time, compile-time, explicit)
- LLM-assisted validation architecture (where it fits, what it can/cannot do)

**Out of scope:**
- Evidence gate evaluation logic (how the engine checks gates at transition time)
- LLM integration implementation details (model selection, prompting strategy)
- Template registry or community distribution
- Built-in template content (that's the quick-task template design, #13)

## Decision Drivers

- **Deterministic machine format**: The engine must parse templates with zero ambiguity. No heading collision, no precedence rules, no whitespace sensitivity.
- **Human-friendly authoring**: Templates should be natural to write and render well on GitHub. Rich directive content (markdown) is a first-class need.
- **Deterministic compilation**: Converting from human format to machine format must produce identical output for identical input. No LLMs in the parse path.
- **LLM assistance at the validation layer only**: LLMs can help fix authoring errors, but the compiler itself is deterministic. This keeps the system predictable.
- **Zero dependencies for core engine**: The engine reads the canonical format using only Go's standard library. Dependencies (if any) are confined to the compiler/tooling layer.
- **Progressive complexity**: Simple templates should be simple to author. Evidence gates, search paths, and LLM validation are opt-in features.
- **Schema-based tooling**: The canonical format should support JSON Schema for editor autocomplete, linting, and validation.

## Implementation Context

### Current Template Format

The parser in `pkg/template/template.go` uses a YAML-like frontmatter (`---` delimiters) with markdown body sections. It handles flat `key: value` pairs and one level of nesting (`variables:` block). States are identified by `## heading` markers. The parser produces a `Template` struct containing a `Machine`, section content, variables, and a SHA-256 hash.

Key limitation: the parser treats ALL `## headings` as state boundaries, creating the heading collision problem. The hand-rolled YAML parser can't handle nested structures (evidence gates).

### Industry Patterns

Research into dual-format systems reveals:

- **Terraform**: HCL and JSON are both first-class. Either can be authored. Same internal representation. Bidirectional conversion.
- **Markdoc (Stripe)**: Markdown compiles to a serializable AST. The AST is the canonical format -- cacheable, validatable, transformable.
- **OpenAPI / Kubernetes**: YAML authoring validated against JSON Schema. Schema drives editor tooling.
- **CUE**: Validation rules embedded in the data definition. Gradual validation (partial specs are valid).
- **LLM-assisted validation**: Research shows LLMs useful for suggestions but core validation must be deterministic. Output validation rules address non-determinism.

No tool in the AI agent workflow space uses a compiled template approach. koto would be the first to separate authoring format from execution format for workflow state machines.

## Considered Options

### Decision 1: Architecture

Should koto use a single format or separate the authoring and execution formats?

#### Chosen: Dual-format with deterministic compilation

Two formats connected by a deterministic compiler:

- **Canonical format** (JSON): What the engine reads. Deterministic, schema-validatable, zero-ambiguity. Stored as `.koto.json` files.
- **Authoring format** (Markdown): What humans write. YAML frontmatter for structure, markdown body for directives. Stored as `.md` files. Renders on GitHub.
- **Compiler**: `koto template compile template.md` produces `template.koto.json`. Same input always produces same output. No LLMs involved.
- **Decompiler**: `koto template decompile template.koto.json` produces a readable markdown file. Enables round-tripping.
- **Linter**: `koto template lint template.md` validates the authoring format and suggests fixes. Optionally uses LLMs for ambiguity resolution.

The engine only reads `.koto.json` files. The `koto init` command accepts either format: if given a `.md` file, it compiles it first, then initializes from the JSON.

#### Alternatives Considered

**Single YAML format (go-yaml v3)**: YAML frontmatter with block scalars for directives. Simpler (no compilation step), but the heading collision problem returns if directives contain markdown with headings. YAML block scalars are awkward for long markdown content. Editor support is good (JSON Schema works for YAML too) but the format still conflates structure and content.

**Single JSON format**: JSON for everything, including directives as string fields. Maximally deterministic, but JSON with embedded markdown (`\n` escapes, no syntax highlighting inside strings) is painful to write. No GitHub rendering. Humans would need tooling assistance for everything.

**Markdown-only with conventions**: Pure markdown with structural conventions (specific heading levels, blockquotes for metadata). Renders beautifully on GitHub but has no schema validation ecosystem, no editor support, and the parser depends on exact formatting conventions. Fragile.

**Directory-based format**: JSON manifest + separate `.md` files per state (`states/assess.md`). Clean separation but one template becomes a directory, complicating distribution, search paths, and template management.

### Decision 2: Canonical Format

What format should the engine read?

#### Chosen: JSON

The engine reads JSON files using Go's `encoding/json` (standard library, zero dependencies). JSON is:

- Deterministic to parse (no implicit typing, no multiple representations)
- Schema-validatable (JSON Schema for editor support and CI validation)
- Already used by koto for state files (consistent ecosystem)
- Well-supported by every language and tool

The canonical format uses the `.koto.json` extension to distinguish from plain JSON files.

#### Alternatives Considered

**YAML**: More readable than JSON for humans, but the engine reads the canonical format -- readability for machines doesn't matter. YAML adds a dependency (go-yaml) to the engine's parse path, violating the zero-dependency goal.

**TOML**: Good for configuration but awkward for the list-of-states pattern. Adds a dependency to the engine. Not widely used for data interchange.

**Custom binary format**: Maximum parsing speed, but templates are read once at init time. The performance benefit doesn't justify the tooling cost.

### Decision 3: Authoring Format

What format do humans write?

#### Chosen: YAML frontmatter + Markdown body

The authoring format uses standard YAML frontmatter (`---` delimiters) for the state machine structure and markdown body for directive content:

```markdown
---
name: quick-task
version: "1.0"
description: A focused task workflow
initial_state: assess

variables:
  TASK:
    description: What to build
    required: true
  REPO:
    description: Repository path
    default: "."

states:
  assess:
    transitions: [plan, escalate]
    gates:
      task_defined:
        type: field_not_empty
        field: TASK
  plan:
    transitions: [implement]
  implement:
    transitions: [done]
    gates:
      tests_pass:
        type: command
        command: go test ./...
  escalate:
    terminal: true
  done:
    terminal: true
---

## assess

Analyze the task: {{TASK}}

Review the codebase in {{REPO}} and determine:
- What files need to change
- How complex the change is
- Whether tests exist for the affected code

## plan

Create an implementation plan for {{TASK}}.

Break the work into steps. Identify tests to write.

## implement

Execute the plan. Write code and tests.

## done

Work is complete.

## escalate

Task could not be completed in this workflow.
```

The YAML frontmatter contains the complete state machine definition. The markdown body contains directive content, matched to states by `## heading` text. GitHub renders this as a nice document with the frontmatter hidden.

**Why YAML for the frontmatter**: YAML is the industry standard for frontmatter. GitHub renders it. Editors support it. go-yaml is a dependency of the compiler/tooling layer (not the engine). The implicit typing concern (bare `yes` becoming boolean) is real but narrow -- template authors already quote version strings (`"1.0"`), and the compiler validates types.

**Heading collision is reduced, not eliminated**: In the authoring format, `## headings` matching state names are state boundaries. Other headings are directive content. The compiler resolves this deterministically and produces unambiguous JSON. If the authoring format is ambiguous, the linter catches it. The engine never sees the ambiguity -- it reads JSON.

#### Alternatives Considered

**Pure YAML**: All content in YAML, directives as block scalars. No heading collision, but long markdown in YAML block scalars is awkward (indentation-sensitive, no syntax highlighting, hard to edit).

**Pure markdown with conventions**: No frontmatter, everything encoded in markdown structure. Renders perfectly but has no schema validation, no editor support, and fragile parsing.

**TOML frontmatter**: Works well for nested config but `+++` delimiters aren't standard. GitHub doesn't render TOML frontmatter. Less familiar than YAML to most developers.

### Decision 4: Evidence Gate Declarations

How do template authors declare evidence requirements?

#### Chosen: Per-state gate declarations

Gates are declared under each state in both formats. Three gate types for Phase 1:

**field_not_empty**: A named field exists and is non-empty in the evidence map.

**field_equals**: A named field equals a specific value.

**command**: A shell command exits 0. Commands are literal strings -- no `{{VARIABLE}}` interpolation (prevents injection). Commands run via `sh -c` from the project root directory.

Gates on a state are exit conditions: all must pass (AND logic) before leaving that state. OR composition is not supported in Phase 1.

#### Alternatives Considered

**Transition-level gates**: Attach gates to specific transitions rather than states. Deferred for Phase 1 -- the common case is "satisfy these conditions before leaving this state." Transition-level gates can be added later.

**Inline gate syntax in markdown**: Declare gates in the body using special syntax. Rejected because it mixes machine configuration with content -- exactly the problem the dual-format approach solves.

### Decision 5: Template Search Path

How does koto find templates?

#### Chosen: Three-tier search with explicit override

Template resolution:

1. **Explicit path**: If `--template` is a file path (contains `/` or `.`), use it directly. Accepts both `.md` and `.koto.json`.
2. **Project-local**: `.koto/templates/<name>.koto.json` or `.koto/templates/<name>.md` (relative to git root or CWD).
3. **User-global**: `~/.config/koto/templates/<name>.koto.json` or `~/.config/koto/templates/<name>.md`.

If a `.md` file is found but no `.koto.json`, the compiler runs automatically. First match wins.

### Decision 6: LLM-Assisted Validation

Where do LLMs fit in the template workflow?

#### Chosen: Optional linter, outside the parse path

LLMs are confined to the `koto template lint` command. They are never invoked during `koto init`, `koto next`, `koto transition`, or any engine operation.

The linter workflow:

1. **Deterministic checks first**: Parse YAML frontmatter, validate against schema, check state references, verify heading matches. These are fast, free, and deterministic.
2. **LLM checks second (optional)**: If deterministic checks pass, optionally invoke an LLM to review:
   - Directive quality (are instructions clear enough for an agent?)
   - Gate completeness (should this state have gates?)
   - Variable usage (are all declared variables used in directives?)
3. **LLM fixes (optional)**: If deterministic checks fail, optionally invoke an LLM to suggest fixes:
   - "State 'assess' has no matching ## heading in body. Did you mean ## Assess?"
   - "Variable TASK is declared but never referenced in any directive."
4. **All fixes are suggestions**: The linter outputs suggestions. The human applies them. The compiler only processes the result after the human is done editing.

The LLM is a tool for humans, not a component of the system. koto works without it.

#### Alternatives Considered

**LLM in the compilation path**: Use LLMs to handle ambiguous markdown-to-JSON conversion. Rejected because it makes compilation non-deterministic. Same input could produce different output depending on model, temperature, or prompt changes.

**No LLM support**: Ignore LLMs entirely. The deterministic tooling is sufficient. The LLM linter is optional and can be added later without changing the architecture. This is a reasonable simplification for Phase 1.

## Decision Outcome

### Summary

koto uses a dual-format template system: a canonical JSON format for the engine and a markdown authoring format for humans. A deterministic compiler converts markdown to JSON. The engine only reads JSON -- no ambiguity, no heading collision, no format debates.

The canonical JSON format uses Go's standard library (`encoding/json`) with zero external dependencies. It contains the complete state machine definition (states, transitions, gates, variables) and directive content for each state.

The markdown authoring format uses standard YAML frontmatter (`---` delimiters) for structure and markdown body for directives. GitHub renders it. go-yaml parses the frontmatter in the compiler/tooling layer only.

An optional LLM-assisted linter helps humans fix authoring errors but sits entirely outside the parse and compilation path. koto works without it.

### Rationale

The dual-format approach eliminates the heading collision problem (JSON has no headings), removes the YAML-vs-TOML debate (JSON for machines, YAML frontmatter for humans), and lets each format excel at what it's designed for. The compilation step is deterministic -- there's no ambiguity about what the engine will read.

The cost is a compilation step and two file representations. This is acceptable because:
- Templates are authored infrequently (write once, run many times)
- The compiler is fast (YAML parse + JSON serialize)
- `koto init` can compile on the fly (no manual step required)
- Round-tripping (decompile) means either format can be the source of truth

### Trade-offs Accepted

- **Two formats**: Templates have a `.md` source and a `.koto.json` canonical form. This adds conceptual overhead for template authors. Mitigated by `koto init` accepting either format transparently.
- **go-yaml dependency in compiler**: The compiler (not the engine) depends on go-yaml for parsing YAML frontmatter. The engine remains dependency-free. Acceptable because the dependency is confined to tooling.
- **Directive duplication**: Directives exist in both the markdown body (human-readable) and the JSON file (as string fields). The compiler ensures consistency. If the JSON is generated from markdown, there's one source of truth.
- **No LLM gates**: Only deterministic gate types (field checks, commands) in Phase 1. LLM-based evaluation (prompt gates) deferred to a separate design.

## Solution Architecture

### Canonical Format (JSON)

A `.koto.json` file contains the complete template:

```json
{
  "schema_version": 1,
  "name": "quick-task",
  "version": "1.0",
  "description": "A focused task workflow",
  "initial_state": "assess",
  "variables": {
    "TASK": {
      "description": "What to build",
      "required": true,
      "default": ""
    },
    "REPO": {
      "description": "Repository path",
      "required": false,
      "default": "."
    }
  },
  "states": {
    "assess": {
      "directive": "Analyze the task: {{TASK}}\n\nReview the codebase in {{REPO}} and determine:\n- What files need to change\n- How complex the change is\n- Whether tests exist for the affected code",
      "transitions": ["plan", "escalate"],
      "gates": {
        "task_defined": {
          "type": "field_not_empty",
          "field": "TASK"
        }
      }
    },
    "plan": {
      "directive": "Create an implementation plan for {{TASK}}.\n\nBreak the work into steps. Identify tests to write.",
      "transitions": ["implement"]
    },
    "implement": {
      "directive": "Execute the plan. Write code and tests.",
      "transitions": ["done"],
      "gates": {
        "tests_pass": {
          "type": "command",
          "command": "go test ./..."
        }
      }
    },
    "escalate": {
      "directive": "Task could not be completed in this workflow.",
      "terminal": true
    },
    "done": {
      "directive": "Work is complete.",
      "terminal": true
    }
  }
}
```

**Schema version**: Enables forward-compatible evolution. The engine rejects unknown schema versions.

**Go types:**

```go
// CanonicalTemplate is the JSON-serializable template format.
type CanonicalTemplate struct {
    SchemaVersion int                        `json:"schema_version"`
    Name          string                     `json:"name"`
    Version       string                     `json:"version"`
    Description   string                     `json:"description,omitempty"`
    InitialState  string                     `json:"initial_state"`
    Variables     map[string]VariableDecl     `json:"variables,omitempty"`
    States        map[string]StateDecl        `json:"states"`
}

type VariableDecl struct {
    Description string `json:"description,omitempty"`
    Required    bool   `json:"required,omitempty"`
    Default     string `json:"default,omitempty"`
}

type StateDecl struct {
    Directive   string               `json:"directive"`
    Transitions []string             `json:"transitions,omitempty"`
    Terminal    bool                 `json:"terminal,omitempty"`
    Gates       map[string]GateDecl  `json:"gates,omitempty"`
}

type GateDecl struct {
    Type    string `json:"type"`
    Field   string `json:"field,omitempty"`
    Value   string `json:"value,omitempty"`
    Command string `json:"command,omitempty"`
}
```

**Validation rules (parse-time):**

| Check | Error |
|-------|-------|
| Invalid JSON | JSON parse error |
| Unknown `schema_version` | `"unsupported schema version: %d"` |
| Missing `name` | `"missing required field: name"` |
| Missing `version` | `"missing required field: version"` |
| Missing `initial_state` | `"missing required field: initial_state"` |
| `initial_state` not in states | `"initial_state %q is not a declared state"` |
| No states declared | `"template has no states"` |
| Transition target not in states | `"state %q references undefined transition target %q"` |
| Gate has unknown type | `"state %q gate %q: unknown type %q"` |
| Gate missing required field | `"state %q gate %q: missing required field %q"` |
| State has empty directive | `"state %q has empty directive"` |

### Authoring Format (Markdown)

A `.md` file with YAML frontmatter:

```markdown
---
name: quick-task
version: "1.0"
description: A focused task workflow
initial_state: assess

variables:
  TASK:
    description: What to build
    required: true
  REPO:
    description: Repository path
    default: "."

states:
  assess:
    transitions: [plan, escalate]
    gates:
      task_defined:
        type: field_not_empty
        field: TASK
  plan:
    transitions: [implement]
  implement:
    transitions: [done]
    gates:
      tests_pass:
        type: command
        command: go test ./...
  escalate:
    terminal: true
  done:
    terminal: true
---

## assess

Analyze the task: {{TASK}}

Review the codebase in {{REPO}} and determine:
- What files need to change
- How complex the change is
- Whether tests exist for the affected code

## plan

Create an implementation plan for {{TASK}}.

Break the work into steps. Identify tests to write.

## implement

Execute the plan. Write code and tests.

## done

Work is complete.

## escalate

Task could not be completed in this workflow.
```

The YAML frontmatter contains the complete state machine definition (identical data to the JSON canonical format). The markdown body provides directive content, matched by `## heading` to state names.

**Compilation rules:**

1. Parse YAML frontmatter using go-yaml v3.
2. Build the set of declared state names from `states:`.
3. Scan markdown body for `## headings` matching declared state names. Content between headings becomes the directive.
4. For each declared state, the directive comes from the markdown body. If a state has no matching heading, compilation fails.
5. The heading collision problem still exists in the authoring format but is contained: the compiler resolves it deterministically, and the engine never sees it.
6. Serialize to JSON. Compute SHA-256 hash of the JSON output.

**What the compiler does NOT do:**
- Parse `**Transitions**:` lines from the body. Transitions come exclusively from the YAML frontmatter. No dual sources.
- Treat non-matching headings as errors. `## Analysis` within a state directive is content, not a state boundary.
- Invoke LLMs. Compilation is deterministic.

### Evidence Gates

**Gate types (Phase 1):**

| Type | Required Fields | Description |
|------|----------------|-------------|
| `field_not_empty` | `field` | Evidence field exists and is non-empty |
| `field_equals` | `field`, `value` | Evidence field equals expected value |
| `command` | `command` | Shell command exits 0 |

**Gate semantics:**
- Gates are exit conditions: all gates on a state must pass before leaving (AND logic).
- Gates are evaluated when `koto transition` is called, between validation and commit.
- Command gates run via `sh -c "<command>"` from the project root. No `{{VARIABLE}}` interpolation in command strings (prevents injection). No timeout in Phase 1.
- Evidence is supplied via `koto transition <target> --evidence key=value` and accumulates in the state file across transitions.

**Engine extension:**
- Add `Evidence map[string]string` to `engine.State` (separate from `Variables`).
- Extend `Engine.Transition` signature: `Transition(target string, opts ...TransitionOption) error` with `WithEvidence(map[string]string)` option for backward compatibility.
- Bump `schema_version` to 2. `Load` accepts v1 (empty Evidence map) and v2.

### Interpolation

The controller builds the interpolation context:

```
context = merge(variable defaults, init-time --var values, evidence values)
```

Evidence wins over variables (higher precedence). Single-pass `{{KEY}}` replacement. Unresolved placeholders remain as-is.

Command gate strings are NOT interpolated. This is a security boundary -- verified by explicit tests.

### Template Search Path

When `--template` doesn't contain `/` or `.`:

1. `.koto/templates/<name>.koto.json` then `.koto/templates/<name>.md` (project-local)
2. `~/.config/koto/templates/<name>.koto.json` then `~/.config/koto/templates/<name>.md` (user-global, respects `$XDG_CONFIG_HOME`)

JSON is preferred over markdown when both exist (skip compilation). First match wins.

### Tooling Commands

```
koto template compile <input.md> [-o output.koto.json]
koto template decompile <input.koto.json> [-o output.md]
koto template validate <file>                # structural + semantic checks
koto template lint <file.md>                 # validate + optional LLM suggestions
koto template new <name>                     # scaffold a new template
```

`koto init --template <file>` accepts either format. If given `.md`, it compiles in memory (no persistent `.koto.json` needed unless the user wants one).

### LLM Linter Architecture

The linter is optional and separate from compilation:

```
Human writes template.md
        |
        v
[Deterministic validation]  ← schema checks, state references, type validation
        |
    pass / fail
       / \
      /   \
     v     v
  [OK]  [LLM suggests fixes]  ← "Did you mean ## assess instead of ## Assess?"
           |
           v
   Human reviews + applies fixes
           |
           v
   [Re-validate]
           |
           v
   [Compile to JSON]  ← deterministic, no LLM
```

The LLM never touches the compilation path. It's a development-time assistant that helps humans write valid templates. koto works without it.

## Implementation Approach

### Phase 1: Canonical JSON Format

Define the JSON schema and implement parsing in `pkg/template/`:
- `CanonicalTemplate` struct with JSON tags
- `ParseJSON(path string) (*Template, error)` -- reads and validates `.koto.json`
- Build `engine.Machine` from parsed canonical template
- JSON Schema file for editor support
- Unit tests for every validation rule

No external dependencies. Uses `encoding/json` from stdlib.

### Phase 2: Markdown Compiler

Implement the compiler as a separate package (`pkg/template/compile/` or `cmd/koto/`):
- Parse YAML frontmatter (go-yaml v3 dependency here, not in engine)
- Extract directive content from markdown body
- Produce `CanonicalTemplate` struct
- `koto template compile` CLI command
- Unit tests for compilation edge cases (heading collision, missing sections)

### Phase 3: Template Search Path

Add search path resolution to the CLI:
- `resolveTemplatePath` in `cmd/koto/`
- Git root detection for project-local search
- XDG config directory for user-global search
- Prefer `.koto.json` over `.md` when both exist

### Phase 4: Evidence Gates

Extend the engine for evidence:
- `Evidence map[string]string` in `engine.State`
- `Transition(target string, opts ...TransitionOption) error`
- Gate evaluation between validation and commit
- `gate_failed` error code
- Command gate execution (`sh -c`, project root CWD, no interpolation)

### Phase 5: Backward Compatibility

Support the legacy format during transition:
- Detect `---` frontmatter with flat `key: value` syntax (legacy)
- Convert to canonical format in memory
- Emit deprecation warning
- Remove after one release cycle (user base is currently zero)

### Phase 6: Validation and Linter

Implement `koto template validate` and `koto template lint`:
- Structural validation (parse-time checks)
- Semantic validation (reachability, cross-references)
- Optional LLM integration for lint suggestions

## Security Considerations

### Download Verification

Not applicable. Templates are local files. No downloads at runtime.

### Execution Isolation

**Command gates execute shell commands**: `sh -c "<command>"` with user permissions from the project root. This is deliberate (template authors define what to run). Commands are literal strings -- no variable interpolation prevents injection.

**No timeout in Phase 1**: A hanging command blocks the transition indefinitely. This must be addressed before unattended agent scenarios. Acceptable for interactive use.

Mitigation: command gates execute only during transitions, not during parse or validate. `koto template validate` does NOT execute commands. Review templates before use (same trust model as Makefiles).

### Supply Chain Risks

**Template files with command gates are executable**: A malicious template could run arbitrary commands. Mitigation: SHA-256 hash stored at init time, verified on every operation. Template modification after init is detected.

**go-yaml dependency**: Confined to the compiler/tooling layer. The engine reads JSON using stdlib only. A go-yaml vulnerability affects template authoring, not workflow execution.

**JSON canonical format**: Go's `encoding/json` is part of the standard library. No supply chain risk from the engine's parser.

### User Data Exposure

**Evidence in state files**: Evidence accumulated during execution is stored in the state file (JSON on disk). Same risk as the existing variables. State files in `wip/` are cleaned before merge.

**Command gate output**: Exit codes only. stdout/stderr not captured or stored.

### Mitigations

| Risk | Mitigation | Residual Risk |
|------|------------|---------------|
| Malicious command gates | SHA-256 hash verification; review before use | Untrusted template used without review |
| go-yaml vulnerability | Confined to compiler, not engine | Zero-day in compiler path |
| Evidence contains sensitive data | Cleaned before merge; same as variables | Exposure on feature branches |
| Template modification after init | Hash check on every operation | Hash collision (negligible) |
| Command injection via variables | No interpolation in command strings; explicit test | Future contributor bypasses this |

## Consequences

### Positive

- Heading collision eliminated: JSON has no headings. The engine never sees markdown ambiguity.
- Format debates resolved: JSON for machines (deterministic), YAML+markdown for humans (readable). Each format does what it's designed for.
- Zero engine dependencies: `encoding/json` is stdlib. go-yaml is confined to the compiler.
- Schema-based tooling: JSON Schema enables editor autocomplete, CI validation, and third-party tool support.
- LLM assistance without LLM dependency: The linter is optional. koto works without it. LLMs help humans, not the system.
- Clean extension point: new gate types, new fields, and schema evolution all work through JSON schema versioning.

### Negative

- Two formats to understand: template authors need to know that `.md` compiles to `.koto.json`. Conceptual overhead.
- Compilation step: an extra command or implicit step at `koto init`. Mitigated by transparent compilation when given `.md`.
- JSON directives are ugly: the canonical format embeds markdown as JSON strings (`\n` everywhere). Humans shouldn't need to read the JSON, but debugging may require it.
- go-yaml dependency in compiler: adds ~15K lines of dependency to the tooling layer. Acceptable for a well-maintained, widely-used library.

### Mitigations

- `koto init` accepts both formats transparently -- most users never see the JSON
- `koto template decompile` enables round-tripping if someone needs to recover the markdown
- JSON Schema provides structure that makes the canonical format navigable even with escaped markdown
- The compiler is a small, focused component (~200 lines) that's easy to understand and maintain
