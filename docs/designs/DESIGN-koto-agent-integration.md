---
status: Proposed
spawned_from:
  issue: 315
  repo: tsukumogami/vision
  parent_design: docs/designs/DESIGN-workflow-tool-oss.md
problem: |
  koto v0.1.0 ships a working state machine engine and is installable, but no AI agent
  will use it because nothing connects the binary on PATH to the agent's execution context.
  There's no template distribution mechanism (koto init requires an explicit filesystem path),
  no integration file generation (koto generate doesn't exist), and no discovery protocol for
  agents to find active workflows or available templates. Without solving this middle layer
  between installation and engine, koto is dead weight.
decision: |
  Add three capabilities: (1) a built-in template registry so koto init can resolve templates
  by name instead of requiring filesystem paths, (2) a koto generate command that produces
  platform-specific agent integration files (Claude Code skills/hooks, AGENTS.md sections),
  and (3) a koto workflows command that gives agents a machine-readable view of available
  templates and active state files. Templates ship embedded in the binary via go:embed with
  a search path that checks project-local, user-level, and built-in locations. Generated
  integration files are committed to repos and maintained by the user.
rationale: |
  The core constraint is that koto is a CLI tool, not a service. Agents can't discover it
  through a protocol -- they need static configuration files that describe koto's capabilities
  and invocation patterns. Embedding templates in the binary eliminates the bootstrap problem
  (users get a working template immediately after installation) while the search path lets
  projects override or add custom templates. Generation over auto-discovery means integration
  files are visible, reviewable, and version-controlled. The alternative -- expecting agents
  to probe PATH and figure out koto's CLI surface -- doesn't work because agents need more
  than binary existence; they need workflow context, evidence key documentation, and response
  schema descriptions.
---

# DESIGN: koto Agent Integration

## Status

**Proposed**

## Upstream Design Reference

This design implements the agent integration component described in the upstream strategic design for koto's multi-workflow orchestration system.

**Relevant sections:**
- Decision 6: Standard Binary Distribution + Skill-Based Agent Integration
- Required Tactical Design: DESIGN-koto-agent-integration.md
- CLI Surface: `koto generate claude-code`, `koto generate agents-md`, `koto workflows --json`

## Context and Problem Statement

koto has a working state machine engine (v0.1.0) that enforces workflow progression through evidence-gated transitions and progressive disclosure. It's installable via GitHub Releases, an install script, and a tsuku recipe. But there's a gap between "koto is on PATH" and "an AI agent uses koto to run a workflow."

The gap has three parts:

**Template distribution.** `koto init` requires `--template /absolute/path/to/template.md`. There's no way to say `koto init --template quick-task` and have koto find the template. The binary ships no built-in templates. A user who just installed koto has nothing to run.

**Agent integration.** No agent knows koto exists. Claude Code needs a skill file describing koto's CLI commands and response schemas. Cursor needs a rules file. Any agent needs documentation of what evidence keys to supply for transitions. None of this exists, and koto can't generate it.

**Workflow discovery.** `koto workflows` lists active state files, but there's no way to list available templates. An agent can't answer "what workflows can I start?" -- only "what workflows are already running?"

These three problems are connected. Embedding a template in the binary is useless if nothing tells the agent how to use it. Generating an agent skill file is useless if there's no template to reference. Listing available templates is useless if the agent doesn't know koto exists. The solution must address all three together.

### Scope

**In scope:**
- Built-in template embedding and search path resolution
- `koto generate` command for producing agent integration files
- Template and workflow discovery for agents
- Claude Code integration (skill, command, hook)
- Generic agent integration (AGENTS.md)

**Out of scope:**
- Template registry or community sharing (future work)
- Template authoring experience improvements
- Human interaction UX (covered by a separate design)
- MCP server integration
- Evidence gate documentation extraction from templates (nice-to-have, not blocking)

## Decision Drivers

- Must work across agent platforms (Claude Code, Cursor, Codex, generic shell agents)
- Templates need stable filesystem paths -- the engine stores absolute paths in state files and verifies template hashes on every operation
- Generated integration files are committed to repos; version drift between koto and the generated files is a concern
- The solution must handle both "start new workflow" and "resume active workflow"
- koto is a CLI tool, not a service -- no background process, no MCP server, no auto-discovery protocol
- Template distribution and agent integration are the same problem: both answer "how does an agent go from zero to running a koto workflow?"
- The first-run experience matters: a user who just installed koto should be able to start a workflow within minutes

## Considered Options

### Decision 1: Template Distribution

How do templates get from koto's repository into a user's project? `koto init` currently requires an absolute filesystem path to a `.md` template file. This means users must know where template files live before they can use koto. The first-run experience is broken: install koto, then... what? There's no template to point at.

The answer shapes everything downstream. If templates aren't discoverable by name, every integration file must hardcode filesystem paths. If templates aren't bundled with the binary, every new user hits a bootstrap problem.

#### Chosen: Embedded Built-in Templates with Search Path

Ship built-in templates embedded in the koto binary via Go's `embed` package. Add a template search path that resolves template names to filesystem paths. The resolution order:

1. **Project-local**: `.koto/templates/<name>.md` in the current directory (or nearest git root)
2. **User-level**: `~/.koto/templates/<name>.md` (or `$KOTO_HOME/templates/`)
3. **Built-in**: Templates embedded in the binary via `go:embed`

When a user runs `koto init --template quick-task`, koto walks the search path and uses the first match. If `--template` is an absolute or relative path (contains `/` or `\`), it's used directly, preserving backward compatibility.

For built-in templates, koto extracts the embedded template to a versioned location (`~/.koto/templates/<version>/<name>.md`) on first use. This gives the engine an absolute filesystem path to store in the state file, which is required for template hash verification on subsequent operations. The version directory prevents conflicts when multiple koto versions are installed -- each version's built-in templates live in their own namespace, so upgrading koto doesn't break workflows initialized by a previous version. The extraction is idempotent: if the file already exists with the same content, it's left alone.

The initial release ships one built-in template: `quick-task` (a 6-phase linear workflow for small tasks).

`koto template list` shows all discoverable templates with their source (project, user, built-in):

```
$ koto template list
NAME          SOURCE    DESCRIPTION
quick-task    built-in  Execute a small task with scope validation
my-workflow   project   Custom review workflow
```

#### Alternatives Considered

**Download-on-demand from a registry.** Templates pulled from a git-hosted registry when referenced by name.
Rejected because it adds a network dependency to `koto init`, requires registry infrastructure, and doesn't solve the "just installed, now what?" problem any faster than embedding. Registry distribution is future work.

**Scaffold-only (koto init creates a template file).** `koto init` generates a starter template in the project directory instead of resolving by name.
Rejected because it forces every project to maintain its own template copy from day one, even for standard workflows. Users should be able to use built-in templates without copying them first.

**No search path, explicit paths only.** Keep the current behavior. Users must always provide a full path.
Rejected because it makes the first-run experience hostile and forces every integration file to hardcode paths that vary by machine.

### Decision 2: Agent Integration Mechanism

How does an AI agent learn that koto exists in a project and should be used? Agents don't probe PATH for binaries and reverse-engineer their CLI surface. They need static configuration files that describe capabilities, commands, response formats, and when to use them.

The design must work across platforms with different integration mechanisms. Claude Code uses skill files (`.claude/skills/`), commands (`.claude/commands/`), and hooks (`.claude/hooks/`). Cursor uses `.cursorrules`. Other agents read `AGENTS.md`. Each platform has different capabilities: Claude Code has hooks that fire on lifecycle events; most others only have static instruction files.

#### Chosen: koto generate with Per-Platform Targets

Add a `koto generate <platform>` command that produces platform-specific integration files. Each platform target generates the files appropriate for that platform's integration model.

**`koto generate claude-code`** produces three files:

1. **Skill file** (`.claude/skills/koto.md`): Documents koto's CLI commands, JSON response schemas, evidence gate types, and the workflow execution loop (next -> execute directive -> transition with evidence -> next). Includes the template's state machine description so the agent understands the workflow structure.

2. **Command file** (`.claude/commands/koto-run.md`): A slash command (`/koto-run`) that humans use to trigger a koto workflow. Takes an optional template name and task description. Wraps `koto init` + `koto next`.

3. **Hook config** (merged into `.claude/hooks.json`): A `Stop` hook that detects active koto state files and reminds the agent to continue the workflow instead of stopping. If `.claude/hooks.json` already exists, koto merges its hook entry into the existing `Stop` array (or creates one). If the file doesn't exist, koto creates it. This prevents the common failure mode where agents quit mid-workflow.

**`koto generate agents-md`** produces a markdown section suitable for appending to `AGENTS.md`:

```markdown
## koto Workflow Engine

This project uses koto for workflow orchestration. koto controls multi-step
task execution through evidence-gated state machines.

### Available Workflows
[auto-populated from koto template list]

### Usage
[CLI commands, response format, execution loop]
```

**Common behavior:**
- Generated files include a header comment: `<!-- Generated by koto v0.1.0. Regenerate with: koto generate claude-code -->`
- `koto generate` reads available templates to populate workflow descriptions
- Running `koto generate` again updates generated files: skill and command files are overwritten; hooks.json is merged (koto's hook entry is replaced without touching other hooks). A `--dry-run` flag previews changes without writing
- Generated files are meant to be committed to the repo

#### Alternatives Considered

**MCP server integration.** Run koto as an MCP server so Claude Code and other MCP-capable agents discover it through the protocol.
Rejected because it requires a background process for the lifetime of the agent session, adds a protocol dependency (MCP) that not all agent platforms support, and is architecturally heavier than the problem warrants. The generation approach produces the same result (agent knows about koto) without runtime infrastructure. MCP also only covers MCP-capable agents -- Cursor, Codex, and generic shell agents would still need the generation path, so MCP would be an additional integration surface, not a replacement.

**Auto-discovery via PATH probing.** Agent detects koto via `which koto` without any project configuration.
Rejected because agents need more than binary existence. They need to know what workflows are available, what evidence keys to supply, and how to parse responses. A skill file provides this context; a binary on PATH doesn't.

**Single universal integration file.** One file format that all platforms read.
Rejected because platforms have fundamentally different integration mechanisms. Claude Code's hook system (which can prevent the agent from stopping) has no equivalent in Cursor's rules file. A universal format would be limited to the lowest common denominator.

**Template-embedded agent instructions.** Put agent instructions directly in the workflow template's markdown.
Rejected because agent instructions and state machine definitions serve different audiences. The template is for koto (the binary); agent instructions are for the LLM. Mixing them creates a file that's hard to maintain for either purpose. The template changes when the workflow changes; agent instructions change when the integration model changes.

### Decision 3: Workflow Discovery Protocol

How does an agent determine what koto workflows are available to start and which are currently running? The agent needs this to answer two questions: "should I start a koto workflow for this task?" and "is there an active workflow I should resume?"

Currently `koto workflows` scans `wip/` for state files and reports running workflows. But there's no way to list available templates, and the output format isn't documented for agent consumption.

#### Chosen: Extend koto workflows to Cover Both Templates and Active State Files

Extend `koto workflows` to report both available templates and active state files in a single JSON response:

```json
{
  "templates": [
    {
      "name": "quick-task",
      "source": "built-in",
      "description": "Execute a small task with scope validation",
      "version": "1.0",
      "states": ["initial_jury", "research", "validation_jury", "setup", "implementing", "pr_created", "done", "escalated"]
    }
  ],
  "active": [
    {
      "path": "wip/koto-my-task.state.json",
      "name": "my-task",
      "template": "quick-task",
      "current_state": "implementing",
      "created_at": "2026-02-23T10:00:00Z"
    }
  ]
}
```

The `--json` flag (or detecting non-TTY stdout) outputs JSON. Without it, `koto workflows` prints a human-readable table. This is consistent with koto's existing pattern where agent-facing commands output JSON and human-facing commands output formatted text.

**Breaking change (acceptable pre-1.0):** The current `koto workflows` JSON output is a bare array of active workflow objects. The new format wraps it in `{"templates": [...], "active": [...]}`. This changes the top-level JSON shape from array to object. Since koto is pre-1.0 and has no external consumers yet, this break is acceptable. The existing `active` array contents are unchanged.

The `templates` array walks the search path and deduplicates by name (first match wins, same as resolution). The `active` array is the existing `koto workflows` behavior.

#### Alternatives Considered

**Separate commands for templates and workflows.** `koto template list --json` for templates, `koto workflows --json` for active.
Rejected as the primary interface because agents need both pieces of information to make a decision ("what can I start?" + "what should I resume?"). Forcing two calls when one suffices adds friction. `koto template list` still exists as a human-friendly command; the combined view is the agent-facing interface.

**Discovery via a manifest file.** A `.koto/manifest.json` that lists available workflows, generated by `koto init` or `koto generate`.
Rejected because it's another file to keep in sync. The binary already knows what templates are available (search path) and what's running (state file scan). A manifest adds a stale-data risk with no upside.

### Uncertainties

- **Hook effectiveness.** The Stop hook preventing agents from quitting mid-workflow is based on experience with one agent platform. Whether this pattern works equally well across Claude Code versions and configurations hasn't been validated broadly.
- **Generated file drift.** When koto updates its CLI surface or response format, generated integration files become stale. The header comment tells users to regenerate, but there's no automated detection. A `koto doctor` command (future work) could check for drift.
- **Template search path conflicts.** A project-local template with the same name as a built-in template shadows it. This is intentional (project customization) but could confuse users who expect the built-in version. `koto template list` shows the source to disambiguate.
- **Cross-platform testing.** The design covers Claude Code and AGENTS.md generation. Cursor-specific generation (`.cursorrules`) is deferred until there's demand. The AGENTS.md format should work as a baseline for platforms without dedicated integration.

## Decision Outcome

**Chosen: Embedded templates with search path + per-platform generate command + extended workflows discovery**

### Summary

koto gets three new capabilities that close the gap between installation and agent usage.

First, built-in templates ship embedded in the binary. When a user runs `koto init --template quick-task`, koto resolves the name through a three-level search path (project-local `.koto/templates/`, user-level `~/.koto/templates/`, built-in). For built-in templates, koto extracts the file to `~/.koto/templates/<version>/` on first use, giving the engine the stable absolute path it needs for state file integrity. The versioned directory prevents cross-version conflicts. The `--template` flag still accepts explicit paths (anything containing a path separator), so existing behavior is unchanged.

Second, `koto generate <platform>` produces agent integration files. `koto generate claude-code` creates a skill file (CLI documentation, response schemas, execution loop), a command file (`/koto-run` slash command), and a Stop hook (prevents mid-workflow abandonment). `koto generate agents-md` produces a markdown section for AGENTS.md. Generated files include version headers and are meant to be committed. Running `koto generate` again overwrites them.

Third, `koto workflows` gains a `--templates` flag (or becomes the default when `--json` is used) to report both available templates and active state files in one response. This gives agents a single call to answer "what can I start?" and "what should I resume?"

The execution loop an agent follows is: call `koto workflows` to check for active state files, resume if one exists (call `koto next`), or start a new one (`koto init --template <name>`). Then loop: `koto next` returns a directive, the agent executes it, `koto transition <state> --evidence key=value` advances the workflow. The agent never sees the full template -- only the current state's directive.

### Rationale

These three pieces solve the bootstrap problem as a unit. Templates without integration files means no agent knows to use them. Integration files without templates means there's nothing to reference. Discovery without either means there's nothing to discover.

Embedding over downloading avoids a network dependency at the critical first-run moment. Generation over auto-discovery means integration files are visible and reviewable in version control -- no magic, no hidden state. The search path with project-local override lets teams customize workflows without forking.

The Stop hook is the one piece of "active" integration (versus static documentation). It's justified because the most common failure mode is agents quitting mid-workflow. Without it, every koto session risks abandonment on the first session boundary.

### Trade-offs Accepted

By choosing this approach, we accept:

- **Generated files can drift.** When koto updates its CLI, generated skill files become stale. There's no automatic sync -- users must run `koto generate` again. This is acceptable because the alternative (runtime discovery) requires koto to be a service, not a CLI tool.
- **Built-in template extraction creates files in ~/.koto/.** Users who prefer zero home-directory footprint can't avoid this if they use built-in templates. The extraction is required because the engine stores absolute paths and verifies template hashes. Configurable via `$KOTO_HOME`. Versioned directories (`~/.koto/templates/<version>/`) prevent cross-version conflicts but accumulate over time.
- **Platform-specific generation is opt-in.** Users must run `koto generate claude-code` explicitly. No auto-detection of the agent platform. This adds a setup step but avoids surprises.
- **AGENTS.md is a lowest-common-denominator format.** It provides instructions but no active behavior (no hooks, no slash commands). Platforms with richer integration models get better integration through their dedicated generators.

## Solution Architecture

### Overview

The implementation adds three packages to the koto binary: template resolution (search path + extraction), integration file generation, and extended discovery. No new external dependencies are needed -- Go's `embed` package handles built-in templates, and the generation commands produce static text files.

### Components

```
cmd/koto/main.go            # Extended: generate subcommand, template list, workflows --json
pkg/
├── registry/                # NEW: template search path resolution
│   ├── registry.go          # Search path walker, name-to-path resolution
│   └── extract.go           # Built-in template extraction to ~/.koto/templates/<version>/
├── generate/                # NEW: integration file generation
│   ├── generate.go          # Shared generation logic (template discovery, versioning)
│   ├── claudecode.go        # Claude Code skill, command, hook generation
│   └── agentsmd.go          # AGENTS.md section generation
├── discover/                # EXTENDED: add template discovery alongside state file discovery
│   ├── discover.go          # Existing state file scanning
│   └── templates.go         # NEW: template enumeration from search path
├── home/                    # NEW: shared KOTO_HOME resolution (used by cache and registry)
│   └── home.go              # kotoHome() helper: $KOTO_HOME or ~/.koto
└── registry/
    └── templates/            # NEW: embedded template files (go:embed)
        └── quick-task.md     # Built-in quick-task template
```

### Key Interfaces

**Template Resolution:**

```go
// registry.Registry resolves template names to filesystem paths.
type Registry struct {
    ProjectDir string // project root (git root or cwd)
    UserDir    string // ~/.koto/templates/ or $KOTO_HOME/templates/
}

// Resolve finds a template by name, walking the search path.
// Returns the absolute filesystem path.
// If name contains a path separator, it's treated as an explicit path.
func (r *Registry) Resolve(name string) (string, error)

// List returns all discoverable templates with metadata.
func (r *Registry) List() ([]TemplateInfo, error)

type TemplateInfo struct {
    Name        string   // template name (filename without extension)
    Source      string   // "project", "user", or "built-in"
    Path        string   // absolute filesystem path
    Description string   // from template YAML frontmatter
    Version     string   // from template YAML frontmatter
    States      []string // state names from compiled template (requires compilation)
}
```

**Integration File Generation:**

```go
// generate.Generator produces integration files for a target platform.
type Generator struct {
    Registry  *registry.Registry
    OutputDir string // project root
    Version   string // koto version for header comments
}

// ClaudeCode generates .claude/ skill, command, and hook files.
func (g *Generator) ClaudeCode() ([]GeneratedFile, error)

// AgentsMD generates an AGENTS.md section.
func (g *Generator) AgentsMD() ([]GeneratedFile, error)

type GeneratedFile struct {
    Path    string // relative path from project root
    Content []byte
    Action  string // "created" or "updated"
}
```

**Extended Discovery:**

```json
{
  "templates": [
    {
      "name": "quick-task",
      "source": "built-in",
      "description": "Execute a small task with scope validation",
      "version": "1.0",
      "states": ["initial_jury", "research", "validation_jury", "setup", "implementing", "pr_created", "done", "escalated"]
    }
  ],
  "active": [
    {
      "path": "wip/koto-my-task.state.json",
      "name": "my-task",
      "template": "quick-task",
      "current_state": "implementing",
      "created_at": "2026-02-23T10:00:00Z"
    }
  ]
}
```

### Data Flow

**First-run experience:**

```
1. User installs koto (already done: install.sh, tsuku recipe, go install)
2. User runs: koto generate claude-code
   → koto discovers built-in templates
   → Generates .claude/skills/koto.md, .claude/commands/koto-run.md, .claude/hooks.json
   → User commits these files
3. User (or agent) runs: koto init --template quick-task --var TASK="description"
   → Registry resolves "quick-task" to built-in template
   → Extracts to ~/.koto/templates/<version>/quick-task.md (if not already there)
   → Engine creates wip/koto-<name>.state.json with absolute path to extracted template
4. Agent calls koto next → gets first directive → executes → transitions → loops
```

**Resume flow:**

```
1. Agent starts a new session
2. Stop hook detects wip/koto-*.state.json exists
3. Hook reminds agent: "There's an active koto workflow. Run koto next to continue."
4. Agent calls koto next → gets current state's directive → continues
```

**Discovery flow:**

```
1. Agent needs to decide whether to use koto
2. Agent calls koto workflows --json
3. Response includes templates[] (what can be started) and active[] (what's running)
4. If active[] is non-empty: agent resumes the workflow
5. If active[] is empty and task matches a template: agent starts a new workflow
```

### Generated File Content

**Claude Code Skill (`.claude/skills/koto.md`):**

The skill file documents:
- koto's purpose (workflow orchestration via evidence-gated state machines)
- The execution loop (next → execute → transition → next)
- CLI command reference with JSON response schemas
- Available templates and their state machines
- Evidence gate types and how to supply evidence on transitions
- Error handling (how to interpret error responses)
- Resume behavior (check for active state files before starting new workflows)

**Claude Code Command (`.claude/commands/koto-run.md`):**

A slash command that wraps workflow initiation:
```
/koto-run [template] [task description]
```
Defaults to `quick-task` if no template specified. Creates the state file and calls `koto next` to get the first directive.

**Claude Code Hook (`.claude/hooks.json`):**

A Stop hook that checks for active koto state files:
```json
{
  "hooks": {
    "Stop": [
      {
        "type": "command",
        "command": "koto workflows --json 2>/dev/null | grep -q '\"active\":\\[\\]' || echo 'Active koto workflow detected. Run koto next to continue.'"
      }
    ]
  }
}
```

The hook uses `koto workflows --json` instead of hardcoding a state directory path, so it works regardless of the `--state-dir` configured at init time. The hook runs on every Stop event. If no state files exist, `koto workflows` returns an empty `active` array and the hook produces no output. If a state file exists, it prints a reminder that the agent incorporates into its response.

## Implementation Approach

### Phase 1: Quick-Task Template and Template Registry

Write the built-in template first (the `go:embed` directive needs it to exist), then build the resolution system:
- Write the `quick-task.md` built-in template: 6-phase linear state machine for small task execution, evidence gates at each transition, clear directive text at each state. Built-in templates must not use command gates -- all evidence gates should be `field_not_empty` or `field_equals` only
- Extract shared `pkg/home/` package for `$KOTO_HOME` / `~/.koto` resolution (currently duplicated in `pkg/cache/`)
- Create `pkg/registry/` with search path resolution logic and `go:embed` for built-in templates
- Implement extraction of built-in templates to `~/.koto/templates/<version>/` with symlink protection (consistent with the engine's state file write guard)
- Modify `koto init` to use the registry when `--template` doesn't contain a path separator
- Implement `koto template list` (human-readable output showing all discoverable templates)
- Emit a stderr warning when a project-local template shadows a built-in name during resolution

### Phase 2: Integration File Generation

Build the `koto generate` command:
- Create `pkg/generate/` with generation logic
- Implement `koto generate claude-code` (skill, command, hook)
- Implement `koto generate agents-md` (AGENTS.md section)
- Include version header in generated files
- Support `--dry-run` flag for previewing generated content
- Hook file merge strategy: read existing `.claude/hooks.json`, find or create `Stop` array, insert/replace koto's hook entry, preserve all other hooks
- Generated skill files must structurally separate koto's authoritative CLI documentation from user-supplied template metadata (descriptions, state names) to reduce prompt injection surface

### Phase 3: Extended Discovery

Extend `koto workflows`:
- Add template enumeration to the JSON output (requires compiling each template to extract state names)
- Include template metadata (name, source, description, version, states)
- Accept the breaking change to the JSON output format (array to object, acceptable pre-1.0)
- Add `--json` flag (or auto-detect non-TTY) for machine-readable output

## Security Considerations

### Download Verification

**Not applicable for the core feature.** Template distribution in this design uses `go:embed` (compiled into the binary) and filesystem paths. No network downloads occur during template resolution or integration file generation.

The extracted built-in templates at `~/.koto/templates/` are copies of the embedded content. On first extraction, the file is created from in-memory bytes. The engine's existing template hash verification (stored at `koto init`, checked on every `koto next` and `koto transition`) protects against post-extraction tampering.

### Execution Isolation

**Generated integration files.** `koto generate` writes files to the project directory (`.claude/skills/`, `.claude/commands/`, `.claude/hooks.json`, `AGENTS.md`). These files are static text that agents read as instructions. They don't execute anything directly.

**One exception: the Stop hook.** The generated Claude Code hook invokes `koto workflows --json` on every Stop event to check for active state files. This means the `koto` binary on PATH gets executed automatically by the hook. The command is read-only (scans for state files, outputs JSON) and has no side effects beyond stdout. A risk: if a malicious `koto` binary is earlier on PATH, it gets invoked automatically. Mitigation: the hook is generated from the koto binary the user explicitly ran, and the generated file is committed and reviewed via PR.

**Template extraction.** Built-in templates are extracted to `~/.koto/templates/<version>/`. koto creates this directory if it doesn't exist. Extraction follows the same symlink protection as engine state file writes: reject symlink targets to prevent arbitrary file overwrites. Write access to the user's home directory (or `$KOTO_HOME`) is required. No elevated permissions needed.

### Supply Chain Risks

**Built-in templates.** Templates embedded via `go:embed` are part of the compiled binary. They're subject to the same supply chain protections as the binary itself (GitHub Actions build, SHA-256 checksums, provenance attestation).

**Generated integration files.** The skill file, command file, and hook config are generated from templates hardcoded in the koto binary. A compromised koto binary could generate malicious integration files (e.g., a hook that exfiltrates data). Mitigation: generated files are committed to version control and reviewed via PR. The `--dry-run` flag lets users inspect output before writing.

**Search path shadowing.** A project-local template (`.koto/templates/foo.md`) shadows a built-in template with the same name. A malicious contributor could add a project-local template that overrides a trusted built-in. In an agent-driven workflow, no human runs `koto template list` to check the source. Mitigations: (1) `koto init` emits a stderr warning when a project-local template shadows a built-in name, making the override visible in agent output; (2) the template hash is locked at init time, so swapping templates mid-workflow causes a hash mismatch error; (3) generated files are committed and reviewed via PR, so the shadowing template would need to pass code review.

### User Data Exposure

**Generated files contain project metadata.** Skill files and AGENTS.md sections include template names and descriptions. These are derived from template frontmatter (which users control) and koto's version string. No secrets, credentials, or source code are included.

**No network transmission.** Template resolution, file generation, and discovery are all local filesystem operations. No data leaves the machine.

**State file paths in generated hooks.** The Stop hook invokes `koto workflows --json`, which scans for state files. The hook output only indicates whether active workflows exist -- it doesn't expose state file contents (workflow name, current state, evidence) to the agent through the hook itself.

### Mitigations

| Risk | Mitigation | Residual Risk |
|------|------------|---------------|
| Malicious project-local template shadows built-in | Stderr warning on shadow; template hash locked at init; shadowing template visible in PR review | Agent may not surface stderr warning to user |
| Stop hook invokes `koto` from PATH on every Stop event | Hook command is hardcoded in generated file, reviewed via PR; `koto workflows` is read-only | Malicious `koto` earlier on PATH gets auto-invoked |
| Stale generated files reference outdated CLI surface | Version header in generated files; `--dry-run` for inspection | No automated drift detection |
| Template extraction writes to home directory | Respects `$KOTO_HOME`; versioned directories prevent cross-version conflicts; symlink protection on extraction target | Users who don't want home-dir writes must use explicit paths; versioned dirs accumulate over time |
| Generated skill file gives agent incorrect instructions | Generated from koto's own source; version-matched | Drift after koto upgrade without regeneration |
| Prompt injection via template metadata in skill files | Skill file structurally separates koto CLI docs from template metadata | Agents may not respect the structural boundary |
| Template extraction target is a symlink | Reject symlinks before writing (same guard as state file writes) | None identified |

## Consequences

### Positive

- First-run experience drops from "install koto, find a template file, figure out the path" to "install koto, run koto generate, go"
- Agents get structured documentation of koto's capabilities without manual skill authoring
- The Stop hook prevents the most common failure mode (agents quitting mid-workflow)
- Template search path enables project-level customization without forking
- `koto workflows --json` gives agents a single endpoint for both "what can I start?" and "what should I resume?"
- Generated files are committed to repos, making integration visible and reviewable

### Negative

- Built-in template extraction adds files to `~/.koto/` that users didn't explicitly put there
- Generated files drift when koto updates -- no automatic sync mechanism
- Per-platform generation means adding support for new agent platforms requires code changes to koto
- The Stop hook only works in Claude Code -- other platforms don't have equivalent lifecycle hooks

### Mitigations

- `$KOTO_HOME` lets users control where extracted templates live; explicit `--template /path` bypasses extraction entirely
- Version headers in generated files signal when regeneration is needed; a future `koto doctor` command could automate drift detection
- AGENTS.md provides a baseline for any platform; dedicated generators only needed for platforms with richer integration models
- Platforms without hooks still work -- agents just don't get the anti-abandonment nudge. The skill file's documentation of the execution loop partially compensates by making resume behavior explicit
