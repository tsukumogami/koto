---
status: Proposed
problem: |
  koto workflows can't express that a step has specific processing needs.
  A security audit step and a file-editing step look identical to koto --
  both are just states with directives. There's no way for a template author
  to say "this step needs deep reasoning" and have koto route it to the
  right tool, because koto has no state metadata beyond transitions, gates,
  and terminal flags. There's also no config system to control routing.
decision: |
  Add an optional tags field (string array) to state declarations and a
  config system that maps tags to delegation targets. Tags use kebab-case
  and are pattern-validated, not enum-restricted -- the initial vocabulary
  (deep-reasoning, large-context, specialized-tooling) is documented, not
  enforced by schema. format_version stays at 1 since tags are additive and
  Go's json.Unmarshal ignores unknown fields. A new pkg/config package
  loads YAML from user and project levels with separated targets and rules.
  The controller resolves tags against config at Next() time and includes
  DelegationInfo in the Directive output. A koto delegate run subcommand
  handles CLI invocation with a defined interface contract: raw prompt via
  stdin, raw text response via stdout, delegate runs read-write in the
  working directory.
rationale: |
  Keeping format_version at 1 preserves backwards compatibility -- old koto
  reads new templates without errors, just ignores the tags. Free-form tags
  with pattern validation give template authors flexibility while preventing
  garbage values. Putting tags in the template layer (not engine.MachineState)
  respects the separation where the engine handles state machine semantics
  and the controller handles agent-facing output. The config system is
  built as general infrastructure because koto will need config for other
  features beyond delegation.
---

# DESIGN: Cross-Agent Delegation

## Status

**Proposed**

## Context and Problem Statement

koto is a workflow engine for coding agents. It defines state machines with states, directives, transitions, and gates. An agent runs `koto next` to get the current directive, does the work, and calls `koto transition` to advance. Every step runs through whichever agent is orchestrating the session.

But workflow steps have different processing characteristics. A security audit needs extended reasoning over large codebases. A refactoring step needs tool calling to read and edit files. Architecture analysis needs a model that can hold a million tokens of context. Today, koto can't express these differences. Every state is just a directive with transitions and gates.

Coding agent CLIs now support headless invocation (`claude -p`, `gemini -p`), and developers often have access to multiple agent ecosystems. koto should let templates describe what a step needs and route it to the right tool based on the user's environment.

This design covers the full delegation feature: state tags in the template format, a config system for routing rules, delegation resolution in the controller, a `koto delegate run` subcommand for CLI invocation, and agent skill updates.

### Scope

**In scope:**
- `tags` field on state declarations (source YAML and compiled JSON)
- Compiled template JSON schema update
- Tag naming conventions and initial vocabulary
- `pkg/config` package (user-level and project-level YAML config)
- Delegation targets and routing rules in config
- `DelegationInfo` in `controller.Directive` output
- `koto delegate run` subcommand
- Delegate interface contract (input/output format, access model)
- Delegate CLI availability checking and invocation
- Agent skill documentation (SKILL.md updates)

**Out of scope:**
- Changes to the engine's state machine semantics (transitions, gates, evidence)
- New gate types for delegation
- Multi-delegate routing (one tag maps to one target)
- Delegate response persistence (the orchestrating agent decides what to do with the response)
- Multi-turn delegation (delegate asks clarifying questions back to the orchestrator)
- Streaming delegate responses
- Sandboxing or restricting delegate filesystem access

Delegation is a synchronous single exchange: one prompt in, one response out. The orchestrating agent crafts a self-contained prompt, koto pipes it to the delegate CLI, and the delegate returns a complete response via stdout. The delegate runs in the working directory with full user permissions -- it can read and write files like any coding agent CLI. This boundary is intentional: it keeps the delegation interface simple and matches how headless coding agent CLIs already work.

**Single-exchange scope guide:** This model works well for focused analysis tasks with bounded context (review a function, analyze a package). It works adequately for broad analysis where relevant context fits in the delegate's window. It works poorly for investigative tasks requiring iteration (tracing bugs across a codebase). Template authors should scope tagged states accordingly.

## Decision Drivers

- **Backwards compatibility.** Old koto must read new templates with tags without error. Go's `json.Unmarshal` ignores unknown fields, so additive changes work. Bumping `format_version` would break old readers.
- **Schema strictness.** The compiled JSON schema uses `additionalProperties: false` on `state_decl`. Any new field must be explicitly added to the schema. This is a feature, not a problem -- it keeps the schema as the source of truth.
- **Tag expressiveness.** Tags should carry enough information for meaningful routing (not just "delegable: yes/no") without coupling templates to specific CLIs.
- **Template portability.** The same template should work in environments with and without delegation. Tags are inert metadata when no config rules match.
- **Config doesn't exist yet.** koto has zero config infrastructure. This design introduces it. The config system should be general-purpose, not delegation-specific.
- **Engine stays focused.** The engine handles state machine semantics (transitions, gates, terminal states). Tags are metadata for the controller and external tools, not for the engine.
- **Delegate invocation is deterministic.** Checking CLI availability, invoking a subprocess, capturing stdout, handling timeouts -- these don't require AI judgment. koto can own them.

## Considered Options

### Decision 1: Schema Versioning Strategy

The compiled template format uses `format_version: 1`. Adding a `tags` field to `state_decl` is a schema change. The question is whether this requires bumping to `format_version: 2`.

This matters because `ParseJSON()` rejects templates where `format_version != 1`. A version bump means old koto can't read new templates, forcing users to upgrade before they can use any template that happens to have tags -- even if they don't use delegation.

#### Chosen: Keep format_version at 1 (additive change)

Tags are optional and `omitempty`. Go's `json.Unmarshal` silently ignores unknown fields (koto doesn't use `DisallowUnknownFields`). This means:

- Old koto reading a new template with tags: `json.Unmarshal` succeeds, tags field is zero-valued (nil slice), everything works. The old koto just doesn't see the tags.
- New koto reading an old template without tags: tags field is nil, no delegation, everything works.

The JSON schema file (`compiled-template.schema.json`) gets updated to allow the `tags` property on `state_decl`. External validators using the updated schema accept tags; validators using the old schema reject them. This is expected -- schema files are versioned alongside the code.

`ParseJSON()` gains optional validation for tags (pattern check on tag strings) but doesn't require them.

#### Alternatives Considered

**Bump to format_version 2.** New koto accepts both v1 and v2. Old koto rejects v2 templates. Rejected because it forces upgrades for a purely additive field that old code can safely ignore. Version bumps should be reserved for changes that actually break the old reader's understanding of the template.

**Version range (accept 1-2).** Change `ParseJSON()` from `!= 1` to a range check. Rejected as unnecessary complexity -- if old code can already read the new format (because `json.Unmarshal` ignores unknowns), there's no reason to signal a version change.

### Decision 2: Tag Format and Vocabulary

Tags are strings on state declarations. The question is how much structure to impose: a strict enum, a naming pattern, or complete free-form.

This matters because tags are a shared contract between template authors and config authors. A template uses `tags: [deep-reasoning]` and the config maps `deep-reasoning` to a target. If template authors use inconsistent naming (`deep-reasoning` vs `reasoning-heavy` vs `deepReasoning`), the contract breaks silently.

#### Chosen: Pattern-validated free-form with documented vocabulary

Tags must match the pattern `^[a-z][a-z0-9-]*$` (kebab-case, starting with a lowercase letter). This prevents garbage values while allowing any meaningful label.

koto documents an initial vocabulary of recommended tags:

| Tag | Meaning | Use When |
|-----|---------|----------|
| `deep-reasoning` | Step benefits from extended chain-of-thought reasoning | Security audits, architecture analysis, complex debugging |
| `large-context` | Step needs a large context window (100K+ tokens) | Codebase-wide analysis, cross-file refactoring |
| `specialized-tooling` | Step benefits from domain-specific tools or capabilities | Static analysis, dependency scanning, specialized linting |

Tags describe processing capabilities, not domains. A security audit needs `deep-reasoning` (the capability), not a `security` tag (the domain). The tag tells the config system what kind of processing the step needs; the directive text tells the delegate what domain to focus on.

These are recommendations, not requirements. Template authors can use custom tags for project-specific needs. The vocabulary is documented in koto's user guide, not enforced by schema or code.

The JSON schema validates the pattern:

```json
"tags": {
  "type": "array",
  "items": {
    "type": "string",
    "minLength": 1,
    "pattern": "^[a-z][a-z0-9-]*$"
  },
  "uniqueItems": true
}
```

`ParseJSON()` validates tag strings match the pattern and rejects duplicates.

#### Alternatives Considered

**Enum in JSON schema.** Tags restricted to a predefined list: `"enum": ["deep-reasoning", "large-context", "security"]`. Rejected because adding a new tag requires a koto release with an updated schema. Template authors can't use custom tags for project-specific needs.

**Prefix convention.** Tags starting with `koto-` are reserved for official vocabulary; user tags are unrestricted. This gives namespace separation but adds complexity. Rejected for v1 -- if naming collisions become a problem, prefixes can be added later without breaking existing tags.

**Structured tags (key-value pairs).** Instead of flat strings, tags are `map[string]string`: `tags: {capability: deep-reasoning, context-size: large}`. This separates the dimension (capability, context-size) from the value, enabling more expressive config matching (a rule could match `capability=*` rather than listing individual tags). Rejected because it adds schema complexity for minimal v1 benefit. Most states will have zero or one tag. If structured tags become valuable, flat tags can evolve to key-value pairs later -- a flat tag `deep-reasoning` maps naturally to `{capability: deep-reasoning}`.

**Completely free-form (no pattern).** Any string is valid. Rejected because it allows `"Deep Reasoning"`, `"DEEP-REASONING"`, `"deep_reasoning"` for the same concept. The kebab-case pattern prevents accidental variation.

### Decision 3: Where Tags Live in the Pipeline

Tags need to flow from the source template through compilation to the controller output. The question is whether tags propagate through `engine.MachineState` or stay in the template layer.

The engine evaluates transitions, gates, and terminal states. It doesn't evaluate tags -- tags are metadata for the controller's delegation logic. Putting tags on `MachineState` gives the engine knowledge it doesn't use.

#### Chosen: Template layer only

Tags flow through:

```
sourceStateDecl.Tags → StateDecl.Tags → template.Template.Tags → controller.Directive.Tags
```

They skip `engine.MachineState`. The controller reads tags from `template.Template.Tags[stateName]` the same way it reads directive text from `template.Template.Sections[stateName]`.

This means `template.Template` gets a new field:

```go
type Template struct {
    Name        string
    Version     string
    Description string
    Machine     *engine.Machine
    Sections    map[string]string
    Tags        map[string][]string    // state name -> tags
    Variables   map[string]string
    Hash        string
    Path        string
}
```

And `ToTemplate()` populates it:

```go
tags := make(map[string][]string)
for name, sd := range ct.States {
    if len(sd.Tags) > 0 {
        t := make([]string, len(sd.Tags))
        copy(t, sd.Tags)
        tags[name] = t
    }
}
```

#### Alternatives Considered

**Tags on engine.MachineState.** `MachineState` gets a `Tags []string` field, populated by `BuildMachine()`. The controller reads tags from the machine like it reads transitions and terminal flags. Rejected because it puts non-functional metadata in the engine layer. `MachineState` carries information the engine acts on (transitions, gates, terminal). Tags are acted on by the controller, not the engine. Mixing concerns makes the engine harder to reason about.

**Tags only in controller (not in template.Template).** The controller reads tags directly from `CompiledTemplate` instead of `template.Template`. Rejected because the controller already depends on `template.Template`, not on `CompiledTemplate`. Adding a second dependency violates the existing data flow where `CompiledTemplate` is consumed once (to produce `Template`) and then discarded.

### Decision 4: Config System Architecture

koto has no config infrastructure. The delegation feature needs config for routing rules, but the config system should be general-purpose so future features don't each invent their own.

#### Chosen: pkg/config with YAML, two-level precedence, separated targets and rules

A new `pkg/config` package loads YAML from two locations:

1. **User config:** `~/.koto/config.yaml` (applies to all projects). Config resolution uses `KOTO_HOME` if set (same as the cache package), falling back to `~/.koto/`.
2. **Project config:** `.koto/config.yaml` in the working directory (project-specific overrides)

The delegation config separates target definitions (what binary to run) from routing rules (which tags map where). This eliminates command duplication when multiple rules route to the same target, and makes the security boundary self-documenting: targets define binaries, rules define routing.

```go
// pkg/config/config.go
type Config struct {
    Delegation *DelegationConfig `yaml:"delegation,omitempty"`
}

type DelegationConfig struct {
    AllowProjectConfig bool                      `yaml:"allow_project_config"`
    Timeout            int                       `yaml:"timeout,omitempty"` // seconds
    Targets            map[string]DelegateTarget `yaml:"targets"`
    Rules              []DelegationRule           `yaml:"rules"`
}

type DelegateTarget struct {
    Command []string `yaml:"command"` // e.g., ["gemini", "-p"]
}

type DelegationRule struct {
    Tag    string `yaml:"tag"`
    Target string `yaml:"target"`
}
```

Loading precedence:
1. Load user config (if it exists; if present but malformed YAML, return an error -- don't swallow parse failures)
2. Load project config (if it exists; same error handling)
3. Merge: project delegation rules append after user rules for tags not already covered by user rules; project targets are ignored
4. If neither exists, `Config{}` is returned (zero value, no delegation)

The CLI integrates config loading in its startup path. The controller receives the resolved `DelegationConfig` (or nil). Unknown YAML keys produce a warning to stderr (catches typos like `delgation:`).

**Project config trust:** Project-level config can override delegation routing. A cloned repo could ship `.koto/config.yaml` with unexpected rules. Two layers of protection:

1. **Opt-in gate:** Project-level delegation rules only take effect when the user's config includes `allow_project_config: true`. Without this, koto ignores project-level delegation rules entirely.

2. **No target definitions from project config.** Project config can only add routing rules (`tag` -> `target` mapping). Target definitions (what binary to run) come exclusively from user config. This prevents a cloned repo from specifying an arbitrary binary for koto to execute. Project config also cannot override `timeout`.

```yaml
# ~/.koto/config.yaml
delegation:
  allow_project_config: true   # default: false
  timeout: 300                 # seconds
  targets:
    gemini:
      command: ["gemini", "-p"]
    claude:
      command: ["claude", "-p", "--model", "opus"]
  rules:
    - tag: deep-reasoning
      target: gemini
```

```yaml
# .koto/config.yaml (project level)
delegation:
  rules:
    - tag: specialized-tooling
      target: gemini    # maps to user-defined target "gemini"
    # targets section ignored here -- user config controls binaries
    # timeout ignored here -- user config controls timeout
```

When merging, project rules that reference a target not defined in user config are dropped with a warning to stderr. Project rules for tags already covered by user rules are also dropped (user rules take priority explicitly, not via list ordering).

#### Alternatives Considered

**Flags only (no config file).** Delegation rules passed as CLI flags: `--delegate deep-reasoning=gemini`. Rejected because delegation rules are persistent preferences, not per-invocation choices. CLI flags would need to be passed on every `koto next` call.

**Environment variables.** `KOTO_DELEGATE_deep_reasoning=gemini`. Rejected because rules are structured data (ordered list with tag-to-target mapping) that doesn't fit environment variable ergonomics. Also makes it hard to disable delegation entirely.

**JSON config instead of YAML.** koto's compiled templates are JSON, so config could be JSON too for consistency. Rejected because YAML is the standard format for user-facing config files (Docker Compose, GitHub Actions, k8s). koto's source templates already use YAML frontmatter.

### Decision 5: Delegation Resolution in Controller

When `koto next` is called, the controller needs to check whether the current state has tags that match delegation rules. The question is how to integrate this into the existing `Next()` function and what the output looks like.

#### Chosen: DelegationInfo struct on Directive

The controller checks tags at `Next()` time:

1. Get current state
2. Look up tags from `template.Template.Tags[current]`
3. If tags exist and config has delegation rules, match tags against rules (first match wins)
4. If a match is found, check delegate CLI availability
5. Include `DelegationInfo` in the `Directive` response

```go
type DelegationInfo struct {
    Target     string `json:"target"`
    MatchedTag string `json:"matched_tag"`
    Available  bool   `json:"available"`
    Fallback   bool   `json:"fallback,omitempty"`
    Reason     string `json:"reason,omitempty"`
}

type Directive struct {
    Action     string          `json:"action"`
    State      string          `json:"state"`
    Directive  string          `json:"directive,omitempty"`
    Message    string          `json:"message,omitempty"`
    Tags       []string        `json:"tags,omitempty"`
    Delegation *DelegationInfo `json:"delegation,omitempty"`
}
```

`Directive.Action` stays `"execute"` during delegation. The `Delegation` field tells the agent *how* to execute: produce a prompt for the delegate rather than handling the directive directly. When `Delegation` is nil, the agent handles the directive as before.

When `Delegation.Available` is false and `Delegation.Fallback` is true, the agent handles the directive itself. The delegation metadata is informational -- it tells the agent why delegation didn't happen.

Tag matching uses ordered rules: iterate rules in config order, check if the state's tags contain the rule's tag, stop at first match. If no rule matches, no delegation.

**Side effect note:** Today, `Next()` is side-effect-free -- it reads state and template data, then returns a directive. Adding delegation resolution introduces an `exec.LookPath` call (filesystem probe for the delegate binary). This is a read-only side effect with no mutation, but it breaks testability: tests of `Next()` with delegation config would need a real binary in PATH. To address this, availability checking is behind a `DelegateChecker` interface:

```go
type DelegateChecker interface {
    Available(command []string) (bool, string)
}
```

The default implementation uses `exec.LookPath`. Tests inject a stub. The controller accepts the checker via `WithDelegateChecker(dc)` option.

#### Alternatives Considered

**Separate delegation endpoint.** A `koto delegate-check` command that the agent calls after `koto next` to check delegation status. Rejected because it adds an extra round-trip for every step. Delegation resolution at `Next()` time is free since the controller already reads the template and state.

### Decision 6: Delegate Subcommand

After the agent receives a directive with delegation metadata, it produces a prompt and needs to hand it to koto for invocation. The question is the interface for this handoff.

#### Chosen: koto delegate run with stdin piping

```bash
koto delegate run --prompt /tmp/prompt.txt
# or, read from stdin using dash convention
echo "prompt text" | koto delegate run --prompt -
```

The subcommand:
1. Reads the current state from the state file
2. Re-resolves the delegation target from tags + config (same logic as `Next()`)
3. Pipes the prompt to the delegate CLI via stdin (not as a CLI argument, to avoid shell escaping and argument length limits)
4. Captures stdout with a configurable timeout (read via `io.LimitReader`, 10 MB cap to prevent memory exhaustion)
5. Returns structured JSON to stdout

**Delegate Interface Contract:**

The interface between koto and the delegate CLI is deliberately simple:

| Aspect | Contract |
|--------|----------|
| **Input** | Raw prompt text piped to the delegate's stdin |
| **Output** | Raw text captured from the delegate's stdout |
| **Working directory** | Delegate runs in the same working directory as koto |
| **Environment** | Delegate inherits koto's full environment |
| **Filesystem access** | Read-write -- delegate can read and write files like any coding agent CLI |
| **Permissions** | Same user permissions as koto (no elevation, no sandboxing) |
| **Interaction model** | Synchronous, non-interactive: prompt in, response out, process exits |

This matches how headless coding agent CLIs already work. `claude -p` and `gemini` both accept prompts via stdin, can read/write files in the working directory, and return responses via stdout. koto doesn't try to restrict or sandbox the delegate because the delegate is a coding agent -- restricting filesystem access would make it useless for code analysis tasks.

The delegate is **read-write by default**. If a template author wants read-only delegation (analysis only, no modifications), the directive text should instruct the delegate accordingly. koto doesn't enforce this -- it's a prompt-level concern, not a systems-level one.

**Response JSON:**

```json
{
  "response": "...",
  "delegate": "gemini",
  "matched_tag": "deep-reasoning",
  "duration_ms": 12345,
  "exit_code": 0,
  "success": true
}
```

On failure:
```json
{
  "response": "",
  "delegate": "gemini",
  "matched_tag": "deep-reasoning",
  "duration_ms": 5000,
  "exit_code": 1,
  "success": false,
  "error": "delegate process exited with code 1"
}
```

On truncation (output exceeded 10 MB):
```json
{
  "response": "...(truncated)...",
  "delegate": "gemini",
  "matched_tag": "deep-reasoning",
  "duration_ms": 45000,
  "exit_code": 0,
  "success": true,
  "truncated": true
}
```

**Exit codes:** `koto delegate run` exits 0 when the delegate was invoked, even if the delegate itself failed (the JSON response carries `success: false` with the error and `exit_code`). Non-zero exit codes indicate koto-level errors (config missing, binary not found, prompt file unreadable). This follows the `gh api` convention and lets agents distinguish "delegation happened but delegate reported an error" from "delegation couldn't be attempted."

**Availability check:** Before invocation, koto checks that the delegate binary is in PATH. This is the same check done at `Next()` time. If the binary disappeared between `koto next` and `koto delegate run`, the command returns a koto-level error (non-zero exit).

**Prompt file lifecycle:** koto reads the prompt file but does not delete it. The agent is responsible for cleanup. SKILL.md instructions should use `mktemp` to avoid collisions.

**Timeout:** Configurable in the delegation config (default 300 seconds / 5 minutes).

#### Alternatives Considered

**Agent invokes delegate directly.** The agent reads the delegation target from `koto next` output and runs `gemini -p` itself. The invocation itself is simple (~15 lines per agent), but the value of koto owning it goes beyond code volume. koto handles timeout enforcement, structured error reporting (JSON with duration, exit code, error message), and availability checking -- all of which agents would each implement slightly differently. More importantly, koto can evolve invocation (add retries, logging, metrics) without touching any agent skill. Rejected because centralizing invocation in koto gives a single tested implementation and a single point of evolution.

**Delegation as a gate.** The delegate invocation happens during transition evaluation, using the existing `evaluateCommandGate` infrastructure. The agent sets evidence with the prompt, calls `koto transition`, and koto intercepts. Rejected primarily because of timing: gates run during transition validation, which is too late. The agent needs to know about delegation *before* doing work, at `koto next` time, so it can craft a prompt instead of handling the directive itself. Beyond timing, this approach forces responses through the evidence map (`map[string]string`), which can't carry structured delegate responses, and couples delegation to transitions rather than directives.

## Decision Outcome

### Summary

State declarations gain an optional `tags` field -- a string array with kebab-case pattern validation. Tags describe processing capabilities, not domains. The initial vocabulary (`deep-reasoning`, `large-context`, `specialized-tooling`) is documented in guides, not enforced by schema. `format_version` stays at 1 since tags are additive and Go's unmarshaller ignores unknown fields.

Tags flow through the template pipeline (`sourceStateDecl` -> `StateDecl` -> `template.Template.Tags`) and skip `engine.MachineState`. The controller reads tags from the template and includes them in the `Directive` output.

A new `pkg/config` package loads YAML from `~/.koto/config.yaml` (user) and `.koto/config.yaml` (project). The delegation config separates target definitions (what binary to run) from routing rules (which tags map where). Project config can add rules but not targets -- only user config defines binaries. Project-level delegation rules require an explicit opt-in.

At `koto next` time, the controller matches state tags against config rules, checks delegate availability via a `DelegateChecker` interface, and includes `DelegationInfo` in the response. The agent reads this, produces a prompt, and hands it back via `koto delegate run`. koto invokes the delegate CLI with a simple interface contract: raw prompt piped to stdin, raw text captured from stdout. The delegate runs read-write in the working directory with full user permissions -- no sandboxing, matching how headless coding agent CLIs already operate.

The full flow:
1. `koto next` returns directive with `delegation: {target: "gemini", available: true}`
2. Agent reads directive, gathers context, crafts a prompt for the delegate. The directive text should include guidance for prompt construction (what context to include, what format to request) since the template author knows what the delegate needs.
3. `koto delegate run --prompt /tmp/prompt.txt`
4. koto invokes the delegate CLI (e.g., `gemini -p`) in the working directory, piping the prompt via stdin. The delegate can read/write files.
5. koto returns `{response: "...", matched_tag: "deep-reasoning", exit_code: 0, success: true}` to the agent
6. Agent uses the response and calls `koto transition` to advance

When delegation isn't configured or the delegate isn't available, `koto next` returns the directive without delegation metadata (or with `fallback: true`), and the agent handles the step as it would today.

### Rationale

Keeping `format_version` at 1 avoids a breaking change for an additive field. Template authors can add tags incrementally -- templates with tags work on old koto (tags ignored) and new koto (tags used for delegation if configured). This is the same pattern as adding `description` to a template: older code ignores it, newer code uses it.

Pattern-validated free-form tags are the right balance for v1. An enum would require koto releases for new tags. Complete free-form would allow inconsistent naming. The kebab-case pattern prevents accidental variation while keeping the vocabulary extensible.

Tags in the template layer (not engine) follows the separation of concerns. The engine handles state machine semantics that affect workflow correctness. Tags are hints for external behavior that don't change transitions, gates, or terminal states.

## Solution Architecture

### Type Changes

**`pkg/template/compile/compile.go`** -- source format:

```go
type sourceStateDecl struct {
    Transitions []string                  `yaml:"transitions"`
    Terminal    bool                      `yaml:"terminal"`
    Gates       map[string]sourceGateDecl `yaml:"gates"`
    Tags        []string                  `yaml:"tags"`     // NEW
}
```

**`pkg/template/compiled.go`** -- compiled format:

```go
type StateDecl struct {
    Directive   string                     `json:"directive"`
    Transitions []string                   `json:"transitions,omitempty"`
    Terminal    bool                       `json:"terminal,omitempty"`
    Gates       map[string]engine.GateDecl `json:"gates,omitempty"`
    Tags        []string                   `json:"tags,omitempty"`  // NEW
}
```

**`pkg/template/template.go`** -- intermediate representation:

```go
type Template struct {
    Name        string
    Version     string
    Description string
    Machine     *engine.Machine
    Sections    map[string]string
    Tags        map[string][]string    // NEW: state name -> tags
    Variables   map[string]string
    Hash        string
    Path        string
}
```

**`pkg/controller/controller.go`** -- output types:

```go
type DelegationInfo struct {
    Target     string `json:"target"`
    MatchedTag string `json:"matched_tag"`
    Available  bool   `json:"available"`
    Fallback   bool   `json:"fallback,omitempty"`
    Reason     string `json:"reason,omitempty"`
}

type Directive struct {
    Action     string          `json:"action"`
    State      string          `json:"state"`
    Directive  string          `json:"directive,omitempty"`
    Message    string          `json:"message,omitempty"`
    Tags       []string        `json:"tags,omitempty"`        // NEW
    Delegation *DelegationInfo `json:"delegation,omitempty"`  // NEW
}
```

**`pkg/config/config.go`** -- new package:

```go
type Config struct {
    Delegation *DelegationConfig `yaml:"delegation,omitempty"`
}

type DelegationConfig struct {
    AllowProjectConfig bool                      `yaml:"allow_project_config"`
    Timeout            int                       `yaml:"timeout,omitempty"` // seconds
    Targets            map[string]DelegateTarget `yaml:"targets"`
    Rules              []DelegationRule           `yaml:"rules"`
}

type DelegateTarget struct {
    Command []string `yaml:"command"` // e.g., ["gemini", "-p"]
}

type DelegationRule struct {
    Tag    string `yaml:"tag"`
    Target string `yaml:"target"`
}

func Load() (*Config, error)                // loads from user + project
func LoadFrom(path string) (*Config, error) // loads from specific path
```

### Controller Constructor Change

`controller.New()` currently takes `(*engine.Engine, *template.Template)`. Adding delegation config uses a functional option pattern consistent with the engine's `TransitionOption`:

```go
type Option func(*Controller)

func WithDelegation(cfg *config.DelegationConfig) Option {
    return func(c *Controller) {
        c.delegationCfg = cfg
    }
}

func New(eng *engine.Engine, tmpl *template.Template, opts ...Option) (*Controller, error) {
    c := &Controller{eng: eng, tmpl: tmpl}
    for _, opt := range opts {
        opt(c)
    }
    return c, nil
}
```

A `DelegateChecker` interface abstracts the `exec.LookPath` call for testability:

```go
type DelegateChecker interface {
    Available(command []string) (bool, string)
}

type execChecker struct{}

func (execChecker) Available(command []string) (bool, string) {
    _, err := exec.LookPath(command[0])
    if err != nil {
        return false, fmt.Sprintf("binary %q not found in PATH", command[0])
    }
    return true, ""
}

func WithDelegateChecker(dc DelegateChecker) Option {
    return func(c *Controller) {
        c.checker = dc
    }
}
```

When `WithDelegation` is provided but `WithDelegateChecker` is not, the controller uses `execChecker{}` as the default.

This preserves backwards compatibility for existing callers of `New(eng, tmpl)` while allowing delegation config to be injected.

### JSON Schema Update

The compiled template schema (`compiled-template.schema.json`) gets a `tags` property on `state_decl`:

```json
{
  "$defs": {
    "state_decl": {
      "type": "object",
      "required": ["directive"],
      "additionalProperties": false,
      "properties": {
        "directive": { "type": "string", "minLength": 1 },
        "transitions": {
          "type": "array",
          "items": { "type": "string" }
        },
        "terminal": { "type": "boolean" },
        "gates": {
          "type": "object",
          "additionalProperties": { "$ref": "#/$defs/gate_decl" }
        },
        "tags": {
          "type": "array",
          "items": {
            "type": "string",
            "minLength": 1,
            "pattern": "^[a-z][a-z0-9-]*$"
          },
          "uniqueItems": true
        }
      }
    }
  }
}
```

`format_version` stays at `"const": 1`. The schema `$id` doesn't change since this is a compatible extension.

### Tag Validation in ParseJSON

`ParseJSON()` gains tag validation after the existing gate validation loop:

```go
// Validate tags
for stateName, sd := range ct.States {
    for _, tag := range sd.Tags {
        if !isValidTag(tag) {
            return nil, fmt.Errorf("state %q: invalid tag %q (must be kebab-case: ^[a-z][a-z0-9-]*$)", stateName, tag)
        }
    }
    // Check for duplicates
    seen := make(map[string]bool, len(sd.Tags))
    for _, tag := range sd.Tags {
        if seen[tag] {
            return nil, fmt.Errorf("state %q: duplicate tag %q", stateName, tag)
        }
        seen[tag] = true
    }
}
```

The pattern `^[a-z][a-z0-9-]*$` is validated by a compiled regex. This matches: `deep-reasoning`, `security`, `a`, `my-custom-tag-123`. It rejects: `Deep-Reasoning`, `CAPS`, `_underscore`, `123-start`, empty string.

### Compile() Changes

`Compile()` copies tags from `sourceStateDecl` to `StateDecl`:

```go
stateDecl := template.StateDecl{
    Directive:   directives[name],
    Transitions: sd.Transitions,
    Terminal:    sd.Terminal,
}

if len(sd.Tags) > 0 {
    tags := make([]string, len(sd.Tags))
    copy(tags, sd.Tags)
    stateDecl.Tags = tags
}
```

### ToTemplate() Changes

`ToTemplate()` populates the new `Tags` field:

```go
tags := make(map[string][]string)
for name, sd := range ct.States {
    if len(sd.Tags) > 0 {
        t := make([]string, len(sd.Tags))
        copy(t, sd.Tags)
        tags[name] = t
    }
}

return &Template{
    Name:        ct.Name,
    // ... existing fields ...
    Tags:        tags,
}, nil
```

### Controller.Next() Changes

The controller resolves delegation at `Next()` time:

```go
func (c *Controller) Next() (*Directive, error) {
    current := c.eng.CurrentState()
    // ... existing logic to build directive text ...

    d := &Directive{
        Action:    "execute",
        State:     current,
        Directive: directive,
    }

    // Include tags if present
    if c.tmpl != nil {
        if tags, ok := c.tmpl.Tags[current]; ok {
            d.Tags = tags
        }
    }

    // Resolve delegation if config exists
    if c.delegationCfg != nil && len(d.Tags) > 0 {
        if info := c.resolveDelegation(d.Tags); info != nil {
            d.Delegation = info
        }
    }

    return d, nil
}

func (c *Controller) resolveDelegation(tags []string) *DelegationInfo {
    for _, rule := range c.delegationCfg.Rules {
        for _, tag := range tags {
            if tag == rule.Tag {
                target, ok := c.delegationCfg.Targets[rule.Target]
                if !ok {
                    continue // target not defined, skip rule
                }
                available, reason := c.checker.Available(target.Command)
                info := &DelegationInfo{
                    Target:     rule.Target,
                    MatchedTag: tag,
                    Available:  available,
                }
                if !available {
                    info.Fallback = true
                    info.Reason = reason
                }
                return info
            }
        }
    }
    return nil
}
```

### Config Loading

```go
// pkg/config/config.go

func Load() (*Config, error) {
    userCfg, userErr := loadFile(userConfigPath())   // $KOTO_HOME/config.yaml or ~/.koto/config.yaml
    if userErr != nil {
        return nil, fmt.Errorf("user config: %w", userErr) // parse error, not "file missing"
    }

    projCfg, projErr := loadFile(projectConfigPath()) // .koto/config.yaml
    if projErr != nil {
        return nil, fmt.Errorf("project config: %w", projErr)
    }

    if userCfg == nil && projCfg == nil {
        return &Config{}, nil
    }
    if userCfg == nil {
        return projCfg, nil
    }
    if projCfg == nil {
        return userCfg, nil
    }

    return merge(userCfg, projCfg), nil
}

// loadFile returns (nil, nil) for missing files, (*Config, nil) for valid files,
// and (nil, error) for files that exist but can't be parsed.

func merge(user, project *Config) *Config {
    result := *user

    // Only merge project delegation if user opted in
    if user.Delegation != nil && user.Delegation.AllowProjectConfig &&
       project.Delegation != nil {
        merged := *user.Delegation
        // Project targets are ignored -- only user config defines binaries
        // Project timeout is ignored -- only user config controls timeout

        // Collect user-covered tags to prevent project overrides
        userTags := make(map[string]bool)
        for _, r := range user.Delegation.Rules {
            userTags[r.Tag] = true
        }

        // Only merge project rules that reference user-defined targets
        // and don't override user-defined tag rules
        for _, r := range project.Delegation.Rules {
            if userTags[r.Tag] {
                continue // user rule takes priority
            }
            if _, ok := user.Delegation.Targets[r.Target]; !ok {
                fmt.Fprintf(os.Stderr, "koto: project config rule for tag %q references unknown target %q (not defined in user config), skipping\n", r.Tag, r.Target)
                continue
            }
            merged.Rules = append(merged.Rules, r)
        }

        result.Delegation = &merged
    }

    return &result
}
```

### Delegate CLI Integration

`koto delegate run` invokes the delegate CLI:

```go
func invokeDelegate(targetName string, target DelegateTarget, matchedTag string, promptPath string, timeout time.Duration) (*DelegateResponse, error) {
    binary, err := exec.LookPath(target.Command[0])
    if err != nil {
        return nil, fmt.Errorf("delegate %q: binary %q not found in PATH", targetName, target.Command[0])
    }

    ctx, cancel := context.WithTimeout(context.Background(), timeout)
    defer cancel()

    prompt, err := os.ReadFile(promptPath)
    if err != nil {
        return nil, fmt.Errorf("read prompt file: %w", err)
    }

    // Use command from target definition (e.g., ["gemini", "-p"])
    cmd := exec.CommandContext(ctx, binary, target.Command[1:]...)
    cmd.Stdin = bytes.NewReader(prompt)
    // Delegate runs in the current working directory with full user permissions

    // Capture stdout with size limit via pipe + io.LimitReader
    stdoutPipe, err := cmd.StdoutPipe()
    if err != nil {
        return nil, fmt.Errorf("create stdout pipe: %w", err)
    }
    limitedReader := io.LimitReader(stdoutPipe, 10*1024*1024) // 10 MB cap

    start := time.Now()
    if err := cmd.Start(); err != nil {
        return nil, fmt.Errorf("start delegate: %w", err)
    }

    output, _ := io.ReadAll(limitedReader)
    runErr := cmd.Wait()
    duration := time.Since(start)

    exitCode := 0
    if runErr != nil {
        if exitErr, ok := runErr.(*exec.ExitError); ok {
            exitCode = exitErr.ExitCode()
        }
        return &DelegateResponse{
            Delegate:   targetName,
            MatchedTag: matchedTag,
            DurationMs: duration.Milliseconds(),
            ExitCode:   exitCode,
            Success:    false,
            Error:      runErr.Error(),
        }, nil
    }

    return &DelegateResponse{
        Response:   string(output),
        Delegate:   targetName,
        MatchedTag: matchedTag,
        DurationMs: duration.Milliseconds(),
        ExitCode:   0,
        Success:    true,
    }, nil
}
```

### Example: Source Template with Tags

```yaml
---
name: research-and-implement
version: "1.0"
description: Research a topic deeply then implement changes
initial_state: gather-context

variables:
  TASK:
    description: What to research and implement
    required: true

states:
  gather-context:
    transitions: [deep-analysis]
  deep-analysis:
    tags: [deep-reasoning]
    transitions: [implement]
  implement:
    transitions: [done]
    gates:
      tests_pass:
        type: command
        command: go test ./...
        timeout: 120
  done:
    terminal: true
---

## gather-context

Read the codebase and collect files relevant to: {{TASK}}

## deep-analysis

Analyze the codebase for: {{TASK}}

Think carefully about what changes are needed and why.

When delegating this step, include in the prompt:
- All source files in the affected packages
- The go.mod file for dependency context
- Any test files for the affected packages
- A clear statement of what analysis is expected

## implement

Based on the analysis, implement the changes for: {{TASK}}

## done

Work complete.
```

### Example: User Config

```yaml
# ~/.koto/config.yaml
delegation:
  allow_project_config: false
  timeout: 300
  targets:
    gemini:
      command: ["gemini", "-p"]
    claude:
      command: ["claude", "-p", "--model", "opus"]
  rules:
    - tag: deep-reasoning
      target: gemini
    - tag: large-context
      target: gemini
```

### File Change Summary

| File | Change |
|------|--------|
| `pkg/template/compile/compile.go` | Add `Tags` to `sourceStateDecl`, copy in `Compile()` |
| `pkg/template/compiled.go` | Add `Tags` to `StateDecl`, validate in `ParseJSON()` |
| `pkg/template/compiled-template.schema.json` | Add `tags` property to `state_decl` |
| `pkg/template/template.go` | Add `Tags map[string][]string` to `Template` |
| `pkg/template/compiled.go` | Populate `Tags` in `ToTemplate()` |
| `pkg/controller/controller.go` | Add `DelegationInfo`, `DelegateChecker`, `Tags`, `Delegation` to `Directive`, functional options, resolve in `Next()` |
| `pkg/config/config.go` | New package: `Config`, `DelegationConfig`, `DelegateTarget`, `Load()`, `merge()` |
| `pkg/config/config_test.go` | Tests for loading, merging, precedence, project restrictions |
| `cmd/koto/main.go` | Load config at startup, pass to controller via `WithDelegation()` |
| `cmd/koto/delegate.go` | New file: `koto delegate run` subcommand |
| `plugins/koto-skills/skills/hello-koto/SKILL.md` | Document delegation flow and prompt construction guidance |
| `docs/guides/delegation.md` | User guide: config, tags, delegate interface, delegation flow |

## Implementation Approach

### Phase 1: Tags in Template Pipeline

Add `Tags []string` to `sourceStateDecl`, `StateDecl`, and `template.Template`. Update `Compile()`, `ToTemplate()`, `ParseJSON()`, and the JSON schema. Include tags in `controller.Directive` output. No delegation logic yet -- just storing and passing through tags.

**Tests:** Compile a template with tags, verify they appear in compiled JSON. Parse compiled JSON with tags, verify validation. Verify old templates without tags still work. Verify tag pattern validation rejects bad values.

### Phase 2: Config System

Build `pkg/config` with `Load()`, `LoadFrom()`, `merge()`. Support user and project levels with YAML parsing. Integrate config loading into the CLI startup path.

**Tests:** Load user config, project config, merged config. Verify `allow_project_config` gating. Verify missing config files are handled gracefully.

### Phase 3: Delegation Resolution

Wire config into the controller. Implement tag-to-target resolution in `Next()`. Add `DelegationInfo` to the output. Implement delegate availability checking (binary in PATH).

**Tests:** `Next()` with delegation config returns `DelegationInfo`. `Next()` without config returns no delegation. Fallback when delegate binary not found.

### Phase 4: Delegate Subcommand

Implement `koto delegate run`. Stdin piping to delegate CLI, stdout capture, timeout handling, structured JSON response.

**Tests:** Submit with mock delegate binary. Timeout handling. Error reporting on delegate failure.

### Phase 5: Agent Skills and Documentation

Update SKILL.md with delegation flow documentation. Write `docs/guides/delegation.md` user guide. Ship the research-and-implement reference template.

## Security Considerations

### Download Verification

Not applicable. Delegation doesn't download artifacts. Tags are string metadata; config maps them to CLIs already installed on the machine.

### Execution Isolation

koto invokes delegate CLIs as subprocesses using `exec.CommandContext` with explicit args -- not `sh -c`. The delegate binary is resolved via `exec.LookPath`. koto never shell-executes the delegate target identifier.

Delegate invocation commands are specified in target definitions in user config. The `command` field is an explicit array of binary + args (e.g., `["gemini", "-p"]`), resolved via `exec.LookPath` against PATH. koto never shell-interprets the command -- it uses `exec.CommandContext` with the array elements directly. The delegate runs in the working directory with full user permissions (read-write filesystem access). koto doesn't sandbox the delegate because it's a coding agent CLI that needs filesystem access to be useful.

### Supply Chain Risks

Templates and config files come from the same sources as before (local files). Tags are strings in YAML frontmatter with no executable content.

Project-level config (`.koto/config.yaml`) is a new trust-bearing artifact. A cloned repository could ship delegation rules that route tags to unexpected targets. Two mitigations: (1) project-level delegation rules require `allow_project_config: true` in the user's config, and (2) the config schema separates targets (binary definitions) from rules (tag routing). Project config can only add rules, not targets -- only user-level config defines what binaries are available. Project config also cannot override `timeout`. This prevents a cloned repo from specifying an arbitrary binary for koto to execute or setting an excessive timeout.

### User Data Exposure

The prompt sent to the delegate is produced by the orchestrating agent, which may include codebase content. This content is sent to the delegate's API (e.g., Google's API via Gemini CLI).

Mitigations:
- Delegation only happens when the user explicitly configures rules. No config means no delegation.
- Config is user-controlled. The user decides which tags route where.
- The `koto next` output includes delegation metadata, so the agent (and user, if watching) knows content will flow to a delegate.

### Prompt Injection

The prompt sent to the delegate may incorporate repository content (file contents, error messages). This content crosses a model boundary in headless mode with no human oversight.

This is an unmitigatable residual risk. koto is a pipe: it passes the agent's prompt to the delegate CLI and returns the response. koto cannot inspect or sanitize the prompt content because it doesn't understand the prompt's semantics. Cross-model injection resistance depends entirely on the delegate provider's safety training, which varies across providers and models.

What koto does:
- Delegate CLIs are invoked in headless mode without flags that bypass safety checks
- `koto next` output includes delegation metadata, so agent skill authors know content will cross a model boundary

What koto doesn't do:
- Prompt sanitization (koto doesn't know what's safe for a given delegate)
- Content filtering (the prompt is opaque bytes to koto)

### Mitigations

| Risk | Mitigation | Residual Risk |
|------|------------|---------------|
| Codebase content sent to third-party API | Config-controlled; user explicitly enables delegation | Users may not track which tags route where |
| Prompt injection across model boundary | Unmitigatable -- koto is a pipe; injection resistance depends on delegate provider | Cross-model injection resistance varies by provider and model |
| Project config routes to arbitrary binaries | Config separates targets (user-only) from rules (project-allowed); project can't define new targets | Users who opt in globally trust all project routing rules |
| Delegate stdout exhausts host memory | `io.LimitReader` caps output at 10 MB | Truncated responses may lose critical content |
| Delegate prompt may contain secrets found in codebase | Agent decides prompt content; koto can't filter | Sensitive data may cross provider boundaries |
| Delegate binary not in PATH | Availability check at `Next()` and `run` time | Workflow loses delegation benefit |
| Delegate binary auth broken | koto detects failure, returns error | Agent must handle fallback |
| Unknown delegate target in config | koto logs warning, returns no delegation | Silent no-op may surprise user |

## Consequences

### Positive

- Templates describe step characteristics without naming specific CLIs. The same template works with or without delegation.
- Tags are useful beyond delegation. They capture semantic metadata that could drive future features (metrics, logging, priority) without more template format changes.
- The config system is general infrastructure. Future features can add their own config sections.
- Backwards compatible. Old koto ignores tags. Old templates work on new koto.
- Graceful degradation. No config means no delegation. Delegate unavailable means fallback.

### Negative

- New subsystem. Config loading, delegation resolution, delegate invocation, and a new CLI subcommand add code and maintenance surface.
- Indirection. Understanding what happens at a tagged step requires checking the config. Tags don't tell you where a step goes.
- Convention-dependent. Tags work when template authors and config authors agree on naming. Inconsistent naming fails silently.
- Adding tags to existing templates changes the compiled hash, breaking in-flight workflows. Mitigation: finish in-flight workflows before adding tags, or use `koto init` to restart. This is the same constraint as any template change (editing a directive also changes the hash). Template authors should treat tag additions like any other template edit.
- Config system introduces a new attack surface (project config as supply chain vector), mitigated by opt-in.
