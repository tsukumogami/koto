# koto

Workflow orchestration engine for AI coding agents. koto enforces execution order through a state machine, persists progress atomically, and makes every state transition recoverable.

Agents call `koto next` to get their current directive and do the work. If something goes wrong, `koto rewind` rolls back to the previous state without losing the audit trail.

## Install

Download the latest binary for your platform from the [GitHub Releases page](https://github.com/tsukumogami/koto/releases).

Or build from source with Rust:

```bash
git clone https://github.com/tsukumogami/koto.git
cd koto
cargo build --release
# binary is at target/release/koto
```

## Quick start

### 1. Create a workflow template

Templates are markdown files with YAML front-matter. Each `##` heading defines a state, and `**Transitions**` lines define which states can follow.

```markdown
---
name: review
version: "1.0"
description: Code review workflow
variables:
  PR_URL: ""
---

## assess

Review the PR at {{PR_URL}} and summarize the changes.

**Transitions**: [feedback]

## feedback

Leave review comments on the PR.

**Transitions**: [done]

## done

Review complete.
```

States without transitions are terminal -- the workflow ends there.

### 2. Initialize a workflow

```bash
koto init review --template review.md
```

```json
{"name":"review","state":"assess"}
```

This creates a state file with three lines: a header (schema version, workflow name, template hash, timestamp), a `workflow_initialized` event, and an initial `transitioned` event.

### 3. Get the current directive

```bash
koto next review
```

```json
{"state":"assess","directive":"Review the PR at {{PR_URL}} and summarize the changes.","transitions":["feedback"]}
```

The `transitions` array shows which states can follow the current one.

> **Note:** `koto transition` (advancing the workflow) is not available in this release. Transitions will be added in a future version.

## Key concepts

**Templates** define the workflow: states, transitions between them, and directive text for each state. Variables (`{{KEY}}`) are interpolated into directives at runtime. Use `koto template compile` to validate templates during development and see the compiled JSON output.

**State files** (`koto-<name>.state.jsonl`) use an event log format. The first line is a header with the schema version, workflow name, template hash, and creation timestamp. Subsequent lines are typed events with monotonic sequence numbers and type-specific payloads. The current state is derived by replaying the log -- specifically, the `to` field of the last state-changing event.

**Template integrity**: The template's SHA-256 hash is locked at init time and stored in the first event. If the compiled template changes, `next` will fail. To update the template, reinitialize the workflow.

## Agent integration

AI coding agents can run koto workflows through the Claude Code plugin. Install it with two commands:

```
/plugin marketplace add tsukumogami/koto
/plugin install koto-skills@koto
```

The plugin ships with **hello-koto**, a minimal two-state skill that walks through the full loop: template setup, variable interpolation, command gates, and state transitions. Run `/hello-koto Hasami` to try it.

Once a skill is installed, the agent follows a simple cycle:

1. `koto init` -- start the workflow from a template
2. `koto next` -- get the current directive
3. Execute the work described in the directive
4. Repeat from step 2

The plugin also includes a Stop hook that detects active workflows when a session ends, so the agent can resume where it left off.

Skills use the [Agent Skills](https://agentskills.io) open standard, which means they work across Claude Code, Codex, Cursor, Windsurf, and other platforms that support it. For project-specific workflows, write a SKILL.md alongside your template in `.claude/skills/<name>/` and commit both to version control.

## Documentation

- [CLI usage guide](docs/guides/cli-usage.md) -- all subcommands with examples, including template authoring tools
- [Error code reference](docs/reference/error-codes.md) -- structured error codes and handling
- [Custom skill authoring](docs/guides/custom-skill-authoring.md) -- creating workflow skills for AI agents

## License

See [LICENSE](LICENSE).
