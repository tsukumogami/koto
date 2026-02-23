# Architect Review: DESIGN-koto-agent-integration.md

Reviewer: architect-reviewer
Date: 2026-02-23
Document: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/docs/designs/DESIGN-koto-agent-integration.md`

## 1. Problem Statement Specificity

The problem statement identifies three gaps (template distribution, agent integration, workflow discovery) and correctly argues they're interdependent. The specificity is adequate for evaluating solutions.

One weakness: the problem statement conflates two distinct user journeys without separating them:

- **Human sets up koto in a project** (runs `koto generate`, commits files, writes/selects templates). This is a one-time setup action.
- **Agent discovers and uses koto during execution** (finds active workflows, reads skill files, runs the execution loop). This is a recurring runtime action.

The design addresses both, but the problem statement doesn't distinguish them. This matters because the evaluation criteria differ: for setup, "how many steps?" matters; for runtime, "how many CLI calls per loop iteration?" matters. The Decision Drivers section partially addresses this ("first-run experience" vs "start new workflow" and "resume active workflow") but the problem statement itself doesn't draw the line.

**Verdict**: Specific enough to evaluate solutions. The interdependency argument is the strongest part. Could be sharper about the setup-vs-runtime distinction, but this doesn't block evaluation.

## 2. Missing Alternatives

### 2a. MCP Server (explicitly scoped out, worth revisiting the rationale)

The design says MCP is out of scope without explanation beyond "koto is a CLI tool, not a service." This is a constraint the design asserts, not a constraint that exists. MCP would solve discovery and agent integration in a single mechanism: the agent already knows how to discover and call MCP tools. No skill file needed. No generation step.

The real argument against MCP should be stated: it requires a background process, adds a dependency on the MCP protocol, and limits koto to agents that support MCP (which is most of them, but not all shell-based agents). These are legitimate trade-offs. The current rejection is a tautology ("koto is a CLI, therefore it can't be a service").

This doesn't need to become the chosen option, but the rejection rationale should be explicit and fair.

### 2b. Template-as-dependency (go:embed alternative)

The design considers downloading templates on demand and scaffolding, but not a third pattern: templates as a separate installable artifact. For example, `koto install-template quick-task` could pull a template from a GitHub release asset or a Git repository tag, placing it in `~/.koto/templates/`. This separates the template versioning lifecycle from the binary versioning lifecycle.

The chosen approach (go:embed) locks template versions to binary versions. When a template bug is found, users must wait for a new koto release. This is probably fine for v0.1, but it's worth naming the trade-off explicitly.

### 2c. Agent integration via CLAUDE.md / project instructions only

The design assumes generated files are necessary, but Claude Code already reads `CLAUDE.md` for project-specific instructions. A template could declare agent instructions in its frontmatter (or a dedicated section), and `koto next` could include those instructions in its JSON output. The agent would learn about koto from the directive itself, not from a pre-generated skill file.

This isn't a complete replacement (it doesn't help with the initial "should I use koto?" decision), but it's a simpler alternative for the ongoing execution loop. The design rejects "template-embedded agent instructions" but the rejection says "agent instructions and state machine definitions serve different audiences." That's true for the template source file, but the compiled output already separates directive text from state machine structure. The real question is whether the skill file is needed at all if `koto next` output is self-describing enough.

### 2d. Hook alternatives beyond Stop

The design proposes a Stop hook to prevent mid-workflow abandonment. It doesn't consider:

- A **PreToolUse** hook on file write operations that checks whether the agent is following the current directive
- A **PostToolUse** hook after shell commands that auto-checks evidence gates
- A **SubagentCompleted** hook that captures evidence from sub-agent results

These may be over-engineering for v0.1, but they should be named as future work or explicitly deferred, since the hook mechanism is the one piece of active (non-static) integration.

## 3. Rejection Rationale Fairness

### Decision 1 (Template Distribution): Fair

The three rejected alternatives (download-on-demand, scaffold-only, explicit paths only) are genuine alternatives with clear reasons for rejection. No strawmen.

### Decision 2 (Agent Integration): Mostly fair, one weak rejection

- **PATH probing rejection**: Fair. Agents don't reverse-engineer CLI surfaces.
- **Single universal file rejection**: Fair. Platform capabilities genuinely differ.
- **Template-embedded instructions rejection**: Weak. The argument "the template is for koto; agent instructions are for the LLM" ignores that the compiled template already separates these concerns. The directive text IS agent instructions -- it's literally what the LLM reads. The rejection would be stronger if it said: "Template-embedded instructions can't cover initial discovery (whether to use koto at all), hook behavior (preventing abandonment), or response schema documentation (how to parse koto's JSON output). These require a separate artifact."

### Decision 3 (Workflow Discovery): Fair, but the rejection of separate commands is fragile

The rejection says "forcing two calls when one suffices adds friction." But the combined response creates a conceptual coupling: templates (static, structural) and active workflows (dynamic, instance-level) are different kinds of data. Combining them in one response means agents must always parse both, and future extensions (e.g., paused vs active workflows, template recommendations) bloat a single endpoint.

More importantly, the design acknowledges `koto template list` still exists as a separate command. So the "one call" argument is weakened: now there are two ways to list templates (standalone and as part of `koto workflows`). This is a mild case of CLI surface duplication.

## 4. Unstated Assumptions

### 4a. Agents read skill files before any CLI invocation

The entire integration model assumes the agent reads `.claude/skills/koto.md` at session start, before encountering any koto state file. If the agent only reads skills on demand (or not at all, in platforms without skill support), the execution loop documentation is never consumed. The AGENTS.md fallback partially addresses this, but only if agents proactively read AGENTS.md.

### 4b. Template extraction to ~/.koto/templates/ is equivalent to template at original path

The design says built-in templates are extracted to `~/.koto/templates/` and the engine stores the absolute path. But if two different projects both use the built-in `quick-task` template, they share a single extracted file at `~/.koto/templates/quick-task.md`. If one project's koto binary is v0.2 (different template) and another is v0.1, the extraction is not safe. The design says extraction is "idempotent: if the file already exists with the same content, it's left alone." What happens when the content differs? This needs to be specified.

Possible strategies: overwrite always (last binary wins, breaks other project's hash), overwrite never (stale template persists), version the filename (`quick-task-v1.0.md`). Each has different consequences for the engine's template hash verification.

### 4c. The Stop hook glob pattern is stable

The hook uses `ls wip/koto-*.state.json`. This assumes:
- State files always live in `wip/` (but `--state-dir` allows override)
- State files always match `koto-*.state.json` (matches current `discover.Find` pattern)

If a user configures `--state-dir build/states/`, the hook won't detect active workflows. The generated hook should probably use the same state directory that `koto generate` was configured with, not a hardcoded path.

### 4d. Generated files are the right granularity for Claude Code

The design generates a single skill file (`.claude/skills/koto.md`). Claude Code skills are loaded into the agent's context at session start. A large skill file consumes context window budget even when the agent isn't running a koto workflow. An alternative is a smaller "discovery" skill that tells the agent koto exists, with the full execution loop documentation delivered via `koto next` output.

### 4e. One built-in template is sufficient for the first release

The design ships one template (`quick-task`). If this template doesn't match the user's workflow structure, there's no built-in alternative. The user must write a custom template from scratch. This is acknowledged in the design ("Phase 4: Quick-Task Template") but the assumption that one template proves the concept deserves explicit statement.

## 5. Strawman Analysis

No option is a strawman. All three rejected alternatives in each decision are plausible approaches that real projects use. The weakest rejection (template-embedded instructions in Decision 2) still has valid points; it just doesn't articulate the strongest version of the argument.

## 6. Architectural Fit with Existing Codebase

### 6a. pkg/registry/ -- new package, correct placement

The proposed `pkg/registry/` package handles template name resolution. This fits the existing `pkg/` public API pattern. It doesn't import any higher-level package (it's at the same level as `pkg/discover/`). The dependency direction is clean: `cmd/koto/main.go` -> `pkg/registry/` -> (filesystem only).

One concern: `pkg/registry/` and `pkg/cache/` both deal with `~/.koto/` subdirectories and both resolve `KOTO_HOME`. There should be a shared function for resolving the koto home directory rather than duplicating the `KOTO_HOME` -> `~/.koto/` fallback logic. The design doesn't address this.

### 6b. pkg/generate/ -- new package, correct placement but has an inversion risk

The proposed `pkg/generate/` package produces agent integration files. It needs to read template metadata (from the registry) and koto's version string. The design shows it importing `pkg/registry/`. This is fine.

The risk is that the generated skill file content must describe koto's CLI commands and JSON response schemas. If this content is hardcoded strings in `pkg/generate/`, it will drift from the actual CLI surface in `cmd/koto/main.go`. The design acknowledges drift ("Generated file drift" in Uncertainties) but doesn't propose a structural solution. One option: the skill file template could be `go:embed`'d from a file that's co-located with the CLI code and tested against actual CLI output.

### 6c. koto workflows extension -- fits existing pattern

The existing `cmdWorkflows()` calls `discover.Find()` and prints JSON. Extending it to include templates from the registry is a natural addition. The `--json` flag proposal is consistent with the non-TTY detection pattern mentioned in the design (though the current codebase always outputs JSON, so this flag is actually a no-op until human-readable formatting is added).

### 6d. Template extraction creates a new filesystem contract

Currently the engine's template path contract is simple: `template_path` in the state file points to the user-provided path. With extraction, a new implicit contract appears: `~/.koto/templates/<name>.md` is managed by koto and must not be modified by the user (since the engine verifies hashes). This contract is mentioned but not enforced. There's no mechanism to detect user modification of an extracted template (the engine's hash check would catch it at transition time, but the error message would be confusing -- "template mismatch" when the user didn't change anything, they just ran a different koto version).

### 6e. cmdInit modification -- backward compatible

The design preserves backward compatibility: explicit paths (containing `/` or `\`) bypass the registry. Name-only values go through the search path. This heuristic is simple and correct for the common case. Edge case: a template file in the current directory like `my-template.md` (no path separator) would be treated as a name lookup. The design should clarify: does name resolution check for `.md` extension?

## 7. Summary of Findings

### Must Address Before Implementation

1. **Template extraction conflict**: What happens when two koto versions try to extract different content to the same `~/.koto/templates/<name>.md` path? The "idempotent extraction" claim is under-specified for the version-mismatch case.

2. **Stop hook hardcodes state directory**: The hook should respect the configured state directory, not hardcode `wip/`. Otherwise `--state-dir` users get no hook protection.

3. **MCP rejection needs real rationale**: "koto is a CLI, not a service" is circular. State the actual trade-offs (background process, protocol dependency, platform support).

### Should Address (Advisory)

4. **KOTO_HOME resolution is duplicated**: `pkg/cache/` already resolves `KOTO_HOME`; `pkg/registry/` will need the same logic. Extract a shared helper.

5. **Generated skill file drift has no structural guard**: Consider co-locating the skill template with CLI code, or testing the generated content against actual CLI output.

6. **Template-embedded instructions rejection**: Strengthen the argument. The real issue is that directive text can't cover initial discovery, response schemas, or hook behavior -- not that templates "serve different audiences."

7. **koto workflows combines conceptually distinct data**: Templates (structural) and active workflows (instance) in one response creates mild CLI surface duplication with `koto template list`.

### Explicitly Deferred (Acknowledged, Not Blocking)

8. One built-in template for v0.1 is a reasonable scope constraint.
9. AGENTS.md as lowest-common-denominator is acceptable with dedicated generators for richer platforms.
10. Hook-based agent nudging is Claude Code specific; other platforms work without it.
