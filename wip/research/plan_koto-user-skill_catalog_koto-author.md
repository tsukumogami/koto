# koto-author skill audit catalog

This catalog drives the koto-author update work. Three sections: incorrect content (must fix), missing content (must add), and correct content (no change needed).

Files audited:

- `plugins/koto-skills/skills/koto-author/SKILL.md`
- `plugins/koto-skills/skills/koto-author/references/template-format.md`
- `plugins/koto-skills/skills/koto-author/references/examples/complex-workflow.md`
- `plugins/koto-skills/skills/koto-author/koto-templates/koto-author.md`
- `plugins/koto-skills/.claude-plugin/plugin.json`

Source of truth consulted:

- `src/cli/mod.rs` — authoritative CLI command list
- `src/gate.rs` — gate evaluator and output field values
- `src/template/compile.rs` — compiler behavior and error messages
- `src/template/types.rs` — `CompiledTemplate::validate()`, D5 diagnostic text, `override_default` field definition
- `src/cli/overrides.rs` — `koto overrides record/list` handler
- `src/cli/next_types.rs` — `NextResponse`, `BlockingCondition` schema
- `test/functional/fixtures/templates/structured-routing.md` — canonical `gates.*` routing example
- `test/functional/fixtures/templates/mixed-routing.md` — gates + evidence mixed routing example
- `test/functional/fixtures/templates/legacy-gates.md` — legacy gate pattern

---

## Section 1 — Incorrect content (must fix)

### 1.1 Phantom command: `koto status`

The CLI has no `koto status` subcommand. The `Command` enum in `src/cli/mod.rs` lists: `Version`, `Init`, `Next`, `Cancel`, `Rewind`, `Workflows`, `Template`, `Session`, `Context`, `Decisions`, `Overrides`, `Config`. There is no `Status` variant.

Occurrences in koto-author content:

| File | Line(s) | Text |
|------|---------|------|
| `SKILL.md` | 55 | `Run \`koto status\` at any point to see where you are.` |
| `SKILL.md` | 88 | `koto preserves state across interruptions. Run \`koto status\` to see where you left off, then \`koto next\` to continue.` |
| `SKILL.md` | 107 | `Run \`koto status\` to check, then either \`koto next\` to resume or start a new session.` |
| `koto-templates/koto-author.md` | 238 | `Resume instructions for interrupted sessions -- \`koto status\` to check state, \`koto next\` to pick up where you left off.` |

Fix: replace all four occurrences with `koto next <session-name>` (which returns the current state directive and is idempotent when called without `--with-data`). Add a note that `koto workflows` lists active sessions if the session name is unknown.

### 1.2 Phantom command: `koto query`

The CLI has no `koto query` subcommand. This command does not appear in the koto-author skill files (it's documented in `CLAUDE.local.md` as a stale reference). No action needed in koto-author files themselves, but confirming it is absent from the skill content.

### 1.3 `koto init` is missing the required positional `name` argument

The `Init` subcommand signature in `src/cli/mod.rs` is:

```
Init {
    name: String,         // positional, required
    --template <path>
    --var KEY=VALUE       // repeatable
}
```

The `koto init` examples in `SKILL.md` (lines 37–43) omit the session name:

```bash
# shown in SKILL.md (wrong — missing session name)
koto init --template ${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md \
  --var MODE=new
```

The correct form is:

```bash
koto init <session-name> --template ${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md \
  --var MODE=new
```

The same error appears in `koto-templates/koto-author.md` line 236, where the instruction reads `initialize with \`koto init --template ...\`` without the session name argument.

Fix: add the positional `<session-name>` argument to all `koto init` examples and instructions in both files.

### 1.4 `koto next` examples are missing the required positional `name` argument

The `Next` subcommand also takes a positional `name`. `SKILL.md` lines 48–52 describe the loop using bare `koto next` and `koto next --with-data` without the session name. Same issue in `koto-templates/koto-author.md` line 236.

Fix: all `koto next` examples need the session name: `koto next <session-name>` and `koto next <session-name> --with-data '{...}'`.

### 1.5 Legacy gate pattern in `complex-workflow.md`

`references/examples/complex-workflow.md` has two states with gates but no `gates.*` routing in their `when` clauses:

- `preflight`: has a `command` gate (`config_exists`) but no `when` on its single transition.
- `build`: has a `context-exists` gate (`build_output`) but no `when` on its single transition.

Under the current compiler with `strict=true` (the default for `koto template compile`), this template will fail with D5:

```
state "preflight": gate "config_exists" has no gates.* routing
  add a when clause referencing gates.config_exists.passed, gates.config_exists.error, ...
  or use --allow-legacy-gates to permit boolean pass/block behavior
```

The example is advertised in `template-format.md` as the canonical reference for "gates, self-loops, split topology" — it's the example authors are directed to read when their workflow needs gates. Shipping an example that fails strict compilation is a direct blocker.

Fix: update `complex-workflow.md` to use `gates.*` routing. Both `preflight` and `build` need transitions that branch on gate output fields. For a `command` gate, the routing uses `gates.<gate-name>.exit_code`. For `context-exists`, the routing uses `gates.<gate-name>.exists`. See `test/functional/fixtures/templates/structured-routing.md` for the exact syntax.

---

## Section 2 — Missing content (must add)

### 2.1 `gates.*` routing syntax in transition `when`-blocks

**Where to add**: `references/template-format.md`, Layer 3 ("Advanced features"), after the existing "Gates" subsection.

**What's missing**: The guide explains gate *declaration* but never shows how gate output fields are referenced in `when` clauses. An author following the current guide will write gates that compile only under `--allow-legacy-gates`, not under strict compilation.

The syntax to document:

```yaml
transitions:
  - target: pass
    when:
      gates.<gate-name>.exit_code: 0
  - target: fix
    when:
      gates.<gate-name>.exit_code: 1
```

The `gates.<name>.<field>` path is a dot-separated namespace injected by the engine. It cannot be submitted by agents — the engine rejects any `--with-data` payload that includes a `gates.*` key (see `GATES_EVIDENCE_NAMESPACE` constant). The compiler validates that at least one `when` clause references a `gates.*` field for each state that has gates (D5 in strict mode).

Also document the mixed-routing pattern where `gates.*` fields and agent evidence fields appear in the same `when` clause (see `test/functional/fixtures/templates/mixed-routing.md`).

### 2.2 Per-gate output field schemas

**Where to add**: `references/template-format.md`, Layer 3, as a sub-table under the updated gates section (alongside or below the existing gate type overview table).

The three gate types emit different structured output fields. Authors need to know these field names to write `when` clauses. The schemas from `src/gate.rs` and `src/template/types.rs`:

| Gate type | Output field | Type | Meaning |
|-----------|-------------|------|---------|
| `command` | `exit_code` | number | Process exit code (0 = passed, non-zero = failed, -1 = error/timeout) |
| `command` | `error` | string | Error message if spawn failed or command timed out; empty on normal exit |
| `context-exists` | `exists` | boolean | `true` if the key exists in the content store |
| `context-exists` | `error` | string | Error message if evaluation failed; empty on normal evaluation |
| `context-matches` | `matches` | boolean | `true` if the content at `key` matches `pattern` |
| `context-matches` | `error` | string | Error message (e.g., invalid regex, key not found); empty on normal evaluation |

These fields are also the valid keys for `override_default` (see 2.3).

### 2.3 `override_default` field documentation

**Where to add**: `references/template-format.md`, Layer 3, as an addition to the gate declaration syntax block.

**What's missing**: The `override_default` field on gate declarations is not mentioned anywhere in the skill. This field enables the override mechanism — when a gate is blocking and the agent can't resolve it normally, the agent records an override with `koto overrides record`. The value that gets applied is resolved in this order:

1. `--with-data` argument to `koto overrides record`
2. `override_default` on the gate declaration (if set)
3. Built-in default for the gate type (always exists for the three built-in types)

When `override_default` is present, `blocking_conditions[].agent_actionable` is `true` in `koto next` responses, signaling to agents that they can call `koto overrides record` for that gate.

The `override_default` value must be a JSON object matching the gate type's output schema exactly (validated by D2 in the compiler). Example:

```yaml
gates:
  ci_check:
    type: command
    command: "cargo test"
    override_default:
      exit_code: 0
      error: ""
```

### 2.4 `koto overrides record` and `koto overrides list` CLI documentation

**Where to add**: `references/template-format.md`, Layer 3, as a new subsection after the override_default documentation. Also cross-reference from `SKILL.md`'s troubleshooting section or as a new "Override mechanism" section.

**What's missing**: The skill has no coverage of the override mechanism at all. Authors can't write useful skills with gates unless they know how to tell agents to unblock a stuck gate.

Document:

```bash
# Record an override for a blocking gate
koto overrides record <session-name> --gate <gate-name> --rationale "<reason>"

# Record with explicit value (overrides the resolution chain)
koto overrides record <session-name> --gate <gate-name> --rationale "<reason>" \
  --with-data '{"exit_code": 0, "error": ""}'

# List all overrides recorded in the session
koto overrides list <session-name>
```

Key behaviors to document:
- `--rationale` is required (no default, no fallback)
- `--with-data` is optional; falls back to `override_default` on the gate, then built-in default
- If no value can be resolved (unknown gate type with no `override_default` and no `--with-data`), the command fails
- `overrides list` returns the full session history across epoch boundaries

### 2.5 `blocking_conditions` item schema

**Where to add**: `references/template-format.md`, Layer 3, near the gates section. This is reference material authors need to write SKILL.md execution-loop instructions that correctly interpret gate-blocked responses.

**What's missing**: The `blocking_conditions` array in `koto next` responses is mentioned in `SKILL.md` line 52 (`Read \`blocking_conditions\` for what's failing`) but the schema of each item is never documented. From `src/cli/next_types.rs` (`BlockingCondition` struct):

| Field | Type | Notes |
|-------|------|-------|
| `name` | string | Gate name as declared in the template |
| `type` | string | Gate type (`command`, `context-exists`, `context-matches`) |
| `status` | string | `failed`, `timed_out`, or `error` |
| `agent_actionable` | boolean | `true` when `koto overrides record` is available for this gate |
| `output` | object | Gate-type-specific structured output (same schema as 2.2 above) |

### 2.6 `--allow-legacy-gates` flag and D5 diagnostic

**Where to add**: `references/template-format.md`, Layer 3, gates section. Also add to `SKILL.md` compile error troubleshooting entries (the `koto-templates/koto-author.md` compile_validation state already lists several error types but is missing D5).

**What's missing**: Authors who write gate-only blocking states (no `gates.*` routing) will hit the D5 error at compile time. The current compile_validation state in `koto-templates/koto-author.md` lists five error types to handle but does not mention D5. Authors will be confused by the error message.

D5 error text (from `src/template/types.rs`):

```
state "X": gate "Y" has no gates.* routing
  add a when clause referencing gates.Y.passed, gates.Y.error, ...
  or use --allow-legacy-gates to permit boolean pass/block behavior
```

Document in `template-format.md`:
- D5 fires in strict mode (default) when a state has gates but no transition `when` clause references `gates.*`
- Fix: add transitions with `gates.<name>.<field>` conditions
- Escape hatch: `koto template compile <path> --allow-legacy-gates` suppresses D5 and allows boolean pass/block behavior (transitional flag, will be removed)

Document in the compile_validation state of `koto-templates/koto-author.md`:
- Add "No `gates.*` routing (D5)" as a named error case with fix instructions

### 2.7 `koto next --full` flag

**Where to add**: `SKILL.md`, execution loop description (lines 48–54).

**What's missing**: The `--full` flag on `koto next` is mentioned in `SKILL.md` line 53 only in the context of the `details` field (`pass --full to force it on repeat visits`). It's not shown as a concrete command example. This matters for authors writing SKILL.md files: they need to tell agents when and how to use `--full` to re-read extended state instructions.

Document: `koto next <session-name> --full` forces the `details` field to be included even after the first visit to a state.

### 2.8 `koto next --to <state>` forced-transition flag

**Where to add**: `references/template-format.md` or `SKILL.md` (troubleshooting section).

**What's missing**: The `--to` flag allows forcing a transition to any named state. Authors writing SKILL.md instructions may want to tell agents how to recover from a wrong state or skip forward. The CLI supports this via `koto next <session-name> --to <state>`.

### 2.9 `koto cancel` command

**Where to add**: `SKILL.md` troubleshooting section.

**What's missing**: When a session needs to be abandoned, the correct command is `koto cancel <session-name>`. The troubleshooting entry for "session already exists" tells the user to either resume or "start a new session" but doesn't say how to abandon the old one.

### 2.10 `description` field is optional in frontmatter

**Where to add**: `references/template-format.md`, Layer 1, frontmatter schema section.

The current guide says required fields are `name`, `version`, `description`, `initial_state`, `states`. The compiler in `src/template/compile.rs` checks: `name`, `version`, `initial_state`, `states` — `description` is not checked. `CompiledTemplate` serializes it with `skip_serializing_if = "String::is_empty"`, meaning it can be empty. The guide incorrectly lists it as required.

---

## Section 3 — Covered correctly (no change needed)

The following content is accurate and should be preserved without modification:

**Frontmatter structure (Layer 1)**
- The YAML frontmatter schema for `name`, `version`, `initial_state`, `variables`, and `states` is correct.
- The `variables` block: `description`, `required`, and `{{VARIABLE_NAME}}` interpolation syntax are accurate.
- The `{{SESSION_NAME}}` built-in variable (no declaration needed) is documented correctly.
- State fields table (`transitions`, `gates`, `accepts`, `terminal`) is accurate.
- The `terminal: true` / no-transitions pattern for terminal states is correct.

**Directive body sections (Layer 1)**
- The `## state_name` markdown heading convention is accurate.
- The `<!-- details -->` marker behavior (directive vs. details split, first marker wins, `--full` flag) is documented correctly.
- The requirement that every frontmatter state must have a body section is accurate (compiler enforces this).

**Evidence routing (Layer 2)**
- The `accepts` block field types (`enum`, `string`, `number`, `boolean`) and their requirements are accurate.
- The `when` condition AND-semantics are accurate.
- The fallback transition (no `when` block) pattern is correct.
- The mutual exclusivity constraint is accurately described and the compiler enforcement is noted correctly.
- Field type rules (`enum` requires `values`, all fields support `required`) are accurate.

**Gates — declaration syntax (Layer 3)**
- Gate type overview table (`context-exists`, `context-matches`, `command`) with key fields is accurate.
- The gate YAML declaration syntax (`type`, `key`, `pattern`, `command` fields) is correct.
- The "gates checked first, then evidence routing" ordering is accurate.
- The self-loop pattern (transition target = own state) is correct.
- The security note about shell injection in `command` gates with variable interpolation is accurate and good advice.

**Mermaid preview generation**
- The `koto template export <template>.md --format mermaid --output <template>.mermaid.md` command is correct.
- The CI enforcement note (missing/stale mermaid fails the build) is accurate.

**SKILL.md structure and coupling convention**
- The `${CLAUDE_SKILL_DIR}/koto-templates/<skill-name>.md` reference pattern is correct.
- The output structure (SKILL.md + koto-templates/ directory + .mermaid.md) is accurate.
- The feature-to-action mapping table in `template-format.md` is accurate for `evidence_required`, `gate_blocked`, `integration`, `integration_unavailable`, `done`, and `confirm`.

**plugin.json**
- Correctly lists `koto-author` as the only skill; no issues.

**koto-author.md template (the skill's own template)**
- The 8-state topology (entry → context_gathering → phase_identification → state_design → template_drafting → compile_validation → skill_authoring → integration_check → done) is structurally sound.
- The `compile_validation` self-loop on `compile_result: fail` is the correct retry pattern.
- The `template_exists` context-exists gate on `compile_validation` is correct (though it uses legacy-gate boolean pattern — it has no `gates.*` routing and would fail D5 under strict compilation; however, this template is part of the skill infrastructure, not an example shown to authors, so it should also be fixed for consistency).
- The `<!-- details -->` marker usage in `state_design` and `template_drafting` is a good example of the pattern.
