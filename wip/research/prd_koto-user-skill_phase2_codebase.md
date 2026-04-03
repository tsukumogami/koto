# Phase 2 Research: Codebase Analyst

## Lead A: CLI surface verification

### Findings

**`koto status` does not exist.** A full audit of `src/cli/mod.rs` confirms the `Command` enum has no `Status` variant. The subcommands actually available are: `Version`, `Init`, `Next`, `Cancel`, `Rewind`, `Workflows`, `Template`, `Session`, `Context`, `Decisions`, `Overrides`, `Config`. There is no `status` subcommand.

**`koto query` does not exist.** Despite being mentioned in `CLAUDE.local.md`, there is no `Query` variant in the `Command` enum. This appears to be a stale reference in the config file, not a real subcommand.

**`koto status` is referenced twice in koto-author SKILL.md.** Both are in resumability contexts: line 55 ("Run `koto status` at any point to see where you are") and line 88 ("Run `koto status` to see where you left off"). Neither reference is accurate.

**The correct replacement for `koto status`.** The closest available equivalent depends on what the user wants:
- To see current state and workflow status: `koto next <name>` (it returns `state`, `action`, and `directive` on every call)
- To see all active workflows: `koto workflows` (lists all active workflows in the current directory as JSON)
- To get the session directory: `koto session dir <name>`

There is no single "show me my current position" read-only command. `koto workflows` shows what workflows exist; `koto next` advances state (or returns current state if nothing to advance).

**Complete confirmed CLI surface for koto-user:**

| Command | Confirmed |
|---------|-----------|
| `koto init <name> --template <path> [--var KEY=VALUE]` | Yes — `Init` variant, name is positional, `--template` required |
| `koto next <name> [--with-data <json>] [--to <state>] [--full] [--no-cleanup]` | Yes — `Next` variant |
| `koto cancel <name>` | Yes — `Cancel` variant |
| `koto rewind <name>` | Yes — `Rewind` variant |
| `koto workflows` | Yes — `Workflows` variant, no args, lists active workflows |
| `koto session dir <name>` | Yes — `SessionCommand::Dir`, prints absolute path |
| `koto session list` | Yes — `SessionCommand::List`, JSON array |
| `koto session cleanup <name>` | Yes — `SessionCommand::Cleanup`, idempotent |
| `koto session resolve <name> --keep local\|remote` | Yes — cloud-backend only |
| `koto decisions record <name> --with-data <json>` | Yes — requires `choice` and `rationale` fields |
| `koto decisions list <name>` | Yes — returns decisions for current state |
| `koto overrides record <name> --gate <name> --rationale <text> [--with-data <json>]` | Yes — `OverridesSubcommand::Record` |
| `koto overrides list <name>` | Yes — full session history, all epochs |
| `koto context add <session> <key> [--from-file <path>]` | Yes — reads stdin or file |
| `koto context get <session> <key> [--to-file <path>]` | Yes — writes stdout or file |
| `koto context exists <session> <key>` | Yes — exits 0/1 |
| `koto context list <session> [--prefix <prefix>]` | Yes — JSON array |
| `koto template compile <source> [--allow-legacy-gates]` | Yes |
| `koto template validate <path>` | Yes |
| `koto template export <input> [--format mermaid\|html] [--output <path>]` | Yes |
| `koto config get <key>` | Yes |
| `koto config set <key> <value> [--user]` | Yes |
| `koto config list [--json]` | Yes |

**`koto next` response schema** (confirmed from `src/cli/next_types.rs`):

Six response variants, all JSON. All include `action`, `state`, `advanced`, `error: null`. The `action` values are:
- `evidence_required` — fields: `directive`, `details?`, `expects`, `blocking_conditions`
- `gate_blocked` — fields: `directive`, `details?`, `expects: null`, `blocking_conditions`
- `integration` — fields: `directive`, `details?`, `expects`, `integration`
- `integration_unavailable` — fields: `directive`, `details?`, `expects`, `integration`
- `done` — fields: `expects: null`
- `confirm` — fields: `directive`, `details?`, `action_output`, `expects`

**`blocking_conditions` schema** (confirmed from `BlockingCondition` struct):
```json
{
  "name": "gate_name",
  "type": "command|context-exists|context-matches",
  "status": "failed|timed_out|error",
  "agent_actionable": true,
  "output": {"exit_code": 1, "error": ""}
}
```
`agent_actionable` is `true` when the gate has either an `override_default` or a built-in default for its type (all three built-in types have built-in defaults). This signals that `koto overrides record` is available.

**`expects` schema** (confirmed from `ExpectsSchema` struct):
```json
{
  "event_type": "evidence_submitted",
  "fields": {"field_name": {"type": "enum|string|number|boolean", "required": true, "values": [...]}},
  "options": [{"target": "next_state", "when": {"field": "value"}}]
}
```

**`koto decisions record` schema** — requires JSON with at least `choice` (string) and `rationale` (string). Optionally includes `alternatives_considered` (array of strings). This is validated at the `DecisionSummary` struct level.

**`koto overrides record` resolution order** — `--with-data` → gate's `override_default` → built-in default for gate type. Fails with exit 2 if none available (only possible for custom/unknown gate types).

**Built-in override defaults by gate type:**
- `command`: `{"exit_code": 0, "error": ""}`
- `context-exists`: `{"exists": true, "error": ""}`
- `context-matches`: `{"matches": true, "error": ""}`

### Implications for Requirements

1. **Remove all `koto status` references from koto-author SKILL.md.** Lines 55 and 88 must be updated. The correct replacement is `koto workflows` (to see what's running) and `koto next` (to get current state/directive). The koto-user skill must not mention `koto status` at all.

2. **Remove `koto query` from CLAUDE.local.md** (or don't document it). It doesn't exist; the PRD should not spec koto-user to document it.

3. **koto-user needs the full `koto next` response schema** documented, including all six `action` values, the conditional presence of `details`, and the `blocking_conditions` array schema with `agent_actionable`.

4. **The `koto decisions record` required fields** (`choice`, `rationale`) should be documented in koto-user. Template authors use `decisions record` as an annotation mechanism; users of koto-backed workflows need to know when and how to call it.

5. **koto-user must document the override flow** in terms of what `agent_actionable: true` means and how to call `koto overrides record` with the correct `--gate` and `--rationale` arguments.

6. **`koto next --to <state>`** is a directed transition mechanism that appears in the source but is not mentioned anywhere in koto-author. koto-user should document when it's appropriate (e.g., skipping a state during debugging, or when a workflow needs manual navigation).

7. **`koto session dir` is the correct way to get the session path**, not a nonexistent `koto status` command. This is relevant for koto-user guidance on accessing context store files.

### Open Questions

1. Should `koto next --to <state>` be documented in koto-user as a normal workflow operation, or only as a "advanced/escape-hatch" feature? The source has it as `--to` (a directed transition), but there's no documentation on when agents should use it.

2. `koto workflows` returns JSON — what's the schema? The source calls `find_workflows_with_metadata` but the exact output shape wasn't read. This needs confirmation before writing the koto-user reference.

3. Is `koto cancel` relevant to koto-user? It prevents further advancement but doesn't clean up. koto-user might need to explain cancel vs. cleanup.

4. `koto session resolve` is cloud-backend only. Should koto-user mention it at all, or limit coverage to the local backend?

---

## Lead B: koto-author update scope

### Findings

Reading all files in `plugins/koto-skills/skills/koto-author/` confirms the gaps identified in the prior explore research. This read adds precision about which files need changes and how much content each gap requires.

**SKILL.md — two `koto status` references (live bug)**

Lines 55 and 88 reference `koto status`, which doesn't exist. These are the only live bugs in the skill (incorrect CLI commands that would fail immediately if an agent ran them). Fix: replace with `koto workflows` + `koto next` as appropriate. Small change, 1-2 lines each.

**`references/template-format.md` — Layer 3 gaps (four high-severity)**

1. **`gates.*` routing syntax** — entirely absent. The current Layer 3 gates section shows only the type/passes-when/key-fields table. It has no mention of `gates.<name>.<field>` syntax in `when` clauses, no mention that this is now required (strict mode), and no examples. Required addition: a new subsection after the gates table explaining the routing syntax, with a snippet showing `gates.ci.exit_code: 0` in a `when` clause. Medium content — maybe 30–50 lines including a code block.

2. **Structured gate output schemas** — absent. The current gates table has no output schema column. Required addition: either a new column ("Output fields") in the existing table or a separate schema table showing `command → {exit_code, error}`, `context-exists → {exists, error}`, `context-matches → {matches, error}`. Small addition.

3. **`override_default` gate field** — absent. The current gates table doesn't include this field. The override mechanism (override_default, koto overrides record, koto overrides list, agent_actionable in blocking_conditions) needs a new section in template-format.md. Medium content — the template-author perspective (how to set override_default on a gate, what the resolution order is). This overlaps with koto-user content; the author-side is about setting it, the user-side is about triggering it.

4. **`--allow-legacy-gates` flag** — absent. Should be a note in the Layer 3 gates section explaining that strict mode (default in `koto template compile`) requires `gates.*` routing, and that `--allow-legacy-gates` bypasses D5 for migration purposes only. Small addition.

**`koto-templates/koto-author.md` — `compile_validation` state needs update (medium)**

The `compile_validation` state directive lists common compile errors but omits D5 ("gate has no `gates.*` routing"). This is the most likely error an agent will see when following the updated Layer 3 guidance. Required addition: a new bullet in the compile_validation directive covering D5 and explaining that `--allow-legacy-gates` suppresses it temporarily. The state's own gate (`template_exists`, a `context-exists` gate) also lacks `gates.*` routing — this is the legacy-gate credibility problem identified in prior research. The gate itself should be updated to use `gates.template_exists.exists: true` in the transition `when` clause, making the skill's own template a correct demonstration.

**`references/examples/complex-workflow.md` — legacy gate pattern (medium)**

The `preflight` state has a `command` gate (`config_exists`) with no `gates.*` routing — it advances unconditionally after the gate passes. The `build` state has a `context-exists` gate (`build_output`) with the same problem. Both would trigger D5 in strict mode.

Two options, each with a tradeoff:
- **Update in place**: Change `preflight` or `build` to demonstrate `gates.*` routing. This updates the authoritative example but may complicate a state that was kept simple intentionally. Best candidate: `preflight` → add `when: gates.config_exists.exit_code: 0` to its transition, which is the natural "proceed if the gate passed" pattern.
- **Add a new example**: Create `gates-routing-workflow.md` that focuses specifically on structured gate output routing. Leaves the existing example intact as a simpler reference. The downside is that `complex-workflow.md` still produces D5 warnings if compiled in strict mode, which is misleading for a reference file.

The prior research note that the existing file "uses legacy gates" suggests the in-place update is preferred. The example is titled "complex-workflow" — demonstrating the current recommended pattern fits its purpose.

**No new reference files needed.** All gaps fit into existing files: `template-format.md` (Layer 3 expansion + override section), `koto-author.md` (D5 bullet in compile_validation + gate update), and `complex-workflow.md` (in-place update to at least one gate state). No new file is needed for the gates-routing pattern specifically, though an optional standalone example remains viable.

**Existing content that does NOT need updating:**
- Layer 1 (structure, variables, directive body) — accurate and complete
- Layer 2 (evidence routing, when conditions, mutual exclusivity) — accurate and complete
- `evidence-routing-workflow.md` — no gates, so not affected by gate-transition roadmap
- SKILL.md execution loop (apart from `koto status` references) — accurate
- Troubleshooting section — accurate (except the `koto status` reference)

### Implications for Requirements

1. **`references/template-format.md` needs a Layer 3 expansion** covering:
   - `gates.*` routing syntax (path format `gates.<gate-name>.<field>`, semantics, when required vs. optional)
   - Structured output schemas per gate type
   - Pure-gate transitions (when clause with only `gates.*` keys, no `accepts` block required)
   - Mixed `gates.*` + agent evidence (still requires `accepts` block)
   - `override_default` gate field (what it is, resolution order, when to set it)
   - Brief note on `--allow-legacy-gates` (migration escape hatch, not permanent)

2. **`koto-templates/koto-author.md` needs two changes:**
   - Add D5 bullet to the `compile_validation` directive's error list
   - Update the `compile_validation` gate to use `gates.*` routing (change the transition from unconditional to `when: gates.template_exists.exists: true`) — this fixes the credibility problem and makes the template a correct demonstration

3. **`references/examples/complex-workflow.md` should be updated in place** (not replaced with a new file) to use `gates.*` routing in at least the `preflight` state. Updating both `preflight` and `build` is cleaner; both have gates, both should demonstrate the pattern. The `staging` and `test` states use evidence routing (not gates), so they stay as-is.

4. **SKILL.md `koto status` references** (lines 55 and 88) are live bugs requiring immediate fix regardless of other scope decisions. They should be fixed as part of the koto-author update.

5. **No new files needed** in koto-author for these updates. All content fits into existing files.

### Open Questions

1. **`compile_validation` gate update tradeoff**: Changing the `compile_validation` gate from unconditional to `gates.template_exists.exists: true` routing changes the semantic from "gate blocks → can't proceed" to "gate output routes the transition." These are different patterns. The unconditional advance (gate-blocked-or-proceed) is actually what most users want when a gate is purely a precondition. Should the example use the routing pattern even if it's not the natural fit here, or should we add a comment explaining the choice?

2. **Override mechanism split**: `override_default` is a template-author concern (goes in template-format.md). The agent-side trigger (`koto overrides record` + `agent_actionable`) is a koto-user concern. How much of the override mechanism should template-format.md cover? Specifically: should it mention `agent_actionable: true` in `blocking_conditions`, or leave that to koto-user?

3. **`SESSION_DIR` runtime variable** (finding 8 from prior research): add to Layer 1 "Variables" section? Low-severity but a genuine omission.

---

## Summary

`koto status` and `koto query` do not exist in the CLI — `koto status` is a live bug in koto-author SKILL.md (two references) and a phantom reference in CLAUDE.local.md. The correct replacement depends on context: `koto workflows` to see active workflows, `koto next` to get current state. The full koto-user CLI surface is confirmed (init, next, cancel, rewind, workflows, session dir/list/cleanup, decisions record/list, overrides record/list, context add/get/exists/list). All koto-author gaps fit into existing files without creating new reference files: `template-format.md` needs a Layer 3 expansion covering `gates.*` routing syntax, structured output schemas, `override_default`, and a note on `--allow-legacy-gates`; `complex-workflow.md` should be updated in place to demonstrate `gates.*` routing in its gate-bearing states; `koto-author.md` needs a D5 bullet in `compile_validation` and its own gate updated to use `gates.*` routing to fix a credibility problem.
