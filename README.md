# koto

Workflow orchestration engine for AI coding agents. koto enforces execution order through a state machine, persists progress atomically, and makes every state transition recoverable.

Agents call `koto next` to get their current directive, do the work, then call `koto transition` to advance. The engine validates each transition against the workflow template and rejects anything out of order. If something goes wrong, `koto rewind` rolls back to a previous state without losing the audit trail.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/tsukumogami/koto/main/install.sh | sh
```

Or with Go:

```bash
go install github.com/tsukumogami/koto/cmd/koto@latest
```

Or build from source:

```bash
git clone https://github.com/tsukumogami/koto.git
cd koto
go build -o koto ./cmd/koto
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
koto init --name review --template review.md --var PR_URL=https://github.com/org/repo/pull/42
```

```json
{"state":"assess","path":"wip/koto-review.state.json"}
```

### 3. Get the current directive

```bash
koto next
```

```json
{"action":"execute","state":"assess","directive":"Review the PR at https://github.com/org/repo/pull/42 and summarize the changes."}
```

### 4. Advance to the next state

```bash
koto transition feedback
```

```json
{"state":"feedback","version":2}
```

### 5. Check workflow status

```bash
koto status
```

```
Workflow: review
State:    feedback
History:  1 entries
```

### 6. Inspect full state

```bash
koto query
```

Returns the complete state as JSON, including workflow metadata, variables, and transition history.

## Key concepts

**Templates** define the workflow: states, transitions between them, and directive text for each state. Variables (`{{KEY}}`) are interpolated into directives at runtime. Use `koto template compile` to validate templates during development and see the compiled JSON output.

**State files** (`koto-<name>.state.json`) track progress. They're written atomically -- a crash mid-write can't corrupt them. A version counter detects concurrent modifications.

**Template integrity**: The template's SHA-256 hash is locked at init time. If someone modifies the template mid-workflow, every operation fails with `template_mismatch`. To change the template, cancel and restart.

## Documentation

- [CLI usage guide](docs/guides/cli-usage.md) -- all subcommands with examples, including template authoring tools
- [Go library guide](docs/guides/library-usage.md) -- using `pkg/engine` as an imported package
- [Error code reference](docs/reference/error-codes.md) -- structured error codes and handling

## License

See [LICENSE](LICENSE).
