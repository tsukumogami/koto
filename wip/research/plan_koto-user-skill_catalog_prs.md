# PR Catalog: Recent koto changes vs. skill/doc coverage

This catalog records user-facing behavior introduced by recent koto PRs and
assesses whether each item is covered in the koto-author skill, cli-usage.md,
and what gaps exist for the future koto-user skill.

**Sources examined:**
- PR #91, #103, #109, #120, #122, #123, #125
- `plugins/koto-skills/skills/koto-author/SKILL.md`
- `plugins/koto-skills/skills/koto-author/references/template-format.md`
- `plugins/koto-skills/skills/koto-author/references/examples/complex-workflow.md`
- `plugins/koto-skills/skills/koto-author/koto-templates/koto-author.md`
- `plugins/koto-skills/AGENTS.md`
- `docs/guides/cli-usage.md`

---

## PR #91 — Eliminate double-call pattern for states without accepts block

**Title:** feat(engine): eliminate double-call pattern for states without accepts block

### User-facing behavior introduced

- New engine stop reason: `UnresolvableTransition`.
- States that have conditional transitions but no `accepts` block previously
  returned `EvidenceRequired` with empty `expects`, requiring a second `koto next`
  call to get a useful response. After this PR they return exit code 2 with a
  `template_error` JSON error naming the stuck state.
- Agents that saw `action: "evidence_required"` with `expects.fields == {}` must
  now check `action` before looking at `expects` — no more empty-expects
  "evidence required" responses. A state without an `accepts` block never produces
  `evidence_required`.
- The `advanced` field semantic clarification: first call after auto-advance from
  evidence submission now returns `advanced: true` from the same call.

### koto-author skill coverage

- The `template-format.md` feature-to-action mapping table correctly documents
  that `accepts` is required to get `evidence_required`. No explicit mention of
  the previous double-call workaround (appropriate — it was removed).
- No gap in koto-author.

### cli-usage.md coverage

- Error table includes `template_error` (exit 3) and describes "unresolvable
  transition" under `template_error` coverage.
- The `advanced` field is documented as informational only.
- Covered.

### koto-user gaps

- None from this PR alone; covered by AGENTS.md's dispatcher classification order
  and error table.

---

## PR #103 — Redesign koto next output contract

**Title:** feat(cli): redesign koto next output contract

### User-facing behavior introduced

**New `action` values** (replaces generic `"execute"`):
- `"evidence_required"` — state with `accepts` block
- `"gate_blocked"` — gates failed on state without `accepts`
- `"integration"` — state declares an integration, runner available
- `"integration_unavailable"` — integration runner not present
- `"confirm"` — default action ran, needs review
- `"done"` — terminal state

**`details` field:**
- States can be split with `<!-- details -->` HTML comment in template body.
- Text before the marker = `directive` (always returned).
- Text after = `details` (returned on first visit only, or with `--full`).
- `--full` flag on `koto next` forces `details` inclusion on every visit.

**`blocking_conditions` on `evidence_required`:**
- `blocking_conditions` is now present (always, as an array) on `evidence_required`
  responses, not only on `gate_blocked`.
- When gates fail on a state with an `accepts` block, `blocking_conditions` is
  populated. Fix gates, then submit evidence.

**Error code restructure:**
- `template_error` (exit 3): structural template problems
- `persistence_error` (exit 3): disk I/O failures
- `concurrent_access` (exit 1): another `koto next` already running

**`--full` flag** on `koto next`.

**`--no-cleanup` flag** on `koto next` (skip auto-cleanup at terminal state).

### koto-author skill coverage

**`template-format.md`:**
- `<!-- details -->` marker: covered in Layer 1 ("The `<!-- details -->` marker"
  section).
- Feature-to-action mapping table: covered.
- No mention of `--allow-legacy-gates` or D5 error (those come from later PRs).

**`koto-author.md` (skill template):**
- Uses `<!-- details -->` in `state_design` and `template_drafting` sections.
- SKILL.md describes the `action` values and `details` field behavior.

**Gap in koto-author:** None from this PR specifically. Covered.

### cli-usage.md coverage

- All six `action` values documented with full response shapes.
- `details` field: documented, including `--full` flag.
- `blocking_conditions` on `evidence_required`: documented.
- `--no-cleanup` flag: documented.
- Error codes: all three categories covered.
- **Covered.**

### koto-user gaps

- The `confirm` action shape: AGENTS.md documents it. cli-usage.md documents it.
  A future koto-user skill needs to teach agents to read `action_output` and
  optionally submit evidence.
- `--no-cleanup` use case (debugging): not in AGENTS.md; koto-user skill should
  mention this as a debugging escape hatch.

---

## PR #109 — Add koto-author skill for template authoring

**Title:** feat(koto-skills): add koto-author skill for template authoring

### User-facing behavior introduced

- New skill: `koto-author`. Agents invoke it to author new or convert existing
  koto-backed skills.
- Template format guide (`references/template-format.md`) added with Layers 1–3.
- Example templates added: `evidence-routing-workflow.md`, `complex-workflow.md`.
- `hello-koto` placeholder skill removed.
- CI change: `references/` directories excluded from template compilation scan.

### koto-author skill coverage

- This PR *is* the koto-author skill creation. Fully covered by definition.

### cli-usage.md coverage

- No CLI changes in this PR. N/A.

### koto-user gaps

- The koto-author skill provides template authoring guidance. It has no
  equivalent for agents *running* workflows. A koto-user skill must cover the
  consumer side: initializing a workflow, reading `koto next` responses, submitting
  evidence, and handling all action variants.

---

## PR #120 — Structured gate output (Feature 1 of gate-transition contract)

**Title:** feat: structured gate output (Feature 1 of gate-transition contract)

### User-facing behavior introduced

**`StructuredGateResult` replaces boolean `GateResult`:**
- Each gate now returns structured JSON output instead of pass/fail boolean.
- Gate output is injected into the resolver evidence map under `gates.<gate_name>.*`.

**`blocking_conditions[].output` field:**
- Each entry in `blocking_conditions` now has an `output` field with structured
  gate-type-specific data.
- Shape varies by gate type:

| Gate type | `output` fields |
|-----------|----------------|
| `command` | `exit_code: number`, `stdout: string`, `stderr: string` |
| `context-exists` | `exists: boolean` |
| `context-matches` | `exists: boolean`, `matched: boolean`, `pattern: string` |

**`blocking_conditions[].status` field:**
- Values: `"failed"`, `"timed_out"`, `"error"` (not a boolean).

**`gates.*` dot-path routing in `when` conditions:**
- Template `when` clauses can now reference gate output via `gates.<gate_name>.<field>`:

```yaml
transitions:
  - target: next_state
    when:
      gates.my_gate.exit_code: 0
```

- The resolver supports three-segment dot-paths (`gates.<name>.<field>`).
- This is the main new template-authoring feature from this PR.

**`GateOutcome` enum:**
- `"passed"`, `"failed"`, `"timed_out"`, `"error"` — used in `status` field of
  `blocking_conditions` entries.

### koto-author skill coverage

**`template-format.md` Layer 3 (Gates):**
- Covers `context-exists`, `context-matches`, `command` gate types.
- Does NOT cover `gates.*` dot-path routing syntax in `when` conditions.
- Does NOT cover the structured `output` field in `blocking_conditions`.
- Does NOT cover `GateOutcome` status values (`timed_out`, `error` in addition
  to `failed`).

**`complex-workflow.md` example:**
- Uses gates but does NOT use `gates.*` when-clause routing. All transitions are
  evidence-routing only. This means the example is a "legacy gate" template (see
  PR #125).

**Gap in koto-author:** Layer 3 must document `gates.*` dot-path routing as a
first-class pattern. The `complex-workflow.md` example should be updated or a
new example added that demonstrates structured gate routing. The `output` field
shape for each gate type should be in the reference.

### cli-usage.md coverage

- `blocking_conditions` entry format is documented with `output` field in the
  `GateBlocked` response example.
- Gate output schemas are mentioned with a cross-reference to
  `custom-skill-authoring.md`.
- `status` values (`"failed"`, `"timed_out"`, `"error"`) documented.
- **Covered** for consumers; the `gates.*` routing syntax is in
  `custom-skill-authoring.md` (as documented in the PR).

### koto-user gaps

- `blocking_conditions[].output` field shape: agents need to know what fields
  to read for each gate type to understand *why* a gate failed. AGENTS.md
  mentions `blocking_conditions` exists but doesn't show the `output` field at
  all (only `name`, `type`, `status`, `agent_actionable`).
- `agent_actionable` field: present in AGENTS.md blocking_conditions example but
  its meaning (whether a default or override is available) is not explained.
  This becomes more important after PR #122.

---

## PR #122 — Add gate override mechanism

**Title:** feat(engine): add gate override mechanism

### User-facing behavior introduced

**New CLI commands:**

```bash
koto overrides record <name> --gate <gate_name> --rationale "<text>"
koto overrides list <name>
```

**`koto overrides record`:**
- Bypasses a blocking gate for the current epoch (until the next state
  transition).
- Records rationale and the gate's actual last output in the event log.
- Gate is treated as `Passed` on the next `koto next` call.
- Override expires automatically at the next state transition (epoch-scoped).

**`koto overrides list`:**
- Returns full override history across the session, including overrides recorded
  before rewinds.
- Output is a JSON array.

**`agent_actionable` flag in `blocking_conditions`:**
- Set to `true` when the blocked gate has a default value available (either an
  `override_default` defined in the template, or a built-in type default).
- Signals to the agent that `koto overrides record` is an option.

**`override_default` on `Gate` struct (template field):**
- Template authors can specify a default value object to use when the gate is
  overridden:

```yaml
gates:
  ci_check:
    type: command
    command: "ci-status.sh"
    override_default:
      exit_code: 0
      stdout: "override applied"
      stderr: ""
```

**Built-in defaults for each gate type:**
- `command`: `{exit_code: 0, stdout: "", stderr: ""}`
- `context-exists`: `{exists: true}`
- `context-matches`: `{exists: true, matched: true, pattern: "<original pattern>"}`

**`"gates"` reserved key in `--with-data`:**
- Passing `"gates"` as a top-level key in `--with-data` is rejected with a
  `precondition_failed` error. Gate output is injected by the engine, not
  submitted by agents.

**New event types in state log:**
- `GateEvaluated`: emitted after each gate check (non-overridden gates).
- `GateOverrideRecorded`: appended by `koto overrides record`.

### koto-author skill coverage

**`template-format.md`:**
- Does NOT mention `override_default` as a field on gates.
- Does NOT mention built-in defaults.
- Does NOT mention when/why a template author would set `override_default`.

**Gap in koto-author:** Layer 3 (Gates) should document `override_default` as an
optional gate field that controls the value injected when the gate is overridden.
Authors should know this determines `agent_actionable: true` and what value the
engine uses for `gates.*` routing after an override.

### cli-usage.md coverage

- `koto overrides record` and `koto overrides list` commands: **NOT documented**
  in `cli-usage.md`. The file has no `overrides` subcommand section.
- The `agent_actionable` field in `blocking_conditions` examples: **NOT shown**
  in the `cli-usage.md` `GateBlocked` example (only `name`, `type`, `status`,
  `output` appear).

**Gap in cli-usage.md:** Missing `overrides record` and `overrides list` command
documentation with syntax, required flags, and example output.

### koto-user gaps

- `koto overrides record` / `koto overrides list`: agents need to know when to
  use overrides (when `agent_actionable: true`), the exact command syntax, and
  that overrides are epoch-scoped.
- `agent_actionable: true` meaning: must be explained — it means `koto overrides
  record` is a valid option to unblock the gate.
- Override lifecycle: expires at next state transition, recorded in event log,
  visible in `koto overrides list` even after rewind.
- The `"gates"` reserved key rejection: agents must not attempt to set `gates`
  fields in `--with-data`.

---

## PR #123 — Validate gate contracts at compile time

**Title:** feat(template): validate gate contracts at compile time

### User-facing behavior introduced

**Three new compile-time validation passes (D2, D3, D4):**

**D2 — `override_default` schema validation:**
- If a gate has `override_default`, all required fields for that gate type must
  be present, no extra fields allowed, and value types must match the schema.
- Error names the state, gate, and specific field.

**D3 — `gates.*` when-clause path validation:**
- Enforces three-segment format for `gates.*` paths.
- Gate name must exist in the state's gate declarations.
- Field name must exist in the gate type's schema.
- Error names the state, gate, and field.

**D4 — reachability check:**
- Applies `override_default` or built-in defaults to all gates and checks that
  at least one transition can fire under those values.
- Gates whose fields are never referenced in any `when` clause emit a non-fatal
  stderr warning.
- Mixed-evidence states (at least one non-`gates.*` when-clause field per
  transition) are exempt.

All three passes run only under `koto template compile` (strict mode). `koto init`
and `koto next` warn and proceed.

### koto-author skill coverage

**`template-format.md`:**
- Does NOT mention D2, D3, or D4 validation errors.
- Does NOT describe what happens when `override_default` has wrong field types.
- Does NOT describe the D4 reachability check or the non-fatal "field never
  referenced" warning.

**`compile_validation` state in `koto-author.md` template:**
- Lists common compile errors (missing transition target, non-mutually-exclusive
  routing, invalid regex, unreferenced variables, missing directive section).
- Does NOT include D2 or D3 errors as fix categories.

**Gap in koto-author:** The compile_validation state directive should document
D2 and D3 error patterns and their fixes. `template-format.md` Layer 3 should
describe the `override_default` schema rules and the `gates.*` path format
validation that D3 enforces. This helps authors write valid gate contracts on
the first attempt.

### cli-usage.md coverage

- `koto template compile` section documented with `--allow-legacy-gates` flag
  (from PR #125, see below) but the D2/D3/D4 error names and what they catch are
  not described. The page says it "exits non-zero with a JSON error on compilation
  failure" without detailing the error categories.

**Gap in cli-usage.md:** Minor — the D-code error categories are design-doc
detail, not necessarily CLI-usage detail. The `--allow-legacy-gates` flag (from
PR #125) is the more important addition.

### koto-user gaps

- None directly. Compile-time validation is an authoring concern, not a
  workflow-running concern. Agents running workflows only see the runtime
  behavior, not compile errors.

---

## PR #125 — Gate backward compatibility for legacy gate templates

**Title:** feat: gate backward compatibility for legacy gate templates

### User-facing behavior introduced

**`--allow-legacy-gates` flag on `koto template compile`:**
- Without this flag: a state that has gates but no `gates.*` when-clause
  references fails with a D5 error (strict mode default).
- With `--allow-legacy-gates`: same template compiles successfully (permissive
  mode).
- The flag is explicitly transitory — it will be removed once legacy templates
  migrate.

**Strict vs. permissive mode distinction:**
- `koto template compile` runs in strict mode: D5 error on legacy gate states.
- `koto init`, `koto next`, `koto export`: run in permissive mode: D5 condition
  emits a warning to stderr and proceeds. D4 reachability warnings are also
  suppressed in permissive mode.
- `compile_cached` (used by `koto init` and `koto next` internally) uses
  permissive mode.

**Legacy gate behavior at runtime:**
- Templates without `gates.*` when-clause references see no `gates` key in the
  resolver evidence map. Boolean pass/block behavior is preserved — gates still
  block or pass, but structured output is not surfaced in routing.

**D5 error:**
- Error code name: D5.
- Triggered by: a state with gates where no `when` clause references `gates.*`.
- Fix: add `gates.*` when-clause references, or compile with `--allow-legacy-gates`
  during migration window.

### koto-author skill coverage

**`template-format.md`:**
- Does NOT mention D5 error.
- Does NOT mention `--allow-legacy-gates` flag.
- Does NOT distinguish strict vs. permissive compile modes.

**`koto-author.md` compile_validation state:**
- Does NOT mention `--allow-legacy-gates` or D5 error in the fix list.

**`complex-workflow.md` example:**
- Uses gates WITHOUT `gates.*` routing — this is a legacy gate template. Under
  strict compile mode (`koto template compile`), it would fail with D5.
- **This is the most concrete koto-author gap:** the primary "advanced" example
  demonstrates a pattern that fails strict compilation.

**Gap in koto-author:** Critical. `complex-workflow.md` must either be updated
to use `gates.*` routing or the example must note it is a legacy-gate template
requiring `--allow-legacy-gates`. Preferred: update to use structured routing so
it compiles clean. `template-format.md` Layer 3 should document the D5 rule and
`--allow-legacy-gates`.

### cli-usage.md coverage

- `koto template compile` section does NOT mention `--allow-legacy-gates`.
- The `template compile` section only says: "Compiles a source template...
  Exits non-zero with a JSON error on compilation failure."

**Gap in cli-usage.md:** `--allow-legacy-gates` flag is missing from the
`template compile` command documentation.

### koto-user gaps

- Agents running workflows (not authoring templates) interact with `koto init`
  and `koto next`, which use permissive mode. They will see warnings to stderr
  for legacy gate templates but the workflow will proceed.
- koto-user skill should note: if a template produces gate warnings on stderr,
  this is expected for legacy gate templates and does not indicate a problem.

---

## Cross-PR summary: gap assessment

### koto-author skill gaps (priority order)

| Priority | File | Gap |
|----------|------|-----|
| Critical | `references/examples/complex-workflow.md` | Uses legacy gate pattern (no `gates.*` routing); fails D5 under strict compile. Must be updated or flagged. |
| High | `references/template-format.md` Layer 3 | Missing: `gates.*` dot-path routing syntax in `when` conditions; `override_default` gate field; built-in gate defaults; D5 rule and `--allow-legacy-gates`. |
| High | `koto-templates/koto-author.md` compile_validation state | Missing D2, D3, D5 error categories and their fixes in the directive. |
| Medium | `references/template-format.md` Layer 3 | Missing: `blocking_conditions[].output` field shape per gate type (useful context for template authors designing `gates.*` routing). |
| Low | `references/template-format.md` Layer 3 | D4 reachability warning (non-fatal) not documented. |

### cli-usage.md gaps

| Priority | Section | Gap |
|----------|---------|-----|
| High | `template compile` | `--allow-legacy-gates` flag undocumented. |
| High | (missing section) | `koto overrides record` and `koto overrides list` commands entirely absent. |
| Low | `koto next` GateBlocked example | `agent_actionable` field not shown in example (field exists per AGENTS.md). |

### koto-user skill gaps (content to create)

The koto-user skill does not exist. Based on this PR catalog, it needs to cover:

| Topic | Source PRs | Notes |
|-------|-----------|-------|
| `koto init` syntax with `--template` and `--var` | #103 | Covered in AGENTS.md; needs first-class coverage |
| `koto next` response dispatch on `action` field | #103, #91 | All six action values, with response shape for each |
| `details` field and `--full` flag | #103 | First-visit vs. repeat-visit behavior |
| `blocking_conditions` on `evidence_required` | #103, #120 | Fix gates before submitting evidence |
| `blocking_conditions[].output` field shapes | #120 | Per gate type: `command`, `context-exists`, `context-matches` |
| `agent_actionable: true` meaning | #122 | Signals override is available |
| `koto overrides record` / `koto overrides list` | #122 | When to use, syntax, epoch scope, audit trail |
| `"gates"` reserved key in `--with-data` | #122 | Must not be submitted by agents |
| `advanced` field semantics | #103 | Informational only; dispatch on `action` |
| `koto rewind` | all | Existing but must be in koto-user |
| Legacy gate stderr warnings at runtime | #125 | Expected for templates without `gates.*` routing |
| `--no-cleanup` flag | #103 | Debugging escape hatch |
| Error codes and exit codes | #103 | Full table, agent action per code |
| `koto decisions record` / `koto decisions list` | (AGENTS.md) | Documented in AGENTS.md; koto-user skill should include |
| `koto workflows` | (existing) | List active sessions; already in AGENTS.md |
