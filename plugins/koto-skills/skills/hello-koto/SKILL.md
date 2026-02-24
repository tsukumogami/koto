---
name: hello-koto
description: |
  Run a hello-koto greeting ritual using koto workflow guidance. Use when the user
  invokes /hello-koto <name> to awaken a tsukumogami spirit.
---

# hello-koto

A two-state workflow that creates a spirit greeting file. Exercises the full koto loop:
template compilation, variable interpolation, command gate evaluation, state transition,
and done detection.

## Prerequisites

- `koto` must be installed and on PATH
- Run `koto version` to verify; if missing, install from https://github.com/tsukumogami/koto

## Template Setup

The hello-koto template (`hello-koto.md`) is in the same directory as this skill file.
Before initializing a workflow, ensure the template is at a stable project-local path:

1. Check if `.koto/templates/hello-koto.md` already exists in the project.
2. If not, create it by copying the template content from this skill's directory:

```bash
mkdir -p .koto/templates
```

Then write the template file to `.koto/templates/hello-koto.md` with the content from
`hello-koto.md` (the file alongside this SKILL.md).

Use `.koto/templates/hello-koto.md` as the `--template` path in all koto commands below.

## Workflow

The user provides a `<name>` argument. The workflow has two states:

1. **awakening** -- Create a greeting file at `wip/spirit-greeting.txt` containing a
   greeting from the named spirit. A command gate (`test -f wip/spirit-greeting.txt`)
   blocks the transition until the file exists.
2. **eternal** -- Terminal state. The ritual is complete.

## Execution

### 1. Initialize the workflow

```bash
koto init --template .koto/templates/hello-koto.md --name hello --var SPIRIT_NAME=<name>
```

Returns `{"state":"awakening"}`. The template is compiled and cached on first init.

### 2. Get the current directive

```bash
koto next
```

Returns:

```json
{
  "action": "execute",
  "state": "awakening",
  "directive": "You are <name>, a tsukumogami spirit awakening for the first time.\n\nCreate a file at `wip/spirit-greeting.txt` containing a greeting from <name> to the world."
}
```

### 3. Execute the directive

Create the greeting file:

```bash
mkdir -p wip
echo "Greetings from <name> to the world." > wip/spirit-greeting.txt
```

### 4. Transition to the terminal state

```bash
koto transition eternal
```

The command gate runs `test -f wip/spirit-greeting.txt`. If the file exists, the
transition succeeds and returns `{"state":"eternal"}`. If the file is missing, the
transition fails with a gate error.

### 5. Confirm completion

```bash
koto next
```

Returns `{"action":"done","state":"eternal","message":"workflow complete"}`.

Output a message to the user: `<name> has manifested. The ritual is complete.`

## Error Handling

- **koto not found**: Tell the user to install koto and add it to PATH.
- **Template not found**: Check the template path. If using a plugin path that can't be
  resolved, copy the template to `.koto/templates/hello-koto.md` and re-init.
- **Gate failure**: The greeting file doesn't exist yet. Create `wip/spirit-greeting.txt`
  before attempting the transition.
- **State file already exists**: A previous hello workflow may be active. Run
  `koto workflows` to check. Cancel with `koto cancel --state wip/koto-hello.state.json`
  if needed, then re-init.

## Resume

If the session is interrupted mid-workflow:

1. Run `koto workflows` to check for active state files.
2. Run `koto next` to get the current directive.
3. Continue from wherever the workflow left off.

The Stop hook detects active workflows and reminds the agent to resume.
