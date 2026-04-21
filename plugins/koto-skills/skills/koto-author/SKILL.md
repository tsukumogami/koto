---
name: koto-author
description: |
  How to author koto-backed skills. Use when creating or converting skills that need structured, resumable workflows.
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

- koto >= 0.8.4 must be installed and on PATH (`koto version` to verify)
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
   - `gate_blocked` -- a precondition hasn't been met. Read `blocking_conditions` for what's failing, fix it, then call `koto next` again
   - `done` -- the workflow finished
3. Read the `directive` for instructions. On first visit to a state, a `details` field may contain extended guidance (pass `--full` to force it on repeat visits)
4. Repeat until `action` is `done`

Each item in `blocking_conditions` has six fields:

| Field | Type | Notes |
|-------|------|-------|
| `name` | string | Gate name as declared in the template |
| `type` | string | Gate type (`command`, `context-exists`, `context-matches`, `children-complete`) |
| `status` | string | `failed`, `timed_out`, or `error` |
| `category` | string | `"corrective"` (fix something) or `"temporal"` (retry later). `children-complete` gates are temporal; all others are corrective. |
| `agent_actionable` | boolean | `true` when `koto overrides record` can unblock this gate |
| `output` | object | Gate-type-specific structured result (e.g., `{"exit_code": 1, "error": ""}` for `command` gates) |

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

## Reference material

The skill bundles reference material, loaded during specific states:

- **Template format guide** (`${CLAUDE_SKILL_DIR}/references/template-format.md`) -- read during state_design and template_drafting. Covers structure (Layer 1), evidence routing (Layer 2), and advanced features (Layer 3). Read only the layers you need.
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
