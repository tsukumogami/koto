---
status: Proposed
problem: |
  koto v0.1.0 has a working state machine engine and is installable, but no AI agent will
  use it because nothing connects the binary to the agent's context. The missing piece isn't
  template distribution or a discovery protocol -- it's that the agent skill (or equivalent)
  is the natural integration unit. A skill bundles the template with agent instructions, and
  koto just needs to compile and cache templates on first use. Today koto init requires an
  explicit filesystem path, which works but the compilation and caching path isn't designed
  for this use case.
decision: |
  Focus on two flows. First, the agent-driven flow: an agent skill contains the workflow
  template (as a file alongside the skill) and instructions telling the agent how to call
  koto. The skill is the distribution unit -- install a skill, get a working koto workflow.
  koto compiles and caches the template on first koto init, which it already does. Second,
  the author-driven flow: a user writing or editing a template validates it with koto
  template compile, which already exists. The design adds koto generate to produce skill
  scaffolds for a given template, and a Stop hook to prevent agents from quitting
  mid-workflow.
rationale: |
  The agent skill is the right integration unit because it naturally bundles what the agent
  needs to know (how to call koto, what evidence to supply) with the template that defines
  the workflow. Treating template distribution and agent integration as separate problems
  led to over-engineering (search paths, go:embed, extraction). The skill already lives in
  the project repo, is version-controlled, and is platform-specific by nature. koto's role
  is to compile templates and run workflows -- not to manage template distribution.
---

# DESIGN: koto Agent Integration

## Status

**Proposed**

## Context and Problem Statement

koto has a working state machine engine (v0.1.0) that enforces workflow progression through evidence-gated transitions and progressive disclosure. It's installable via GitHub Releases, an install script, and a tsuku recipe. But there's a gap between "koto is on PATH" and "an AI agent uses koto to run a workflow."

The gap isn't about template distribution or discovery protocols. It's simpler than that: the agent needs a skill (or equivalent integration file) that tells it koto exists, what commands to call, and which template to use. The skill is the natural integration unit because it bundles two things that always go together:

1. **The workflow template** -- the `.md` file that defines states, transitions, and evidence gates
2. **Agent instructions** -- how to call koto, what evidence keys to supply, how to interpret responses

These two pieces must travel together. A template without agent instructions means the agent doesn't know koto exists. Agent instructions without a template means there's nothing to run. The skill file solves both.

There are two distinct flows to support:

**Agent-driven flow.** An agent skill references a template file and tells the agent how to use koto. The user installs the skill into their project (by committing the skill files), and the agent follows the instructions. koto compiles and caches the template on first `koto init`, which it already does.

**Author-driven flow.** A user writing or editing a workflow template wants to validate it compiles correctly and has well-formed states, transitions, and gates. `koto template compile` already handles this. The feedback loop is: edit template, run `koto template compile`, fix errors, repeat.

### Scope

**In scope:**
- `koto generate` command to scaffold agent skill files from a template
- Stop hook generation for session continuity
- Claude Code skill structure (skill file + command file + hook)
- AGENTS.md generation for generic platform support
- Documenting the skill-as-distribution-unit pattern

**Out of scope:**
- Template search paths or built-in template embedding (not needed)
- Template registry or community sharing (future work)
- Human interaction UX beyond the author validation flow (separate design)
- MCP server integration

## Decision Drivers

- The agent skill is the natural integration unit -- it bundles template + agent instructions
- koto already compiles and caches templates on first use (no new compilation infrastructure needed)
- `koto template compile` already exists for the author validation flow
- Must work across agent platforms (Claude Code, Cursor, Codex, generic shell agents)
- Templates need stable filesystem paths -- the engine stores absolute paths in state files
- koto is a CLI tool, not a service -- no background process, no auto-discovery
- The solution should be minimal: don't build infrastructure for problems that agent platform conventions already solve

## Considered Options

### Decision 1: How Templates Reach Agents

The question is how a workflow template gets from "someone wrote it" to "an agent uses it in a project." The template must end up as a file in the project (koto init needs a filesystem path), and the agent must know it exists.

#### Chosen: Skill-as-Distribution-Unit

The agent skill file is the distribution mechanism. A koto workflow skill consists of:

```
.claude/skills/quick-task/
├── SKILL.md         # Agent instructions: how to call koto, evidence keys, response schemas
└── quick-task.md    # The workflow template itself
```

The skill's `SKILL.md` tells the agent: "When the user asks for a quick task, run `koto init --template .claude/skills/quick-task/quick-task.md --name <task-name> --var TASK='<description>'`, then call `koto next` and follow the directive."

The template lives alongside the skill file in the project repo. It's version-controlled, reviewable, and doesn't require any special distribution mechanism. koto compiles and caches it on first `koto init` (the cache lives at `~/.koto/cache/`), so subsequent operations are fast.

For non-Claude Code platforms:
- **Cursor/Codex**: The template still lives in the project. An AGENTS.md section or `.cursorrules` snippet provides the equivalent agent instructions.
- **Generic shell agents**: The template path is just a file path. Any agent with shell access can call `koto init --template <path>`.

This pattern works because agent platforms already have conventions for distributing instructions to agents. koto doesn't need to reinvent this -- it just needs to fit into it.

`koto generate <platform>` scaffolds these files from an existing template. Given a template path, it produces the platform-specific skill files with the correct koto CLI instructions, evidence key documentation extracted from the template, and response schema docs.

#### Alternatives Considered

**Embedded built-in templates with search path.** Ship templates in the koto binary via `go:embed`. Add a three-level search path (project, user, built-in) to resolve template names.
Rejected because it solves the wrong problem. Embedding a template in the binary doesn't help if nothing tells the agent to call koto. The search path adds complexity (shadowing, extraction, versioned directories) for a problem that the agent platform's own skill/config system already solves. The template must live in the project anyway (for version control and team sharing), so embedding it in the binary is redundant.

**Template registry with download-on-demand.** Templates pulled from a git-hosted registry when referenced by name.
Rejected because it adds a network dependency and registry infrastructure. The skill file pattern distributes templates through git (clone the repo, the skill files are there). No separate registry needed.

**koto init scaffolds a template.** `koto init` generates a starter template in the project directory.
Rejected as the primary flow because it conflates workflow initiation (creating a state file to start running) with template creation (authoring a new workflow). These are different actions by different people at different times. Template authoring is the author-driven flow; workflow initiation is the agent-driven flow.

### Decision 2: What koto generate Produces

Given that the skill is the distribution unit, what exactly should `koto generate` produce? The generator reads an existing template and scaffolds the platform-specific files around it.

#### Chosen: Platform-Specific Skill Scaffolds

`koto generate <platform> --template <path>` reads the template, extracts metadata (states, evidence gates, variables), and produces integration files.

**`koto generate claude-code --template <path>`** produces:

1. **Skill directory** with:
   - `SKILL.md`: Agent instructions including koto CLI reference, the execution loop (init → next → execute → transition → next), evidence keys extracted from the template's gate definitions, JSON response schemas, error handling, and resume behavior
   - A copy or symlink of the template file (so the skill is self-contained)

2. **Command file** (`.claude/commands/koto-run.md`): A `/koto-run` slash command humans use to trigger the workflow. Takes an optional task description. Wraps `koto init` + `koto next`.

3. **Hook entry** (merged into `.claude/hooks.json`): A Stop hook that checks for active koto state files and reminds the agent to continue. Uses `koto workflows` to detect active workflows regardless of state directory.

**`koto generate agents-md --template <path>`** produces a markdown section for AGENTS.md with the same information as the skill file but in a platform-agnostic format.

**Common behavior:**
- Generated files include a version comment: `<!-- Generated by koto vX.Y.Z from template-name -->`
- `--dry-run` previews output without writing
- Running again overwrites skill/command files; merges hook entries (replace koto's entry, preserve others)
- The template file is copied into the skill directory so the skill is self-contained

#### Alternatives Considered

**Generate only a skill file, reference template by path.** The skill points to the template elsewhere in the repo.
Rejected as the default because it creates a fragile cross-directory dependency. If the template moves, the skill breaks. Copying the template into the skill directory makes the skill self-contained. Users who want to share one template across multiple skills can use a relative path instead.

**No generate command -- users write skills manually.** Document the skill structure and let users author their own.
Rejected as the only approach because extracting evidence keys, gate types, and state machine structure from a template is tedious and error-prone. The generator automates the mechanical part while users customize the agent instructions. However, manual authoring remains a valid advanced path.

### Uncertainties

- **Hook effectiveness.** The Stop hook preventing agents from quitting mid-workflow has been validated with Claude Code but not broadly across versions and configurations.
- **Template copy vs reference.** Copying the template into the skill directory means two copies in the repo. For teams with many skills sharing one template, this could be annoying. A `--reference` flag (generating a path reference instead of a copy) is a straightforward future addition.
- **Evidence key extraction quality.** The generator extracts evidence keys from gate definitions, but the human-readable descriptions of what each key should contain come from the template author. If the template doesn't document its evidence keys well, the generated skill won't either.
- **`--evidence` flag doesn't exist yet.** The generated skill describes `koto transition --evidence key=value`, but this flag isn't implemented in v0.1.0. The skill should either note this as a future capability or the flag should be implemented before the first skill is generated. Evidence is currently only accessible via the Go library API.

## Decision Outcome

**Chosen: Skill-as-distribution-unit with koto generate for scaffolding**

### Summary

The agent skill is the distribution unit for koto workflows. A skill bundles the workflow template with platform-specific agent instructions. koto's job is to compile templates and run workflows -- not to distribute templates.

`koto generate <platform> --template <path>` scaffolds skill files from an existing template. For Claude Code, it produces a skill directory (SKILL.md + template copy), a `/koto-run` command file, and a Stop hook. For other platforms, it produces an AGENTS.md section. The generator extracts evidence keys, state names, and variable definitions from the template so the agent instructions are accurate.

The agent-driven flow works like this: the skill's SKILL.md tells the agent to run `koto init --template <path-to-template-in-skill-dir>`, then loop `koto next` / execute / `koto transition`. koto compiles and caches the template on first init (existing behavior). The Stop hook detects active state files and reminds the agent to continue if it tries to stop mid-workflow.

The author-driven flow is already supported: `koto template compile <path>` validates a template, reports errors, and outputs the compiled JSON. Authors edit their template, compile to check, fix errors, repeat. No new infrastructure needed for this flow.

### Rationale

The skill-as-distribution-unit pattern works because it aligns with how agent platforms already work. Claude Code has skills. Cursor has rules. Every platform has some mechanism for giving agents project-specific instructions. koto doesn't need its own distribution system -- it needs to fit into the ones that already exist.

This eliminates the search path, go:embed extraction, versioned directories, and template registry from the previous design iteration. Those solved a problem that doesn't exist: templates don't need their own distribution channel because the skill file already distributes them.

koto's compile-and-cache behavior (implemented in v0.1.0) handles the performance side. The first `koto init` compiles the template and stores the result in `~/.koto/cache/`. Subsequent operations load from cache. No new caching infrastructure needed.

### Trade-offs Accepted

- **Template duplication.** Copying the template into the skill directory means two copies if the template also lives elsewhere. Acceptable because skills should be self-contained, and the template is small (a few KB of markdown).
- **No built-in templates.** koto doesn't ship with a ready-to-use template embedded in the binary. Users need a skill file in their project before an agent will use koto. Acceptable because the `koto generate` command makes creating a skill fast, and the quick-task template will be published as a standalone file that anyone can download or copy.
- **Platform-specific generation.** Supporting a new agent platform means adding a generator target to koto. Acceptable because AGENTS.md covers the generic case, and dedicated generators are only needed for platforms with richer integration models (hooks, slash commands).
- **Generated files can drift.** When koto updates its CLI surface, generated skill files become stale. Version headers signal when regeneration is needed. No automatic sync.

## Solution Architecture

### Overview

The implementation adds one new package (`pkg/generate/`) and extends the CLI with the `koto generate` subcommand. The existing compile and cache infrastructure handles template processing. No new external dependencies.

### Components

```
cmd/koto/main.go              # Extended: generate subcommand
pkg/
├── generate/                  # NEW: skill scaffold generation
│   ├── generate.go            # Shared logic: template parsing, metadata extraction
│   ├── claudecode.go          # Claude Code: skill dir, command file, hook entry
│   └── agentsmd.go            # AGENTS.md section generation
├── template/compile/          # EXISTING: template compilation (used by generate)
├── cache/                     # EXISTING: compiled template caching
└── discover/                  # EXISTING: state file scanning (used by hook)
```

### Key Interfaces

**Skill Generation:**

```go
// generate.Generator scaffolds integration files from a template.
type Generator struct {
    TemplatePath string // path to the source template
    OutputDir    string // project root
    KotoVersion  string // for version headers
}

// ClaudeCode produces a skill directory, command file, and hook entry.
func (g *Generator) ClaudeCode() ([]GeneratedFile, error)

// AgentsMD produces an AGENTS.md section.
func (g *Generator) AgentsMD() ([]GeneratedFile, error)

type GeneratedFile struct {
    Path    string // relative path from project root
    Content []byte
    Action  string // "created" or "updated"
}

// TemplateMetadata holds extracted template info for skill generation.
// Implementation note: most fields map directly to CompiledTemplate fields.
// Consider using CompiledTemplate directly rather than copying into a new type.
type TemplateMetadata struct {
    Name        string
    Description string
    Version     string
    States      []StateInfo
    Variables   []VariableInfo
}

type StateInfo struct {
    Name        string
    Terminal    bool
    Transitions []string
    Gates       []GateInfo
}

type GateInfo struct {
    Name  string
    Type  string // "field_not_empty", "field_equals", "command"
    Field string
}

type VariableInfo struct {
    Name        string
    Description string
    Required    bool
    Default     string
}
```

### Data Flow

**Agent-driven flow (using a skill):**

```
1. User runs: koto generate claude-code --template my-workflow.md
   → Generator compiles the template, extracts metadata
   → Produces .claude/skills/my-workflow/SKILL.md + my-workflow.md
   → Produces .claude/commands/koto-run.md
   → Merges Stop hook into .claude/hooks.json
   → User commits all files to repo

2. Agent reads skill, user says "/koto-run fix the login bug"
   → Agent runs: koto init --template .claude/skills/my-workflow/my-workflow.md \
                   --name login-fix --var TASK="fix the login bug"
   → koto compiles template (or loads from cache), creates state file
   → Agent calls: koto next
   → koto returns JSON directive for current state
   → Agent executes directive, then: koto transition <state> --evidence key=value
   → Loop until koto returns {"action": "done"}

3. If agent session ends mid-workflow:
   → Next session: Stop hook runs koto workflows, detects active state file
   → Hook outputs: "Active koto workflow detected. Run koto next to continue."
   → Agent resumes with koto next
```

**Author-driven flow (writing a template):**

```
1. Author creates or edits my-workflow.md
2. Author runs: koto template compile my-workflow.md
   → Compiler validates YAML frontmatter, state declarations, transitions, gates
   → Reports errors and warnings to stderr
   → On success: outputs compiled JSON to stdout (or --output file)
3. Author fixes errors, repeats until clean
4. Author runs: koto generate claude-code --template my-workflow.md
   → Generates skill files for testing with an agent
5. Author tests the workflow end-to-end with an agent
```

### Generated Skill Content

**Claude Code Skill (`SKILL.md`) structure:**

```markdown
---
name: <workflow-name>
description: <from template frontmatter>
---

# <Workflow Name>

## When to Use

<Describes when the agent should use this workflow.
Placeholder for author customization.>

## koto CLI Reference

<Generated from koto's CLI surface. Documents init, next,
transition, query, status, rewind, cancel, validate.
JSON response schemas for each command.>

## Workflow States

<Generated from template. Lists each state, its transitions,
evidence gates, and what the agent should do in each state.>

## Evidence Keys

<Generated from template gates. Documents each evidence key,
its type (field_not_empty, field_equals, command), and what
value the agent should supply.>

## Execution Loop

1. Check for active workflows: `koto workflows`
2. If active, resume: `koto next`
3. If starting new: `koto init --template <path> --name <name> --var KEY=VALUE`
4. Get directive: `koto next`
5. Execute the directive
6. Transition: `koto transition <target> --evidence key=value`
7. Repeat from step 4 until done

## Error Handling

<Documents error response format, common error codes,
and recovery actions for each.>
```

**Stop Hook (`.claude/hooks.json`):**

```json
{
  "hooks": {
    "Stop": [
      {
        "type": "command",
        "command": "koto workflows 2>/dev/null | grep -q '\"path\"' && echo 'Active koto workflow detected. Run koto next to continue.'"
      }
    ]
  }
}
```

The hook runs `koto workflows` (which already outputs JSON by default) and checks whether the response contains workflow entries. If the output is an empty array `[]`, grep finds no match and the hook stays silent. If active workflows exist, their `"path"` fields trigger the match and the hook reminds the agent to continue.

**Hook merge strategy:** When merging into an existing `hooks.json`, `koto generate` identifies its own hook entry by matching commands that start with `koto workflows`. On regeneration, it replaces the matching entry. If no match exists, it appends to the `Stop` array (creating it if needed). If `hooks.json` doesn't exist or is invalid JSON, koto creates a fresh file.

## Implementation Approach

### Phase 1: Template Metadata Extraction

Build the foundation for generation:
- Create `pkg/generate/generate.go` with `TemplateMetadata` extraction from compiled templates
- Extract state names, transitions, gate definitions, and variable declarations
- This uses the existing `pkg/template/compile` package -- no new compilation logic

### Phase 2: Claude Code Generation

Build `koto generate claude-code`:
- Implement skill directory generation (SKILL.md + template copy)
- Implement command file generation (`/koto-run`)
- Implement hook merge logic (read existing hooks.json, insert/replace koto entry, preserve others)
- Support `--dry-run` flag
- Include version header comments in generated files

### Phase 3: AGENTS.md Generation

Build `koto generate agents-md`:
- Produce a markdown section with the same information as the skill file
- Suitable for appending to an existing AGENTS.md
- Platform-agnostic format

## Security Considerations

### Download Verification

**Not applicable.** This design doesn't download anything. Templates are local files in the project repo. The existing compile-and-cache path reads from the filesystem only. If a template registry with remote downloads is added in the future, this dimension would need revisiting.

### Execution Isolation

**Generated integration files.** `koto generate` writes files to the project directory. Skill files and command files are static text -- agents read them as instructions but they don't execute directly.

**Stop hook.** The generated hook invokes `koto workflows` on every Stop event. This executes the `koto` binary from PATH. The command is read-only (scans for state files, outputs JSON). Risk: a malicious `koto` binary earlier on PATH gets invoked automatically. Mitigation: the generated hook file is committed and reviewed via PR; `koto workflows` has no side effects.

**Template copy.** `koto generate` copies the template file into the skill directory and writes generated files to `.claude/`. All write targets follow the same symlink protection as engine state file writes: reject symlink targets at the destination to prevent arbitrary file overwrites. The symlink check (currently inline in `pkg/engine/engine.go`) should be extracted into a shared utility for use by both the engine and the generator.

### Supply Chain Risks

**Generated skill files.** The skill file, command file, and hook are generated from templates in the koto binary. A compromised koto binary could generate malicious files (e.g., a hook that exfiltrates data). Mitigation: generated files are committed to version control and reviewed via PR. The `--dry-run` flag lets users inspect output before writing.

**Template content.** The skill's SKILL.md includes content extracted from the template (state names, evidence keys, descriptions). A malicious template could embed instructions that influence agent behavior through the generated skill file. Mitigation: the generator structurally separates koto's authoritative CLI documentation from template-derived metadata, and the generated files are reviewed via PR before agents use them.

### User Data Exposure

**Generated files contain template metadata.** Skill files include state names, evidence key names, and variable descriptions from the template. No secrets, credentials, or source code are included.

**No network transmission.** All operations are local filesystem reads and writes.

### Mitigations

| Risk | Mitigation | Residual Risk |
|------|------------|---------------|
| Stop hook invokes `koto` from PATH on every Stop event | Hook is generated, committed, and reviewed via PR; `koto workflows` is read-only | Malicious `koto` earlier on PATH gets auto-invoked |
| Malicious template content in generated skill files | CLI docs separated from template metadata; files reviewed via PR | Agent may not respect structural boundary |
| Stale generated files after koto upgrade | Version header in generated files; `--dry-run` for inspection | No automated drift detection |
| Template copy destination is a symlink | Reject symlinks before writing (same guard as state file writes) | None identified |
| Compromised koto binary generates malicious files | Generated files committed and reviewed via PR | Reviewer doesn't catch malicious content |

## Consequences

### Positive

- Aligns with how agent platforms already work -- skills, rules files, AGENTS.md are established patterns
- No new distribution infrastructure (search paths, registries, go:embed extraction)
- Self-contained skills: the template travels with the agent instructions
- `koto generate` automates the tedious part (extracting evidence keys, state machines) while leaving agent instructions customizable
- Author-driven flow already works via `koto template compile` -- no changes needed
- The Stop hook addresses the most common failure mode (agents quitting mid-workflow)

### Negative

- No built-in templates in the binary -- users need to get a template file from somewhere before `koto generate` can scaffold a skill
- Template duplication: the template lives in the skill directory and possibly also in its original location
- Generated files drift when koto updates its CLI surface

### Mitigations

- The quick-task template will be published in koto's repo (`templates/quick-task.md`) and downloadable as a single file. `koto generate` can point at it directly
- Template duplication is a few KB of markdown; self-containment is more valuable than deduplication
- Version headers in generated files signal when regeneration is needed; a future `koto doctor` command could automate drift detection
