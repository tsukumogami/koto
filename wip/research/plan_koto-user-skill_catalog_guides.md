# Guide Catalog: What the Docs Cover vs. What Skills Must Cover

Produced as part of the koto-user skill design phase. Catalogs every human-facing guide for
currency and coverage so implementers know what the `koto-user` skill can safely delegate to
documentation vs. what it must document itself.

---

## Guide-by-Guide Catalog

### 1. `docs/guides/cli-usage.md`

**What it documents:**

- Session storage model: `~/.koto/sessions/<repo-id>/<name>/`, event-log format, no `--state`/`--state-dir` flags
- `koto init`: positional name, `--template`, JSON response shape, terminal behavior
- `koto next`: all flags (`--with-data`, `--to`, `--full`, `--no-cleanup`), `{{SESSION_DIR}}` variable substitution
- Complete response variant table with all six action types: `evidence_required`, `gate_blocked`, `integration`, `integration_unavailable`, `confirm`, `done`
- Per-variant JSON examples for all six actions, including field presence/absence rules
- `blocking_conditions` semantics: when populated vs. empty, how gate failure maps to response type
- Dispatcher classification order (explicit 6-step priority list)
- Structured error responses: `code`/`message`/`details` JSON, exit code table, all error codes
- `koto rewind`: output, non-destructive append semantics, initial-state constraint
- `koto workflows`: output shape, empty-array behavior
- `koto session dir/list/cleanup` subcommands
- `koto context add/get/exists/list` with all flags and exit codes
- `koto template compile/validate/export` with all flags and compatibility rules
- `koto config get/set/unset/list` with project vs. user config distinction
- Config keys reference table (all six keys, project-config allowlist)
- Cloud sync: how it integrates into existing commands, `koto session resolve`
- `koto version`
- Typical agent workflow loop (shell pseudocode dispatching on `action`)

**Currency assessment:**

Current as of post-#120. Covers structured gate output in `blocking_conditions`, the `output` field
per gate type (referenced by pointer to custom-skill-authoring guide), `gates.*` routing (mentioned
in the gate output schemas section of custom-skill-authoring — cli-usage itself points there). The
response variant table includes all six action types. The `details` field and `--full` flag are
documented. The `--no-cleanup` flag is documented. The `agent_actionable` field appears in a
`blocking_conditions` example but is not defined inline.

**What it covers that skills can reference (last-resort pointer targets):**

- Full `koto next` response schema — every field, every variant
- Exit codes and error code taxonomy
- Session storage location and event-log format
- `koto context` command set (complete)
- `koto session` subcommands
- `koto config` keys and project/user config distinction
- Cloud sync configuration and conflict resolution
- `koto rewind` semantics
- `koto workflows` output shape
- Template authoring commands (`template compile/validate/export`)
- `{{SESSION_DIR}}` substitution

**What it omits that skills must cover themselves:**

- Override mechanism (`koto overrides record/list`) — absent from cli-usage.md entirely
- `koto decisions record/list` — absent (this is in AGENTS.md, not cli-usage.md)
- `koto cancel` — absent
- `koto status` — absent (koto-author SKILL.md references it, but cli-usage.md has no `status` section)
- `agent_actionable` field semantics — field appears in an example but is never explained
- Practical dispatch loop for agents (only a shell pseudocode sketch; a skill needs prose explanation)
- How to interpret `gates.*` structured output in `blocking_conditions.output` — cli-usage.md says "see the gate output schemas in the custom skill authoring guide" but does not inline the schemas; agents reading only cli-usage.md will miss them
- What to do when `blocking_conditions` is non-empty on `evidence_required` vs. `gate_blocked` — documented in cli-usage.md but only briefly; AGENTS.md gives a richer worked example

---

### 2. `docs/guides/custom-skill-authoring.md`

**What it documents:**

- What a skill is (SKILL.md + template file pairing)
- Step-by-step skill authoring: write template, compile, extract evidence keys with jq, write SKILL.md
- SKILL.md YAML frontmatter spec (name, description)
- Seven SKILL.md body sections: Prerequisites, Template Setup, Execution loop, Evidence keys, Response schemas, Error handling, Resume
- Template locality constraint for plugin-distributed skills (must copy to `.koto/templates/`)
- Project-scoped vs. plugin-distributed deployment
- Gate types: `context-exists`, `context-matches`, with YAML examples
- Gate output schemas table: all three types (`command`, `context-exists`, `context-matches`) with full field shapes
- `gates.*` dot-path routing in `when` conditions (template side)
- `koto context` command set (as skill-authoring guidance)
- Content ownership rationale
- Security considerations for directive text
- Eval harness setup and CI validation

**Currency assessment:**

Current as of post-#120. Gate output schemas are present and correct. The `gates.*` routing pattern
is documented with a working YAML example. `context-exists` and `context-matches` gates are
documented as replacements for old `command`-gate-with-`test -f` patterns. No override mechanism is
documented here — this guide predates or was not updated for overrides.

**What it covers that skills can reference:**

- Gate output schemas for all three types (authoritative location)
- `gates.*` dot-path routing syntax (YAML/template side)
- `koto context` usage from the agent/skill perspective
- SKILL.md structure and the seven required sections

**What it omits:**

- Override mechanism (`koto overrides record/list`) — absent
- `koto decisions record/list` — absent
- Runtime agent behavior (this guide is author-facing, not agent-user-facing)
- `blocking_conditions.output` parsing from the agent perspective — schemas are here for template
  authors, but reading them as an agent consumer is not explained

---

### 3. `docs/guides/template-freshness-ci.md`

**What it documents:**

- Reusable GitHub Actions workflow for committed diagram freshness checks
- `koto template export --check` flag
- `koto template export --format mermaid/html` flags
- Inputs: `template-paths`, `koto-version`, `check-html`, `html-output-dir`
- Workflow for template authors maintaining committed diagrams
- `.gitattributes` line ending recommendation

**Currency assessment:**

Narrow scope (CI/authoring tooling). No agent runtime content. Not relevant to koto-user skill.

**What it covers that skills can reference:**

Nothing relevant to koto-user. This guide is exclusively for template authors setting up CI.

**What it omits relative to agent usage:**

Everything — this guide has no agent-runtime content.

---

### 4. `docs/guides/library-usage.md`

**What it documents:**

- One paragraph: koto is binary-only; Go packages were removed in the Rust migration; integrate via CLI subprocess + JSON parsing.

**Currency assessment:**

Current (reflects Rust migration). Irrelevant to agent skill usage.

**What it covers that skills can reference:**

Nothing relevant to koto-user.

**What it omits:**

Everything — this stub is only useful for developer integration questions.

---

### 5. `docs/guides/cloud-sync-setup.md`

**What it documents:**

- Install instructions (install script, cargo)
- `koto config set` commands for cloud backend configuration
- `--project` flag for team-shared settings
- `--user` flag for credentials
- Environment variable credential injection (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)
- Transparent sync: `init`, `next`, `context add` sync automatically; no new commands needed
- Conflict detection and `koto session resolve --keep local/remote`
- Config reference table (same six keys as cli-usage.md)
- Supported providers with endpoint formats

**Currency assessment:**

Current and detailed for cloud sync configuration. Contains one discrepancy vs. cli-usage.md:
`cloud-sync-setup.md` uses `koto session resolve --keep local` (no positional name argument) while
`cli-usage.md` uses `koto session resolve <name> --keep local` (with positional name). One of
these is stale. Skills should use the form documented in cli-usage.md since it is the primary
command reference.

**What it covers that skills can reference:**

- Cloud backend setup steps (complete, step-by-step)
- Credential management (env vars vs. user config, never project config)
- Conflict resolution pattern (pointer target if a koto-user skill includes a cloud sync section)

**What it omits relative to agent usage:**

- All agent runtime commands and response parsing
- Override mechanism, decisions, cancel, status

---

## Cross-Reference: AGENTS.md and koto-author Skill

### `plugins/koto-skills/AGENTS.md`

AGENTS.md is a full agent runtime reference targeted at Codex/Windsurf users. It covers:

- `koto init`, `koto next` (all flags including `--full`)
- `koto decisions record/list` — present and well documented with JSON examples
- `koto rewind`, `koto cancel`, `koto workflows`, `koto template compile`
- All six `action` variants with JSON examples
- `blocking_conditions` handling (gate-blocked vs. evidence-required with populated conditions)
- `details` field and `--full` flag
- `advanced` field semantics
- Error codes table and per-code explanation
- Resume pattern
- Execution loop worked examples (koto-author and work-on)

AGENTS.md does NOT cover:
- `koto overrides record/list` — absent
- `koto session` subcommands — absent
- `koto context` commands — absent
- `koto config` commands — absent
- Cloud sync — absent
- Gate output schemas (blocking_conditions.output) — `blocking_conditions` examples show the array
  but omit the `output` field entirely; agents relying only on AGENTS.md won't know to parse
  structured gate output
- `agent_actionable` field — present in examples but not explained

### `plugins/koto-skills/skills/koto-author/SKILL.md`

koto-author is author-facing. Its runtime loop section is a summary (init, `koto next`, dispatch,
repeat). References `koto status` (a command that does not appear in cli-usage.md). Points agents
to cli-usage.md via the guides index for further reading. No override mechanism. No gate output
schema parsing. No `koto decisions` documentation (agent-authors need to know decisions exist but
the skill doesn't explain them to the workflow runner).

---

## Summary Table

| Surface | In cli-usage.md? | In koto-author skill? | In AGENTS.md (plugin)? | Needs to be in koto-user skill? |
|---------|:----------------:|:---------------------:|:----------------------:|:-------------------------------:|
| `koto init` syntax (name, --template, --var) | yes | yes (summary) | yes | reference cli-usage.md |
| `koto next` basic call | yes | yes | yes | reference cli-usage.md |
| `koto next --with-data` evidence submission | yes | yes | yes | yes — with worked examples |
| `koto next --to` directed transition | yes | no | yes | reference cli-usage.md |
| `koto next --full` flag | yes | yes (mentioned) | yes | mention; reference cli-usage.md |
| `koto next --no-cleanup` flag | yes | no | no | low priority; reference cli-usage.md |
| Response: `action` dispatch model | yes | yes (summary) | yes | yes — core agent loop |
| Response: `evidence_required` shape | yes | no | yes | yes — with fields breakdown |
| Response: `gate_blocked` shape | yes | no | yes | yes — with blocking_conditions |
| Response: `integration` / `integration_unavailable` | yes | no | yes | reference cli-usage.md |
| Response: `confirm` shape | yes | no | yes | yes — explain action_output |
| Response: `done` (terminal) | yes | yes | yes | yes — one line |
| `blocking_conditions` — when populated vs. empty | yes | no | yes (partial) | yes — this is confusing without explanation |
| `blocking_conditions.output` structured gate output | no (pointer only) | yes (author side) | no | yes — agents need to parse this |
| `gates.*` routing (template side) | no (pointer only) | yes | no | no (template authoring, not agent usage) |
| `agent_actionable` field | partial (appears, unexplained) | no | partial (appears, unexplained) | yes — explain the field |
| `details` field and `--full` | yes | yes | yes | reference AGENTS.md or cli-usage.md |
| `advanced` field semantics | yes | no | yes | reference AGENTS.md or cli-usage.md |
| Error codes table | yes | no | yes | reference cli-usage.md; summarize top 3 |
| `koto rewind` | yes | no | yes | yes — when and why to use it |
| `koto overrides record/list` | no | no | no | yes — must be documented in skill; no other source |
| `koto decisions record/list` | no | no | yes (well covered) | reference AGENTS.md |
| `koto cancel` | no | no | yes | mention; reference AGENTS.md |
| `koto workflows` | yes | yes | yes | reference cli-usage.md |
| `koto session dir/list/cleanup` | yes | no | no | mention; reference cli-usage.md |
| `koto context add/get/exists/list` | yes | yes (skill-authoring side) | no | yes — agent needs to know how to use context |
| `koto config` commands | yes | no | no | no (setup topic; reference cloud-sync-setup.md) |
| Cloud sync (transparent behavior) | yes | no | no | brief mention; reference cloud-sync-setup.md |
| `koto session resolve` conflict | yes (with name arg) | no | no | reference cli-usage.md |
| Resume pattern | yes | yes | yes | yes — standard section |
| Template setup (copy to .koto/templates/) | no | yes (detailed) | yes | yes — agents need this before init |
| `{{SESSION_DIR}}` substitution | yes | no | no | mention; reference cli-usage.md |
| Session storage location | yes | no | no | reference cli-usage.md |
| Gate output schemas (all 3 types) | no (pointer only) | yes (author side) | no | yes — agents parsing blocking_conditions.output need this |
| `koto status` command | no | referenced | no | do not document (not in cli-usage.md; likely stale reference) |

---

## Key Findings for koto-user Skill Implementers

### 1. The override mechanism has no documentation anywhere

`koto overrides record/list` is absent from cli-usage.md, AGENTS.md, custom-skill-authoring.md,
and the koto-author SKILL.md. The koto-user skill is the only planned location for this
documentation. Implementers cannot use a pointer here — the content must be written from scratch
based on source code and functional tests.

### 2. cli-usage.md is authoritative for the `koto next` response schema

cli-usage.md has the most complete, accurate response variant table. The koto-user skill can rely
on it as a pointer for field-level detail, but must still explain the dispatch model in prose
because agents need conceptual framing, not just a JSON schema.

### 3. Gate output in `blocking_conditions.output` is not explained where agents read it

The gate output schemas exist in custom-skill-authoring.md (author side) but cli-usage.md only
points there. AGENTS.md omits the `output` field from `blocking_conditions` examples entirely. A
koto-user agent has no documented path to understand what's in `blocking_conditions[*].output`
unless the koto-user skill covers it.

### 4. `koto decisions record/list` is well covered in AGENTS.md

AGENTS.md has the best documentation of decisions with worked examples. koto-user can point to
AGENTS.md for this surface without duplicating.

### 5. `koto status` appears in koto-author SKILL.md but not in cli-usage.md

This command either doesn't exist in the Rust codebase or was renamed. Do not document it in the
koto-user skill without verifying. If it doesn't exist, the koto-author SKILL.md reference is a
stale artifact that needs correction.

### 6. `koto session resolve` has a signature discrepancy

cloud-sync-setup.md omits the `<name>` positional argument; cli-usage.md includes it. Use the
cli-usage.md form in the koto-user skill and flag the discrepancy in cloud-sync-setup.md for
correction.

### 7. `agent_actionable` appears in examples but is never explained

Both cli-usage.md and AGENTS.md include `agent_actionable` in `blocking_conditions` array entries
but neither defines what it means. The koto-user skill should define it: when `true`, the agent
can take action to resolve the blocking condition; when `false`, the condition is externally
controlled and the agent should report it to the user rather than attempting to fix it.

### 8. `koto context` is covered in cli-usage.md but not framed for agent consumers

cli-usage.md documents `koto context` as a complete command set. Custom-skill-authoring.md
explains it from the template author's perspective. Neither explains it from the running agent's
perspective: when does an agent need to call `koto context get` to retrieve prior work? The
koto-user skill needs a short section explaining the agent's content interaction pattern.

---

## Reliability of cli-usage.md as a Last-Resort Pointer

cli-usage.md is reliable for: all `koto next` response shapes, exit codes, error codes, session
storage, `koto context`, `koto session`, `koto config`, `koto rewind`, `koto workflows`, and
`{{SESSION_DIR}}`. These surfaces are safe to pointer-delegate with the note "for full field
reference, see cli-usage.md."

cli-usage.md is NOT reliable for: `koto overrides record/list` (absent), `koto decisions
record/list` (absent), `koto cancel` (absent), gate output schema parsing from the agent
perspective (pointer-only), `agent_actionable` semantics (present but undefined).

For those gaps, the koto-user skill must document from source — no guide covers them sufficiently.
