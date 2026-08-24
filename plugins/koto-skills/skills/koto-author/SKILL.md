---
name: koto-author
description: >-
  Build a skill whose workflow is enforced by a state machine instead of by
  prose the agent has to remember to follow: it produces the whole skill
  directory, SKILL.md and paired template together, through a guided workflow
  that compile-validates before it ships. Reach for this INSTEAD of
  hand-writing the SKILL.md yourself whenever you are asked for a skill, a
  slash command, or a repeatable procedure with ordered phases, branching, or
  a checkpoint: "write me a skill for our deploy pipeline", "add a command
  that walks someone through onboarding a service", "fan out one child agent
  per issue and wait for all of them". Reach for it too when an existing skill
  is failing at exactly this - it loses its place when a session is
  interrupted, tracks phases in a state file it writes itself, or has grown an
  if/else tree nobody can follow - since converting one is a first-class mode
  here. Hand-writing the template instead is where it goes wrong: overlapping
  when clauses and unrouted gates fail compilation, and the rule that keeps
  the engine from auto-running a command whose successful exit is itself the
  irreversible event (gh pr create) is not something a hand-author discovers.
  Do NOT use it to run a workflow that already exists (koto-user), or for a
  one-off task you will never repeat and need not commit a template for
  (koto-adhoc). The skill-creator skill complements it rather than replacing
  it: this one gets the structure right, that one grades the resulting
  behavior.
---

# koto-author

Walks you through creating a koto-backed skill from scratch or converting an existing prose-based skill to use a koto template. Produces a complete skill directory with a SKILL.md entry point and a paired koto template that drives the workflow.

Intended for agent developers who want to build structured, resumable skills on top of koto's state machine.

## When to use koto-author

Use this skill when you want **structured, resumable workflows** in your skills. koto is a good fit when:

- Your skill has multiple phases that must run in order
- Phases have conditional branching (different paths based on agent decisions)
- You want resumability if a session is interrupted
- You want to separate workflow mechanics (ordering, branching, gating) from domain logic
- Your skill fans out a dynamic list of subtasks to child workers (batch workflows) — see the batch authoring reference

If your skill is a single linear task with no decision points, koto adds unnecessary overhead. A plain SKILL.md is simpler.

## Prerequisites

- koto >= 0.12.1 must be installed and on PATH (`koto version` to verify)
- This skill is installed via the koto-skills plugin

If koto is not installed or the version is too old, install the latest release:

```bash
# Detect platform
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m); [ "$ARCH" = "x86_64" ] && ARCH="amd64"; [ "$ARCH" = "aarch64" ] && ARCH="arm64"

# Download and install
gh release download -R tsukumogami/koto -p "koto-${OS}-${ARCH}" -D /tmp
chmod +x "/tmp/koto-${OS}-${ARCH}"
mv "/tmp/koto-${OS}-${ARCH}" ~/.local/bin/koto
```

## Usage

Choose your mode:

- **new**: You have the intent but no existing skill -- creating from scratch
- **convert**: You have a prose-based SKILL.md and want to move its workflow to koto

```bash
# New skill
koto init --template ${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md \
  --var MODE=new

# Convert existing skill
koto init --template ${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md \
  --var MODE=convert
```

After init, follow the koto execution loop:

1. Run `koto next` to get the current state's response
2. Check the `action` field to determine what's needed:
   - `evidence_required` -- the state needs you to submit data. Do the work, then call `koto next --with-data '{"field": "value"}'`
   - `gate_blocked` -- a precondition hasn't been met. Read `blocking_conditions` for what's failing, fix it, then call `koto next` again. A state's `default_action` that failed arrives here too, under the reserved name `__action__` -- see below
   - `confirm` -- a state's `default_action` ran successfully and wants confirmation before the workflow advances. Read `directive` and `action_output` (`command`, `exit_code`, `stdout`, `stderr`), then confirm with the evidence the state's `expects` asks for, or submit evidence that redirects
   - `done` -- the workflow finished
3. Read the `directive` for instructions. A `details` field may contain extended guidance -- it's delivered when you arrive at a state (from a different state, or via a rewind into it) and omitted on every later tick until you arrive again, including on a self-transition, which is a lap rather than an arrival (pass `--full` to force it through anyway). `koto status <session-name>` retrieves the current state's `directive`/`details`/`expects` unconditionally, without depending on delivery state -- useful for recovering guidance you've lost track of, and the way back to it inside a loop without ticking the workflow (`--full` also works, at the cost of a tick)
4. Repeat until `action` is `done`

Each item in `blocking_conditions` has six fields:

| Field | Type | Notes |
|-------|------|-------|
| `name` | string | Gate name as declared in the template, or the reserved `__action__` for a failed `default_action` |
| `type` | string | Gate type (`command`, `context-exists`, `context-matches`, `children-complete`), or `action` |
| `status` | string | `failed`, `timed_out`, or `error` |
| `category` | string | `"corrective"` (fix something) or `"temporal"` (retry later). `children-complete` gates are temporal; all others are corrective. |
| `agent_actionable` | boolean | `true` when `koto overrides record` can unblock this gate. Always `false` for `__action__` -- an action failure has nothing to override |
| `output` | object | Gate-type-specific structured result (e.g., `{"exit_code": 1, "error": ""}` for `command` gates). For `__action__`: `state`, `command`, `failure_kind`, `stdout`, `stderr`, `truncated`, and `exit_code` only when `failure_kind` is `nonzero_exit` |

`__action__` is reserved: the compiler rejects a state that declares a gate by that name, so the condition can never be confused with one of yours. Route on `failure_kind` (`nonzero_exit`, `spawn_failed`, `timed_out`, `wait_failed`, `capture_failed`) rather than on `status` or on message wording. When an action fails, the state's gates are not evaluated at all -- the tick returns first -- so a state whose action failed reports exactly one condition.

To check where you are at any point, call `koto next <session-name>` without `--with-data` — it returns the current state directive and is idempotent. If you don't know the session name, `koto workflows` lists active sessions.

## What to expect

The workflow has 8 states:

1. **entry** -- confirm your mode (new or convert)
2. **context_gathering** -- describe your skill's workflow (new) or analyze the existing SKILL.md (convert)
3. **phase_identification** -- map workflow phases to state machine states
4. **state_design** -- define states, transitions, evidence routing, and gates. You'll read the format guide and pick an example template here.
5. **template_drafting** -- write the koto template file
6. **compile_validation** -- run `koto template compile` to validate. If it fails, you get 3 attempts to fix errors before escalating.
7. **skill_authoring** -- write the paired SKILL.md (new) or refactor the existing one (convert)
8. **integration_check** -- verify the coupling convention and generate the mermaid preview

## Deciding who runs a command

Every time your workflow needs a command run, you're choosing between three things, and the choice is a design decision rather than a style preference:

- **A gate** when the command's *result* is the question -- the workflow must not proceed until something is objectively true. Gates route on their output; they don't carry it forward.
- **A `default_action`** when the command's *effect or output* is the point -- read the branch name, create the directory, run the formatter. koto runs it on entering the state, before that state's gates, and `capture_stdout_as` carries one line of its stdout into later states.
- **Prose in the directive** when the agent should run it. That's the right answer more often than authors expect, and it's the only answer for one whole category of command.

The category: **does the command's risk live in a bad success, or only in a bad failure?** Keep `default_action` off any command whose *successful* exit is itself the irreversible, externally visible event -- `gh pr create`, `gh pr comment`, `gh pr ready`. Nothing arriving afterward can un-fire it, so no koto release will make it engine-runnable. Allow it where the only irreversibility is bounded and repairable after a successful run: a bad failure is a diagnosis problem, and diagnosing failures is exactly what the action failure path does.

An action must also be safe to re-run -- it fires on every tick that enters the state without evidence, gate-blocked retries and self-loops included.

The [`default_action` authoring guide](../../../../docs/guides/default-action-authoring.md) carries the rule in full, with worked examples on both sides, the burden-of-proof rule for a classification that turns on an unchecked claim, and the failure, capture, and anchoring mechanics. The [template format guide](references/template-format.md) carries the field schema. Read the rule before you write your first action.

## Reference material

The skill bundles reference material, loaded during specific states:

- **Template format guide** (`${CLAUDE_SKILL_DIR}/references/template-format.md`) -- read during state_design and template_drafting. Covers structure (Layer 1), evidence routing (Layer 2), and advanced features (Layer 3). Read only the layers you need.
- **`default_action` authoring guide** (`docs/guides/default-action-authoring.md` in the koto repository) -- read before declaring a state's `default_action`. Covers which commands the engine may run, the field schema, the failure path and its `failure_kind` vocabulary, `capture_stdout_as`, and execution anchoring.
- **Batch authoring guide** (`${CLAUDE_SKILL_DIR}/references/batch-authoring.md`) -- read when your workflow fans out a dynamic task list to child workers. Covers `materialize_children`, the `failure_reason` convention (W5), the `skipped_marker` child-template requirement (F5), aggregate-boolean routing (W4), and two-hat coordinators.
- **Example templates** (`${CLAUDE_SKILL_DIR}/references/examples/`) -- read during state_design. Pick the one matching your complexity:
  - Branching workflows? `evidence-routing-workflow.md`
  - Gates, retries, split topology? `complex-workflow.md`
  - Batch fan-out with dependent tasks? `batch-coordinator.md` + `batch-worker.md` (parent/child pair)
  - Simple linear flow? This skill's own template is a good mid-complexity reference

Additional guides are available at https://github.com/tsukumogami/koto/tree/main/docs/guides. To list them:

```bash
gh api repos/tsukumogami/koto/contents/docs/guides --jq '.[].name'
```

## Namespaces koto reserves

Two wire prefixes belong to koto, and a template must not borrow either:

- `request_store.` -- the reserved evidence-`kind` family. A `--with-data` payload whose `fields.kind` starts with it is rejected at parse time, so don't declare an `accepts` enum that can produce one.
- `request.` -- the event types the request store writes on a request's own log (`request.created`, `request.leg_bound`, `request.leg_progress`, `request.leg_result`, `request.leg_abandoned`, `request.closed`). koto owns the whole prefix. Nothing rejects a homemade `request.*` event type at compile time, because templates don't write event types at all -- which is exactly why an authored tool or consumer that invents one can collide with a variant koto ships later, and degrade silently rather than fail.

The template format guide carries the longer version.

## Resuming interrupted sessions

koto preserves state across interruptions. Call `koto next <session-name>` to see where you left off and pick up where you stopped. If you don't remember the session name, `koto workflows` lists active sessions.

## Output

The skill produces a new skill directory containing:
- `SKILL.md` -- the skill definition with koto execution loop
- `koto-templates/<skill-name>.md` -- the paired koto template
- `koto-templates/<skill-name>.mermaid.md` -- state diagram preview

Both files follow the coupling convention: the SKILL.md references the template via `${CLAUDE_SKILL_DIR}/koto-templates/<skill-name>.md`.

## Troubleshooting

**"koto: command not found"** -- koto isn't on PATH. Install it or add its directory to PATH.

**"template not found"** -- `${CLAUDE_SKILL_DIR}` may not be set. Verify with `echo $CLAUDE_SKILL_DIR` and check the template exists at `$CLAUDE_SKILL_DIR/koto-templates/koto-author.md`.

**Template won't compile after 3 attempts** -- the directive tells you to escalate. Common causes: state name typos, overlapping evidence routing conditions, missing directive body sections. Run `koto template compile <path>` manually to see the full error.

**"session already exists"** -- a previous run didn't finish. Call `koto next <session-name>` to resume where you left off. If you don't know the session name, `koto workflows` lists active sessions.

## Optional: skill-creator for eval

The `/skill-creator:skill-creator` skill is a separate, optional complement. If it's available, use it after koto-author completes to test the authored skill's behavioral quality. skill-creator spawns parallel test runs, grades output, and iterates -- it catches problems that compile validation can't (like a skill that compiles but produces poor results).

koto-author handles structural correctness. skill-creator handles behavioral quality. You don't need both, but they work well together.

## This skill's own template

This skill is itself koto-backed. Its template at `${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md` serves as a mid-complexity example (8 states, evidence routing, self-loop, gates). You can inspect it to learn template patterns.
