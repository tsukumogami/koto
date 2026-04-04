# Koto template format

This guide covers the koto template format in three layers: structure, evidence routing, and advanced features. Each layer builds on the previous one. Snippets are minimal -- see the linked examples for complete, compilable templates.

## Layer 1: Structure

A koto template is a markdown file with YAML frontmatter defining the state machine and body sections containing directive text for each state.

### Frontmatter schema

The frontmatter declares the machine's shape:

```yaml
---
name: my-workflow
version: "1.0"
description: What this workflow does
initial_state: first_state

variables:
  MY_VAR:
    description: Explain what this variable is for
    required: true

states:
  first_state:
    transitions:
      - target: second_state
  second_state:
    transitions:
      - target: done
  done:
    terminal: true
---
```

Required fields: `name`, `version`, `initial_state`, `states`.

Optional fields: `description`, `variables`.

### Variables

Variables are declared at the root level and interpolated into directive text using `{{VARIABLE_NAME}}` syntax. The agent supplies values at init time via `--var KEY=VALUE`. Each variable has a `description` and a `required` flag.

Koto also provides two built-in variables that don't need to be declared:

- `{{SESSION_NAME}}` -- the active session name
- `{{SESSION_DIR}}` -- the session directory path

### States

Each state is a key under `states:`. A state can have:

| Field | Type | Purpose |
|-------|------|---------|
| `transitions` | list | Where the machine can go next |
| `gates` | map | Conditions checked before transitioning |
| `accepts` | map | Evidence schema for agent-submitted data |
| `terminal` | bool | Marks this as an end state |

Every non-terminal state needs at least one transition. Terminal states need `terminal: true` and no transitions.

### Transitions

Transitions are a list of objects with a `target` field and an optional `when` condition:

```yaml
transitions:
  - target: next_state
```

When there's only one transition and no conditions, the machine advances unconditionally (after gates pass). We'll cover conditional transitions in Layer 2.

### Directive body sections

Below the frontmatter, each state gets a markdown section headed by `## state_name`. This is the directive text the agent receives when it calls `koto next` in that state.

```markdown
## first_state

Clone {{REPO_URL}} and check out the {{BRANCH}} branch.

## done

Repository is ready.
```

Every state declared in the frontmatter must have a corresponding body section. The compiler will reject templates with missing sections.

### The `<!-- details -->` marker

A directive section can be split into two parts using the `<!-- details -->` HTML comment:

```markdown
## state_design

Define the full state machine: states, transitions, evidence routing, gates, and variables.

<!-- details -->

Read the template format guide at `${CLAUDE_SKILL_DIR}/references/template-format.md`. Read only the layers you need:

- **Layer 1 (Structure)** -- always read this.
- **Layer 2 (Evidence routing)** -- read if your workflow has decision points.
- **Layer 3 (Advanced)** -- read if you need gates, self-loops, or split topology.

Work through the design in this order:

1. List every distinct phase in your workflow.
2. Draw the transitions between them.
3. Identify decision points -- these need evidence routing.
4. Identify retry loops -- these need self-loops.
5. Identify preconditions -- these need gates.
```

Content before the marker is the **directive** -- always returned by `koto next`. Content after is the **details** -- returned only on first visit to the state, or when the caller passes `--full`.

Use details for multi-paragraph instructions, step-by-step procedures, or reference material that clutters the directive on repeat visits. Keep the directive itself short: a one- or two-line summary of what the state expects.

States without the marker behave exactly as before -- everything is the directive, and `details` is empty.

If a section contains multiple `<!-- details -->` markers, only the first one counts. Everything after the first marker is details.

### Feature-to-action mapping

Different template features produce different `action` values in the `koto next` response. This table shows what the caller sees for each feature:

| Template feature | Caller sees `action` |
|-----------------|---------------------|
| State with `accepts` block | `evidence_required` |
| State with failing `gates` (no accepts) | `gate_blocked` |
| State with `integration` | `integration` or `integration_unavailable` |
| Terminal state (`terminal: true`) | `done` |
| State with `default_action` + `requires_confirmation` | `confirm` |

Knowing these values helps you predict how callers will interact with each state. A state with an `accepts` block always surfaces as `evidence_required` -- the caller's automation can key on that string to know it needs to submit data.

## Layer 2: Evidence routing

Evidence routing lets the agent submit structured data that determines which transition fires. This is how you build branching workflows.

### The accepts block

Define an `accepts` block on a state to declare what fields the agent can submit:

```yaml
states:
  triage:
    accepts:
      severity:
        type: enum
        values: [critical, normal, low]
        required: true
      notes:
        type: string
        required: false
    transitions:
      - target: escalate
        when:
          severity: critical
      - target: process
        when:
          severity: normal
      - target: backlog
        when:
          severity: low
```

### Field types

| Type | Requires | Notes |
|------|----------|-------|
| `enum` | `values` list | Agent must submit one of the listed values |
| `string` | -- | Free-form text |
| `number` | -- | Numeric value |
| `boolean` | -- | True or false |

All fields support `required: true/false`.

### The when condition

A `when` block on a transition matches against submitted evidence. The transition fires only if all fields in the `when` block match the submitted values (AND semantics).

```yaml
transitions:
  - target: deploy
    when:
      result: pass
      environment: production
```

This transition fires only when `result` is `pass` AND `environment` is `production`.

A transition without a `when` block is unconditional -- it fires if no conditional transition matches first. Use this as a fallback:

```yaml
states:
  process:
    accepts:
      outcome:
        type: enum
        values: [success, error, unknown]
        required: true
    transitions:
      - target: complete
        when:
          outcome: success
      - target: failed
        when:
          outcome: error
      - target: review
```

Here, `outcome: success` goes to `complete`, `outcome: error` goes to `failed`, and anything else (including `unknown`) falls through to `review`.

### Mutual exclusivity

The compiler enforces that conditional transitions don't overlap. For any pair of conditional transitions from the same state, at least one shared field must have different values. If two transitions could both match the same evidence, compilation fails.

This is valid (the `verdict` field differentiates):

```yaml
transitions:
  - target: approved
    when:
      verdict: approve
  - target: rejected
    when:
      verdict: reject
```

This would fail (both transitions match when `status` is `ready`):

```yaml
# WON'T COMPILE
transitions:
  - target: fast_track
    when:
      status: ready
  - target: normal_track
    when:
      status: ready
```

See [evidence-routing-workflow.md](examples/evidence-routing-workflow.md) for a full compilable template using this pattern.

## Layer 3: Advanced features

### Gates

Gates are preconditions evaluated before any transition fires. A state can have multiple gates -- all must pass before the engine attempts transition resolution.

| Type | Passes when | Required fields |
|------|-------------|-----------------|
| `context-exists` | A key exists in the context store | `key` |
| `context-matches` | Content for a key matches a regex | `key`, `pattern` |
| `command` | A shell command exits 0 | `command` |

```yaml
gates:
  plan_ready:
    type: context-exists
    key: plan.md
  plan_has_steps:
    type: context-matches
    key: plan.md
    pattern: "^## Step \\d+"
```

### Gate output fields

Each gate type produces structured output that the engine injects into the evidence map under the `gates.<gate_name>` namespace. Use these fields in `when` conditions to route on gate results.

| Gate type | Field | Type | Meaning |
|-----------|-------|------|---------|
| `command` | `exit_code` | number | Process exit code. `0` = passed; positive = failed; `-1` = timed out or spawn error. |
| `command` | `error` | string | Empty on normal pass or fail. `"timed_out"` on timeout. OS error message on spawn failure. |
| `context-exists` | `exists` | boolean | `true` if the key was found in the context store. |
| `context-exists` | `error` | string | Empty on normal pass or fail. Error message when the context store is unavailable. |
| `context-matches` | `matches` | boolean | `true` if the content at `key` matches `pattern`. |
| `context-matches` | `error` | string | Empty on normal pass or fail. Error message when the store is unavailable or the pattern is invalid. |

`passed` is not a field name in any gate type. Don't use it in `when` conditions.

### Routing on gate output (`gates.*` paths)

Reference gate output in `when` conditions using `gates.<gate_name>.<field>`. When at least one `when` clause on a state references a `gates.*` key, the engine injects gate outputs and resolves transitions automatically -- no agent action is needed.

**`command` gate routing on exit code:**

```yaml
states:
  check:
    gates:
      ci_check:
        type: command
        command: "cargo test"
    transitions:
      - target: passed
        when:
          gates.ci_check.exit_code: 0   # gate passed
      - target: failed
        when:
          gates.ci_check.exit_code: 1   # gate failed with exit code 1
```

The engine evaluates `ci_check`, injects `gates.ci_check.exit_code` and `gates.ci_check.error` into the evidence map, and resolves the matching transition. No agent submission required.

**`context-exists` gate routing on existence:**

```yaml
states:
  await_doc:
    gates:
      doc_check:
        type: context-exists
        key: research/lead.md
    transitions:
      - target: proceed
        when:
          gates.doc_check.exists: true    # key present, advance
      - target: await_doc                 # self-loop: wait for the key
        when:
          gates.doc_check.exists: false
```

**Path format rules:**

- Exactly three dot-separated segments: `gates.<gate_name>.<field>`.
- `<gate_name>` must be declared in the same state's `gates` block.
- `<field>` must be a valid output field for that gate type.
- The compiler enforces all three rules (D3 check) and rejects malformed paths.
- Agents can't submit evidence with a `gates.*` key -- the engine rejects it.

### `override_default` on gate declarations

Add `override_default` to a gate to control what value the engine uses when an operator records an override with `koto overrides record`. It must be a JSON object matching the gate type's output schema exactly.

```yaml
gates:
  ci_check:
    type: command
    command: "cargo test"
    override_default:
      exit_code: 0
      error: ""
```

When `koto overrides record` runs, the value to inject is resolved in this order:

1. `--with-data <json>` supplied on the command line (highest priority)
2. `override_default` declared on the gate
3. Built-in default for the gate type (lowest priority)

Built-in defaults for all three gate types:

| Gate type | Built-in default |
|-----------|-----------------|
| `command` | `{"exit_code": 0, "error": ""}` |
| `context-exists` | `{"exists": true, "error": ""}` |
| `context-matches` | `{"matches": true, "error": ""}` |

All three built-in types always have a built-in default, so `koto overrides record` always succeeds for them without `--with-data` or `override_default`. Setting `override_default` is useful when you want a specific non-passing value injected (for example, a known exit code that triggers a particular routing branch).

The compiler validates `override_default` at compile time (D2 check): all required fields must be present, no extra fields, and each value must match the expected type.

### Override commands

When a gate is blocking and can't be resolved normally, an operator can record an override to unblock it:

```bash
# Override a gate using the built-in or declared default
koto overrides record <session-name> --gate <gate-name> --rationale "<reason why>"

# Override with an explicit value (takes priority over override_default and built-in)
koto overrides record <session-name> --gate <gate-name> --rationale "<reason why>" \
  --with-data '{"exit_code": 0, "error": ""}'

# List all overrides recorded in the session
koto overrides list <session-name>
```

`--rationale` is required. `--with-data` is optional. The override is epoch-scoped -- it applies until the next state transition and is then superseded. The override is recorded in the session event log and appears in `koto overrides list` output even after a rewind.

In `koto next` responses, `blocking_conditions[].agent_actionable` is `true` for all three built-in gate types, signaling that `koto overrides record` is available.

### Combining gates and evidence routing

Gates and `accepts` blocks work together on the same state. Use mixed `when` conditions -- combining `gates.*` fields and agent evidence fields -- when you want the engine to verify both a gate result and an explicit agent decision before advancing.

```yaml
states:
  review:
    gates:
      lint:
        type: command
        command: "cargo clippy --quiet"
    accepts:
      decision:
        type: enum
        values: [approve, reject]
        required: true
    transitions:
      - target: merge
        when:
          gates.lint.exit_code: 0   # lint must have passed
          decision: approve          # agent must approve
      - target: revise
        when:
          decision: reject           # agent rejects regardless of lint
```

The `merge` transition fires only when lint exited 0 AND the agent submitted `{"decision": "approve"}`. The `revise` transition fires on rejection regardless of the lint result. States using mixed routing must declare an `accepts` block for the agent evidence fields.

### D5 diagnostic and `--allow-legacy-gates`

If a state has gates but none of its `when` clauses reference a `gates.*` key, the compiler rejects it in strict mode with a D5 error:

```
state "preflight": gate "config_exists" has no gates.* routing
  add a when clause referencing gates.config_exists.exit_code, gates.config_exists.error, ...
  or use --allow-legacy-gates to permit boolean pass/block behavior
```

**Fix:** add transitions with `gates.<name>.<field>` conditions as shown in the examples above.

**Escape hatch during migration:** if you're working with a template that predates `gates.*` routing, compile it with `--allow-legacy-gates` to suppress D5 temporarily:

```bash
koto template compile --allow-legacy-gates <template-path>
```

This flag is transitional. New templates should always use `gates.*` routing and won't need it.

`koto init` always runs in permissive mode and never requires the flag -- it emits a warning for legacy-gate states and initializes anyway.

### Self-loops

A transition whose target is its own state creates a retry loop. The agent (or the engine via gate routing) stays in the state until conditions change:

```yaml
transitions:
  - target: proceed
    when:
      gates.doc_check.exists: true
  - target: await_doc           # self-loop: re-evaluate until the key appears
    when:
      gates.doc_check.exists: false
```

### Split topology

A state with multiple outbound `when` transitions is a split point. The mutual exclusivity constraint from Layer 2 applies -- the transition conditions must be unambiguous. Gate-only splits (no agent evidence) are mutually exclusive naturally as long as the gate field values differ across transitions.

## Mermaid previews

Every template ships with a `.mermaid.md` preview file alongside it. This preview renders as a state diagram on GitHub and is validated by CI -- if it's missing or stale, the build fails.

Generate it with:

```bash
koto template export <template>.md --format mermaid --output <template>.mermaid.md
```

For a template at `koto-templates/my-skill.md`, the preview goes at `koto-templates/my-skill.mermaid.md`. Regenerate after every template change.

## Security note

Koto performs `{{VARIABLE}}` substitution in `command` gate strings before passing them to `sh -c`. If a variable contains user-supplied input, this creates a shell injection risk.

Prefer `context-exists` gates over `command` gates when checking paths or files that come from variable interpolation. The `context-exists` and `context-matches` gate types don't invoke a shell and aren't vulnerable to injection.

## References

- **Evidence routing example**: [evidence-routing-workflow.md](examples/evidence-routing-workflow.md) -- branching with accepts/when
- **Advanced example**: [complex-workflow.md](examples/complex-workflow.md) -- gates, self-loops, split topology
- **SKILL.md conventions**: [Custom skill authoring guide](../../../../../docs/guides/custom-skill-authoring.md)
