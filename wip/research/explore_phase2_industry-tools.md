# Industry Tool Research: Workflow Templates and State Management

Research conducted 2026-02-22. Focused on how state-of-the-art AI agent orchestration tools and production workflow engines handle workflow templates, state storage, and execution control.

## 1. Beads (steveyegge/beads)

**What it is**: A distributed, git-backed graph issue tracker designed for AI agents. Provides persistent structured memory for coding agents, replacing loose markdown plans with a dependency-aware graph.

### Template / Definition Format

Beads does not use workflow templates in the koto sense. It tracks individual tasks (issues) with dependencies between them, not ordered state machines. Tasks are defined via CLI commands:

```bash
bd create "Title" -p 0        # create a P0 task
bd dep add <child> <parent>    # link task dependencies
```

Tasks live in JSONL format (`.beads/issues.jsonl`), one JSON object per line. Each issue has: id (hash-based like `bd-a1b2`), title, description, status, priority, issue_type, timestamps, and dependencies.

Hierarchical IDs support epics: `bd-a3f8` (epic), `bd-a3f8.1` (task), `bd-a3f8.1.1` (subtask).

### State Storage

**Two-layer model**: Primary data lives in a Dolt database (version-controlled SQL) at `.beads/dolt/`. JSONL is maintained as a git-portable mirror via git hooks. Configuration lives in `.beads/config.yaml`.

- `.beads/` directory in the project root
- Dolt database is the source of truth (SQLite-like, but with cell-level merge)
- JSONL exported for git portability
- Optional "stealth mode" (`bd init --stealth`) stores locally without committing
- Contributor mode routes planning issues to `~/.beads-planning/` (separate from upstream repo)

### State Portability

Git-native distribution. JSONL travels with the repo via `git push/pull`. Auto-import detects when JSONL is newer than the local database and merges. Hash-based IDs prevent merge collisions across branches and agents. Content hashing enables idempotent imports (same hash = skip, different hash = update).

### Evidence / Gate / Condition Mechanisms

No formal evidence gates. Dependency tracking (`bd dep add`) prevents starting tasks whose blockers are incomplete. `bd ready` lists tasks with all dependencies satisfied. The "land the plane" workflow enforces manual quality gates: run lint, run tests, push to remote -- but these are documented conventions, not enforced by the tool.

### Key Takeaway for koto

Beads solves a different problem (task tracking, not workflow enforcement) but its git-based state portability is well-executed. The JSONL-per-entity format avoids merge conflicts. The hash-based ID system enables concurrent multi-agent work without coordination. Worth noting: Beads uses a daemon with debounced flush for write performance -- koto's simpler single-file atomic write is appropriate for its lower write frequency.

---

## 2. Claude Code

**What it is**: Anthropic's CLI agent for code development. Uses CLAUDE.md files for persistent context, skills/slash commands for reusable workflows, and hooks for lifecycle automation.

### Template / Definition Format

Claude Code has three mechanisms that function as workflow templates:

**Skills** (`.claude/skills/<name>/SKILL.md`): Markdown files with YAML frontmatter. The body contains natural language instructions that Claude follows. Frontmatter configures behavior:

```yaml
---
name: deploy
description: Deploy the application to production
context: fork            # run in isolated subagent
disable-model-invocation: true  # manual trigger only
allowed-tools: Bash(npm *)
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: command
          command: "./scripts/security-check.sh"
---
Deploy the application:
1. Run the test suite
2. Build the application
3. Push to the deployment target
```

Skills support argument substitution (`$ARGUMENTS`, `$0`, `$1`), dynamic shell injection (`!`command``), and supporting files (templates, examples, scripts in the skill directory).

**Hooks** (`.claude/settings.json`): JSON configuration that fires shell commands, LLM prompts, or agent evaluations at lifecycle events. 17+ event types including `PreToolUse`, `PostToolUse`, `Stop`, `TaskCompleted`, `SessionStart`, `SessionEnd`. Hooks can block actions (exit code 2), inject context, modify tool inputs, or run async background processes.

```json
{
  "hooks": {
    "PreToolUse": [{
      "matcher": "Bash",
      "hooks": [{
        "type": "command",
        "command": ".claude/hooks/block-rm.sh"
      }]
    }]
  }
}
```

**CLAUDE.md** files: Hierarchical context files loaded at global (`~/.claude/CLAUDE.md`), project (`.claude/CLAUDE.md`), and directory levels. Provide persistent instructions without being "workflows" per se.

### State Storage

Session state lives in `~/.claude/projects/<project_hash>/`. Transcripts are stored as JSONL files. No cross-session workflow state -- each session is independent unless explicitly resumed (`--resume`, `--continue`). The `/compact` feature compresses context when the window fills.

Hooks receive `session_id` and `transcript_path` as input context. There's no persistent state file equivalent to koto's `*.state.json`.

### State Portability

Session state is local to the machine (stored under `~/.claude/`). Skills and hooks are portable via the project's `.claude/` directory (committed to git). Global settings at `~/.claude/settings.json` are machine-local.

### Evidence / Gate / Condition Mechanisms

**Hooks as gates**: `PreToolUse` hooks can deny tool calls. `Stop` hooks can force Claude to continue working (block stopping). `TaskCompleted` hooks can reject completion with feedback. These serve as enforceable quality gates:

```bash
# TaskCompleted hook that blocks completion if tests fail
if ! npm test 2>&1; then
  echo "Tests not passing" >&2
  exit 2  # blocks task completion
fi
```

**Prompt and agent hooks**: LLM-evaluated gates where a model decides allow/block based on context:

```json
{
  "type": "agent",
  "prompt": "Verify that all unit tests pass. Run the test suite and check the results. $ARGUMENTS",
  "timeout": 120
}
```

### Key Takeaway for koto

Claude Code's skill system (markdown with YAML frontmatter) is the closest analog to koto templates, but skills are instruction sets rather than state machines. The hooks system is directly relevant to koto's planned evidence gates: hooks demonstrate that gates can be shell commands (deterministic), LLM prompts (single-turn evaluation), or agents (multi-turn with tool access). The three-type gate classification (command/prompt/agent) maps well to evidence gate types koto might support.

The `TaskCompleted` hook pattern (block completion via exit code 2, feed stderr back as feedback) is a clean model for evidence gates that need to reject with explanation.

---

## 3. Gemini CLI (google-gemini/gemini-cli)

**What it is**: Google's open-source terminal AI agent, providing access to Gemini models with built-in tools, MCP support, and conversation checkpointing.

### Template / Definition Format

**Custom commands** (`.gemini/commands/<name>.toml`): TOML files with a `prompt` field and optional `description`. Subdirectories create namespaced commands (`git/commit.toml` becomes `/git:commit`).

```toml
description = "Generates a fix for a given issue."
prompt = "Please provide a code fix for the issue described here: {{args}}."
```

Commands support argument injection (`{{args}}`), shell command injection (`!{command}` blocks with auto-escaping), and security confirmation before shell execution.

**GEMINI.md** context files: Hierarchical system identical in concept to CLAUDE.md. Global (`~/.gemini/GEMINI.md`), workspace, and JIT (just-in-time, discovered when tools access files). Supports `@file.md` imports for modularization. Configurable filename (`settings.json` can set `context.fileName` to look for `AGENTS.md`, `CONTEXT.md`, etc.).

### State Storage

**Checkpointing** (opt-in via `settings.json`): Before file modifications, creates a checkpoint containing:
- A git snapshot in a shadow repository at `~/.gemini/history/<project_hash>`
- Conversation history saved as JSON in `~/.gemini/tmp/<project_hash>/checkpoints`
- The tool call that triggered the checkpoint

Checkpoints enable `/restore` to revert both files and conversation state. This is version-control for agent state, not workflow progression.

**Session state** is local, stored under `~/.gemini/`. No persistent workflow state across sessions.

### State Portability

Commands (`.gemini/commands/`) are project-portable (committable to git). Session state and checkpoints are machine-local under `~/.gemini/`. The shadow git repo for checkpointing is deliberately separate from the project's own git history.

### Evidence / Gate / Condition Mechanisms

No evidence gates or workflow enforcement. Security confirmations exist for shell commands in custom commands. Gemini CLI is a conversational tool, not a workflow engine.

### Key Takeaway for koto

Gemini CLI's custom commands use TOML, showing that simple definition formats work. The checkpointing system (shadow git repo + conversation JSON) is interesting for state preservation but fundamentally different from koto's needs. The `@file.md` import syntax for GEMINI.md is a pattern worth considering for koto template modularity if templates grow large.

---

## 4. TaskMaster AI (eyaltoledano/claude-task-master)

**What it is**: An AI-powered task management system designed for AI-driven development workflows, with MCP integration for Cursor, VS Code, and Claude Code.

### Template / Definition Format

Tasks are defined in JSON at `.taskmaster/tasks/tasks.json`. The structure uses **tagged task lists** (v0.17+) for multi-context management:

```json
{
  "master": {
    "tasks": [
      {
        "id": 1,
        "title": "Setup Express Server",
        "description": "Initialize and configure Express.js server",
        "status": "pending",
        "dependencies": [],
        "priority": "high",
        "details": "Create Express app with CORS, body parser...",
        "testStrategy": "Start server and verify health check...",
        "subtasks": [
          {
            "id": 1,
            "title": "Configure middleware",
            "status": "pending",
            "dependencies": []
          }
        ]
      }
    ]
  },
  "feature-branch": {
    "tasks": [...]
  }
}
```

Tags organize tasks into separate contexts (branches, environments, project phases). Task statuses: `pending`, `in-progress`, `done`, `review`, `deferred`, `cancelled`.

Individual task files also exist as plain text at `.taskmaster/tasks/task_NNN_<tag>.txt`.

Tasks are generated from a PRD (Product Requirements Document) at `.taskmaster/docs/prd.txt` via AI parsing. A complexity analysis system scores tasks 1-10 and recommends expansion into subtasks.

### State Storage

All state lives in the `.taskmaster/` project directory:
- `config.json`: model configuration, global settings, project defaults
- `state.json`: runtime state (current tag, last switch time, migration notices, exported brief metadata)
- `tasks/tasks.json`: the full task graph
- `tasks/task_NNN_<tag>.txt`: individual task files
- `templates/`: PRD templates
- `docs/`: PRDs and documentation

### State Portability

Fully project-directory based. Everything under `.taskmaster/` can be committed to git. Tags enable branch-specific task contexts. Manual git integration via `--from-branch` flag (no automatic branch-tag switching). State migration from legacy format is automatic.

### Evidence / Gate / Condition Mechanisms

**Dependency enforcement**: Tasks track `dependencies` (array of prerequisite task IDs). The `next` command finds tasks with all dependencies satisfied. Tasks with pending dependencies are blocked.

**Complexity analysis**: AI-driven scoring recommends which tasks need expansion. Not an enforcement gate, but a planning aid.

**No runtime evidence gates**: Status transitions (pending -> in-progress -> done) are manual. No verification that work was actually completed before marking done.

### Key Takeaway for koto

TaskMaster demonstrates that JSON-based task definitions in the project directory work well for AI agent workflows. The tagged task lists system (multi-context management per branch/environment) is a pattern koto could adapt for multi-workflow scenarios. The dependency system is simpler than a state machine (DAG of tasks vs. explicit state transitions) but the core concept maps. TaskMaster's project-directory state storage (`.taskmaster/`) validates koto's `wip/` approach.

The PRD-to-tasks generation pattern is interesting: users write intent in natural language, AI decomposes into structured tasks. koto's template system serves a similar decomposition role but with human-authored state machine definitions.

---

## 5. GitHub Actions

**What it is**: GitHub's CI/CD platform using YAML workflow definitions. The most widely adopted declarative workflow format in the developer ecosystem.

### Template / Definition Format

YAML files in `.github/workflows/`. Three-level structure:

```yaml
name: CI Pipeline
on:
  push:
    branches: [main]
  pull_request:
    types: [opened, synchronize]

jobs:
  test:
    runs-on: ubuntu-latest
    outputs:
      coverage: ${{ steps.cov.outputs.value }}
    steps:
      - name: Run tests
        id: cov
        run: |
          npm test
          echo "value=$(cat coverage.txt)" >> $GITHUB_OUTPUT

  deploy:
    needs: [test]
    if: ${{ github.ref == 'refs/heads/main' }}
    runs-on: ubuntu-latest
    steps:
      - name: Deploy
        run: npm run deploy
```

Key structural elements:
- **Triggers** (`on`): event-driven activation with filters (branches, paths, types)
- **Jobs**: parallel by default, sequential via `needs` keyword
- **Steps**: sequential within a job
- **Conditions** (`if`): expressions using contexts (`github`, `inputs`, `needs`)
- **Outputs**: explicit declaration, passed between steps via `$GITHUB_OUTPUT` and between jobs via `needs.<job>.outputs`
- **Matrix strategies**: generate job variations across dimensions (OS, language version)
- **Inputs** (`workflow_dispatch`): typed parameters with descriptions and defaults
- **Concurrency**: grouping with cancel-in-progress option

### State Storage

State is managed by GitHub's infrastructure, not local files. Workflow run state includes: run ID, status (queued/in_progress/completed), conclusion (success/failure/cancelled), job statuses, step outputs, and artifacts. Accessible via API (`gh api`) but not designed for local manipulation.

### State Portability

Workflow definitions (.yaml files) are fully portable via git. Execution state is bound to GitHub's infrastructure. Secrets are environment-specific and non-portable. Reusable workflows (`workflow_call`) enable composition across repositories.

### Evidence / Gate / Condition Mechanisms

**Conditional execution** (`if`): Expressions that evaluate against context (commit info, branch, previous job results, manual inputs). Both jobs and individual steps can be conditional.

**Job dependencies** (`needs`): Creates a DAG of job execution. Failed dependencies block dependent jobs (unless overridden with `always()` or `if: failure()`).

**Environment protection rules**: Required reviewers, wait timers, deployment branches. External to YAML, configured in repo settings.

**Status functions**: `success()`, `failure()`, `cancelled()`, `always()` for conditional flow based on prior outcomes.

**Concurrency control**: Prevents parallel runs in the same group; optionally cancels in-progress runs.

### Key Takeaway for koto

GitHub Actions provides the most mature model for declarative workflow conditions. The `if` expression syntax is clean and composable. The `needs` keyword for job dependencies maps directly to koto's state transition concept (a state's transitions list is analogous to what a job's `needs` unlocks). The `outputs` mechanism for passing data between jobs is relevant to koto's evidence model -- evidence from one state could be passed forward to condition entry into subsequent states.

The three-level hierarchy (workflow > jobs > steps) mirrors koto's planned progression: workflow templates define states, states contain directives (like jobs contain steps), and evidence gates condition transitions (like `if` conditions control execution).

GitHub Actions' typed inputs (`workflow_dispatch.inputs`) with description, type, and choices is a clean model for koto template variables.

---

## 6. Production Workflow Engines (Temporal, Prefect, Dagster)

### Temporal.io

**Definition format**: Code-based (Go, Java, TypeScript, Python). Workflows are functions with a `workflow.Context` parameter. Activities are separate functions for side effects.

**State management**: Event sourcing via an Event History. The runtime replays the entire history to reconstruct state after crashes. Deterministic execution is mandatory -- workflows cannot use real clocks, random numbers, or direct I/O.

**Key patterns**:
- **Durable execution**: Workflows survive infrastructure failures by replaying from event history
- **Signals**: Asynchronous writes to a running workflow (fire-and-forget external input)
- **Queries**: Read-only state observation without modifying history
- **Updates**: Synchronous tracked writes with acknowledgment
- **Struct-based parameters**: Recommended for forward compatibility (adding fields without breaking signatures)

**Relevance to koto**: Temporal's signal/query/update model maps to how external processes interact with a koto workflow. `koto transition` is like a signal (advancing state from outside). `koto query` / `koto status` are like queries (observing state without modification). The event history concept validates koto's transition history design. Temporal's deterministic execution requirement parallels koto's principle that the engine (not the agent) is the authority on workflow progression.

### Prefect

**Definition format**: Python decorators (`@flow`, `@task`). Native Python control flow (if/else, loops) rather than a static DAG.

**State management**: Tracks success, failure, and retry states per task. Can resume from the last successful checkpoint. Caches task results to prevent redundant work.

**Key patterns**:
- **Dynamic execution**: Workflows adapt at runtime based on data (no pre-planned DAG required)
- **Task mapping**: Create tasks dynamically based on actual data
- **State-based chaining**: Workflows chain based on states, conditions, or custom logic

**Relevance to koto**: Prefect's task result caching (preventing redundant work on resume) is conceptually close to koto's state persistence. Prefect proves that simple decorator-based definitions can drive sophisticated execution control. For koto, the analog is that simple YAML state definitions in a template header can drive the same execution control without code.

### Dagster

**Definition format**: Python decorators on functions. Four decorator types: `@asset` (single output), `@multi_asset` (multiple outputs), `@graph_asset` (ops composed into one asset), `@graph_multi_asset`. Dependencies declared via function parameters or `deps` argument.

**State management**: Asset materialization tracking. `code_version` tags detect when assets need re-materialization. Execution context provides access to system information.

**Key patterns**:
- **Asset-centric model**: Focus on what gets produced, not the task sequence
- **Dependency inference**: The framework infers execution order from function signatures
- **Code versioning**: Track whether code changed since last materialization

**Relevance to koto**: Dagster's asset-centric model is an interesting counterpoint. While koto tracks "where are we in the process" (state-centric), Dagster tracks "what has been produced" (output-centric). Evidence gates bridge these: checking that an artifact exists (output-centric) before allowing state advancement (state-centric). Dagster's `code_version` for change detection parallels koto's template hash for template integrity.

### Collective Takeaways from Workflow Engines

The three engines validate several of koto's design choices:
- **Event history / audit trail**: All three track execution history. koto's transition history is the right pattern.
- **Deterministic execution authority**: Temporal enforces it via replay; koto enforces it via the engine validating transitions.
- **Resume from interruption**: All three handle crash recovery. koto's atomic persistence + state file achieves the same for a file-based tool.
- **Typed parameters**: Temporal recommends struct-based parameters; koto uses template variables. Both avoid brittle positional arguments.

Key difference: all three engines are server-based (database-backed state, network APIs). koto achieves the same execution guarantees with file-based state and CLI invocations. This is a significant simplification for the target use case (single-agent development workflows).

---

## Cross-Cutting Analysis

### Template Format Comparison

| Tool | Format | Location | Structure |
|------|--------|----------|-----------|
| Beads | JSONL (generated) | `.beads/` in project | Flat task list with graph links |
| Claude Code Skills | Markdown + YAML frontmatter | `.claude/skills/` | Instructions + config |
| Gemini CLI Commands | TOML | `.gemini/commands/` | Prompt + description |
| TaskMaster | JSON | `.taskmaster/tasks/` | Tagged task lists with DAG |
| GitHub Actions | YAML | `.github/workflows/` | Triggers > Jobs > Steps |
| Temporal | Go/Python/TS code | Application code | Functions + decorators |
| Prefect | Python + decorators | Application code | @flow/@task functions |
| Dagster | Python + decorators | Application code | @asset decorators |

### State Storage Comparison

| Tool | Primary Storage | Portability | Cross-Machine |
|------|----------------|-------------|---------------|
| Beads | Dolt DB + JSONL in `.beads/` | Git-native via JSONL | Yes, via git push/pull |
| Claude Code | `~/.claude/projects/` | Machine-local | No |
| Gemini CLI | `~/.gemini/` | Machine-local | No |
| TaskMaster | `.taskmaster/` in project | Git-portable | Yes, via git |
| GitHub Actions | GitHub infrastructure | N/A (cloud) | Via GitHub |
| Temporal | PostgreSQL/MySQL | Server-dependent | Via server |
| Prefect | Server/Cloud | Server-dependent | Via server |
| Dagster | Instance storage | Server-dependent | Via server |

### Evidence / Gate Patterns

| Tool | Gate Type | Enforcement | Mechanism |
|------|-----------|-------------|-----------|
| Beads | Dependency blocking | Soft (convention) | `bd ready` filters |
| Claude Code Hooks | Pre/Post tool gates | Hard (exit code 2 blocks) | Shell command, LLM prompt, or agent |
| TaskMaster | Dependency satisfaction | Soft (advisory) | `next` command skips blocked |
| GitHub Actions | Conditional expressions | Hard (job/step skip) | `if` expressions on contexts |
| Temporal | Activity completion | Hard (replay enforced) | Event history replay |
| Prefect | Task state checks | Hard (state-based) | Checkpoint resume |
| Dagster | Asset materialization | Hard (dependency graph) | Execution order enforcement |

---

## Patterns Most Relevant to koto Template Format Design

### 1. YAML Header + Markdown Body (Claude Code Skills Pattern)

Claude Code skills use YAML frontmatter for machine-readable configuration and markdown body for human/agent-readable content. This is directly applicable to koto templates, which already use this pattern. The skills system validates that this format is adoptable by multiple tools.

### 2. Declarative Condition Expressions (GitHub Actions Pattern)

GitHub Actions' `if` expressions provide a clean, readable way to gate execution:
```yaml
if: ${{ steps.test.outputs.passed == 'true' && github.ref == 'refs/heads/main' }}
```

For koto evidence gates, a similar declarative expression system would let templates specify conditions without embedding shell scripts:
```yaml
gates:
  - type: file_exists
    path: "wip/plan.md"
  - type: command
    run: "go test ./..."
    expect: exit_code_0
```

### 3. Three-Type Gate Classification (Claude Code Hooks Pattern)

Claude Code hooks support three gate types: command (shell script), prompt (single-turn LLM evaluation), and agent (multi-turn LLM with tools). This taxonomy maps directly to koto evidence gates:
- **Command gates**: deterministic, fast (file existence, command exit code)
- **Prompt gates**: LLM-evaluated, single-turn (does this PR description look complete?)
- **Agent gates**: LLM-evaluated, multi-turn (verify the test suite passes and coverage exceeds 80%)

### 4. Output Passing Between Stages (GitHub Actions + Temporal Pattern)

Both GitHub Actions (job outputs via `$GITHUB_OUTPUT`) and Temporal (activity return values) pass data forward through the workflow. koto's template variables are currently set at init time and immutable. Evidence gate results could become a forward-passing mechanism: evidence collected in state A is available as interpolation context in state B's directive.

### 5. Project-Directory State with Git Portability (Beads + TaskMaster Pattern)

Both Beads (`.beads/`) and TaskMaster (`.taskmaster/`) store state in the project directory with formats designed for git compatibility. koto's `wip/` approach follows this pattern. The key learning: state files that are ephemeral (cleaned before merge) can use simpler formats (monolithic JSON) since merge-friendly properties (JSONL, hash-based IDs) don't add value for single-writer, single-lifecycle files.

### 6. Tagged Contexts for Multi-Workflow (TaskMaster Pattern)

TaskMaster's tagged task lists enable working across branches/environments without conflicts. koto already handles multiple concurrent workflows via separate state files (`koto-*.state.json`), but the tag concept could extend to template organization: grouping related templates by context (development, review, release).

### 7. Event-Driven Lifecycle Hooks (Claude Code Pattern)

Claude Code's hook lifecycle (SessionStart, PreToolUse, PostToolUse, Stop, TaskCompleted) provides extension points without modifying core behavior. koto could offer similar hooks in templates: pre-transition, post-transition, on-error, on-complete. This keeps the core engine simple while allowing template authors to add custom behavior.
