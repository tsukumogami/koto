---
status: Proposed
problem: |
  koto has a working template parser but the format is undocumented and incomplete.
  Evidence gates have no syntax. The header parser silently ignores unknown keys.
  State section boundaries can collide with markdown headings in directive text.
  There's no template search path, no validation contract, and no way to declare
  variable types or requirements. Downstream designs (quick-task template, agent
  integration) can't build on an unspecified format.
decision: |
  Formalize the template format as TOML front matter plus markdown state sections.
  TOML replaces the current YAML-like header because koto already needs structured
  data (nested gate declarations, typed variables) that the hand-rolled parser can't
  handle, and Go has a zero-CGo TOML library (BurntSushi/toml) that's been stable
  for a decade. Evidence gates are declared per-state in the TOML header with three
  types: field_not_empty, field_equals, and command. Template search follows a
  three-tier path: explicit path, project-local .koto/templates/, user-global
  ~/.config/koto/templates/. State sections use ## headings with a parsing rule
  that only headings matching declared state names are treated as boundaries. An
  explicit initial_state field identifies the starting state (no reliance on TOML
  table ordering).
rationale: |
  TOML handles the nested structures evidence gates require (per-state gate
  declarations with type-specific fields) without external dependencies that
  would compromise koto's zero-dependency goal for the core engine. The heading
  collision problem is solved by declared-state matching rather than escape
  sequences, keeping templates readable. A three-tier search path gives users
  progressive complexity: start with explicit paths, graduate to project templates,
  optionally share global templates. Evidence gates use declarative types rather
  than embedded scripts because the common checks (field presence, value match,
  command exit code) cover the primary use cases without portability concerns.
---

# DESIGN: koto Template Format Specification

## Status

**Proposed**

## Context and Problem Statement

koto templates are markdown files that define workflow state machines. The engine (implemented in PR #2) parses these templates into `Machine` instances, extracts state directives, and uses them to guide AI agents through multi-step workflows. The current parser works but the format has several gaps that block downstream work.

**Undocumented format**: The template structure exists only in code comments and test fixtures. Template authors have no specification to work from, and the parser's behavior on edge cases is undefined.

**No evidence gates**: The engine design reserves space for evidence-gated transitions (requiring proof before advancing) but the template format has no syntax for declaring gates. This is the primary extension koto needs to move from "enforces ordering" to "enforces proof of work."

**Header format limitations**: The current parser handles flat `key: value` pairs and one level of indentation (`variables:` block). Evidence gate declarations need nested structures (per-state, per-transition gate definitions with type-specific fields). The hand-rolled YAML parser can't express this without becoming a full YAML parser.

**Heading collision**: The parser treats every `## heading` as a state boundary. If a directive tells an agent to "write a section with `## Analysis`" and `Analysis` happens to be a state name, the parser would split the directive at the wrong point.

**No search path**: Templates are specified by absolute path at `koto init` time. There's no way to reference a template by name (e.g., `koto init --template quick-task`) or maintain project-local or user-global template libraries.

**No validation contract**: The parser validates structural correctness (transition targets exist, delimiters present) but doesn't validate variable declarations, state name formats, or the bidirectional consistency between header declarations and body sections.

### Scope

**In scope:**
- Header format specification (TOML vs YAML, required/optional fields, evidence gate syntax)
- State section parsing rules (boundary identification, heading collision resolution)
- Variable and evidence interpolation contract
- Template search path and resolution order
- Bidirectional validation rules
- Validation timing (init vs operation vs explicit validate)
- At least one valid and two invalid template examples

**Out of scope:**
- Evidence gate evaluation logic (how the engine checks gates at transition time)
- Template registry, sharing, or community distribution
- Template versioning migration between schema versions
- Built-in template content (that's the quick-task template design, #313)

## Decision Drivers

- **Zero external dependencies for core engine**: The engine, controller, and discover packages use only the Go standard library. The template package can add a dependency if the benefit is clear, since it's a leaf package that nothing else imports (except the CLI).
- **Backward compatibility**: Existing templates must continue to work, or the migration path must be trivial.
- **Readability**: Templates are authored by humans and read by both humans and AI agents. The format should be natural to write and easy to understand.
- **Nested structure support**: Evidence gates require per-state, per-transition declarations with type-specific fields. The header format must handle 2-3 levels of nesting cleanly.
- **Declared-state-first parsing**: State sections should be identified by matching declared state names, not by arbitrary markdown heading patterns.
- **Progressive complexity**: Simple templates (no gates, no search path) should be minimal. Advanced features add syntax only when used.

## Implementation Context

### Current Template Format

The parser in `pkg/template/template.go` handles:

```
---
name: workflow-name
version: "1.0"
description: A workflow description
variables:
  KEY: default-value
---

## state-name

Directive text for the agent.

**Transitions**: [next-state-a, next-state-b]
```

Key behaviors: first `##` heading becomes the initial state. States without a `**Transitions**:` line are terminal. SHA-256 hash covers the full file. The header parser uses `strings.SplitN(line, ":", 2)` -- not a YAML library.

### Industry Patterns

Research into AI agent orchestration tools reveals consistent patterns:

**Format convergence**: Claude Code skills, Gemini CLI configuration, and GitHub Actions all use structured headers (YAML/TOML) with human-readable bodies. The separation of machine-parseable configuration from human/agent-readable content is the standard approach.

**Evidence patterns**: Three gate types cover the spectrum: command gates (shell exit code), field checks (declarative conditions on state data), and prompt gates (LLM evaluation). koto's Phase 1 should cover the first two; prompt gates require an LLM integration that's out of scope.

**Search paths**: Most tools follow a project-local > user-global > built-in precedence order. Claude Code uses `.claude/` in the project, `~/.claude/` globally. Go tools use `$XDG_CONFIG_HOME` or `~/.config/`.

**State storage**: Beads (`.beads/`) and TaskMaster (`.taskmaster/`) validate project-directory state with ephemeral lifecycle. koto's `wip/` pattern fits this model.

## Considered Options

### Decision 1: Header Format

The template header needs to express nested structures for evidence gates. A gate declaration looks something like: "for state X, before transitioning, require that field Y is not empty and command Z exits 0." This is inherently 2-3 levels deep. The current hand-rolled parser handles one level of nesting (the `variables:` block) and would need significant extension.

#### Chosen: TOML front matter

Replace the YAML-like `---` delimiters with TOML `+++` delimiters. Use BurntSushi/toml (BSD license, zero dependencies, stable since 2013) for parsing. TOML's native table and array-of-tables syntax handles the nested gate declarations naturally:

```toml
+++
name = "quick-task"
version = "1.0"
description = "A linear task workflow"
initial_state = "assess"

[variables]
TASK = {description = "What to build", required = true}

[states.assess.gates.task_defined]
type = "field_not_empty"
field = "TASK"

[states.implement.gates.tests_pass]
type = "command"
command = "go test ./..."
+++
```

This adds one dependency to `pkg/template/` (which is a leaf package -- nothing imports it except the CLI). The engine, controller, and discover packages remain dependency-free.

#### Alternatives Considered

**JSON front matter**: JSON is in Go's standard library (zero external dependencies) and handles arbitrary nesting. Rejected because JSON is significantly noisier than TOML for configuration: it requires quoting all keys, doesn't support comments, and lacks bare string values. A state machine definition in JSON would be harder to read and write than the TOML equivalent. The zero-dependency benefit is real but doesn't outweigh the readability cost for a format that humans author by hand.

**Full YAML parser (go-yaml/yaml)**: More familiar syntax, widely used. The upstream strategic design document used YAML-style syntax in its template examples, reflecting the hand-rolled parser that existed at the time. However, YAML's implicit typing creates surprises in template headers -- go-yaml v3 uses YAML 1.2 which improves on YAML 1.1's worst behaviors (bare `yes` becoming boolean), but the multiple-representation problem persists (flow vs block, single vs double quotes). TOML is less familiar than YAML to most developers, but koto's primary audience is Go developers who encounter TOML regularly in Go tooling. The dependency size comparison (go-yaml ~15K lines vs BurntSushi/toml ~3K lines) further favors TOML.

**Extend hand-rolled parser**: Add nested block handling to the current `parseHeader` function. Rejected because the parser would need to handle indentation-based nesting for gate declarations, essentially becoming a partial YAML parser without the benefit of a tested library. The maintenance cost grows with each new feature.

**Keep YAML-like with `---` delimiters, use go-yaml**: Parse the existing format with a real YAML library. Rejected because it doesn't solve the implicit typing problem and adds a larger dependency than BurntSushi/toml for a less suitable format.

### Decision 2: State Section Boundaries

The parser must identify where one state's directive ends and another begins. The current approach treats every `## heading` as a state boundary, which fails when directive text contains markdown headings.

#### Chosen: Declared-state matching

Only `## headings` whose text matches a state name declared in the TOML header are treated as state boundaries. All other `##` headings are regular markdown content within the current state's directive.

States must be declared in the TOML header. The header is the source of truth for which states exist; the body sections provide content for those states. This means the parser reads the header first, builds the set of declared state names, then scans the body looking only for headings that match.

```toml
+++
name = "example"
version = "1.0"
initial_state = "assess"

[states]
assess = {transitions = ["plan"]}
plan = {transitions = ["implement"]}
implement = {transitions = ["done"]}
done = {terminal = true}
+++

## assess

Analyze the problem. Write your findings under:

## Analysis

This heading is NOT a state boundary because "Analysis" is not in [states].

**Transitions**: [plan]
```

#### Alternatives Considered

**Escape syntax**: Use `\## heading` or `<!-- koto-escape -->` to mark headings that aren't state boundaries. Rejected because it requires template authors to know which headings collide and to maintain escape markers as directives evolve. It's error-prone and ugly.

**Different heading level**: Use `# heading` (H1) for state boundaries instead of `## heading` (H2). Rejected because H1 is conventionally used for the document title, and this would conflict with templates that want a readable document structure.

**Fenced sections**: Use code-fence-like markers (`:::state-name` / `:::`) instead of headings. Rejected because it makes templates less readable as standalone markdown documents. One of koto's strengths is that templates are valid, readable markdown.

### Decision 3: Template Search Path

When a user runs `koto init --template quick-task`, the CLI needs to find the template file. The current implementation requires an explicit file path. A search path enables named template references and library organization.

#### Chosen: Three-tier search with explicit path override

Template resolution follows this order:

1. **Explicit path**: If `--template` is a file path (contains `/` or `.`), use it directly.
2. **Project-local**: Look for `.koto/templates/<name>.md` relative to the git root (or CWD if not in a git repo).
3. **User-global**: Look for `~/.config/koto/templates/<name>.md` (respects `$XDG_CONFIG_HOME`).

No built-in templates are embedded via `go:embed` in the initial release. Built-in templates would create a versioning problem (template content tied to binary version) and mask whether the user has a local override. Templates are distributed as files, installed by copying.

#### Alternatives Considered

**Embedded built-ins via go:embed**: Ship default templates inside the binary. Rejected for the initial release because it couples template content to binary release cycles and makes template customization unclear (is the user running the built-in or a local override?). Can be added later if demand exists.

**Single search directory**: Only project-local, no global. Rejected because users working across multiple projects would need to copy templates into each project. A global directory enables shared templates.

**Registry with remote fetch**: `koto init --template github.com/user/templates/quick-task`. Rejected as over-engineering for the initial release. File copying covers the use case without network dependencies.

### Decision 4: Evidence Gate Declarations

Evidence gates block transitions until conditions are met. The engine design reserves the extension point (`MachineState` struct); this decision specifies how template authors declare gates.

#### Chosen: Per-state gate declarations in TOML header

Gates are declared under `[states.<name>.gates.<gate-name>]` in the TOML header. Each gate has a `type` field and type-specific parameters. Three gate types for Phase 1:

**field_not_empty**: Checks that a named field exists and is non-empty in the evidence map.

```toml
[states.plan.gates.task_defined]
type = "field_not_empty"
field = "TASK"
```

**field_equals**: Checks that a named field equals a specific value.

```toml
[states.finalize.gates.tests_passed]
type = "field_equals"
field = "CI_STATUS"
value = "passed"
```

**command**: Runs a shell command and checks the exit code.

```toml
[states.implement.gates.lint_clean]
type = "command"
command = "go vet ./..."
```

Gates on a state are evaluated when attempting to leave that state (before the transition commits). If any gate fails, the transition is rejected with a `gate_failed` error that includes which gate failed and why. In other words, gates are exit conditions: "you must satisfy these conditions before leaving this state."

Gate names are scoped to the state they're declared on. They appear in error messages and history entries for debuggability.

#### Alternatives Considered

**Inline gate syntax in markdown sections**: Declare gates within the state section body using a special syntax like `**Gate**: field_not_empty(TASK)`. Rejected because mixing machine-parseable gate declarations with human-readable directive text makes both harder to read. Separation of concerns: the header is for machine configuration, the body is for human/agent content.

**Transition-level gates instead of state-level**: Attach gates to specific transitions rather than states. For example, "only the assess->plan transition requires TASK to be defined, but assess->escalate doesn't." Rejected for Phase 1 because it adds significant complexity (gates indexed by `from,to` pairs) and the common case is "this state requires these conditions before you can leave it." Transition-level gates can be added later if needed.

### Decision 5: Variable and Evidence Interpolation

Templates use `{{KEY}}` placeholders in directive text. The question is how variables (set at init time) and evidence (accumulated during execution) interact in the interpolation context.

#### Chosen: Merged context with evidence-wins precedence

When the controller interpolates a directive, it builds the context by merging:
1. Template variable defaults (lowest priority)
2. Init-time variable values (from `--var` flags)
3. Evidence values accumulated during execution (highest priority)

If a key exists in both variables and evidence, the evidence value wins. This enables patterns like: define a default `TASK` variable at init, then let evidence from a later state refine it.

Unresolved placeholders remain as-is (not an error). This is the existing behavior and allows templates to include placeholders that are resolved at different points in the workflow.

#### Alternatives Considered

**Separate namespaces**: Use `{{var.TASK}}` for variables and `{{evidence.CI_STATUS}}` for evidence. This has a real benefit: disambiguation. If a template variable and an evidence field share the same key, the merged context silently picks the evidence value. A template author who defines a `STATUS` variable with a default and later accumulates evidence with the same key sees the default overridden without warning. However, this is the intended behavior -- evidence represents current ground truth and should take precedence over stale defaults. The disambiguation benefit doesn't justify the verbosity cost for every placeholder reference in every template. Naming conventions (variables in SCREAMING_CASE, evidence in lower_case) can reduce collision risk without syntax overhead.

**Go's `text/template` package**: The standard library's template engine handles `{{.Key}}` natively with zero dependencies, supports conditionals and loops, and is well-tested. Rejected because `text/template` allows arbitrary logic in templates (conditionals, range loops, function calls), which would introduce security concerns (template authors could invoke arbitrary Go functions) and template debugging complexity. koto's single-pass literal replacement is deliberately simpler: it does one thing (substitute values) with no evaluation, no control flow, and no side effects.

**Error on unresolved placeholders**: Fail if any `{{KEY}}` can't be resolved. Rejected because some placeholders are intentionally left for later states to fill (evidence not yet collected), and templates may include example syntax in directives that looks like placeholders.

### Decision 6: Validation Contract

Templates need validation at multiple points: when parsed, when a workflow is initialized, and on explicit `koto validate` invocation. The question is what gets checked and when.

#### Chosen: Parse-time structural validation plus explicit semantic validation

**At parse time** (every template load):
- TOML header is syntactically valid
- Required fields present (name, version)
- All states in `[states]` have corresponding `## heading` sections in the body
- All `## heading` sections in the body that match declared state names have content
- All transition targets reference declared states
- Gate declarations have valid types and required type-specific fields
- No duplicate state names

**At `koto validate` time** (explicit invocation):
- All parse-time checks
- Variable declarations are consistent (no variable referenced in a gate but not declared)
- Reachability analysis: all states are reachable from the initial state
- Terminal states have no transitions declared
- Warning (not error) for states with no outgoing transitions that aren't marked terminal

**Not validated** (by design):
- State name format (any non-empty string is valid)
- Variable naming conventions
- Directive content quality

#### Alternatives Considered

**Validate everything at parse time**: Run reachability analysis and cross-reference checks on every parse. Rejected because these checks add latency to every `koto next` and `koto transition` call, and parse-time is the hot path. Structural validity (the template won't crash the parser) matters on every load; semantic validity (the template is well-designed) is a development-time check.

**No explicit validation command**: Rely on parse-time checks only. Rejected because template authors need a way to check their work during development without running a full workflow. `koto validate` is the development-time quality gate.

## Decision Outcome

### Summary

The koto template format uses TOML front matter (delimited by `+++`) for machine-readable configuration and markdown body sections for state directives. The TOML header declares the complete state machine: state names, transitions, evidence gates, and variable definitions. The markdown body provides directive content for each state, matched by `## heading` text against the declared state names.

Evidence gates are declared per-state in the TOML header using three types: `field_not_empty` (a named field exists and is non-empty), `field_equals` (a named field matches a specific value), and `command` (a shell command exits 0). Gates are evaluated before a transition commits. Template variables and evidence values merge into a single interpolation context with evidence taking precedence over variables.

Template resolution follows a three-tier search path: explicit file path, project-local `.koto/templates/`, user-global `~/.config/koto/templates/`. The `pkg/template/` package adds a dependency on BurntSushi/toml for header parsing while the rest of the engine remains dependency-free. Validation splits between parse-time structural checks (every load) and explicit semantic checks (`koto validate`).

### Rationale

The decisions reinforce a separation between structure (TOML header) and content (markdown body). The header is the machine-parseable source of truth: it declares which states exist, what transitions are allowed, and what evidence is required. The body provides the human/agent-readable directives. This split means the parser reads the header first and uses it to interpret the body, resolving the heading collision problem without escape syntax.

TOML fits naturally because koto's configuration is inherently tabular: each state is a table with fields (transitions, gates). TOML tables map directly to this structure. The single-dependency cost is confined to the template package and buys correct parsing of nested structures, quoted strings, and typed values.

The three-tier search path gives users a progression: start with explicit paths (zero setup), move to project-local templates (team sharing), optionally add global templates (personal library). No built-in templates avoids coupling template content to binary versions.

### Trade-offs Accepted

- **One dependency in pkg/template/**: BurntSushi/toml is well-maintained, BSD-licensed, and ~3K lines. The engine, controller, and discover packages remain dependency-free. Acceptable because the template package is a leaf with no downstream importers.
- **Breaking change from YAML-like to TOML**: Existing templates need migration (change `---` to `+++`, convert YAML-like syntax to TOML). Acceptable because the koto user base is currently zero (no public release yet) and the migration is mechanical.
- **No prompt/LLM gates**: Only deterministic gate types (field checks, commands) in Phase 1. Acceptable because deterministic gates cover the primary use cases (CI status, file existence, field values) and LLM evaluation requires a separate design for model selection, cost, and reliability.
- **No built-in templates**: Users must obtain template files separately. Acceptable for the initial release; `go:embed` can be added later if demand exists.

## Solution Architecture

### Template File Structure

A complete template file has two parts:

```
+++
<TOML configuration>
+++

<Markdown state sections>
```

### TOML Header Schema

```toml
+++
# Required fields
name = "workflow-name"        # kebab-case, used in state file naming
version = "1.0"               # semver string (metadata only; hash is the integrity mechanism)
initial_state = "assess"      # must match a key in [states]

# Optional fields
description = "What this workflow does"

# Variable declarations
[variables]
TASK = {description = "What to build", required = true}
REVIEWER = {description = "Who reviews", required = false, default = "team"}

# State declarations
[states.assess]
transitions = ["plan", "escalate"]

[states.assess.gates.task_defined]
type = "field_not_empty"
field = "TASK"

[states.plan]
transitions = ["implement"]

[states.implement]
transitions = ["done"]

[states.implement.gates.tests_pass]
type = "command"
command = "go test ./..."

[states.implement.gates.lint_clean]
type = "command"
command = "go vet ./..."

[states.escalate]
terminal = true

[states.done]
terminal = true
+++
```

**Required fields:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Template identifier, used in state file naming |
| `version` | string | Template version, metadata only (the SHA-256 hash is the integrity mechanism) |
| `initial_state` | string | Name of the starting state, must match a key in `[states]` |

**Optional fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `description` | string | `""` | Human-readable description |

**Variable declarations** (`[variables]`):

Each variable is a table with optional fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `description` | string | `""` | What this variable is for |
| `required` | bool | `false` | Must be provided at init time |
| `default` | string | `""` | Default value if not provided |

Simple variables with no metadata can use shorthand: `TASK = "default-value"` (equivalent to `TASK = {default = "default-value", required = false}`). An empty string shorthand (`TASK = ""`) is equivalent to `{default = "", required = false}`. A variable with `required = true` and no `default` field has an effective default of `""` but must be explicitly provided at init time.

**State declarations** (`[states.<name>]`):

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `transitions` | string array | `[]` | Allowed target states |
| `terminal` | bool | `false` | Whether this is an end state |

A state with no `transitions` and `terminal = false` produces a validation warning (unreachable dead end).

**Gate declarations** (`[states.<name>.gates.<gate-name>]`):

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | string | yes | Gate type: `field_not_empty`, `field_equals`, `command` |
| `field` | string | for field types | Evidence field name to check |
| `value` | string | for `field_equals` | Expected field value |
| `command` | string | for `command` | Shell command to run (no interpolation, literal string) |

**Gate evaluation semantics**: All gates on a state are evaluated with AND logic -- every gate must pass for the transition to proceed. OR composition (any gate passes) is not supported in Phase 1. If needed, the `command` gate type can implement OR logic within a single shell command.

**Command gate execution**: Commands run via `sh -c "<command>"` (not a login shell) with the working directory set to the project root (git root or CWD, matching the directory where `koto init` was run -- not the state file directory, since commands like `go test ./...` expect to run from the project root). Environment variables are inherited from the koto process. There is no timeout in Phase 1; a hanging command blocks the transition indefinitely. This is acceptable for interactive use but must be addressed before any unsupervised agent scenario -- a timeout mechanism is a pre-requisite for unattended operation. Only the exit code is checked (0 = pass, non-zero = fail). stdout and stderr are not captured or stored.

**Command gate interpolation**: Template variable placeholders (`{{KEY}}`) are NOT expanded in command gate strings. Command strings are treated as literals. This prevents injection attacks where a variable value like `foo; rm -rf /` could be spliced into a shell command. If a command needs access to a variable value, it can read it from the koto state file via `koto query`.

### Go Struct Definitions

The TOML header unmarshals into these Go structs:

```go
// TemplateConfig is the top-level TOML structure.
type TemplateConfig struct {
    Name         string                       `toml:"name"`
    Version      string                       `toml:"version"`
    InitialState string                       `toml:"initial_state"`
    Description  string                       `toml:"description,omitempty"`
    Variables    map[string]VariableDecl       `toml:"variables,omitempty"`
    States       map[string]StateDecl          `toml:"states"`
}

// VariableDecl defines a template variable.
// Implements toml.Unmarshaler to support shorthand: TASK = "value"
// unmarshals as VariableDecl{Default: "value"}.
// The custom unmarshaler type-switches: if the TOML value is a string,
// it creates {Default: s}; if it's a table, it does normal unmarshal.
type VariableDecl struct {
    Description string `toml:"description,omitempty"`
    Required    bool   `toml:"required,omitempty"`
    Default     string `toml:"default,omitempty"`
}

// StateDecl defines a state in the state machine.
type StateDecl struct {
    Transitions []string             `toml:"transitions,omitempty"`
    Terminal    bool                  `toml:"terminal,omitempty"`
    Gates       map[string]GateDecl   `toml:"gates,omitempty"`
}

// GateDecl defines an evidence gate.
type GateDecl struct {
    Type    string `toml:"type"`              // "field_not_empty", "field_equals", "command"
    Field   string `toml:"field,omitempty"`   // for field_not_empty, field_equals
    Value   string `toml:"value,omitempty"`   // for field_equals
    Command string `toml:"command,omitempty"` // for command
}
```

### Markdown Body Sections

Each declared state must have a corresponding `## heading` in the markdown body. The heading text must exactly match the state name from the TOML header.

```markdown
## assess

Evaluate the problem and determine the approach.

Consider these factors:
- Complexity of the change
- Files affected
- Test coverage needed

## plan

Create an implementation plan for {{TASK}}.

### Recommended Structure

Use this heading structure for the plan:

## Overview

This `## Overview` heading is NOT a state boundary because "Overview"
is not in [states]. It's part of the plan directive.
```

**State name rules**: State names are case-sensitive. Matching between `## heading` text and declared state names uses exact string comparison after trimming leading/trailing whitespace from the heading text. State names should use kebab-case (`assess-task`, not `Assess Task`) and must not contain characters that would be ambiguous in TOML keys or markdown headings. Recommended pattern: `[a-z][a-z0-9_-]*`.

**Parsing rules:**

1. Read the TOML header first. Build the set of declared state names. The initial state is the value of `initial_state` (must match a key in `[states]`).
2. Scan the markdown body line by line.
3. A line starting with `## ` followed by text that, after whitespace trimming, exactly matches a declared state name starts a new state section.
4. All other lines (including `##` headings that don't match declared states) are content within the current section.
5. A `**Transitions**:` line within a recognized state section is consumed by the parser and excluded from the section content. A `**Transitions**:` line that appears before the first recognized state heading, or within body content that's part of a non-state heading, is left as-is (treated as regular content). The markdown transitions line is optional -- transitions declared in the TOML header take precedence.
6. Duplicate state headings: if the body contains `## assess` twice and `assess` is a declared state, the second occurrence reopens the section. Content from both occurrences is concatenated. (This is unlikely in practice but the behavior should be deterministic.)

**Known limitation**: The parser does line-by-line string matching and does not track fenced code blocks. A `## state-name` line inside a fenced code block (``` or ~~~) will be incorrectly treated as a state boundary. Template authors should avoid using declared state names as headings inside code blocks. This limitation will be addressed in a future release.

**Transition declaration precedence**: If a state has transitions in both the TOML header and a `**Transitions**:` line in the body, the TOML header wins. If both are present and disagree (different lists), a parser warning is emitted noting the discrepancy and stating which value is used. The `**Transitions**:` line is supported for backward compatibility and for templates that want inline readability.

### Template Search Path

When `--template` value doesn't contain `/` or `.`:

1. `.koto/templates/<name>.md` (relative to git root, or CWD)
2. `~/.config/koto/templates/<name>.md` (or `$XDG_CONFIG_HOME/koto/templates/<name>.md`)

First match wins. If no match, error with the paths searched.

When `--template` contains `/` or `.`, treat it as a file path (absolute or relative).

### Interpolation Contract

The controller builds the interpolation context as a `map[string]string`:

```
context = merge(template.Variables defaults, init-time --var values, evidence values)
```

Later entries override earlier ones. The `Interpolate` function does single-pass replacement of `{{KEY}}` placeholders. Unresolved placeholders remain as-is.

Evidence values are set via `koto transition <target> --evidence key=value`. They accumulate in the state file across transitions and are available in all subsequent directives. This requires extending the `engine.State` struct with an `Evidence map[string]string` field (separate from `Variables`). Variables are immutable after init; evidence accumulates during execution. The state file schema version increments to accommodate this field.

### Validation Rules

**Parse-time (structural):**

| Check | Error |
|-------|-------|
| TOML syntax invalid | TOML parse error with line number |
| Missing `name` field | `"template missing required field: name"` |
| Missing `version` field | `"template missing required field: version"` |
| Missing `initial_state` field | `"template missing required field: initial_state"` |
| `initial_state` not in `[states]` | `"initial_state %q is not a declared state"` |
| State in header has no body section | `"state %q declared in header but has no section in body"` |
| Body section matches no declared state | (ignored -- it's regular markdown content) |
| Transition target not in declared states | `"state %q references undefined transition target %q"` |
| Gate has unknown type | `"state %q gate %q: unknown type %q"` |
| Gate missing required field | `"state %q gate %q: missing required field %q"` |
| Empty transitions list with terminal = false | warning, not error |

**Explicit validation (`koto validate --template`):**

| Check | Severity |
|-------|----------|
| All parse-time checks | error |
| All states reachable from initial state | warning |
| Required variables have no default and must be provided at init | info |
| Gate references field not in variables | warning |

### Example: Valid Template

```toml
+++
name = "quick-task"
version = "1.0"
description = "A simple linear task workflow"
initial_state = "assess"

[variables]
TASK = {description = "What to build", required = true}

[states.assess]
transitions = ["plan"]

[states.plan]
transitions = ["implement"]

[states.implement]
transitions = ["done"]

[states.implement.gates.tests_pass]
type = "command"
command = "go test ./..."

[states.done]
terminal = true
+++

## assess

Analyze the task: {{TASK}}.

Determine the scope, identify affected files, and assess complexity.

## plan

Create an implementation plan for {{TASK}}.

Break the work into steps. Identify tests to write.

## implement

Execute the plan. Write code and tests.

## done

Work is complete. The task has been implemented and tested.
```

### Example: Invalid Template (missing state section)

```toml
+++
name = "broken"
version = "1.0"
initial_state = "start"

[states.start]
transitions = ["middle"]

[states.middle]
transitions = ["end"]

[states.end]
terminal = true
+++

## start

Begin the work.

## end

Done.
```

**Error**: `state "middle" declared in header but has no section in body`

### Example: Invalid Template (undefined transition target)

```toml
+++
name = "broken"
version = "1.0"
initial_state = "start"

[states.start]
transitions = ["nonexistent"]
+++

## start

Begin the work.
```

**Error**: `state "start" references undefined transition target "nonexistent"`

## Implementation Approach

### Phase 1: TOML Header Parser

Replace `parseHeader` with TOML parsing using BurntSushi/toml:
- Define Go structs matching the TOML schema
- Parse `+++` delimited front matter
- Build `engine.Machine` from parsed states
- Extract variable declarations and gate definitions
- Update `splitFrontMatter` to handle both `---` (legacy) and `+++` (TOML) delimiters. The `---` support will be removed in the next release after the format change ships. Since koto has no public release yet, the transition period is one release cycle at most.

### Phase 2: Declared-State Section Parsing

Update `parseSections` to use the declared state set:
- Read state names from parsed TOML header
- Only treat `## heading` as boundary if heading matches declared state
- Handle `**Transitions**:` as optional (TOML transitions take precedence)

### Phase 3: Template Search Path

Add search path resolution to the CLI layer:
- `resolveTemplatePath` function in `cmd/koto/main.go`
- Git root detection for project-local search
- XDG config directory for user-global search
- No changes to `pkg/template/` (it always receives a resolved path)

### Phase 4: Evidence Gate Types

Add gate types to the template struct and extend the engine API:
- Gate declaration structs in `pkg/template/`
- Validation of gate declarations at parse time
- Gate evaluation integration in `pkg/engine/` (extend `MachineState`)
- `gate_failed` error code in `TransitionError`
- Extend `Engine.Transition` to accept evidence: `Transition(target string, opts ...TransitionOption) error` using functional options (`WithEvidence(map[string]string)`). This is backward-compatible -- existing callers pass no options.
- Add `Evidence map[string]string` field to `engine.State`. Bump `schema_version` to 2. `Load` accepts both v1 (empty Evidence map) and v2. `Init` always writes v2.
- Add explicit test: a command gate containing `{{TASK}}` must NOT expand (verifies the no-interpolation security boundary).

**Note**: Phase 1 parses gate declarations from the TOML header but does not carry them to `engine.MachineState`. The parsed `GateDecl` data is retained in the `Template` struct for Phase 4 to consume. This is intentional -- it allows templates with gates to be parsed and validated structurally even before the engine supports gate evaluation.

### Phase 5: Validation Command

Enhance `koto validate` with a `--template` flag for template-only validation (no state file needed). The current `koto validate` checks template hash integrity against a running workflow's state file -- a different mode. With `--template`, validation runs the semantic checks below against a template file directly:
- Reachability analysis (BFS from initial state)
- Cross-reference checks (gates referencing variables)
- Rich error output with line numbers where possible

## Security Considerations

### Download Verification

Not applicable. The template format design doesn't involve downloading anything. Templates are local files read from disk.

### Execution Isolation

**Command gates execute shell commands**: The `command` gate type runs arbitrary commands via the user's shell. This is a deliberate feature (the template author defines what commands to run) but carries the same risks as any shell execution:
- Commands run with the user's permissions
- Templates from untrusted sources could run malicious commands
- Command output is not sandboxed

Mitigation: command gates only execute when a transition is attempted, not when a template is parsed or validated. The `koto validate` command does NOT execute command gates -- it only checks that the declaration is syntactically valid. Users should review template files before running `koto init`, the same way they'd review a Makefile or shell script.

### Supply Chain Risks

**Template files as code**: Templates with command gates are effectively executable. A modified template in a shared repository could introduce malicious commands. Mitigation: koto's existing template hash verification detects any modification after `koto init`. The SHA-256 hash covers the entire file including gate declarations.

**TOML parser dependency**: Adding BurntSushi/toml introduces a supply chain dependency. Mitigation: the library is BSD-licensed, has been stable since 2013 with minimal churn, has 4.5K+ stars, and is maintained by a well-known Go community member. The dependency is confined to `pkg/template/`.

### User Data Exposure

**Evidence values in state files**: Evidence accumulated during execution is stored in the state file. If evidence includes sensitive data (test output, API responses), it persists on disk and in git history if the state file is committed. Mitigation: this is the existing behavior for variables; evidence follows the same pattern. State files in `wip/` are cleaned before merge.

**Command gate output**: Command gates check exit codes only; stdout/stderr is not captured or stored. No command output leaks into the state file.

### Mitigations

| Risk | Mitigation | Residual Risk |
|------|------------|---------------|
| Malicious command gates in templates | Template hash verification after init; user reviews templates before use | User runs untrusted template without review |
| TOML parser vulnerability | Well-maintained, widely-used library; confined to template package | Zero-day in parser |
| Sensitive data in evidence | State files cleaned before merge; same pattern as variables | Exposure on feature branches |
| Template modification after init | SHA-256 hash on every operation; no bypass flag | Hash collision (negligible) |

## Consequences

### Positive

- Templates become self-documenting: the TOML header is the complete state machine definition, readable without running the parser
- Evidence gates enable the core value proposition: enforcing proof of work, not just ordering
- The heading collision problem is solved cleanly without escape syntax or format changes to directive content
- Template search path enables project-local template libraries without requiring explicit paths
- The `pkg/template/` dependency boundary means the TOML parser doesn't affect the engine's zero-dependency guarantee

### Negative

- One external dependency (BurntSushi/toml) in `pkg/template/`
- Breaking change from `---` to `+++` delimiters (no existing users, so the cost is effectively zero)
- Command gates introduce shell execution, expanding the security surface
- Duplicate transition declaration (TOML header and `**Transitions**:` line) could confuse template authors

### Mitigations

- The TOML dependency is well-maintained, BSD-licensed, and confined to a single package
- A transition period supporting both `---` and `+++` delimiters eases migration
- Command gate security follows the same trust model as Makefiles and CI scripts -- review before use
- Parser warnings on redundant transition declarations guide authors toward the canonical form
