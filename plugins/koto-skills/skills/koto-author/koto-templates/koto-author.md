---
name: koto-author
version: "1.0"
description: Guides agents through authoring koto-backed skills with paired SKILL.md and template
initial_state: entry

variables:
  MODE:
    description: "Authoring mode: 'new' to create from scratch, 'convert' to migrate an existing prose skill"
    required: true

states:
  entry:
    accepts:
      mode_confirmed:
        type: enum
        values: [new, convert]
        required: true
    transitions:
      - target: context_gathering
  context_gathering:
    accepts:
      context_captured:
        type: enum
        values: [done]
        required: true
    transitions:
      - target: phase_identification
  phase_identification:
    accepts:
      phases_identified:
        type: enum
        values: [done]
        required: true
    transitions:
      - target: state_design
  state_design:
    accepts:
      states_designed:
        type: enum
        values: [done]
        required: true
    transitions:
      - target: template_drafting
  template_drafting:
    accepts:
      template_drafted:
        type: enum
        values: [done]
        required: true
    transitions:
      - target: compile_validation
  compile_validation:
    gates:
      template_exists:
        type: context-exists
        key: koto-templates/*.md
    accepts:
      compile_result:
        type: enum
        values: [pass, fail]
        required: true
    transitions:
      - target: skill_authoring
        when:
          compile_result: pass
      - target: compile_validation
        when:
          compile_result: fail
  skill_authoring:
    accepts:
      skill_authored:
        type: enum
        values: [done]
        required: true
    transitions:
      - target: integration_check
  integration_check:
    accepts:
      checks_passed:
        type: enum
        values: [done]
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## entry

Confirm the authoring mode before proceeding.

If `{{MODE}}` is `new`, you'll be creating a koto-backed skill from scratch -- a paired SKILL.md and koto template. If `{{MODE}}` is `convert`, you'll be migrating an existing prose-based skill to use koto for workflow orchestration.

Review the mode and submit `mode_confirmed` with either `new` or `convert` to proceed.

## context_gathering

Gather the information needed to design the skill's state machine.

**If {{MODE}} is "new":** Capture the intent for the new skill. Ask these questions:

- What does the skill do? What's the end-to-end workflow it drives?
- What phases or stages does the workflow have?
- What triggers transitions between phases? Are there decision points?
- What's the expected shape -- linear, branching, looping?

For supplementary material on template patterns, check the koto guides:

```bash
gh api repos/tsukumogami/koto/contents/docs/guides --jq '.[].name'
```

Read any relevant guide with `gh api repos/tsukumogami/koto/contents/docs/guides/<name> --jq '.content' | base64 -d`.

**If {{MODE}} is "convert":** Read the existing SKILL.md that you're converting. Look for these patterns:

- **Phases**: headings like `## Phase 1`, `### Step 3`, or numbered sections that define a sequence.
- **Resume logic**: code like "if wip/state.json exists, read it and skip to phase N" or "check if X file exists, if so resume from here." These become koto gates.
- **Gate checks**: file existence tests (`test -f`), validation steps, or preconditions that must pass before continuing. These become koto gates.
- **Branching**: if/else blocks that route to different sections based on conditions. These become evidence routing.
- **Progress tracking**: writing state files, updating progress markers, checkpoint artifacts. Koto handles this automatically.

Separate what you find into two piles:
- **Workflow mechanics** (ordering, branching, resume, progress) -- these move into the koto template.
- **Domain logic** (what to actually build, check, or produce) -- these stay in SKILL.md.

Submit `context_captured: done` when you have a clear picture of the workflow.

## phase_identification

Derive the state machine's phases from the context you gathered.

**If {{MODE}} is "new":** Work from the captured intent. Determine:

- The state topology: linear chain, branching tree, or something with loops.
- Where evidence routing is needed -- decision points where the agent's output determines the next state.
- Whether any states need self-loops (retry patterns, validation cycles).

**If {{MODE}} is "convert":** Extract phases from the existing prose skill:

- Map each prose phase to a candidate koto state.
- Identify resume patterns (writing state files, checking for prior progress) -- these become gates.
- Map ad-hoc branching (if/else blocks in the prose) to evidence routing with `accepts` and `when` conditions.

Submit `phases_identified: done` when you have a list of states and know how they connect.

## state_design

Define the full state machine: states, transitions, evidence routing, gates, and variables.

Read the template format guide at `${CLAUDE_SKILL_DIR}/references/template-format.md`. Read only the layers you need:

- **Layer 1 (Structure)** -- always read this. Covers states, transitions, variables, terminal states.
- **Layer 2 (Evidence routing)** -- read if your workflow has decision points where the agent's output determines the next state. Covers `accepts` blocks, `when` conditions, and the mutual exclusivity constraint.
- **Layer 3 (Advanced)** -- read if you need gates (preconditions), self-loops (retries), or split topology.

Work through the design in this order:

1. List every distinct phase in your workflow.
2. Draw the transitions between them -- what follows what?
3. Identify decision points: places where the agent chooses between paths. These need evidence routing (`accepts` + `when`).
4. Identify retry loops: places where work might fail and repeat. These need self-loops.
5. Identify preconditions: things that must be true before a phase starts. These need gates.

For the mutual exclusivity constraint: if two conditional transitions from the same state could both match the same evidence values, the compiler rejects it. Make sure each conditional transition is distinguished by at least one field with a different value.

Pick an example from `${CLAUDE_SKILL_DIR}/references/examples/` that matches your workflow:

- Simple linear flow with no branching? Inspect `hello-koto` (2 states, 1 gate).
- Decision points where the agent chooses a path? Use `evidence-routing-workflow.md`.
- Gates, retries, or precondition checks? Use `complex-workflow.md`.

Submit `states_designed: done` when the state machine design is complete.

## template_drafting

Write the koto template as a markdown file with YAML frontmatter and directive body sections.

Place the file at `<target-dir>/koto-templates/<skill-name>.md`. The frontmatter defines the state machine (name, version, description, initial_state, variables, states). The body contains `## <state_name>` sections with the directive text the agent receives in each state.

Follow the format from `${CLAUDE_SKILL_DIR}/references/template-format.md`:

- Every non-terminal state needs at least one transition.
- Terminal states use `terminal: true` with no transitions.
- Every state in the frontmatter must have a matching `## state_name` body section.
- Use double-brace syntax (e.g., `{{MODE}}`) for variable interpolation in directives.

When writing directive text for each state, follow these guidelines:

- Each directive should be completable in one agent turn. If it's too big, the state should be split.
- Tell the agent what to do, what to produce, and what evidence to submit when done.
- Be specific: name files, paths, and commands. "Write the template" is vague. "Write the template to `koto-templates/<name>.md` with states for X, Y, Z" is actionable.
- For mode-conditional behavior, use clear "If {{MODE}} is new / convert" blocks rather than mixing instructions.

Example directive (from this skill's own template):

```
Run `koto template compile <path>` to validate the template.
If compilation succeeds, submit `compile_result: pass`.
If it fails, read the error, fix the template, and submit `compile_result: fail` to retry.
```

Notice the pattern: action, success path, failure path. Each directive should make the agent's next steps obvious.

Submit `template_drafted: done` when the template file is written.

## compile_validation

Run `koto template compile <path-to-drafted-template>` to validate the template.

If compilation succeeds, submit `compile_result: pass` to advance.

If compilation fails, read the error message and apply the matching fix:

- **Missing transition target** -- a state name in a `target:` field doesn't match any declared state. Fix: check for typos. Common mistake: `validate` vs `validation`, `check` vs `checking`.
- **Non-mutually-exclusive evidence routing** -- two conditional transitions could both match the same evidence. Fix: add a discriminating field to one transition's `when` block, or change an enum value so they don't overlap.
- **Invalid regex in context-matches gates** -- malformed pattern. Fix: test the regex separately, escape special characters with `\\`.
- **Unreferenced variables** -- a double-brace variable reference in a directive body doesn't match any declared variable (or a declared variable isn't used anywhere). Fix: add the missing declaration, or remove the unused variable.
- **Missing directive section** -- a state exists in the frontmatter but has no `## state_name` section in the body. Fix: add the missing markdown heading and directive text.

Fix the template and recompile. Maximum 3 attempts before escalating to the user. Submit `compile_result: fail` to loop back and try again.

## skill_authoring

Write or refactor the SKILL.md to work with the koto template.

**If {{MODE}} is "new":** Create the SKILL.md from scratch. It should include:

- YAML frontmatter with name and description.
- Koto execution loop instructions: initialize with `koto init --template ${CLAUDE_SKILL_DIR}/koto-templates/<name>.md`, retrieve directives with `koto next`, submit evidence with `koto next --with-data`.
- A prerequisites section (koto must be on PATH).
- Resume instructions for interrupted sessions -- `koto status` to check state, `koto next` to pick up where you left off.
- References to `${CLAUDE_SKILL_DIR}/references/template-format.md` and `${CLAUDE_SKILL_DIR}/references/examples/` for agents who want to understand the underlying template.

For SKILL.md structure conventions, read the custom skill authoring guide:

```bash
gh api repos/tsukumogami/koto/contents/docs/guides/custom-skill-authoring.md --jq '.content' | base64 -d
```

**If {{MODE}} is "convert":** Refactor the existing SKILL.md:

- Remove workflow boilerplate. This includes: progress files (state.json, checkpoint markers), resume logic ("if X exists, skip to step N"), phase ordering comments ("## Phase 1, 2, 3"), and inline gate checks ("test -f" conditionals in prose).
- Add koto integration: init, next, evidence submission commands.
- Keep all domain-specific instructions -- the WHAT. The template handles the HOW.
- The result should be a leaner SKILL.md that focuses on domain knowledge, with koto managing the workflow.

Submit `skill_authored: done` when the SKILL.md is complete.

## integration_check

Verify the coupling convention between SKILL.md and the koto template.

Check these five things:

1. **Template file exists** at `<target-dir>/koto-templates/<skill-name>.md`.
2. **SKILL.md references the template** -- it should contain `${CLAUDE_SKILL_DIR}/koto-templates/<skill-name>.md` in its koto init instructions.
3. **Mermaid preview exists** -- run `koto template export <template-path> --format mermaid --output <template-path-without-.md>.mermaid.md` to generate the state diagram preview. CI validates that every template has a fresh `.mermaid.md` alongside it.
4. **Output stays within bounds** -- the output directory is within the expected target path with no path traversal (`../`).
5. **No shell injection risk** -- the template doesn't use `command` gates with unsanitized variable interpolation. Prefer `context-exists` or `context-matches` gates for variable-derived paths.

If all checks pass, submit `checks_passed: done`.

## done

The skill is authored. The output directory contains a paired SKILL.md and koto template. The template compiles successfully and follows the coupling convention.
