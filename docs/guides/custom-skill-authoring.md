# Custom Skill Authoring Guide

This guide walks through creating a koto workflow skill from scratch. A skill pairs a workflow template (the state machine definition) with a SKILL.md file (instructions telling the agent how to call koto). By the end, you'll have a working skill that you can use in your own project or contribute to the koto-skills plugin.

This guide builds one worked example end to end -- a greeting ritual called hello-koto -- and every piece of it appears inline below, so nothing here depends on a skill you have to go and find. For a shipped skill to read alongside it, `plugins/koto-skills/skills/koto-author/` is the closest in shape: a SKILL.md paired with a template under `koto-templates/`.

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
        type: context-exists
        key: spirit-greeting.txt
  eternal:
    terminal: true
---

## awakening

You are {{SPIRIT_NAME}}, a tsukumogami spirit awakening for the first time.

Write a greeting from {{SPIRIT_NAME}} to the world and submit it with `koto context add {{SESSION_NAME}} spirit-greeting.txt`.

## eternal

The spirit has manifested. The ritual is complete.
```

A few things to note about this template:

- **Frontmatter** declares the machine structure: states, transitions, gates, variables.
- **Body sections** (`## awakening`, `## eternal`) contain directive text. When the agent calls `koto next`, it gets the directive for the current state, with variables interpolated.
- **Gates** are conditions that must be satisfied before a transition. The `context-exists` type checks whether a key exists in the content store. The `command` type runs a shell command and checks the exit code.
- **Variables** are interpolated at runtime using `{{VARIABLE_NAME}}` syntax. The agent supplies them via `--var KEY=VALUE` on `koto init`.

The supported gate types are `command`, `context-exists`, `context-matches`, `children-complete`, and `request-leg` (see [Routing on another session's result](#routing-on-another-sessions-result-request-leg-gates)). For full template-authoring guidance, use the `koto-author` skill (in the koto-skills plugin), which compiles and validates templates interactively.

### Constraining variables

Every variable value already has to pass koto's character allowlist. When a variable should only ever hold a few specific values, or values of a known shape, say so in the declaration and let koto refuse anything else at `koto init`, before a gate command or directive ever sees it. That keeps argument checking out of your SKILL.md prose.

```yaml
variables:
  INTENT_FLAG:
    description: The caller's intent token, or empty
    pattern: ^(continue|stop)?$
  MERGE:
    values: ["true", "false"]
    default: "false"
    rebind: true
  MAX_ROUNDS:
    pattern: "([1-9]|[1-4][0-9]|50)?"
    rebind: true
```

- **`values:`** is a closed list. A value must equal one of the entries exactly. The list can't be empty, and every entry must itself pass the allowlist.
- **`pattern:`** is a regular expression in the Rust `regex` crate's syntax. koto matches it against the whole value, applying it as `^(?:<pattern>)$`, so `pattern: "[a-z]+"` refuses `abc-1` even though you didn't write anchors. Anchors you do write are harmless. The crate has no lookaround or backreferences, and an expression it can't compile is a compile error.
- A variable declares at most one of `values:` and `pattern:`.
- A non-empty `default` must satisfy the constraint. An optional variable with no default resolves to the empty string when it isn't passed, so its constraint has to accept `""` (as `INTENT_FLAG` and `MAX_ROUNDS` above do); otherwise give it a default or mark it `required: true`.
- Unknown keys in a variable declaration are compile errors, so a misspelled `valuez:` can't silently leave a variable unconstrained.

At `koto init`, a value that fails its constraint (or the allowlist) exits with code 2, creates no session, and reports `"code": "invalid_var"` with the variable, the value, and the constraint. A repeated `--var` key reports `duplicate_var` and an undeclared one `unknown_var`. See [the error code reference](../reference/error-codes.md#init).

**`rebind: true`** marks a per-invocation setting, such as whether this run may merge, as opposed to an identity variable like a topic or a document path. Variables are fixed when the session is created. A `rebind: true` variable is the exception: when a later invocation attaches to the same live session, koto re-applies it from that invocation, using the invocation's value or else the declared default, so the setting is never inherited from an earlier run. Rebinding happens only through an accepted `koto init --attach-live`; there's no standalone command that changes a variable, and a refused attach leaves every variable as it was. The session log records each rebind as a `variables_rebound` event. `rebind` must be a YAML boolean.

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
          "type": "context-exists",
          "key": "spirit-greeting.txt"
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
description: >-
  Awaken a tsukumogami spirit and walk it through its greeting ritual, one named
  spirit at a time. Reach for this whenever someone asks to greet, name, or wake
  the spirit of an object -- "give the old kettle a name", "say hello to the
  workshop tools" -- because the ritual's steps have to run in order and the
  greeting has to be recorded, and performing it by hand leaves no trace of which
  spirit was woken. Do NOT use it to author a new ritual template (koto-author)
  or to drive a workflow that is already running (koto-user).
---
```

- `name` -- Short identifier. Match the template name.
- `description` -- The only text an agent reads when deciding whether to load your
  skill, so write it to select rather than to summarize:
  - Lead with the job in terms an agent can match against its own situation, not
    with the artifact you produce or a role label.
  - Push on the cases where nobody says your skill's name. Those are the ones that
    go uncaught; a request that already names the skill needs no help.
  - Name what goes wrong when the skill isn't loaded.
  - Close with a negative boundary naming the sibling skills yours gets confused
    with.
  - Spend nothing on flags, file layouts, or anything else an agent can't act on
    before loading. Use the `>-` folded scalar so the whole thing reads as one
    paragraph.

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

The hello-koto template is in the same directory as this skill file.
Before initializing a workflow, ensure the template is at a stable project-local path:

1. Check if `.koto/templates/hello-koto.md` already exists in the project.
2. If not, create it by copying the template content from this skill's directory:

    mkdir -p .koto/templates

Then write the template file to `.koto/templates/hello-koto.md` with the content from
`hello-koto.md` (the file alongside this SKILL.md).

Use `.koto/templates/hello-koto.md` as the `--template` path in all koto commands below.
```

#### Execution loop

The step-by-step koto command sequence. This is the core of the skill. Walk the agent through `koto init`, `koto next`, executing the directive, and directed transitions (`koto next <name> --to <state>`) for each state.

Include the exact commands, flag values, and expected JSON responses. The agent needs to know what success looks like.

```markdown
## Execution

### 1. Initialize the workflow

    koto init hello --template .koto/templates/hello-koto.md --var SPIRIT_NAME=<name>

Returns `{"name":"hello","state":"awakening"}`. The template is compiled and cached on first init.

### 2. Get the current directive

    koto next hello

Returns:

    {"action":"evidence_required","state":"awakening","directive":"You are <name>...","advanced":false,"expects":{...},"blocking_conditions":[],"error":null}

### 3. Execute the directive

Create the greeting file.

### 4. Transition to the terminal state

    koto next hello --to eternal

### 5. Confirm completion

    koto next hello

Returns `{"action":"done","state":"eternal","advanced":true,"expects":null,"error":null}`.
```

#### Evidence keys

Document each gate from the template. The agent needs to know what conditions must hold before requesting a directed transition with `koto next <name> --to <state>`.

```markdown
The `awakening` state has one gate:

- **greeting_exists** (context-exists gate): checks for key `spirit-greeting.txt` in the content
  store. The agent must submit the greeting via `koto context add` before transitioning to `eternal`.
  Produces `{"exists": bool, "error": ""}` output, available under `gates.greeting_exists` in evidence.
```

For templates with multiple gates across several states, list them per-state so the agent can look up what's needed at each transition.

Each gate type produces structured output matching its schema (see [Gate output schemas](#gate-output-schemas)). The engine injects this output into the evidence map under `gates.<name>` after evaluation. Templates can reference these fields in `when` conditions using dot-path syntax: `gates.<name>.<field>`. For example, `gates.greeting_exists.exists: true` would route a transition only when the context-exists gate found the key.

#### Response schemas

Document the JSON shapes returned by `koto next` (including directed transitions via `koto next <name> --to <state>`) so the agent can parse them correctly. `plugins/koto-skills/skills/koto-user/references/response-shapes.md` catalogues them, and the [CLI usage guide](cli-usage.md) has the full command reference.

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
2. Run `koto next <name>` to get the current directive.
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
koto init my-workflow --template .claude/skills/my-workflow/my-workflow.md
```

This is the simplest path. No plugin infrastructure, no extra setup. Just two files in your repo.

### Plugin-distributed skills

For skills you want to share across projects, add them to the koto-skills plugin. The plugin lives at `plugins/koto-skills/` in the koto repo:

```
plugins/koto-skills/
├── .claude-plugin/
│   └── plugin.json
├── skills/
│   └── koto-author/
│       ├── SKILL.md
│       ├── koto-templates/
│       ├── references/
│       └── evals/
│           └── evals.json
├── hooks.json
└── hooks/
```

To add a new skill:

1. Create a directory under `plugins/koto-skills/skills/` named after your skill.
2. Add your `SKILL.md` and template file.
3. Update `plugin.json` to include the new skill path:

```json
{
  "name": "koto-skills",
  "version": "0.12.1-dev",
  "description": "Workflow skills for koto -- the state machine engine for AI agent workflows",
  "skills": [
    "./skills/koto-adhoc",
    "./skills/koto-author",
    "./skills/koto-user",
    "./skills/your-new-skill"
  ]
}
```

4. Add eval cases (covered in the testing section below).
5. Submit a PR to the koto repo.

#### Template locality for plugins

Plugin-distributed skills have a constraint that project-scoped skills don't. When Claude Code loads a plugin, the agent receives the SKILL.md content as text but doesn't necessarily have a stable filesystem path to the template. koto stores absolute template paths in state files and verifies SHA-256 hashes on every operation, so the template must be at a path that won't change during the workflow.

The SKILL.md handles this by instructing the agent to copy the template to a project-local path (like `.koto/templates/<name>.md`) before running `koto init`. This is a one-time step per project. After the copy, the template is local and stable.

Your SKILL.md's Template Setup section should include these instructions. `plugins/koto-skills/skills/koto-author/SKILL.md` is a shipped example of the pattern.

## Security: directive text is agent-visible

Workflow templates contain directive text in their `## STATE:` sections. When the agent calls `koto next`, it receives this text as instructions and acts on it. A template with malicious directive text could instruct the agent to run harmful commands, delete files, or exfiltrate data.

Review directive text with the same care you'd give to the SKILL.md itself. Both files directly influence what the agent does. For project-scoped skills, this means standard code review on the PR. For plugin-distributed skills, the koto maintainers review the template as part of the plugin PR.

## Content ownership

koto owns all workflow content through a CLI interface. Agents don't write files to the session directory directly -- they submit and retrieve content through `koto context`.

### Why content goes through koto

When agents write files directly to the session directory, the engine can't track what was produced or verify that work was done. The content interface gives koto visibility into agent output, which enables content-aware gates and makes the audit trail complete.

### The context commands

| Command | Purpose |
|---------|---------|
| `koto context add <session> <key> [--from-file <path>]` | Submit content. Reads from stdin by default, or from a file with `--from-file`. |
| `koto context get <session> <key> [--to-file <path>]` | Retrieve content. Writes to stdout by default, or to a file with `--to-file`. |
| `koto context exists <session> <key>` | Check if a key exists. Exits 0 if present, 1 if not. |
| `koto context list <session> [--prefix <prefix>]` | List keys as a JSON array. Optionally filter by prefix. |

### Submitting content from a skill

The simplest pattern pipes content directly:

```bash
echo "Greetings from Hasami to the world." | koto context add hello spirit-greeting.txt
```

For larger artifacts, write to a temporary file first and use `--from-file`:

```bash
koto context add hello plan.md --from-file /tmp/plan.md
```

To read content back:

```bash
koto context get hello spirit-greeting.txt
```

Or write it to a file:

```bash
koto context get hello plan.md --to-file ./plan.md
```

### Content-aware gates

Templates can gate transitions on content state instead of shell commands. Two gate types are available:

**`context-exists`** -- passes when a key exists in the content store:

```yaml
gates:
  greeting_exists:
    type: context-exists
    key: spirit-greeting.txt
```

**`context-matches`** -- passes when the content for a key matches a regex pattern:

```yaml
gates:
  plan_has_steps:
    type: context-matches
    key: plan.md
    pattern: "^## Step \\d+"
```

These gates are evaluated automatically when the agent calls `koto next` (including directed transitions via `koto next <name> --to <state>`). They replace the older pattern of using `command` gates with `test -f` checks against the session directory.

### Gate output schemas

Every gate type produces structured output, available under `gates.<gate_name>` in the evidence map and in `blocking_conditions` when a gate fails.

| Gate type | Output schema |
|-----------|--------------|
| `command` | `{"exit_code": number, "error": string}` |
| `context-exists` | `{"exists": boolean, "error": string}` |
| `context-matches` | `{"matches": boolean, "error": string}` |
| `request-leg` | `{"found": boolean, "disposition": string, "bound": boolean, "source": string, "status": string, "final_state": string, "template": string, "outcome": string, "step": string, "reason": string, "valid": boolean, "payload": object, "error": string}` |

For `command` gates, `error` is `""` on normal exit (pass or fail). On timeout it's `"timed_out"` with `exit_code: -1`. On spawn errors it's the error message with `exit_code: -1`.

Templates can route transitions based on gate output using dot-path `when` clauses. The path format is `gates.<gate_name>.<field>`:

```yaml
states:
  build:
    transitions: [deploy, fix]
    gates:
      ci_check:
        type: command
        command: "make test"
```

```yaml
transitions:
  - target: deploy
    when:
      gates.ci_check.exit_code: 0
  - target: fix
    when:
      gates.ci_check.exit_code: 1
```

Single-segment paths like `decision: proceed` still work for agent-submitted evidence. Dot-path traversal (`gates.ci_check.exit_code`) is for gate output fields injected by the engine.

### Writing context from a transition

A transition can write context keys itself when it fires, through `context_assignments`. It's a map of context key to a string value:

```yaml
states:
  review:
    accepts:
      verdict:
        type: enum
        values: [approve, block]
        required: true
      detail:
        type: string
    gates:
      ci:
        type: command
        command: "make test"
    transitions:
      - target: done
        when:
          verdict: approve
          gates.ci.exit_code: 0
        context_assignments:
          outcome: landed
          topic: "{{TOPIC}}"
          ci_exit: "${gates.ci.exit_code}"
      - target: done_blocked
        when:
          verdict: block
        context_assignments:
          outcome: blocked
          failure_reason: "review blocked: ${evidence.detail}"
```

A value takes four forms, and references may sit inside a string literal as `failure_reason` does above:

| Form | Resolves to |
|------|-------------|
| a literal (`landed`) | itself |
| `{{VAR}}` | the session's variable, including a `capture_stdout_as` value delivered earlier in the same `koto next` |
| `${evidence.<field>}` | the value submitted for `<field>` in the evidence that drove the transition |
| `${gates.<gate>.<path>}` | a dot path into the gate's structured output for that tick; it walks any nesting, so it works for every gate type |

The compiler checks every assignment. Each key must be a usable context key (letters, digits, `.`, `_`, `-`, and `/` between components). A value must be a string; numbers and booleans are written as text, and a mapping or a list is an error. `${evidence.<field>}` must name a field in the state's `accepts` block, `${gates.<gate>...}` must name a gate declared on the state, and `{{VAR}}` must name a declared variable or capture. Any other `${...}`, such as `${context.key}`, is refused. So is any key on a transition other than `target`, `when` and `context_assignments`: a typo like `context_assignment:` fails compilation instead of being ignored.

At run time only the edge that fires writes anything. An evidence field that wasn't submitted, or a gate path that isn't in that tick's output, resolves to the empty string and the transition still happens. Resolved values are stored exactly as resolved: a submitted value that contains `{{X}}` or `${context.y}` is written literally, never expanded a second time. A later write to the same key, whether from another transition or `koto context add`, replaces the earlier value.

The resolved values are recorded on the transition's own event in the session log, so a transition and its assignments can't be separated by a crash. If writing them to the context store fails after that, the next `koto context get`, `koto context exists`, or context gate restores them from the log.

A command gate's output is only `exit_code` and `error`, so a gate path can't carry what a script printed. To get a script's output into context, have a `default_action` run `koto context add`.

### Gates that refuse overrides

By default an agent can force any gate with `koto overrides record`, which logs a rationale and substitutes the gate's output (from `--with-data`, the gate's `override_default`, or the gate type's built-in default) for the next `koto next`. That is the right escape hatch for most gates: a flaky check, a condition a human confirmed by hand.

Some gates shouldn't have one. Declare `overridable: false` on a gate whose output decides something nothing downstream re-checks: whether to merge, whether a run counts as done, which report a parent receives. It works on every gate type:

```yaml
states:
  merge_decide:
    default_action:
      command: "./merge-verdict.sh"   # writes merge.verdict to context
    gates:
      verdict:
        type: context-matches
        key: merge.verdict
        pattern: "^ready$"
        overridable: false
    transitions:
      - target: merge
        when:
          gates.verdict.matches: true
      - target: wait
        when:
          gates.verdict.matches: false
```

With the flag set:

- `koto overrides record` on the gate exits 2 with the typed code `gate_not_overridable`, whatever `--with-data` holds, and appends nothing to the state log.
- The gate reports `agent_actionable: false` in `blocking_conditions`, so an agent reading the response isn't told to override it.
- If the log already holds an override for the gate (written by an older koto, or by hand), `koto next` ignores it and evaluates the gate for real.

A good rule: a gate that routes on a context key your own `default_action` script wrote should be `overridable: false`, because otherwise an override lets the agent supply the value the script exists to produce. Leave gates that only guard against a transient failure overridable, so a stuck run has a logged way forward.

The compiler holds you to the declaration. `overridable` accepts only `true` or `false` (`"no"` is an error), `override_default` on a gate with `overridable: false` is an error because nothing could ever apply it, and an unknown key on a gate, such as the misspelling `overrideable`, fails `koto template compile` with an error naming the state, the gate, and the key.

### The reachability check, and why non-overridable gates are exempt

Strict compilation (`koto template compile` without `--allow-legacy-gates`) runs a reachability check on every state whose `when` clauses route only on gate output. It builds each gate's override value, its `override_default` or else its type's built-in default, and requires that at least one of those pure-gate transitions fires on it. The check exists because an override is the escape hatch for a stuck state: if forcing every gate still matches no arm, the override can't move the state, and the template has a dead end. The failure reads `no transition fires when all gates use override defaults`, and the fix is an `override_default` that selects an arm.

That premise doesn't hold for a gate declared `overridable: false`, since no override can ever apply to it. So the check leaves out every pure-gate transition whose `when` clause references a non-overridable gate, and a state whose pure-gate transitions all reference one is exempt. The arms still have to be right; they're just reached by the gate's real output rather than by an override. Transitions that reference only overridable gates are checked exactly as before, including on a state that also has a non-overridable gate. Without the exemption, a state that routes a non-overridable gate on values its default can't produce (the `payload.outcome` of a `request-leg` gate, say, or a `context-matches` gate routed only on `matches: false`) could never compile strictly, because the `override_default` that would satisfy the check is itself a compile error on such a gate.

### Routing on another session's result: `request-leg` gates

A `request-leg` gate reads one leg of a koto request (`koto request create`) and reports what the session answering that leg recorded. It's how a coordinating workflow waits for, and then routes on, the result of a workflow it handed work to, without the agent relaying anything.

```yaml
variables:
  REQ:
    required: true
states:
  scope_run:
    gates:
      scope_leg:
        type: request-leg
        request: "{{REQ}}"
        leg: scope
        expect:
          outcome: [scoped, declined]
        overridable: false
    transitions:
      - target: scoped
        when:
          gates.scope_leg.disposition: resolved
          gates.scope_leg.payload.outcome: scoped
        context_assignments:
          plan_path: "${gates.scope_leg.payload.plan_path}"
      - target: declined
        when:
          gates.scope_leg.disposition: resolved
          gates.scope_leg.payload.outcome: declined
      - target: request_abandoned
        when:
          gates.scope_leg.disposition: abandoned
```

**Fields.** `request` (the request id) and `leg` (the leg name) are required. Both may use `{{VAR}}` references, which the tick substitutes. A literal value is checked at compile time against the same rules `koto request` applies to ids and leg names; a substituted value is checked when the gate is evaluated, and a bad one makes the gate report outcome `error` with the reason in `error`, without reading the store. `expect` is optional: a map from a payload key to the list of scalar values that key may carry. An empty map, an empty list, or a list element that is an object, an array, or null is a compile error.

**Output.**

| Field | Meaning |
|-------|---------|
| `found` | `true` when the request and leg exist and were read. |
| `disposition` | `open`, `resolved`, `abandoned`, or `missing`; empty when the gate errored. |
| `bound` | Whether a session is bound to the leg. |
| `source` | For a resolved leg, how the result got there: `promoted` (from the bound session's terminal tick), `explicit` (`koto request resolve`), or `refused` (koto turned away the session that was to answer it). Empty otherwise. |
| `status` | The result's `status`: `success`, `failure`, or `skipped`. Empty until resolved. |
| `final_state` | The terminal state a promoted result came from. Empty for explicit and refused results. |
| `template` | The bound session's template source file name, as the attach recorded it. Empty for explicit and refused results. |
| `outcome`, `step`, `reason` | Copied from the payload's string keys of the same names; empty when absent or not a string. |
| `valid` | `true` only for a resolved leg whose payload is an object carrying every `expect` key with a listed value. Keys `expect` doesn't name are ignored. With no `expect`, any resolved object payload is valid. |
| `payload` | The result's payload object, or `{}` when it has none (or has one that isn't an object). |
| `error` | Why the gate couldn't read the leg; empty on a normal read. |

**Dispositions.** A `resolved` leg passes the gate. An `abandoned` leg passes too, as does an unresolved leg on a request that was abandoned or closed, because nothing can answer it any more; route it to a state that handles the abandonment. A `missing` request or leg fails the gate, and an arm keyed on `disposition: missing` can still fire, since gate output reaches `when` clauses whether or not the gate passed. An `open` leg fails the gate and is a temporal block: `koto next` stops with `gate_blocked` (or `evidence_required` on a state with `accepts`) and `category: "temporal"`, the same wait a `children-complete` gate gives. Don't write an arm for the open case; the stop is the wait. When no request store is reachable (no home directory, or a host where request records aren't available), the gate reports outcome `error` with `found: false` rather than passing.

**Payload paths.** A `when` clause routes on a key inside the payload with `gates.<gate>.payload.<key>`, and deeper keys work too (`gates.<gate>.payload.detail.kind`). This is the one place a `gates.*` path may run past three segments; every other field and gate type still takes exactly `gates.<gate>.<field>`. A `when` clause on the whole `payload` object is a compile error, since a scalar can never equal an object. The same paths work in `context_assignments` (`${gates.scope_leg.payload.pr}`), which is how a coordinator copies what the child reported into its own context.

**The gate only reads.** Evaluating it never appends to the request log, binds, resolves, or abandons the leg.

**Make it non-overridable.** The built-in default an override would inject is a resolved, valid record with an empty payload, so it names no outcome, and an override on a leg gate would let an agent claim a child finished when it didn't. Declare `overridable: false` on any leg gate whose outcome decides what happens next; that also exempts the state from the reachability check, which its payload arms couldn't otherwise pass.

### Updating your SKILL.md

When your template uses content-aware gates, update the SKILL.md to instruct the agent to submit content through `koto context add` rather than writing files to `{{SESSION_DIR}}`. The evidence keys section should document the expected content keys and their purpose.

## Terminal results

When a workflow finishes, koto records a result: a `status` (`success`, `failure` or `skipped`, from the terminal's `failure` and `skipped_marker` flags), a one-line `summary`, and an optional structured `payload`. A terminal state can declare what goes in that payload with a `result:` map, so the outcome a caller routes on comes from the template instead of from text the agent composes.

```yaml
states:
  done_error:
    terminal: true
    failure: true
    result:
      outcome: error
      step: "${context.step}"
      pr: "${context.home_pr}"
      topic: "{{TOPIC}}"
      state: "merge-state:${context.state}"
```

Each value is a string built from three forms, which can be mixed within one value:

| Form | Resolves to |
|------|-------------|
| literal text | itself |
| `{{VAR}}` | the session's value for a declared variable (or a runtime name such as `SESSION_NAME`) |
| `${context.<key>}` | the content stored under `<key>` with `koto context add`, read as UTF-8 |

The rules the compiler enforces:

- `result:` is only allowed on a terminal state.
- A map holds at most 32 keys. Keys follow the context-key grammar (letters, digits, `.`, `_`, `-`, with `/` between components).
- `missing` is reserved and can't be declared (see below).
- Values must be strings. A number or boolean is taken as its text; a nested mapping or a list is an error.
- A `{{VAR}}` must name a declared variable, and a `${context.<key>}` must name a valid context key. No other `${...}` form is allowed here: `${evidence.x}` or `${gates.g.x}` fails compilation. To report something a gate or evidence produced, write it to the context store first and reference it as `${context.<key>}`.

**When it resolves.** koto resolves the map once, on the tick that lands the session in the terminal state, and records the result on the session's own log. Every later read returns that recorded value, so writing to a context key after the terminal doesn't change what the session reported. Resolution is single-pass: a value that itself contains `{{X}}` or `${context.y}` is copied literally, not expanded again.

**Unresolved references.** A `${context.<key>}` whose key doesn't exist, or whose content isn't valid UTF-8, resolves to the empty string, and the result key it sits in is listed in a `missing` array inside the payload. `missing` is present only when something didn't resolve. The terminal tick still succeeds; check `missing` if a caller depends on a key.

```json
{"status": "failure", "summary": "failed at done_error",
 "payload": {"outcome": "error", "step": "scope:push", "pr": "", "topic": "my-topic",
             "state": "merge-state:open", "missing": ["pr"]}}
```

**It replaces the evidence-derived payload.** Without a `result:` map, the payload is the evidence fields submitted on the terminal state, as it always was. With one, the payload is exactly the resolved map; terminal evidence fields aren't merged in. `status` and `summary` are derived the same way either way.

**Where the result appears.** The same value is carried everywhere a result goes:

- the `result` field of the `koto next` response that reaches the terminal (`"action": "done"`), and of every later tick on a session kept with `--no-cleanup`;
- the `result` field of `koto status` on a session standing in a terminal state (a non-terminal session's status has no `result`);
- the result of a request leg the session is bound to (`koto request get`);
- the `result` of the `ChildCompleted` event appended to a parent workflow's log when a child finishes.

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

Structural validation says a skill compiles. The eval harness says an agent
reading it does the right thing, which is the failure structural validation
cannot see. `scripts/run-evals.sh` spawns a `claude -p` session per skill,
runs each case with and without the skill in context, and grades both against
the case's assertions -- so the report tells you not just whether the skill
works but whether it adds anything.

Evals live beside the skill they test, at
`plugins/<plugin>/skills/<name>/evals/evals.json`. One file per skill, holding
a `skill_name` and an `evals` array:

```json
{
  "skill_name": "your-new-skill",
  "evals": [
    {
      "id": 1,
      "name": "init-with-the-right-template",
      "prompt": "Awaken a spirit called Hasami.",
      "expected_output": "Agent copies the template to a project-local path, runs koto init with it, then drives the run loop with koto next.",
      "files": [],
      "assertions": [
        "Response runs `koto init` with `--template` pointing at the project-local copy",
        "Response drives the workflow with `koto next` rather than asserting completion",
        "Response submits the greeting through `koto context add` before expecting the gate to pass"
      ]
    }
  ]
}
```

Write assertions about what the agent *does* -- which commands, in which order,
with which evidence -- rather than about wording. An assertion on phrasing fails
on a rewrite that changed nothing that matters, and a skill whose evals fail
that way gets its evals deleted rather than fixed.

`files` is a list of fixture files the case needs on disk; leave it empty when
the prompt stands alone.

Run them:

```bash
# One skill
scripts/run-evals.sh your-new-skill

# Every skill that has evals
scripts/run-evals.sh --all

# Which skills have evals at all
scripts/run-evals.sh --list

# Re-grade the last run without spending tokens
scripts/run-evals.sh --validate your-new-skill
```

Running evals needs an `ANTHROPIC_API_KEY` and spawns real Claude sessions, so
it is a manual step rather than a CI one. What CI does enforce is that every
skill *has* at least one eval: `scripts/check-evals-exist.sh`, run by the
`eval-plugins` workflow on any PR touching `plugins/`. Include the results table
in your PR description -- `CLAUDE.md` has the format.

## Worked example: hello-koto

Pulling it all together, here's how the hello-koto skill was built. Use this as a template for your own.

### The template

The template, as given in Step 1 above, defines two states:

- **awakening** -- The agent writes a greeting and submits it via `koto context add`. A `context-exists` gate (`key: spirit-greeting.txt`) blocks the transition until the key exists in the content store.
- **eternal** -- Terminal state. Nothing to do.

One variable, `SPIRIT_NAME`, is interpolated into the awakening directive.

### Compiling and extracting

```bash
$ koto template compile .koto/templates/hello-koto.md | jq '.states | keys'
[
  "awakening",
  "eternal"
]

$ koto template compile .koto/templates/hello-koto.md | jq '.states.awakening.gates'
{
  "greeting_exists": {
    "type": "context-exists",
    "key": "spirit-greeting.txt"
  }
}
```

This tells us the SKILL.md needs to document one gate (`greeting_exists`) on the `awakening` state.

### The SKILL.md

The SKILL.md covers all seven sections:

- **Prerequisites**: koto on PATH
- **Template Setup**: copy to `.koto/templates/hello-koto.md`
- **Workflow**: two states, what happens in each
- **Execution**: five steps with exact commands and expected JSON
- **Error Handling**: four failure cases with recovery instructions
- **Resume**: check `koto workflows`, run `koto next`, continue

### The eval

The eval case sends `Awaken a spirit called Hasami` as the prompt and asserts that the response runs `koto init` with the template path and drives the loop with `koto next`.

### The full agent flow

When a user invokes `/hello-koto Hasami`:

1. Agent reads the SKILL.md.
2. Copies the template to `.koto/templates/hello-koto.md` if needed.
3. Runs `koto init hello --template .koto/templates/hello-koto.md --var SPIRIT_NAME=Hasami`.
4. Runs `koto next hello` -- gets the awakening directive.
5. Submits the greeting via `koto context add hello spirit-greeting.txt`.
6. Runs `koto next hello --to eternal` -- the `context-exists` gate passes.
7. Runs `koto next hello` -- gets `{"action":"done"}`.
8. Reports completion to the user.

## Cross-platform support

SKILL.md files follow the [Agent Skills standard](https://agentskills.io). This means a skill you write for koto works across Claude Code, OpenAI Codex CLI, Cursor, Windsurf, Gemini CLI, and GitHub Copilot without any changes to the SKILL.md itself.

For platforms that don't support the Agent Skills standard natively, the same instructions can be adapted to platform-specific formats like `AGENTS.md` (Codex, Windsurf) or `.cursor/rules/*.mdc` (older Cursor versions). The content stays the same -- only the file format changes.
