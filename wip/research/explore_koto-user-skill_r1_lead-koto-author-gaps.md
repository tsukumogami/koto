# Lead: koto-author gaps after gate-transition roadmap

## Findings

### 1. `gates.*` routing — entirely absent from template-format.md

The single largest gap. The roadmap introduced `gates.*` when-clause routing: a transition's `when` block can reference `gates.<gate-name>.<field>` paths to branch on the structured output of a gate (e.g., `gates.ci.exit_code: 0`). This is now the expected pattern for all gate-bearing states. Templates that omit it compile with a D5 warning in permissive mode and fail with a hard error in strict mode (`koto template compile` without `--allow-legacy-gates`).

**Affected file:** `references/template-format.md` — Layer 3 "Gates" section.

**What's missing:** The Layer 3 gates table and examples show only pass/block semantics. There is no mention of:
- `gates.<name>.<field>` path syntax in `when` clauses
- Which fields are available per gate type (see below)
- That a `when` clause referencing only `gates.*` keys is valid even without an `accepts` block
- That mixing `gates.*` and agent-evidence keys in the same `when` clause still requires an `accepts` block

**Source:** `src/template/types.rs` lines 535–574 (D5 detection), lines 608–660 (reachability), lines 718–754 (when-clause parsing and D3 validation). `src/cache.rs` lines 87–89 (strict flag description).

**Severity: high.** An agent following the current guide will write gates that compile only because the cache uses permissive mode, but `koto template compile` without the flag will reject them. The agent will not know to add `gates.*` routing.

---

### 2. Structured gate output schemas — not documented

Each gate type produces a typed JSON object that is injected into evidence under the `gates` namespace. The schemas are:

| Gate type | Output fields |
|-----------|--------------|
| `command` | `exit_code` (number), `error` (string) |
| `context-exists` | `exists` (boolean), `error` (string) |
| `context-matches` | `matches` (boolean), `error` (string) |

**Affected file:** `references/template-format.md` — Layer 3 "Gates" table.

**What's missing:** The current table (`key`, `type`, "Passes when") has no column for structured output. An agent authoring a `when` clause like `gates.ci.exit_code: 0` needs to know which fields exist and their types.

**Source:** `src/gate.rs` (functions `evaluate_command_gate`, `evaluate_context_exists_gate`, `evaluate_context_matches_gate` — JSON literals show exact schemas). `src/template/types.rs` `gate_type_schema()` lines 176–184.

**Severity: high.** Without the schema, the agent can't write valid `gates.*` when conditions.

---

### 3. `override_default` gate field — not documented

Gates accept an optional `override_default` field (a JSON object matching the gate's output schema). When present, it serves as the fallback value for `koto overrides record` when `--with-data` is not supplied. The resolution order is: `--with-data` → `override_default` → built-in default for gate type.

**Affected file:** `references/template-format.md` — Layer 3 "Gates" section and/or a new "Override mechanism" section.

**What's missing:** No mention of `override_default` as a gate field that template authors can set. The current gates table shows only `type`, `command`/`key`/`pattern` fields.

**Source:** `src/template/types.rs` lines 101–108 (`Gate.override_default` field). `src/cli/overrides.rs` `resolve_override_applied()` lines 54–73.

**Severity: high.** Authors who want to customize what the override mechanism applies (e.g., a non-default exit code) have no way to discover this field.

---

### 4. `koto overrides record` / `koto overrides list` — not documented

The override mechanism is entirely absent from koto-author. The CLI has two new subcommands under `koto overrides`:

- `koto overrides record <name> --gate <name> --rationale <text> [--with-data <json>]` — records a `GateOverrideRecorded` event, substituting a blocked gate's output with the override value.
- `koto overrides list <name>` — returns all recorded overrides across the full session history.

**Affected file:** `references/template-format.md` — a new "Override mechanism" section. Also `SKILL.md` "What to expect" or "Reference material" sections.

**What's missing:** No coverage of when and how agents use `koto overrides record`, what `agent_actionable: true` in `blocking_conditions` means, or what `--with-data` expects.

**Source:** `src/cli/overrides.rs` (full file). `src/cli/next_types.rs` lines 354–365 (`BlockingCondition` struct with `agent_actionable` and `output` fields). `src/cli/mod.rs` lines 139–143 (CLI command declaration).

**Severity: high.** Agents running workflows with non-passable gates (e.g., environment constraints) can't discover that `koto overrides record` is the resolution path, or that `agent_actionable: true` in `blocking_conditions` signals it's available.

---

### 5. `blocking_conditions` structured output — partially documented, details missing

`SKILL.md` mentions `blocking_conditions` in the execution loop description (step 2) but only as "Read `blocking_conditions` for what's failing." The actual schema is richer:

```json
{
  "name": "ci_check",
  "type": "command",
  "status": "failed",
  "agent_actionable": true,
  "output": {"exit_code": 1, "error": ""}
}
```

**Affected file:** `SKILL.md` execution loop section. Also `references/template-format.md`.

**What's missing:** The `output` field (gate-type-specific structured data), `agent_actionable` flag and its meaning, and `status` values (`failed`, `timed_out`, `error`).

**Source:** `src/cli/next_types.rs` lines 354–365 (`BlockingCondition` struct). `src/cli/next_types.rs` lines 405–435 (`blocking_conditions_from_gates()` which shows how `agent_actionable` is set).

**Severity: medium.** The agent knows a gate is blocking but can't programmatically read its output or know whether it can use the override mechanism.

---

### 6. `--allow-legacy-gates` flag — not documented in compile_validation state

The `koto template compile` command gained an `--allow-legacy-gates` flag that suppresses D5 errors for templates with gates but no `gates.*` routing. The koto-author template's `compile_validation` state directive lists common error types to fix but omits this flag and the D5 error.

**Affected file:** `koto-templates/koto-author.md` — `compile_validation` state directive. Also `references/template-format.md` — a note on the compile subcommand.

**What's missing:** Mention of `--allow-legacy-gates`, when to use it (legacy templates being migrated), and the D5 error code ("gate has no `gates.*` routing").

**Source:** `src/cli/mod.rs` lines 319–326 (`--allow-legacy-gates` arg declaration and TODO comment).

**Severity: medium.** An agent following the compile_validation directive won't know what to do when D5 fires. It will not recognize the error and the three-attempt loop will exhaust without a fix path.

---

### 7. `complex-workflow.md` example uses legacy gate pattern

The example at `references/examples/complex-workflow.md` has two states with gates (`preflight` with a `command` gate, `build` with a `context-exists` gate) but no `gates.*` when-clause routing in either state. Both advance unconditionally after the gate passes. This is the legacy boolean pass/block pattern — it compiles only in permissive mode and would trigger D5 warnings.

**Affected file:** `references/examples/complex-workflow.md`

**What's missing:** At least one example state should demonstrate `gates.*` routing: a `when` condition that branches on `gates.<name>.exit_code` or `gates.<name>.exists`. Currently the example illustrates gates only in their legacy form.

**Source:** Reading the example file against the D5 check in `src/template/types.rs` lines 535–574. The koto-author.md template itself has the same issue: its `compile_validation` state has a `context-exists` gate with no `gates.*` when routing.

**Severity: medium.** Agents reading this example will model their own templates after it and produce D5 warnings (or errors in strict mode).

---

### 8. `SESSION_DIR` runtime variable — not documented

The compiler allows two undeclared runtime variable names: `SESSION_NAME` (mentioned in template-format.md) and `SESSION_DIR` (not mentioned). Template authors can use `{{SESSION_DIR}}` in directives to reference the session directory path without declaring it as a variable.

**Affected file:** `references/template-format.md` — Layer 1 "Variables" section.

**What's missing:** `SESSION_DIR` is listed in `RUNTIME_VARIABLE_NAMES` in types.rs but only `SESSION_NAME` is documented in the skill.

**Source:** `src/template/types.rs` line 266.

**Severity: low.** Omitting it just means authors don't know the convenience variable exists. It doesn't cause compile failures.

---

## Implications

The `gates.*` routing gap (findings 1–3) is the most consequential. It's the central new behavior that the gate-transition roadmap introduced, and koto-author's Layer 3 documentation gives agents no way to discover it. An agent following koto-author today will produce legacy-style templates (gates only used for pass/block, no structured output routing), which will trigger compiler warnings and fail in strict mode.

The override mechanism gap (finding 4) matters most for koto-user, but template authors also need to know about `override_default` to design gates that interact gracefully with the override flow.

The fact that koto-author's own template (`koto-templates/koto-author.md`) uses legacy gate syntax (finding 7) is a credibility problem: the skill instructs agents to write `gates.*` routing, but its own template doesn't demonstrate it.

These findings define a concrete update scope:
- Layer 3 of `template-format.md` needs a structured gate output table and `gates.*` routing syntax with examples.
- An override mechanism section needs to be added to `template-format.md`.
- `complex-workflow.md` needs at least one state updated to use `gates.*` routing.
- `koto-templates/koto-author.md` needs the `compile_validation` gate updated to use `gates.*` routing or explicitly flagged as a legacy-gate example with an explanation.
- The `compile_validation` directive needs a D5 error entry.

---

## Surprises

The koto-author template itself is a legacy-gate template. The skill presents itself as a mid-complexity reference (SKILL.md line 117) but would trigger the D5 warning it should be teaching agents to avoid. This is the clearest signal that koto-author was last updated before the gate-transition roadmap landed.

The `--allow-legacy-gates` flag has a TODO comment in `src/cli/mod.rs` noting it's transitory and will be removed once the shirabe `work-on` template migrates. This means the flag is not intended to be a permanent escape hatch — agents shouldn't be instructed to routinely use it; they should learn `gates.*` routing instead.

The `gates.*` when clause can appear without an `accepts` block (pure gate routing), which is a pattern that template-format.md doesn't describe at all. This enables a state that branches solely on gate output without requiring agent evidence submission.

---

## Open Questions

1. Should `koto-templates/koto-author.md` be updated to use `gates.*` routing on the `compile_validation` gate, or documented as an intentional legacy example with `--allow-legacy-gates`? The former is cleaner but requires deciding what field to branch on (the gate is `context-exists`, so `gates.template_exists.exists: true`).

2. The override mechanism documentation belongs in both koto-author (for the `override_default` gate field) and koto-user (for the `koto overrides record` command). How much overlap is acceptable vs. duplication?

3. The `SESSION_DIR` variable (finding 8) — is it stable enough to document, or is it likely to change?

4. Should the `complex-workflow.md` example be updated in place, or should a new `gates-routing-workflow.md` example be added that demonstrates the full structured gate output pattern while leaving the existing example as-is for simpler cases?

---

## Summary

koto-author's Layer 3 documentation and example templates predate the gate-transition roadmap; none of the three key additions (`gates.*` routing syntax, structured gate output schemas, `override_default` field, or the `koto overrides` CLI) appear anywhere in the skill. The main implication is that agents following koto-author today will produce legacy-style gate templates that trigger D5 warnings in strict mode, with no guidance on how to fix them. The biggest open question is whether to update koto-author's own template (`compile_validation` state) to demonstrate `gates.*` routing, or to document it explicitly as a legacy-gate example — that decision determines the scope of the example-file changes.
