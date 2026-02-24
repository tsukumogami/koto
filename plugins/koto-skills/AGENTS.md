# koto Workflow Agent Instructions

> **Placement**: Copy this file to the root of your project repository. Codex and
> Windsurf discover agent instructions from `AGENTS.md` at the repo root. Other
> platforms that read `AGENTS.md` will also pick it up.

## What is koto?

koto is a state machine engine for AI agent workflows. It enforces workflow
progression through evidence-gated transitions. You call `koto` commands to
initialize a workflow, get directives, execute them, and transition between states.

## Prerequisites

- `koto` must be installed and on PATH
- Run `koto version` to verify; if missing, install from https://github.com/tsukumogami/koto

## Template Setup

Workflow templates define the states, transitions, and gates for a koto workflow.
Before running a workflow, ensure the template file exists at a stable project-local
path.

For the hello-koto workflow:

1. Check if `.koto/templates/hello-koto.md` already exists in the project.
2. If not, create the directory and write the template there:

```bash
mkdir -p .koto/templates
```

Then write the template file to `.koto/templates/hello-koto.md` with the content from
`plugins/koto-skills/skills/hello-koto/hello-koto.md` in the koto-skills plugin. If the
plugin isn't installed locally, find the template in the koto repository at that path.

## Execution Loop

Every koto workflow follows the same loop: init, next, execute, transition, repeat.

### 1. Initialize the workflow

```bash
koto init --template .koto/templates/hello-koto.md --name hello --var SPIRIT_NAME=<name>
```

Returns JSON with the initial state:

```json
{"state": "awakening"}
```

The template is compiled and cached on first init.

### 2. Get the current directive

```bash
koto next
```

Returns JSON with an action, the current state, and a directive to execute:

```json
{
  "action": "execute",
  "state": "awakening",
  "directive": "You are <name>, a tsukumogami spirit awakening for the first time.\n\nCreate a file at `wip/spirit-greeting.txt` containing a greeting from <name> to the world."
}
```

### 3. Execute the directive

Follow the instructions in the `directive` field. For hello-koto, create the
greeting file:

```bash
mkdir -p wip
echo "Greetings from <name> to the world." > wip/spirit-greeting.txt
```

### 4. Transition to the next state

```bash
koto transition eternal
```

The engine evaluates gates before allowing the transition. For hello-koto, the
command gate runs `test -f wip/spirit-greeting.txt`. If the file exists, the
transition succeeds:

```json
{"state": "eternal"}
```

If the gate fails, the transition is rejected with a gate error. Fix the issue
(create the missing file) and retry the transition.

### 5. Confirm completion

```bash
koto next
```

When the workflow reaches a terminal state, `koto next` returns:

```json
{"action": "done", "state": "eternal", "message": "workflow complete"}
```

The workflow is finished. Report completion to the user.

## Error Handling

- **koto not found**: Tell the user to install koto and add it to PATH.
- **Template not found**: Verify the template path. Copy the template to
  `.koto/templates/hello-koto.md` if it's missing.
- **Gate failure**: The transition's precondition isn't met. Read the error message,
  fix the issue, and retry `koto transition`.
- **State file already exists**: A previous workflow may be active. Run
  `koto workflows` to check. Cancel with
  `koto cancel --state wip/koto-hello.state.json` if needed, then re-init.

## Resume

If a session is interrupted mid-workflow:

1. Run `koto workflows` to find active state files.
2. Run `koto next` to get the current directive.
3. Continue from wherever the workflow left off.

State files in `wip/` persist between sessions. The workflow resumes from the last
completed transition.
