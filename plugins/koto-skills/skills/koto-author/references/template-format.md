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

Required fields: `name`, `version`, `description`, `initial_state`, `states`.

Optional fields: `variables`.

### Variables

Variables are declared at the root level and interpolated into directive text using `{{VARIABLE_NAME}}` syntax. The agent supplies values at init time via `--var KEY=VALUE`.

```yaml
variables:
  REPO_URL:
    description: Repository to clone
    required: true
  BRANCH:
    description: Branch to check out
    required: false
```

Koto also provides the built-in variable `{{SESSION_NAME}}`, which resolves to the active session name. You don't need to declare it.

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

### Complete evidence routing example

Here's a state with accepts, conditional transitions, and a fallback:

```yaml
states:
  review:
    accepts:
      verdict:
        type: enum
        values: [approve, request-changes, defer]
        required: true
    transitions:
      - target: merge_prep
        when:
          verdict: approve
      - target: revision
        when:
          verdict: request-changes
      - target: parked
        when:
          verdict: defer
```

See [evidence-routing-workflow.md](examples/evidence-routing-workflow.md) for a full compilable template using this pattern.

## Layer 3: Advanced features

### Gates

Gates are conditions checked before a transition is allowed. They're evaluated automatically when the agent calls `koto next` or `koto transition`.

Three gate types are available:

**context-exists** -- passes when a key exists in the content store:

```yaml
gates:
  plan_ready:
    type: context-exists
    key: plan.md
```

**context-matches** -- passes when content for a key matches a regex:

```yaml
gates:
  plan_has_steps:
    type: context-matches
    key: plan.md
    pattern: "^## Step \\d+"
```

**command** -- passes when a shell command exits 0:

```yaml
gates:
  config_present:
    type: command
    command: "test -f deploy.conf"
```

A state can have multiple gates. All must pass before any transition fires.

### Combining gates and evidence routing

Gates and `accepts` blocks work together on the same state. Gates are checked first -- if any gate fails, the agent can't advance regardless of evidence. Once all gates pass, evidence routing determines which transition fires.

```yaml
states:
  deploy:
    gates:
      config_valid:
        type: context-exists
        key: deploy.conf
      tests_pass:
        type: command
        command: "test -f test-results.txt && grep -q PASS test-results.txt"
    accepts:
      target:
        type: enum
        values: [staging, production]
        required: true
    transitions:
      - target: staging_deploy
        when:
          target: staging
      - target: production_deploy
        when:
          target: production
```

Both gates must pass (config exists, tests passed) before the agent can submit evidence choosing the deploy target.

### Self-loops

A transition whose target is its own state creates a loop. This is useful for retry patterns -- the agent stays in the same state until conditions change.

```yaml
states:
  build:
    accepts:
      result:
        type: enum
        values: [pass, fail]
        required: true
    transitions:
      - target: deploy
        when:
          result: pass
      - target: build
        when:
          result: fail
```

When the agent submits `result: fail`, it loops back to `build` and receives the same directive again.

### Split topology

When a state has multiple outbound transitions with `when` conditions, it's a split point. The agent's evidence determines which path the workflow takes.

Splits require an `accepts` block with enough fields to differentiate every conditional transition (the mutual exclusivity constraint from Layer 2 applies). You've already seen this pattern in the evidence routing section -- split topology is just the name for it at the graph level.

### Integration tags and default action

Templates can include `integration_tags` (a list of string labels) and `default_action` (the action returned by `koto next` for non-terminal states, defaults to `"execute"`). These are optional and rarely needed for most workflows.

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

- **Simple example**: [hello-koto](../../hello-koto/hello-koto.md) -- two states, one gate, one variable
- **Evidence routing example**: [evidence-routing-workflow.md](examples/evidence-routing-workflow.md) -- branching with accepts/when
- **Advanced example**: [complex-workflow.md](examples/complex-workflow.md) -- gates, self-loops, split topology
- **SKILL.md conventions**: [Custom skill authoring guide](../../../../../docs/guides/custom-skill-authoring.md)
