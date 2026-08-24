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

Variables are declared at the root level and interpolated into directive text using `{{VARIABLE_NAME}}` syntax. The agent supplies values at init time via `--var KEY=VALUE`. Each variable has a `description`, a `required` flag, and an optional `default`.

An optional variable (`required: false`) that the caller omits resolves to its `default`, or to an empty string when no default is declared. Every declared variable is always materialized, so a `{{VARIABLE_NAME}}` reference never fails to resolve. When such a reference lands unquoted in a gate or action command (`--flag {{VAR}}`) and the value is empty, koto renders it as an explicit empty argument (`--flag ''`) so the command stays well-formed instead of dropping the token.

Koto also provides two built-in variables that don't need to be declared. Both resolve everywhere a declared variable does: directives, details, gate commands, and a `default_action` command and its `working_dir`.

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

Content before the marker is the **directive** -- always returned by `koto next`. Content after is the **details** -- delivered when the agent **arrives** at the state, or whenever the caller passes `--full`.

An arrival is the workflow entering the state from a different one, however it got there: a conditional or unconditional transition, a directed (`--to`) transition, a loop-back from later in the workflow, or a multi-hop tick that passes through another state and comes back. A `koto rewind` into the state is also an arrival, whichever state it came from, because a rewind means redo this rather than continue -- useful when a phase is meant to be redone with its full instructions in view again.

This is what an author can rely on, and the part worth reading twice: **a self-loop is not an arrival.** A state that transitions to itself, and a `koto next --to <this state>` issued while the workflow is already there, are laps around a loop the agent is already inside -- the agent still holds the procedure, so `details` is not repeated. Neither is a tick that does not move at all: a gate-blocked state re-evaluating the same failing gate shows `details` once and not again on every retry. Don't write directive text that assumes any of these repeat.

Use details for multi-paragraph instructions, step-by-step procedures, or reference material that clutters the directive on repeat ticks. Keep the directive itself short: a one- or two-line summary of what the state expects, since it's what the agent sees on every tick regardless of delivery state.

States without the marker behave exactly as before -- everything is the directive, and `details` is empty.

An agent that has lost track of a state's `details` can retrieve them unconditionally with `koto status <session-name>` -- it returns the current state's `directive`, `details`, and `expects` regardless of what's already been delivered, and records no delivery itself. `koto next` responses also carry a short pointer to this command in `directive` whenever the current state declares instructions, whether or not this particular response included them.

If a section contains multiple `<!-- details -->` markers, only the first one counts. Everything after the first marker is details.

### Feature-to-action mapping

Different template features produce different `action` values in the `koto next` response. This table shows what the caller sees for each feature:

| Template feature | Caller sees `action` |
|-----------------|---------------------|
| State with `accepts` block | `evidence_required` |
| State with failing `gates` (no accepts) | `gate_blocked` (with `category: "temporal"` for `children-complete`, `"corrective"` for others) |
| State with `integration` | `integration` or `integration_unavailable` |
| Terminal state (`terminal: true`) | `done` |
| State with `default_action` + `requires_confirmation` | `confirm` (only after a successful run) |
| State whose `default_action` failed | `gate_blocked`, with a condition named `__action__` |

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

### Reserved evidence field values

A few `--with-data` field values are reserved by koto and rejected at parse time:

- The top-level `"gates"` key is reserved — agents can't submit it; koto fills it in from gate evaluation.
- The `fields.kind` discriminator collides with the request-store audit-event family if its value is one of `ChildDispatched`, `ChildRedelegated`, `RequesterWoken`, `RequesterRespawn`, or anything starting with `request_store.`. Template authors authoring `accepts` blocks must pick a different value for any `kind`-shaped enum (`"verdict"`, `"scrutineer"`, `"decision"`, etc.).

Both rejections surface as `invalid_submission` with a `reserved`-flavored message before any disk write.

### Reserved event-type namespaces

Two prefixes in koto's wire namespace are koto-owned. They're separate namespaces, and confusing them is easy:

- **`request_store.`** — the evidence-`kind` family described just above, plus the `request_store.result` event. This one the CLI actively rejects on submission.
- **`request.`** — the event types the request store writes on a request's own log: `request.created`, `request.leg_bound`, `request.leg_progress`, `request.leg_result`, `request.leg_abandoned`, `request.closed`. koto owns the whole prefix, not just those six.

Templates don't write event types, so there's nothing to reject at compile time — which is exactly why it's worth saying. Don't author a template, a tool, or a downstream consumer that emits or expects a `request.*` event of its own invention: the six variants are a closed enumeration, koto adds to it as the request lifecycle grows, and a name you pick today can collide with one koto ships tomorrow. A reader on an older koto build deserializes an unrecognized `request.*` type to `Unknown` and keeps going rather than erroring, so a collision degrades silently instead of failing loudly.

If your workflow needs to record something about a request, record it through `koto request progress` (an ordered append that belongs to the leg) or as ordinary evidence on your own session — not as a new event type.

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
| `children-complete` | All child workflows have reached their completion condition | (none required) |

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

#### `children-complete` gate type

The `children-complete` gate waits for child workflows to finish. It discovers children by scanning session headers for workflows whose `parent_workflow` matches the current workflow.

```yaml
gates:
  children-done:
    type: children-complete
    completion: "terminal"        # optional, default "terminal"
    name_filter: "research."      # optional, prefix filter
```

| Field | Required | Description |
|-------|----------|-------------|
| `completion` | No | When a child counts as complete. Default: `"terminal"` (child reached a terminal state). `"state:<name>"` and `"context:<key>"` are reserved for future releases. |
| `name_filter` | No | Prefix filter for child names. Only children whose names start with this prefix are checked. Useful when a parent has multiple fan-out phases with different child name prefixes. |

The compiler rejects unknown completion prefixes. If zero children match the filter, the gate fails (prevents vacuous pass).

When the gate blocks, the blocking condition's `category` is `"temporal"` — the agent should retry later rather than take corrective action.

**Single-state fan-out pattern.** The most common hierarchy pattern puts the directive (telling the agent what to spawn) and the children-complete gate on the same state. The agent reads the directive, spawns children, then polls `koto next` until children finish:

```yaml
states:
  fan_out:
    gates:
      children-done:
        type: children-complete
    transitions:
      - target: converge
        when:
          gates.children-done.all_complete: true
          gates.children-done.needs_attention: false
      - target: triage_failures
        when:
          gates.children-done.all_complete: true
          gates.children-done.needs_attention: true
  converge:
    # ... process child results
  triage_failures:
    # ... retry, skip, or give up
```

This is the one shape to memorize, and every part of it is load-bearing.

**Two conjuncts per branch, not one.** `all_complete: true` is the "the batch has
stopped moving" half; `needs_attention` is the "and here's how it went" half.
Splitting them across branches -- `all_success: true` on one, `needs_attention:
true` on the other -- is rejected at compile time, because the mutual-exclusivity
rule needs the two branches to share a field with differing values, and those two
share none. Repeating `all_complete: true` in both branches gives them the shared
field; `needs_attention` differing across them makes the pair exclusive.

**Two branches, and deliberately no third one for the waiting case.** While
children are still running, `all_complete` is `false`, so the first conjunct
fails on *both* branches and neither matches. The tick doesn't advance: a state
with no `accepts` block stops with `gate_blocked` and `category: "temporal"`, and
a coordinator that also accepts a task list stops with `evidence_required`
carrying the same gate output. Either way the workflow sits still and the agent
ticks again. That stop **is** the wait -- you don't write a transition for it.

**Never guard a branch on a `false` aggregate on its own.** While children are
pending, `all_complete`, `all_success`, and `needs_attention` are *all* `false`.
So a lone `needs_attention: false` branch fires while the batch is still running
and converges on results that don't exist yet. Pairing it with
`all_complete: true`, as above, is what holds it back.

**Don't add a self-loop to "keep polling."** A `gates.children-done.all_complete: false`
transition pointing back at `fan_out` looks like it keeps the agent cycling, but
the engine takes it: it records a `fan_out -> fan_out` transition that moved
nothing, re-evaluates the same gate on the next lap of the advance loop, resolves
to `fan_out` a second time, and now sees a state it has already visited this
tick. `koto next` then fails with `template_error` (exit 3), "cycle detected:
advancement loop would revisit state 'fan_out'" -- every poll, for as long as the
children run. Leaving the transition out gets the wait described above instead.

**Never route the clean branch on `all_complete` alone.** `all_complete` only
says every child stopped. A child that stopped *in failure* satisfies it too, so
a success branch guarded by `all_complete` by itself walks a failed batch
straight into `converge`. That's the second conjunct's whole job. On a state that
also declares `materialize_children`, the compiler catches the omission as
warning W4 -- see [batch-authoring.md](batch-authoring.md) for the full rule.

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
| `children-complete` | `total` | number | Total number of matching children. |
| `children-complete` | `completed` | number | Children in a terminal state (success + failure + skipped). |
| `children-complete` | `pending` | number | Children not yet terminal (covers both "not yet spawned" and "spawned and running"). |
| `children-complete` | `success` | number | Terminal children whose final state is not flagged `failure: true` or `skipped_marker: true`. |
| `children-complete` | `failed` | number | Terminal children whose final state carries `failure: true`. |
| `children-complete` | `skipped` | number | Terminal children whose final state carries `skipped_marker: true` (synthesized when a dependency failed). |
| `children-complete` | `blocked` | number | Tasks that declare `waits_on` dependencies whose upstream children are non-terminal. |
| `children-complete` | `spawn_failed` | number | Tasks the scheduler could not spawn (template resolve errors, collisions, I/O). |
| `children-complete` | `all_complete` | boolean | `pending == 0 AND blocked == 0 AND spawn_failed == 0`. Gate passes when true. |
| `children-complete` | `all_success` | boolean | `all_complete AND failed == 0 AND skipped == 0 AND spawn_failed == 0`. The clean-completion route guard. |
| `children-complete` | `any_failed` | boolean | `failed > 0`. |
| `children-complete` | `any_skipped` | boolean | `skipped > 0`. |
| `children-complete` | `any_spawn_failed` | boolean | `spawn_failed > 0`. |
| `children-complete` | `needs_attention` | boolean | `any_failed OR any_skipped OR any_spawn_failed`. Route to retry / analysis states on this boolean. |
| `children-complete` | `children` | array | Per-child detail: `[{"name", "state", "complete", "outcome", ...}]`. Each entry carries `outcome` (`success \| failure \| skipped \| pending \| blocked \| spawn_failed`); failed entries add `failure_mode` + `reason_source: "state_name"`; skipped entries add `skipped_because` (direct blocker), `skipped_because_chain` (all unique failed ancestors, closest-first), and `reason_source: "skipped"`; blocked entries add `blocked_by` (non-terminal `waits_on` entries). |
| `children-complete` | `error` | string | Empty on normal evaluation. Error message on backend failures. |

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

**When presence gating is the wrong gate.** A `context-exists` gate answers "is
this key here", never "is what is here current". That is sound only when the key
cannot survive from one evaluation of the gate into another, by any path.

The example above is sound: the key does not exist yet, and the state waits for
it to appear. It stops being sound as soon as the state can be re-entered after
the key was written — a review phase that loops back to `implementation` on a
failure and then walks forward into itself again, say. On re-entry the gate finds
the *previous* round's artifact, reports `exists: true`, and the state advances on
work that predates the fix.

Note "by any path" is doing real work in that sentence. It is a property of the
key, not the state: a state entered exactly once can still read a stale key, if
an upstream state that wrote it sits on a cycle.

Two ways out, and the choice matters:

- **`koto context remove <session> <key>` on the loop-back edge.** The key goes
  away, so the gate reports absent and the state cannot advance until something
  writes a fresh one. This is the direct answer, and it is content-agnostic —
  it works whatever the artifact looks like, including markdown.
- **Gate on content with `context-matches` instead**, and overwrite the key with
  a value the pattern rejects. Works when the artifact has a shape you can write
  a pattern against, and couples the gate to that shape.

What does **not** work is overwriting the key while keeping `context-exists`.
`context add` replaces content but leaves the key present, so the gate is
satisfied by the replacement and the state advances against whatever you wrote.

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
| `children-complete` | `{"total": 0, "completed": 0, "pending": 0, "success": 0, "failed": 0, "skipped": 0, "blocked": 0, "spawn_failed": 0, "all_complete": true, "all_success": true, "any_failed": false, "any_skipped": false, "any_spawn_failed": false, "needs_attention": false, "children": [], "error": ""}` |

All four built-in types always have a built-in default, so `koto overrides record` always succeeds for them without `--with-data` or `override_default`. Setting `override_default` is useful when you want a specific non-passing value injected (for example, a known exit code that triggers a particular routing branch).

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

In `koto next` responses, `blocking_conditions[].agent_actionable` is `true` for all four built-in gate types, signaling that `koto overrides record` is available.

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

### Compiler validation for `children-complete`

The compiler validates `children-complete` gate fields at compile time:

- `completion` must use a recognized prefix: `"terminal"` (the only one shipped so far), `"state:<name>"`, or `"context:<key>"`. Unknown prefixes are rejected.
- `name_filter` is optional and not validated beyond being a string (the prefix match happens at runtime).
- Like all gate types, `children-complete` gates must have corresponding `gates.*` when-clause routing or the D5 check will fail.

### `default_action` — a command the engine runs

A state can declare a command koto runs itself, on entering the state, before that state's gates are evaluated. It's how a workflow does the mechanical step instead of writing prose asking the agent to do it and then gating on whether it did.

**Action, gate, or prose.** All three run commands, and they answer different questions. A **gate** is for a command whose *result* is the question -- the workflow must not proceed until something is objectively true; a gate routes on its output and carries nothing forward. A **`default_action`** is for a command whose *effect or output* is the point, and `capture_stdout_as` is what carries the output into later states. **Prose in the directive** hands the command to the agent, which is the right answer whenever the rule below puts it there, and a reasonable answer any time the step needs judgment the engine can't apply.

**Read the rule before writing one.** The [default_action authoring guide](../../../../../docs/guides/default-action-authoring.md) states which commands the engine may run, with worked examples on both sides. In one line: *does the command's risk live in a bad success, or only in a bad failure?* A command whose successful exit is itself the irreversible, externally visible event -- `gh pr create`, `gh pr comment`, `gh pr ready` -- stays with the agent permanently, because no signal arriving afterward can un-fire it. A command whose only irreversibility is local and repairable is engine-runnable; its bad-failure risk is what the failure path below exists to answer.

```yaml
states:
  detect:
    default_action:
      command: git rev-parse --abbrev-ref HEAD
      capture_stdout_as: BRANCH
      fallback: "Read the branch name yourself and carry on with it."
    transitions:
      - target: write_up
```

| Field | Required | Type | Meaning |
|---|---|---|---|
| `command` | Yes | string | The command line, passed to `sh -c` as a single string |
| `capture_stdout_as` | No | string | A name the command's trimmed stdout is delivered under, readable by later states |
| `fallback` | No | string | Prose the agent reads when the action fails. Spliced onto `directive` after substitution, so write it as literal text |
| `working_dir` | No | string | A **relative** path under the session's execution anchor. An absolute literal is a compile error |
| `requires_confirmation` | No | bool | After a *successful* run, stop for confirmation before transitioning |
| `polling` | No | map | `interval_secs` + `timeout_secs`. Re-runs the command on an interval, re-evaluating the state's gates between runs, until they pass or `timeout_secs` expires |

**Invocation.** One `sh -c` argument, in its own process group, inheriting the environment of the `koto next` process. Every single run gets 30 seconds and that isn't configurable -- `polling`'s `timeout_secs` bounds how long koto keeps retrying, not how long one attempt may take. A command that can't finish in 30 seconds isn't one the engine can run. `{{VARIABLE}}` references are substituted in the shell-safe form before the shell sees the string; quote the reference when a value must stay one argument.

**When it runs.** Whenever the advance loop enters the state on a tick carrying no evidence for it. A tick that submits evidence skips the action -- which is how confirming doesn't re-run the command. Every other tick that reaches the state runs it again, gate-blocked retries and self-loops included, so the command must be safe to re-run (`mkdir -p`, not `mkdir`).

**Where it runs.** At the session's execution anchor, not the directory `koto next` was typed in. `working_dir` moves one action to a subdirectory: an absolute value is refused before any join, then the value is joined to the anchor, then canonicalized and refused if it escaped via `..`. The anchor guarantees the directory a workflow's commands *start* in, checked on every tick. It does not bound what an authorized command can reach once running -- a command is still free to name absolute paths or change directory -- so don't author as if it did, and don't describe it to anyone else as if it did.

**What it must not do.** Call `koto next`. A command runs inside a tick, and a tick started from inside one advances the session while the outer tick goes on reporting the state it started with -- the caller gets a wrong answer, not a missing one. koto refuses the nested call with the `nested_invocation` error code, so an author who reaches for it finds out immediately rather than shipping a workflow that lies. The refusal is scoped to the process tree, so it covers a tick on any session, not just this one. Every other `koto` subcommand is fine from a command; `koto context` reads and writes in particular are a supported pattern. This applies to command gates too. One caveat if you write a command that detaches: the marker is inherited and has no liveness, so a process that escapes the process-group kill at timeout keeps it and gets refused by a tick that already exited. The message names the way out; not leaving processes behind a command is the better answer.

**Its output.** Every run appends a `default_action_executed` event with the command, exit code, both streams, and a `truncated` flag; each stream is bounded at 64KB. On a successful run with no `capture_stdout_as`, that log entry is where the output ends -- the agent never sees it.

**When it fails.** The tick stops at the state that ran the command, in an ordinary blocked response (not an error envelope) carrying a condition named `__action__` whose `output` holds the command, `failure_kind`, both streams, and `state`. `exit_code` is present only for `nonzero_exit`. Route on `failure_kind`: `nonzero_exit`, `spawn_failed`, `timed_out`, `wait_failed`, `capture_failed`. The state's `fallback` prose rides the `directive`. It all arrives in the tick that ran the command.

`__action__` is a reserved condition name, so `agent_actionable` is always `false` on it and the compiler rejects any state that declares a gate called `__action__`. A caller can therefore tell an action failure from a gate failure by name alone, and never has to wonder which one it's looking at.

**Its gates do not evaluate after it fails.** A state's gates judge the work its action did, and the action didn't happen. Running them anyway would let a passing gate carry the workflow past a failed command. This holds for a state with no gates at all, which is the case that detected nothing before. Failure is classified ahead of `requires_confirmation`, so a failing action stops as a failure whether or not the flag is set.

#### `capture_stdout_as`

The declared name carries the command's trimmed stdout to states entered after it ran -- in a later tick, and in the same tick when the engine auto-advances through to the reading state.

The name is its own declaration and deliberately does **not** go in the `variables:` block: a declared variable is materialized by `koto init`, so a run that never entered the producing state would render the reference as an empty string. The compiler validates `{{KEY}}` references against the union of the variables block, every state's capture name, and the runtime names, and rejects a capture name colliding with a declared variable, a reserved runtime name, or another state's capture.

Read it anywhere a variable can be read: a later directive or details section, a `vars.NAME: {is_set: true}` when-clause (against a capture, `is_set` answers "has the producing command run yet"), a gate command, a later action command.

Two bounds apply and they do different jobs. **64KB per stream** bounds what the response and event log carry. **4096 bytes** bounds what a capture may deliver, measured after trimming -- far smaller because a captured value is a token landing in prose and possibly a shell word, not a transcript.

Delivery fails three ways, all of them action failures with `failure_kind: "capture_failed"` and a `capture_error` object naming the case: the trimmed output is `empty`, it is `too_large` (over 4096 bytes), or it holds a `disallowed_character` -- the same allowlist declared variables pass, `^[a-zA-Z0-9._/:@ \-]*$`, which forbids newlines and so makes multi-line capture unrepresentable. A skip would be a silent drop, and it would move the error to the reading state where the message names a variable instead of the command that failed to produce it.

Reading a name the run never delivered stops the tick with the `capture_unset` error code rather than rendering an empty string or a raw `{{NAME}}` token.

Lifetime: re-entering the producing state runs the command again and the later value wins; two states declaring the same name is a compile error; a `koto rewind` past the producing state **leaves the value in place**, because a rewind appends an event and truncates nothing; a captured value holding a `{{...}}` token is never re-expanded; and a capture is delivered on a tick that stops for confirmation as well as one that advances.

### skip_if — automatic transitions

`skip_if` lets a state advance automatically when certain conditions hold, without waiting for evidence from the agent. The engine evaluates `skip_if` before asking for evidence. If the conditions match, the engine fires the matching transition and loops — consecutive `skip_if` states chain within a single `koto next` call.

#### Field syntax

`skip_if` is a flat dict of dot-path keys and expected values. The key format is the same as `when` clauses:

```yaml
states:
  check_context:
    skip_if:
      gates.context_check.exists: true
    transitions:
      - target: already_done
        when:
          gates.context_check.exists: true
      - target: do_work
```

The engine evaluates all `skip_if` keys against a merged map of gate outputs and workflow variables. If all keys match, the engine resolves the matching transition (the one whose `when` clause the `skip_if` values satisfy) and advances.

#### Motivating condition types

**Gate output** — skip if a gate already produced a particular result:

```yaml
states:
  await_file:
    gates:
      file_check:
        type: context-exists
        key: output.md
    skip_if:
      gates.file_check.exists: true
    transitions:
      - target: process
        when:
          gates.file_check.exists: true
      - target: await_file
        when:
          gates.file_check.exists: false
```

This is the idiomatic workaround when a `context-exists` gate would otherwise block indefinitely: the self-loop keeps the agent polling, and `skip_if` fires as soon as the key appears.

**Template variable existence** — skip based on whether an optional variable was provided at init time:

```yaml
variables:
  SHARED_BRANCH:
    description: Shared branch to use if pre-created
    required: false
    default: ""

states:
  branch_setup:
    skip_if:
      vars.SHARED_BRANCH:
        is_set: true
    transitions:
      - target: use_shared_branch
        when:
          vars.SHARED_BRANCH:
            is_set: true
      - target: create_branch
```

**Direct evidence value** — skip when a specific piece of evidence is already present in the workflow's merged state:

```yaml
states:
  check_result:
    skip_if:
      result: pass
    transitions:
      - target: done
        when:
          result: pass
      - target: retry
```

#### Compile-time rules

| Rule | Type | Condition |
|------|------|-----------|
| `E-SKIP-TERMINAL` | Error | `skip_if` declared on a terminal state |
| `E-SKIP-NO-TRANSITIONS` | Error | State has `skip_if` but no transitions declared |
| `E-SKIP-AMBIGUOUS` | Error | `skip_if` values match zero or more than one conditional transition |
| `W-SKIP-GATE-ABSENT` | Warning | A `gates.NAME.*` key in `skip_if` references a gate not declared on the same state |

`E-SKIP-AMBIGUOUS` identifies which transitions matched and which values caused the conflict. Fix it by ensuring the `skip_if` values uniquely satisfy exactly one `when` clause.

`W-SKIP-GATE-ABSENT` is a warning — compilation succeeds but the gate output will be absent at runtime, so the condition will never match. Add the missing gate or correct the key.

#### Chaining behavior

When the engine fires a `skip_if` transition, it immediately re-evaluates the new state. If that state also has a matching `skip_if`, the engine advances again — all within the same `koto next` call. The response always reflects the final landing state. `advanced: true` appears in the response whenever at least one `skip_if` fired during the call.

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

A self-loop is a lap, not an arrival, so the state's `details` are not repeated on
it — see the `<!-- details -->` section above. Write the directive so it stands on
its own across iterations, and point an agent that has lost the procedure at
`koto status <session-name>` rather than expecting the next lap to hand it back.

### Split topology

A state with multiple outbound `when` transitions is a split point. The mutual exclusivity constraint from Layer 2 applies -- the transition conditions must be unambiguous. Gate-only splits (no agent evidence) are mutually exclusive naturally as long as the gate field values differ across transitions.

## Parent-child template pair

A parent template fans out work to child workflows and waits for them. The child template is a normal template — it doesn't know or care that it has a parent.

**Parent template** (`research-coordinator.md`):

```yaml
---
name: research-coordinator
version: "1.0"
description: Fan out research to agents, then synthesize
initial_state: fan_out

states:
  fan_out:
    gates:
      children-done:
        type: children-complete
        name_filter: "research."
    transitions:
      - target: synthesize
        when:
          gates.children-done.all_complete: true
          gates.children-done.needs_attention: false
      - target: note_gaps
        when:
          gates.children-done.all_complete: true
          gates.children-done.needs_attention: true
  note_gaps:
    accepts:
      gaps:
        type: string
        required: true
    transitions:
      - target: synthesize
  synthesize:
    accepts:
      summary:
        type: string
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## fan_out

Spawn child workflows for each research topic. Use `koto init <name> --parent {{SESSION_NAME}} --template <child-template>` for each child. Name them with a `research.` prefix (e.g., `research.topic-1`).

<!-- details -->

After spawning children, call `koto next {{SESSION_NAME}}` to check progress. The `children-done` gate will block until all `research.*` children reach a terminal state. You don't need to do anything to unblock it — just wait for the children to finish, then call `koto next` again.

## note_gaps

At least one research child failed or was skipped. Read `blocking_conditions[0].output.children` to see which ones and why, then submit a short `gaps` note describing what the synthesis will be missing.

## synthesize

All research agents have finished. Read their results with `koto context get <child-name> findings` for each child, then synthesize a summary.

## done

Research complete.
```

**Child template** (`research-agent.md`):

```yaml
---
name: research-agent
version: "1.0"
description: Research a single topic
initial_state: research

variables:
  TOPIC:
    description: The topic to research
    required: true

states:
  research:
    accepts:
      findings:
        type: string
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## research

Research {{TOPIC}} and submit your findings.

## done

Research complete.
```

The parent creates children with `koto init research.topic-1 --parent coordinator --template research-agent.md --var TOPIC="memory safety"`. Each child runs independently. The parent's `children-done` gate passes once every `research.*` child reaches a terminal state.

## Mermaid previews

Every template ships with a `.mermaid.md` preview file alongside it. This preview renders as a state diagram on GitHub and is validated by CI -- if it's missing or stale, the build fails.

Generate it with:

```bash
koto template export <template>.md --format mermaid --output <template>.mermaid.md
```

For a template at `koto-templates/my-skill.md`, the preview goes at `koto-templates/my-skill.mermaid.md`. Regenerate after every template change.

## Security note

Koto performs `{{VARIABLE}}` substitution in `command` gate strings before passing them to `sh -c`. Values supplied via `--var` are validated at init time against an allowlist (letters, digits, `. _ - /`, `:`, `@`, and spaces); shell metacharacters such as `;` `|` `&` `$` `(` `)` `<` `>` `*` `?`, quotes, backticks, and newlines are rejected, so a value cannot inject a command.

The allowlist blocks command injection, not word splitting. A value may contain spaces (for structured names like a calendar title), and an unquoted interpolation splits it into multiple shell arguments. Quote the reference when a value must stay a single argument:

```yaml
# A value like "Weekly Planning" splits into two arguments here:
command: mytool --calendar {{CALENDAR}}
# Quote it to keep it one argument:
command: mytool --calendar "{{CALENDAR}}"
```

Prefer `context-exists` gates over `command` gates when checking paths or files that come from variable interpolation. The `context-exists` and `context-matches` gate types don't invoke a shell and aren't vulnerable to injection.

## Batch template primitives

The batch child-spawning release added a small set of template primitives. The summary here is deliberately thin — see [batch-authoring.md](batch-authoring.md) for the authoring walkthrough, compile rules, and worked examples.

### New accepts field type and state fields

| Primitive | Where | Purpose |
|---|---|---|
| `type: tasks` on an accepts field | state's `accepts` block | Structured task-list field consumed by `materialize_children`. The compiler auto-generates `item_schema` on the response so agents don't hand-write the entry shape. |
| `materialize_children` | `TemplateState` | Binds a `tasks` accepts field to a child template and declares the batch `failure_policy` (`skip_dependents` default, or `continue`). |
| `failure: true` | terminal `TemplateState` | Marks a terminal state as a failure outcome. `children-complete` counts these in `failed` and flips `any_failed` / `needs_attention`. |
| `skipped_marker: true` | terminal `TemplateState` | The target the scheduler writes directly when `failure_policy: skip_dependents` materializes a skip for a dependent. `children-complete` counts these in `skipped`. |

### The `present` matcher in `when` clauses

A `when` clause value of the string `"present"` fires when the named field exists in the evidence map, regardless of value. It's only valid under the `evidence.<field>` namespace:

```yaml
transitions:
  - target: handle_retry
    when:
      evidence.retry_failed: present
```

The compiler emits **W6** (non-fatal) when `"present"` appears against any other path (a flat agent-evidence key, a `gates.*` path, `context.*`, etc.) — it almost always means the author meant presence matching but used the wrong prefix.

### The `is_set` matcher for template variables in `when` clauses

A `when` clause can check whether a template variable was provided at init time using the `vars.<VARIABLE_NAME>: {is_set: true/false}` syntax. This lets templates branch based on whether an optional variable was set:

```yaml
variables:
  SHARED_BRANCH:
    description: "Shared branch name"
    required: false
    default: ""

states:
  start:
    transitions:
      - target: use_shared_branch
        when:
          vars.SHARED_BRANCH:
            is_set: true
      - target: create_branch
        when:
          vars.SHARED_BRANCH:
            is_set: false
```

A variable counts as "set" when its value is a non-empty string. Variables that are absent or have an empty string default are "not set".

The compiler enforces:
- `vars.*` keys must use `{is_set: true}` or `{is_set: false}` as the value. Equality matchers (e.g., `vars.FOO: "bar"`) are rejected.
- The variable name after `vars.` must be declared in the template's `variables` block.
- `{is_set: true}` and `{is_set: false}` on the same field are disjoint (no mutual exclusivity conflict). Two identical `{is_set: true}` conditions on different transitions are flagged as conflicting.

### `deny_unknown_fields` narrowed to source templates

`#[serde(deny_unknown_fields)]` applies only to `SourceState` (the YAML-frontmatter surface). Compiled template JSON files no longer reject unknown fields, so adding a new compiled-template field in a release doesn't brick state files created by earlier versions. Template authors still get strict rejection at compile time.

### Compile and runtime rule vocabulary

Batch authoring introduces error (E), warning (W), and runtime (R) rule IDs used in compiler and `koto next` error messages.

| Prefix | Range | Scope | Details |
|---|---|---|---|
| E | E1-E10 | Compile-time errors on `materialize_children` | See [batch-authoring.md](batch-authoring.md) for the full table |
| E | E-SKIP-TERMINAL | Compile-time error on `skip_if` | `skip_if` declared on a terminal state — remove it or remove `terminal: true` |
| E | E-SKIP-NO-TRANSITIONS | Compile-time error on `skip_if` | State has `skip_if` but no transitions — add at least one transition |
| E | E-SKIP-AMBIGUOUS | Compile-time error on `skip_if` | `skip_if` values match zero or more than one conditional transition — ensure values satisfy exactly one `when` clause |
| W | W1-W5 | Compile-time warnings on `materialize_children` / `failure` / `skipped_marker` | See [batch-authoring.md](batch-authoring.md) |
| W | W6 | Compile-time warning on `present` matcher misuse | Fires when `"present"` appears outside `evidence.<field>` paths |
| W | W-SKIP-GATE-ABSENT | Compile-time warning on `skip_if` | A `gates.NAME.*` key references a gate not declared on the state — add the gate or fix the key |
| F | F5 | Compile-time warning on child template reachability | Child template has no reachable `skipped_marker: true` terminal. See [batch-authoring.md](batch-authoring.md) |
| R | R0-R9 | Pre-append runtime rules on a submitted task list | Validated in `koto next`. See [batch-workflows.md](../../koto-user/references/batch-workflows.md) |

## References

- **Evidence routing example**: [evidence-routing-workflow.md](examples/evidence-routing-workflow.md) -- branching with accepts/when
- **Advanced example**: [complex-workflow.md](examples/complex-workflow.md) -- gates, self-loops, split topology
- **Batch authoring**: [batch-authoring.md](batch-authoring.md) -- `materialize_children`, E/W/F/R rules, worked examples
- **`default_action` authoring**: [default-action-authoring.md](../../../../../docs/guides/default-action-authoring.md) -- which commands the engine may run, the failure path, output capture, and execution anchoring
- **SKILL.md conventions**: [Custom skill authoring guide](../../../../../docs/guides/custom-skill-authoring.md)
