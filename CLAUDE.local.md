# koto

Workflow orchestration engine for AI coding agents. Enforces execution order through a state machine, persists progress atomically, and makes every state transition recoverable.

## Repo Visibility: Public

This is a public repository. Content should be written for external consumption:
- **Design docs**: Focus on external audience clarity
- **Issues/PRs**: Polished language, avoid internal references
- **Code comments**: Clear for open-source contributors
- **Commits**: Follow conventional commits without internal context

## Default Scope: Tactical

This repo is for tactical planning. When running /tsukumogami:explore or /tsukumogami:plan here:
- Designs focus on "how to build it" (implementation)
- Issues are atomic, implementable work items
- Reference upstream strategic designs if applicable
- Link to specific commits/PRs that implement each issue

Override with `--strategic` when doing product-focused work (e.g., major architecture RFC).

## Repository Structure

```
koto/
├── cmd/koto/        # CLI entry point
├── internal/        # Internal packages
├── pkg/             # Public Go library
│   ├── cache/       # Cache layer
│   ├── controller/  # Workflow controller
│   ├── discover/    # Template discovery
│   ├── engine/      # Core state machine engine
│   └── template/    # Template parsing and compilation
├── plugins/         # Agent skill plugins
├── docs/            # Documentation and guides
└── .github/         # CI/CD pipelines
```

## Quick Reference

```bash
# Build
go build -o koto ./cmd/koto

# Test
go test ./...

# Install locally
go install ./cmd/koto

# Lint
go vet ./...
```

## Key Commands

| Command | Description |
|---------|-------------|
| `koto init` | Initialize a workflow from a template |
| `koto next` | Get the current state directive |
| `koto transition <state>` | Advance to a new state |
| `koto status` | Check workflow status |
| `koto query` | Inspect full workflow state as JSON |
| `koto rewind` | Roll back to a previous state |
| `koto template compile` | Validate and compile a template |

## Environment

API keys and secrets are stored in `.local.env` at the repo root. Source this file when you need credentials (e.g., `GH_TOKEN`):

```bash
source .local.env
```

This file is gitignored and installed by the workspace `install.sh` script.

## Key Points

- All Go code must pass `gofmt` formatting
- CI runs tests and linting on every PR
- Templates are markdown files with YAML front-matter
- State files are written atomically to prevent corruption
