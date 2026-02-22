# Industry Research: AI Agent Workflow State Management

Phase 2 research for koto workflow orchestration design. Surveys how existing AI coding tools handle workflow state, persistence, concurrency, and git integration.

---

## 1. Claude Code

### Evolution: TodoWrite to Tasks API

Claude Code underwent a significant architectural shift in v2.1.16 (January 2026), replacing the ephemeral TodoWrite system with a persistent Tasks API.

**TodoWrite (legacy):**
- Stored tasks in the conversation context window (in-memory only)
- Lost on `/compact`, `/clear`, or session exit
- Consumed context tokens -- a 50-task plan ate into the 200K token budget
- No cross-session coordination

**Tasks API (current):**
- Persists to `~/.claude/tasks/<task-list-id>.jsonl` (JSONL format, one task per line)
- Survives compaction, session exit, terminal restarts
- Zero context token consumption since everything is on disk
- Cross-session coordination via `CLAUDE_CODE_TASK_LIST_ID` environment variable

### Where State Lives

Filesystem: `~/.claude/tasks/` directory, outside the project tree. The task list ID determines the filename (e.g., `auth-system` maps to `~/.claude/tasks/auth-system.jsonl`). If no ID is set, tasks go to `default.jsonl`.

### State Format

JSONL (JSON Lines). Each line is a task object:

```json
{"id": "task-1", "subject": "Implement auth backend", "description": "...", "status": "in_progress", "blockedBy": ["task-0"], "metadata": {}}
```

**Task fields:** `id`, `subject`, `description`, `status`, `blockedBy` (array of task IDs), `metadata` (arbitrary key-value).

**Status values:** `pending`, `in_progress`, `completed`, `failed`.

**Operations:** `TaskCreate`, `TaskList` (summary only), `TaskGet` (full detail), `TaskUpdate`, `TaskComplete`.

### Dependency Tracking

The `blockedBy` field creates a DAG of task dependencies. Tasks with unfulfilled blockers cannot transition to `in_progress`. No circular dependency detection -- that is left to the developer/agent to avoid.

### Concurrency Handling

Multiple Claude sessions can point at the same task file via shared `CLAUDE_CODE_TASK_LIST_ID`. The concurrency model is last-write-wins with no conflict resolution. Since the file is JSONL and tasks are appended, this works acceptably for low-concurrency scenarios but has no locking or CAS.

### Hooks for Workflow Control

Claude Code hooks (v2.0.10+) provide lifecycle events: `PreToolUse`, `PostToolUse`, `SessionStart`, `Stop`, `Notification`, `UserPromptSubmit`. Hooks receive JSON on stdin, return results via exit code and stdout/stderr. Exit code 2 blocks the action. Since v2.0.10, PreToolUse hooks can modify tool inputs before execution (not just block). Async hooks (`async: true`) run in background without blocking.

### Git Pollution

Not a concern. Tasks live in `~/.claude/tasks/` (user home), not in the project directory. They never appear in git history.

### Multi-Agent / Swarm

Claude Code agent teams (experimental, v2.1+) spawn multiple Claude instances sharing a task list. The `CLAUDE_CODE_TASK_LIST_ID` env var is the coordination mechanism -- all agents read/write the same JSONL file.

---

## 2. Gemini CLI

### Session Management

Gemini CLI (v0.20.0+) provides automatic session saving with full conversation replay.

### Where State Lives

Filesystem: `~/.gemini/tmp/<project_hash>/chats/`. The `<project_hash>` is derived from the project's root directory, making sessions project-scoped. Switching directories switches session context.

### State Format

Currently JSON. An active migration to JSONL is underway (tracked in gemini-cli issue #15292). The proposed JSONL structure:

```jsonl
{"type":"session_metadata","sessionId":"...","projectHash":"...","startTime":"..."}
{"type":"user","id":"msg1","content":[{"text":"Hello"}]}
{"type":"gemini","id":"msg2","content":[{"text":"Hi"}]}
{"type":"message_update","id":"msg2","tokens":{"input":10,"output":5}}
```

### What Gets Saved

Prompts and responses, all tool executions (inputs and outputs), token usage statistics, reasoning summaries. This is conversation replay, not workflow state.

### Session Resumption

`/resume` opens a session browser with chronological list, message counts, one-line summaries, and search/filter by ID or content. Configuration controls `maxAge` (e.g., "7d") and `maxCount` for automatic cleanup.

### Git Pollution

Not a concern. Sessions live in `~/.gemini/tmp/`, outside the project tree.

### Limitations for Workflow State

Gemini CLI sessions are conversation replay, not structured workflow state. There is no task/dependency model, no status tracking, no evidence-gated transitions. It solves the "resume where I left off" problem but not the "track multi-step workflow progress" problem.

---

## 3. Beads

### Overview

Beads (by Steve Yegge) is a Git-native issue tracker designed specifically for AI coding agents. It provides persistent project memory across sessions through a SQLite + JSONL dual-storage architecture.

### Where State Lives

Project directory: `.beads/` committed to git. Contents:

| File | Git-tracked | Purpose |
|------|-------------|---------|
| `issues.jsonl` | Yes | Source of truth for all issues/tasks |
| `metadata.json` | Yes | Database metadata |
| `config.yaml` | Yes | User configuration |
| `interactions.jsonl` | Yes | Agent audit log |
| `beads.db` | No (gitignored) | Local SQLite cache |
| `beads.db-shm`, `beads.db-wal` | No | SQLite WAL files |
| `daemon.*` | No | Background sync daemon |
| `bd.sock` | No | Unix socket for CLI/agent communication |

### State Format

**Primary:** JSONL (issues.jsonl) -- append-only, one JSON object per line. Each line represents a change (create, update, status transition), forming a complete change history.

**Secondary:** SQLite (beads.db) -- local read-model cache "hydrated" from JSONL on startup. All queries run against SQLite for speed. The JSONL is the source of truth; SQLite is disposable.

**Export:** JSONL uses `json.Encoder`, always sorted by ID for deterministic output.

### Git Integration (Key Innovation)

Beads is built around git as a distributed database:

- **Append-only JSONL** means merge conflicts are rare since both sides just added lines
- **Hash-based IDs** (e.g., `bd-a1b2`) prevent collision across branches and agents
- **When conflicts do occur:** keep the line with the newer `updated_at` timestamp
- **Auto-sync daemon** detects changes to `issues.jsonl` after `git pull` and imports them into the local SQLite cache

### Concurrency / Multi-Agent

Multiple agents on different branches create tasks independently. Because IDs are hash-based and the log is append-only, merging branches merges task sets cleanly. The daemon process handles sync between the JSONL file and local SQLite.

### Git Pollution

This is a real concern by design -- Beads commits `.beads/issues.jsonl` and related files to the repo. The tradeoff is deliberate: git IS the distributed database, so the data must live in the repo. The append-only JSONL format keeps diffs clean and merge-friendly, but the files are always in the history.

### Evidence/Completion Patterns

Beads supports DAG-based dependencies with priority systems. Issues track status transitions with timestamps, creating an audit trail. The `interactions.jsonl` file separately logs all agent interactions for debugging.

---

## 4. TaskMaster AI

### Overview

TaskMaster (claude-task-master) is an AI-powered task management system designed to work with AI editors (Cursor, Windsurf, Claude Code, etc.). It generates tasks from a PRD and tracks implementation progress.

### Where State Lives

Project directory: `.taskmaster/` committed to git.

| File | Purpose |
|------|---------|
| `.taskmaster/tasks/tasks.json` | Main task store |
| `.taskmaster/config.json` | Configuration and model settings |
| `.taskmaster/state.json` | Runtime state (current tag, last switched) |
| `.taskmaster/docs/prd.txt` | Source PRD for task generation |

### State Format

**JSON** (not JSONL). The structure uses tagged task lists:

```json
{
  "tagName": {
    "tasks": [
      {
        "id": 1,
        "title": "Implement auth",
        "description": "...",
        "status": "pending",
        "dependencies": [3, 5],
        "priority": "high",
        "details": "...",
        "testStrategy": "...",
        "subtasks": [...],
        "metadata": {}
      }
    ]
  }
}
```

**Status values:** `pending`, `in-progress`, `done`, `review`, `deferred`, `cancelled`.

**Dependencies:** Array of numeric IDs referencing tasks within the same tag context.

**Tags:** Enable multi-context task management (different branches, environments, project phases). State.json tracks the current active tag.

### Concurrency

TaskMaster adopted the `modifyJson` pattern for atomic read-modify-write operations on `tasks.json`, fixing race conditions when multiple Claude Code windows write simultaneously. This is file-level locking, not field-level.

### Multi-Agent Coordination

TaskMaster evolved into a coordination platform using:
- MCP Filesystem server for shared memory
- SQLite-based coordination layer (separate from task storage)
- Planner/Worker/Judge role separation

### Git Pollution

Moderate concern. `.taskmaster/tasks/tasks.json` is committed to git. Since it is a single JSON file (not append-only), concurrent modifications on different branches create merge conflicts. The `move` command helps resolve these, but the fundamental format (monolithic JSON) is not merge-friendly.

---

## 5. StateFlow (COLM 2024)

### Overview

Academic paper presenting a state machine formalism for LLM task-solving. Not a tool but a design pattern.

### Formal Model

StateFlow models workflows as a finite state machine: **S, s_0, F, delta, Gamma, Omega** where:
- **S**: Set of states (distinct workflow phases)
- **s_0**: Initial state
- **F**: Final/terminal states
- **delta**: Transition function `(current_state, context_history) -> next_state`
- **Gamma**: Output alphabet (prompts, LLM responses, tool feedback)
- **Omega**: Output functions executed on state entry

### Where State Lives

**In-memory only.** State is the current position in the FSM plus the cumulative context history (all past interactions). There is no file persistence. The framework runs as a single execution -- if it crashes, state is lost.

### Transition Mechanisms

Two strategies:
1. **String matching:** Pattern detection in LLM output or tool results (e.g., "if 'Error' in execution output, transition to Error state")
2. **LLM-based evaluation:** Use the LLM itself to evaluate conditions and determine the next state

### Evidence-Gated Transitions

Transitions are conditioned on execution outcomes. Example from SQL workflow:
- Successful `DESC` command -> transition to Solve state
- Successful `SELECT` command -> transition to Verify state
- Error in execution -> transition to Error state
- Successful submit -> transition to End state

### Key Insight

StateFlow separates "process grounding" (state + transitions = where am I in the workflow) from "sub-task solving" (actions within a state = what do I do here). This decomposition reduces prompt size 5x and improves success rates 13-28% over ReAct.

### Relevance to Koto

StateFlow validates the state machine approach but is not a production tool. Its in-memory, single-execution model doesn't address persistence, concurrency, or multi-workflow needs. The formal model (sextuple FSM) is a useful theoretical foundation.

---

## 6. LangGraph

### Overview

LangGraph (by LangChain) is the most mature framework for stateful multi-agent workflows, using graph-based architecture with built-in persistence.

### Where State Lives

Pluggable checkpoint storage:
- **SQLite** (`langgraph-checkpoint-sqlite`): Local development
- **PostgreSQL** (`langgraph-checkpoint-postgres`): Production (used in LangSmith)
- **In-memory**: Testing only

### State Format

Checkpoints use a structured format with JSON serialization:

```json
{
  "v": 1,
  "ts": "2024-...",
  "id": "checkpoint-uuid",
  "channel_values": { /* actual state data */ },
  "channel_versions": { /* version per channel */ },
  "versions_seen": { /* which node versions processed */ }
}
```

Serialization uses `JsonPlusSerializer` which handles LangChain primitives, datetimes, enums, and more.

### Persistence and Resumption

Checkpoints are saved at every graph "super-step." Resumption uses `thread_id` as primary key. LangGraph supports time-travel debugging -- replay from any checkpoint, with steps before the checkpoint replayed (not re-executed) and steps after forked as new execution.

### Concurrency

PostgreSQL checkpointer uses channel-level versioning -- only changed values are stored per checkpoint. This enables concurrent reads. However, concurrent writes to the same thread are not safe without external coordination.

### Git Pollution

Not applicable. State lives in a database (SQLite or PostgreSQL), not in the project tree.

---

## Comparative Analysis

### State Location Spectrum

| Tool | Location | In Project Tree? | Git Tracked? |
|------|----------|-------------------|--------------|
| Claude Code Tasks | `~/.claude/tasks/` | No | No |
| Gemini CLI | `~/.gemini/tmp/` | No | No |
| Beads | `.beads/` | Yes | Yes (by design) |
| TaskMaster | `.taskmaster/` | Yes | Yes |
| StateFlow | In-memory | No | No |
| LangGraph | SQLite/PostgreSQL | Configurable | No |

### Format Comparison

| Tool | Format | Append-Only? | Merge-Friendly? |
|------|--------|--------------|-----------------|
| Claude Code Tasks | JSONL | Yes | N/A (not in git) |
| Gemini CLI | JSON (migrating to JSONL) | No (migrating to yes) | N/A (not in git) |
| Beads | JSONL + SQLite cache | Yes | Yes (hash IDs) |
| TaskMaster | JSON | No | No (monolithic) |
| StateFlow | In-memory | N/A | N/A |
| LangGraph | JSON in DB | N/A | N/A |

### Concurrency Model

| Tool | Model | Safety |
|------|-------|--------|
| Claude Code Tasks | Last-write-wins on shared JSONL | Low (no locking) |
| Gemini CLI | Single-session only | N/A |
| Beads | Append-only JSONL + hash IDs | High (merge-safe) |
| TaskMaster | modifyJson atomic R/M/W | Medium (file-level lock) |
| LangGraph | DB-level concurrency | High (PostgreSQL) |

### Workflow Sophistication

| Tool | Dependencies? | Evidence-Gated? | Multi-Workflow? |
|------|---------------|-----------------|-----------------|
| Claude Code Tasks | Yes (blockedBy) | No | Yes (via task list ID) |
| Gemini CLI | No | No | No |
| Beads | Yes (DAG) | No | Yes (branches) |
| TaskMaster | Yes (numeric IDs) | No | Yes (tags) |
| StateFlow | Via FSM transitions | Yes (string/LLM match) | No |
| LangGraph | Via graph edges | Yes (conditional edges) | Yes (thread IDs) |

---

## Key Patterns and Insights

### 1. The Great Split: Home Directory vs Project Directory

Tools divide cleanly into two camps:
- **Home directory** (Claude Code, Gemini CLI): State is ephemeral to the user, invisible to git, no pollution. But state doesn't travel with the project and can't be shared across machines via git.
- **Project directory** (Beads, TaskMaster): State travels with the code, shared via git. But creates pollution concerns and merge complexity.

### 2. JSONL as the Winning Format for Git-Tracked State

Both Beads and Gemini CLI (in its migration) converged on JSONL. Advantages:
- Append-only writes are crash-safe
- Line-based diffs are clean in git
- Merge conflicts are rare and easy to resolve
- Each line is independently valid JSON

Monolithic JSON (TaskMaster) creates painful merge conflicts when multiple agents/branches modify tasks.

### 3. The SQLite Cache Pattern

Beads pioneered a pattern worth noting: JSONL as source of truth (git-portable), SQLite as local read cache (fast queries). This gives you both git compatibility and query performance. TaskMaster adopted a similar pattern for its coordination layer.

### 4. Evidence-Gated Transitions are Academic, Not Practical (Yet)

StateFlow's evidence-gated transitions (string matching or LLM evaluation to determine state changes) are powerful in controlled benchmarks but no production tool implements them. Every production tool uses explicit status updates (agent or human sets status to "done").

### 5. Concurrency Remains Unsolved at the File Level

No file-based tool has a satisfying concurrency story:
- Claude Code: last-write-wins (data loss risk)
- TaskMaster: file-level locking (serializes all access)
- Beads: append-only mitigates but doesn't eliminate conflicts

Database-backed tools (LangGraph with PostgreSQL) handle concurrency properly but lose git portability.

### 6. Session State vs Workflow State are Different Problems

Gemini CLI and Claude Code's session persistence solve "resume the conversation." Beads and TaskMaster solve "track multi-step project progress." These are complementary, not competing. A workflow orchestrator needs the latter.

---

## Implications for Koto Design

1. **State location decision is foundational.** Home directory avoids git pollution but loses portability. Project directory enables git-backed distribution but requires merge-friendly formats.

2. **If project-directory, use JSONL with hash-based IDs.** Beads proved this works. Monolithic JSON (TaskMaster) doesn't scale for concurrent workflows.

3. **Consider the SQLite cache pattern.** JSONL for persistence + SQLite for queries gives the best of both worlds.

4. **Evidence-gated transitions from StateFlow are worth adopting** but need a practical implementation -- file existence checks, test results, command exit codes rather than LLM evaluation.

5. **Multi-workflow support needs a namespace mechanism.** Claude Code uses env var task list IDs. TaskMaster uses tags. Beads uses git branches. Each has tradeoffs.

6. **The wip/ directory approach (current koto design) maps closest to TaskMaster** but with individual files per workflow instead of a monolithic JSON. This is a reasonable middle ground if the files use a merge-friendly format.

---

## Sources

- [Claude Code Task Management Guide](https://claudefa.st/blog/guide/development/task-management)
- [Tasks API vs TodoWrite (DeepWiki)](https://deepwiki.com/FlorianBruniaux/claude-code-ultimate-guide/7.1-tasks-api-vs-todowrite)
- [Claude Code Tasks Update (VentureBeat)](https://venturebeat.com/orchestration/claude-codes-tasks-update-lets-agents-work-longer-and-coordinate-across)
- [Claude Code Hooks Guide](https://code.claude.com/docs/en/hooks-guide)
- [Claude Code Agent Teams](https://code.claude.com/docs/en/agent-teams)
- [Gemini CLI Session Management](https://geminicli.com/docs/cli/session-management/)
- [Gemini CLI Session Management (Google Blog)](https://developers.googleblog.com/pick-up-exactly-where-you-left-off-with-session-management-in-gemini-cli/)
- [Gemini CLI JSONL Migration (Issue #15292)](https://github.com/google-gemini/gemini-cli/issues/15292)
- [Beads GitHub Repository](https://github.com/steveyegge/beads)
- [Beads Architecture (DeepWiki)](https://deepwiki.com/steveyegge/beads)
- [Beads Rust Port Architecture Doc](https://github.com/Dicklesworthstone/beads_rust/blob/main/EXISTING_BEADS_STRUCTURE_AND_ARCHITECTURE.md)
- [Beads Best Practices (Steve Yegge)](https://steve-yegge.medium.com/beads-best-practices-2db636b9760c)
- [TaskMaster AI GitHub](https://github.com/eyaltoledano/claude-task-master)
- [TaskMaster Task Structure Docs](https://docs.task-master.dev/capabilities/task-structure)
- [TaskMaster Multi-Agent Coordination](https://deankeesey.com/blog/ai-multi-agent-coordination/)
- [StateFlow Paper (arXiv)](https://arxiv.org/abs/2403.11322)
- [StateFlow COLM 2024 (OpenReview)](https://openreview.net/forum?id=3nTbuygoop)
- [LangGraph Persistence Docs](https://docs.langchain.com/oss/python/langgraph/persistence)
- [LangGraph Checkpoint Implementations (DeepWiki)](https://deepwiki.com/langchain-ai/langgraph/4.2-checkpoint-implementations)
- [From Beads to Tasks (Paddo.dev)](https://paddo.dev/blog/from-beads-to-tasks/)
- [Claude Code Hidden Multi-Agent System (Paddo.dev)](https://paddo.dev/blog/claude-code-hidden-swarm/)
- [AI Coding Agents in 2026 (Mike Mason)](https://mikemason.ca/writing/ai-coding-agents-jan-2026/)
