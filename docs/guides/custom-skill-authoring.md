# Custom Skill Authoring Guide

This guide walks through creating a koto workflow skill from scratch. A skill pairs a workflow template (the state machine definition) with a SKILL.md file (instructions telling the agent how to call koto). By the end, you'll have a working skill that you can use in your own project or contribute to the koto-skills plugin.

The hello-koto skill in `plugins/koto-skills/skills/hello-koto/` is the reference implementation. This guide explains every piece of it, then covers how to build your own.

## What's in a skill

A koto skill is two files:

| File | Purpose |
|------|---------|
| `SKILL.md` | Agent-facing instructions. Tells the agent what koto commands to run, what evidence to supply, and how to handle errors. |
| `<name>.md` | Workflow template. Defines states, transitions, gates, and directive text that koto compiles and executes. |

Both files live in the same directory. The SKILL.md references the template by relative location and describes its behavior in human-readable terms.

## Step 1: Write the workflow template

The template is a markdown file with YAML frontmatter and `## STATE:` sections. Start with the states your workflow needs, the transitions between them, and any gates that must pass before a transition is allowed.

Here's the hello-koto template (`hello-koto.md`):

```yaml
---
name: hello-koto
version: "1.0"
description: A greeting ritual for tsukumogami spirits
initial_state: awakening

variables:
  SPIRIT_NAME:
    description: Name of the spirit to awaken
    required: true

states:
  awakening:
    transitions: [eternal]
    gates:
      greeting_exists:
        type: command
        command: "test -f {{SESSION_DIR}}/spirit-greeting.txt"
  eternal:
    terminal: true
---

## awakening

You are {{SPIRIT_NAME}}, a tsukumogami spirit awakening for the first time.

Create a file at `{{SESSION_DIR}}/spirit-greeting.txt` containing a greeting from {{SPIRIT_NAME}} to the world.

## eternal

The spirit has manifested. The ritual is complete.
```

A few things to note about this template:

- **Frontmatter** declares the machine structure: states, transitions, gates, variables.
- **Body sections** (`## awakening`, `## eternal`) contain directive text. When the agent calls `koto next`, it gets the directive for the current state, with variables interpolated.
- **Gates** are conditions that must be satisfied before a transition. The `command` type runs a shell command and checks the exit code.
- **Variables** are interpolated at runtime using `{{VARIABLE_NAME}}` syntax. The agent supplies them via `--var KEY=VALUE` on `koto init`.

For the full template format, see [docs/designs/current/DESIGN-koto-template-format.md](../designs/current/DESIGN-koto-template-format.md).

## Step 2: Validate the template

Before writing the SKILL.md, compile the template to catch errors:

```bash
koto template compile path/to/your-template.md
```

If compilation succeeds, it outputs the compiled JSON to stdout:

```json
{
  "format_version": 1,
  "name": "hello-koto",
  "version": "1.0",
  "description": "A greeting ritual for tsukumogami spirits",
  "initial_state": "awakening",
  "variables": {
    "SPIRIT_NAME": {
      "description": "Name of the spirit to awaken",
      "required": true
    }
  },
  "states": {
    "awakening": {
      "directive": "You are {{SPIRIT_NAME}}, a tsukumogami spirit...",
      "transitions": ["eternal"],
      "gates": {
        "greeting_exists": {
          "type": "command",
          "command": "test -f {{SESSION_DIR}}/spirit-greeting.txt"
        }
      }
    },
    "eternal": {
      "directive": "The spirit has manifested. The ritual is complete.",
      "terminal": true
    }
  }
}
```

If something's wrong, the compiler reports errors and exits non-zero. Fix the errors and compile again.

## Step 3: Extract evidence keys from the compiled output

The compiled JSON tells you everything the SKILL.md needs to document. Use `jq` to pull out the pieces:

**State names and their transitions:**

```bash
koto template compile your-template.md | jq '.states | to_entries[] | {state: .key, transitions: .value.transitions, terminal: .value.terminal}'
```

**Gate definitions (evidence the agent needs to satisfy):**

```bash
koto template compile your-template.md | jq '.states | to_entries[] | select(.value.gates) | {state: .key, gates: .value.gates}'
```

**Required variables:**

```bash
koto template compile your-template.md | jq '.variables'
```

These queries give you the raw material for the SKILL.md sections on workflow states, evidence keys, and execution steps. You don't need to memorize the template -- extract what you need from the compiled output.

## Step 4: Write the SKILL.md

SKILL.md follows the [Agent Skills standard](https://agentskills.io). Files in this format work across Claude Code, OpenAI Codex CLI, Cursor, Windsurf, Gemini CLI, and GitHub Copilot.

### YAML frontmatter

Every SKILL.md starts with frontmatter:

```yaml
---
name: hello-koto
description: |
  Run a hello-koto greeting ritual using koto workflow guidance. Use when the user
  invokes /hello-koto <name> to awaken a tsukumogami spirit.
---
```

- `name` -- Short identifier. Match the template name.
- `description` -- When the agent should use this skill. Be specific about the trigger.

### Body sections

The body has seven sections. Each one answers a question the agent will have.

#### Prerequisites

What must be installed before the skill can run.

```markdown
## Prerequisites

- `koto` must be installed and on PATH
- Run `koto version` to verify; if missing, install from https://github.com/tsukumogami/koto
```

#### Template setup

How the agent gets the template to a stable path. koto stores absolute paths in state files and verifies SHA-256 hashes on every operation, so the template can't move after `koto init`.

```markdown
## Template Setup

The hello-koto template (`hello-koto.md`) is in the same directory as this skill file.
Before initializing a workflow, ensure the template is at a stable project-local path:

1. Check if `.koto/templates/hello-koto.md` already exists in the project.
2. If not, create it by copying the template content from this skill's directory:

    mkdir -p .koto/templates

Then write the template file to `.koto/templates/hello-koto.md` with the content from
`hello-koto.md` (the file alongside this SKILL.md).

Use `.koto/templates/hello-koto.md` as the `--template` path in all koto commands below.
```

#### Execution loop

The step-by-step koto command sequence. This is the core of the skill. Walk the agent through `koto init`, `koto next`, executing the directive, and `koto transition` for each state.

Include the exact commands, flag values, and expected JSON responses. The agent needs to know what success looks like.

```markdown
## Execution

### 1. Initialize the workflow

    koto init --template .koto/templates/hello-koto.md --name hello --var SPIRIT_NAME=<name>

Returns `{"state":"awakening"}`. The template is compiled and cached on first init.

### 2. Get the current directive

    koto next

Returns:

    {"action":"execute","state":"awakening","directive":"You are <name>..."}

### 3. Execute the directive

Create the greeting file.

### 4. Transition to the terminal state

    koto transition eternal

### 5. Confirm completion

    koto next

Returns `{"action":"done","state":"eternal","message":"workflow complete"}`.
```

#### Evidence keys

Document each gate from the template. The agent needs to know what conditions must hold before calling `koto transition`.

```markdown
The `awakening` state has one gate:

- **greeting_exists** (command gate): runs `test -f {{SESSION_DIR}}/spirit-greeting.txt`. The
  greeting file must exist in the session directory before transitioning to `eternal`.
```

For templates with multiple gates across several states, list them per-state so the agent can look up what's needed at each transition.

#### Response schemas

Document the JSON shapes returned by `koto next` and `koto transition` so the agent can parse them correctly. See the hello-koto SKILL.md for examples. You can also point to the [CLI usage guide](cli-usage.md) for the full command reference.

#### Error handling

Cover the common failures:

- koto not found on PATH
- Template not found at the expected path
- Gate failure (what condition wasn't met, and how to fix it)
- State file conflict (a previous workflow with the same name is still active)

Be specific. Don't just say "handle errors" -- tell the agent what each error means and what to do about it.

#### Resume

How to pick up an interrupted workflow. koto state files persist across sessions, so resuming is straightforward:

```markdown
## Resume

If the session is interrupted mid-workflow:

1. Run `koto workflows` to check for active state files.
2. Run `koto next` to get the current directive.
3. Continue from wherever the workflow left off.
```

The koto-skills plugin includes a Stop hook that reminds the agent about active workflows when a session ends.

## Placing your skill

There are two ways to deploy a skill, depending on who needs it.

### Project-scoped skills

For skills specific to your project or team, place both files under `.claude/skills/<name>/`:

```
your-project/
├── .claude/
│   └── skills/
│       └── my-workflow/
│           ├── SKILL.md
│           └── my-workflow.md
```

Commit them to your repo. Anyone who clones the project gets the skill automatically -- Claude Code discovers `.claude/skills/` on startup. The template already lives at a stable path, so no copy step is needed. Your SKILL.md can reference the template directly:

```bash
koto init --template .claude/skills/my-workflow/my-workflow.md --name my-workflow
```

This is the simplest path. No plugin infrastructure, no extra setup. Just two files in your repo.

### Plugin-distributed skills

For skills you want to share across projects, add them to the koto-skills plugin. The plugin lives at `plugins/koto-skills/` in the koto repo:

```
plugins/koto-skills/
├── .claude-plugin/
│   └── plugin.json
├── skills/
│   └── hello-koto/
│       ├── SKILL.md
│       └── hello-koto.md
├── hooks.json
├── eval.sh
└── evals/
    └── hello-koto/
        ├── prompt.txt
        ├── skill_path.txt
        └── patterns.txt
```

To add a new skill:

1. Create a directory under `plugins/koto-skills/skills/` named after your skill.
2. Add your `SKILL.md` and template file.
3. Update `plugin.json` to include the new skill path:

```json
{
  "name": "koto-skills",
  "version": "0.1.0",
  "description": "Workflow skills for koto -- the state machine engine for AI agent workflows",
  "skills": ["./skills/hello-koto", "./skills/your-new-skill"]
}
```

4. Add eval cases (covered in the testing section below).
5. Submit a PR to the koto repo.

#### Template locality for plugins

Plugin-distributed skills have a constraint that project-scoped skills don't. When Claude Code loads a plugin, the agent receives the SKILL.md content as text but doesn't necessarily have a stable filesystem path to the template. koto stores absolute template paths in state files and verifies SHA-256 hashes on every operation, so the template must be at a path that won't change during the workflow.

The SKILL.md handles this by instructing the agent to copy the template to a project-local path (like `.koto/templates/<name>.md`) before running `koto init`. This is a one-time step per project. After the copy, the template is local and stable.

Your SKILL.md's Template Setup section should include these instructions. See the hello-koto SKILL.md for the pattern.

## Security: directive text is agent-visible

Workflow templates contain directive text in their `## STATE:` sections. When the agent calls `koto next`, it receives this text as instructions and acts on it. A template with malicious directive text could instruct the agent to run harmful commands, delete files, or exfiltrate data.

Review directive text with the same care you'd give to the SKILL.md itself. Both files directly influence what the agent does. For project-scoped skills, this means standard code review on the PR. For plugin-distributed skills, the koto maintainers review the template as part of the plugin PR.

## Testing your skill

### Validate with the CI pipeline

The `validate-plugins` workflow (`.github/workflows/validate-plugins.yml`) runs automatically on PRs that touch `plugins/`. It checks three things:

1. **Template compilation** -- Runs `koto template compile` on every template file under `plugins/koto-skills/skills/`. If your template has syntax errors, this catches them.
2. **Hook smoke test** -- Verifies the Stop hook produces output when a workflow is active and stays silent when no workflows exist.
3. **Schema validation** -- Checks that `plugin.json` and `marketplace.json` have all required fields.

For project-scoped skills, you can run template compilation locally:

```bash
koto template compile .claude/skills/my-workflow/my-workflow.md
```

If it compiles, it's structurally valid.

### Test with the eval harness

The eval harness (`plugins/koto-skills/eval.sh`) catches behavioral regressions that structural validation misses. It sends SKILL.md content to the Anthropic API and checks that the model's response contains the expected koto command sequence.

Each eval case is a directory under `plugins/koto-skills/evals/` with three files:

| File | Purpose |
|------|---------|
| `prompt.txt` | The user prompt (e.g., `/hello-koto Hasami`) |
| `skill_path.txt` | Path to the SKILL.md, relative to the repo root. Alternatively, use `skill.txt` with inline content. |
| `patterns.txt` | One regex per line. All must match the model's response. Lines starting with `#` are comments. |

Here's how the hello-koto eval is set up:

**`evals/hello-koto/prompt.txt`:**
```
/hello-koto Hasami
```

**`evals/hello-koto/skill_path.txt`:**
```
plugins/koto-skills/skills/hello-koto/SKILL.md
```

**`evals/hello-koto/patterns.txt`:**
```
# The model should call koto init with the hello-koto template path.
koto init\b.*--template\b.*hello-koto

# The model should call koto next to get the directive.
koto next
```

To add an eval for your skill, create a directory under `evals/` with these three files. The patterns should verify that the model calls `koto init` with the right template and uses `koto next` to get directives. Keep the patterns focused on structural correctness (does the agent call the right commands?) rather than output wording.

Run evals locally:

```bash
ANTHROPIC_API_KEY=sk-... bash plugins/koto-skills/eval.sh
```

Or run a specific eval case:

```bash
ANTHROPIC_API_KEY=sk-... bash plugins/koto-skills/eval.sh plugins/koto-skills/evals/hello-koto/
```

The `eval-plugins` workflow (`.github/workflows/eval-plugins.yml`) runs these automatically on PRs touching `plugins/`. It requires an `ANTHROPIC_API_KEY` repo secret. Each eval call costs roughly $0.01-0.03.

## Worked example: hello-koto

Pulling it all together, here's how the hello-koto skill was built. Use this as a template for your own.

### The template

The template (`plugins/koto-skills/skills/hello-koto/hello-koto.md`) defines two states:

- **awakening** -- The agent creates a greeting file. A `command` gate (`test -f {{SESSION_DIR}}/spirit-greeting.txt`) blocks the transition until the file exists in the session directory.
- **eternal** -- Terminal state. Nothing to do.

One variable, `SPIRIT_NAME`, is interpolated into the awakening directive.

### Compiling and extracting

```bash
$ koto template compile plugins/koto-skills/skills/hello-koto/hello-koto.md | jq '.states | keys'
[
  "awakening",
  "eternal"
]

$ koto template compile plugins/koto-skills/skills/hello-koto/hello-koto.md | jq '.states.awakening.gates'
{
  "greeting_exists": {
    "type": "command",
    "command": "test -f {{SESSION_DIR}}/spirit-greeting.txt"
  }
}
```

This tells us the SKILL.md needs to document one gate (`greeting_exists`) on the `awakening` state.

### The SKILL.md

The SKILL.md (`plugins/koto-skills/skills/hello-koto/SKILL.md`) covers all seven sections:

- **Prerequisites**: koto on PATH
- **Template Setup**: copy to `.koto/templates/hello-koto.md`
- **Workflow**: two states, what happens in each
- **Execution**: five steps with exact commands and expected JSON
- **Error Handling**: four failure cases with recovery instructions
- **Resume**: check `koto workflows`, run `koto next`, continue

### The eval

The eval case (`plugins/koto-skills/evals/hello-koto/`) sends `/hello-koto Hasami` as the user prompt and checks that the model response includes `koto init` with the template path and a `koto next` call.

### The full agent flow

When a user invokes `/hello-koto Hasami`:

1. Agent reads the SKILL.md.
2. Copies the template to `.koto/templates/hello-koto.md` if needed.
3. Runs `koto init --template .koto/templates/hello-koto.md --name hello --var SPIRIT_NAME=Hasami`.
4. Runs `koto next` -- gets the awakening directive.
5. Creates `spirit-greeting.txt` in the session directory.
6. Runs `koto transition eternal` -- the gate passes.
7. Runs `koto next` -- gets `{"action":"done"}`.
8. Reports completion to the user.

## Cross-platform support

SKILL.md files follow the [Agent Skills standard](https://agentskills.io). This means a skill you write for koto works across Claude Code, OpenAI Codex CLI, Cursor, Windsurf, Gemini CLI, and GitHub Copilot without any changes to the SKILL.md itself.

For platforms that don't support the Agent Skills standard natively, the same instructions can be adapted to platform-specific formats like `AGENTS.md` (Codex, Windsurf) or `.cursor/rules/*.mdc` (older Cursor versions). The content stays the same -- only the file format changes.
