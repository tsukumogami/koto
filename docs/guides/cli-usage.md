# CLI Usage Guide

koto's CLI manages workflow state for AI coding agents. All commands output JSON. All commands exit with code 0 on success and non-zero on failure. Errors are printed to stdout as JSON:

```json
{"error":"workflow 'my-workflow' not found","command":"next"}
```

## Session storage

Each workflow's state lives in a dedicated session directory under `~/.koto/sessions/<name>/`. The namespace is flat and shared across repositories: a session name identifies a session on this machine, wherever it was started.

Older koto versions namespaced sessions by repository (`~/.koto/sessions/<repo-id>/<name>/`). koto migrates that layout on first use. A name that was reused across repositories has several old-layout sources and one flat destination, so only one of them can keep the name; the rest are moved aside into `~/.koto/sessions/.migration-conflicts/<repo-id>/<name>/` and are not listed until you restore them. See [session recover](#session-recover).

The state file inside each session directory is named `koto-<name>.state.jsonl` and uses an event log format:

- **Line 1 (header):** JSON object with `schema_version`, `workflow`, `template_hash`, and `created_at`.
- **Lines 2+:** Typed events, each with a monotonic `seq` number, `timestamp`, `type`, and a type-specific `payload`.

The current state is derived by replaying the log -- it's the `to` field of the last state-changing event (`transitioned`, `directed_transition`, or `rewound`).

```
~/.koto/sessions/my-workflow/koto-my-workflow.state.jsonl
~/.koto/sessions/task-42/koto-task-42.state.jsonl
```

There are no `--state` or `--state-dir` flags. All commands take the workflow name as a positional argument and resolve the session directory automatically.

## Commands

### init

Creates a new workflow from a template file.

```bash
koto init <name> --template <path>
```

**Positional argument:**
- `<name>` -- Workflow name. Used as the session directory name and in the state file name (`koto-<name>.state.jsonl`).

**Required flags:**
- `--template` -- Path to the workflow template file.

**Variables:**
- `--var KEY=VALUE` -- Set a template variable (repeatable). A value may hold letters, digits, `.`, `_`, `-`, `/`, `:`, `@`, `+` and spaces, because it can land unquoted in a gate command run through `sh -c`. Every other character, including `;` `|` `&` `$` `(` `)` `<` `>` `*` `?`, quotes, backticks and newlines, is refused with `invalid_var` (`constraint: "allowlist"`). A variable that declares `values:` or `pattern:` narrows that set further. The same check runs wherever a value is set later: `--vars-file`, a rebind through `--attach-live`, and `--replace-terminal`.

**Dispatcher:**
- `--dispatched-by <name>` -- Record that session `<name>` dispatched this one: a dispatched-by link in the new (or attached, or replacement) session's own context, the same link [`koto session link`](#session-link) writes. Nothing is written into `<name>`, which need not exist. A name that fails the session-name rule, or that is the session's own, exits 2 with `invalid_usage` before anything is created. A link that can't be written after the session was created prints a warning on stderr and leaves the session in place; `koto session link` records it later. Works with every init path, `--from-stdin` and `--parent` included.

**Agent-session anchor:**
- `KOTO_ANCHOR` -- When this variable names an anchor (see [session anchor](#session-anchor)) other than the session being created, init writes a held-by link, `koto/links/held-by/<anchor>`, into the new session's context, unless it already holds one, and appends an `anchor_activity` event to that anchor. When the anchor reads closed by an observed close and its family has a later anchor (`<anchor>-s2` and so on), the session also gets a held-by link to that one, and the activity goes there. Activity is throttled: at most one event per tenth of the idle period (`retention.idle_period`, default `7d`, see [config](#config-keys-reference)), judged from the anchor's last event; a malformed value falls back to the default here and never fails the command. A variable that is unset, empty, or names no anchor does nothing and prints nothing. koto reads only `KOTO_ANCHOR`, never `KOTO_DISPATCHER_ANCHOR`. No other session is written.

**Output (JSON):**

```json
{"name":"my-workflow","state":"assess"}
```

This creates a session directory at `~/.koto/sessions/<name>/` and writes a state file inside it. The state file starts with three lines: a header, a `workflow_initialized` event (seq 1), and an initial `transitioned` event (seq 2, from: null, to: the template's initial state).

Exits non-zero if a workflow with that name already exists or if the template is invalid.

Every new session's header carries an **origin record**, `origin`: the session's execution anchor (the canonical directory its commands run in) and the identity of the session store that holds it (`kind`, `local` or `cloud`, and the canonical sessions directory `base`). Session names are machine-wide, so the record is what tells two same-named sessions from different worktrees or stores apart. `koto session rebind` moves the record's anchor along with the session. Sessions created before this field existed have none, and nothing backfills it.

#### Command environment

`koto init` also records the environment the session's gates and default actions will run with. It records `PATH` (with empty and relative entries dropped), `HOME` and `XDG_CONFIG_HOME` from its own process, plus koto's default list of variable names whose values are read live on each tick. Every command then runs as `/bin/sh -c` in a cleared environment holding those names' live values, the names the template declares in `pass_env:`, the three recorded values, and `KOTO_TICK_SESSION` and `KOTO_SESSIONS_BASE`. Nothing else from your shell reaches it. A child, whether spawned by a batch, `koto init --parent` or `koto session start`, copies its parent's record, and records from its own process only when the parent has none yet. The default-action guide's [What a command's environment is](default-action-authoring.md#what-a-commands-environment-is) publishes the lists.

When recording drops a `PATH` entry, or leaves a value unset because it was relative or contained a token's or proxy URL's value, the response carries an `environment` report naming them:

```json
{"name":"my-workflow","state":"assess","environment":{"dropped_path_entries":["node_modules/.bin"],"unset":[]}}
```

The record can't be changed. If a tool moves off the recorded `PATH`, or you need different values, start a new session: `koto cancel --cleanup <name>`, then `koto init` again. When a gate or action fails and a recorded value no longer exists, or the command wasn't found, the `koto next` response's `directive` opens with a note naming the gate or action, the missing values, and that remedy.

- `--legacy-environment` -- Run this session's commands with the caller's whole environment, plus `KOTO_TICK_SESSION`, as koto did before it recorded one. It's recorded in the session at creation, children copy it, and no later command can set or clear it, so it has no effect when `--attach-live` attaches to an existing session. It's refused with `--parent`, since a child takes its parent's record. **Temporary:** it will be removed in the next release, once the test harnesses of shirabe (a workflow skills plugin built on koto) have migrated off it. Declare what your commands need in the template's `pass_env:` instead.

A session created by an earlier koto has no record. Its first `koto next` adopts one from the ticking process, appends an `environment_adopted` event, and opens that response's `directive` with a one-time notice showing the recorded `PATH`, `HOME` and `XDG_CONFIG_HOME` and any dropped entries.

#### Entry flags

A skill that enters the same session on every invocation passes these flags instead of reading the session first and deciding what to do. With any of them, or with `--vars-file`, one `koto init` call ends in one of four outcomes:

- a new session (`"outcome": "created"`);
- an attached live session (`"outcome": "attached"`), with its `rebind: true` variables re-applied;
- a fresh session replacing a finished one (`"outcome": "replaced"`);
- a refusal with a typed `code`, almost always exit 2, that changes nothing.

```bash
koto init <name> --template <path> --vars-file <file> [--attach-live] [--replace-terminal] [--koto-leg <request-id>:<leg>]
```

- `--vars-file <path>` reads the variables from a JSON list of `["KEY", "VALUE"]` string pairs, such as `[["TOPIC","t1"],["MERGE","true"]]`. A list rather than an object, so a repeated key survives to be refused as `duplicate_var`. Values pass the same checks as `--var`. The file must be a regular file of at most 64 KiB and not a symlink; anything else, including malformed JSON or a pair that isn't two strings, is refused with `invalid_vars_file`. It can't be combined with `--var`. With `--vars-file` or any entry flag, variables are validated before the name is looked up, so a bad value against an existing session is reported as the variable error, not "already exists".
- `--replace-terminal` replaces a session that is finished (in a terminal state, or cancelled) with a fresh one under the same name. The output carries the old session's `replaced_state` and `replaced_result`, its workflow result (`null` for a cancelled session that recorded none). What happens to the old run depends on the retention rule (see [session hierarchy](#session-hierarchy)):
  - **Released** (nothing in its hierarchy is alive and `retention.keep` has passed since the release instant): koto removes it, with its terminal descendants, as it always did. Removing the session removes its override log too.
  - **Kept** (anything else, including a run that only just finished): the old run moves aside instead, so whoever holds it can still read it. It is renamed `<name>_r<N>`, N the lowest value that leaves every name the move needs free, in the local store and, under the cloud backend, in the bucket. Its koto descendants (found by `parent_workflow`, whatever their names) move with it: one named `<name>.<rest>` or `<name>~<rest>` becomes `<name>_r<N>.<rest>` or `<name>_r<N>~<rest>`, and one with any other name keeps it. Each moved session's `parent_workflow` is re-pointed under the new name; the old run keeps its own. Logs, keys and links are not touched, and the new run starts with no children. The output gains `moved_to` (the old run's new name), `moved_reason` (why the rule keeps it) and `moved`, every rename as `{"from", "to"}`. A retention setting that is malformed, or a store that can't be read, counts as kept, with a warning on stderr.
  - A descendant that is still running, waits to promote its result to a request leg, or can't be read refuses the replace with `replace_held_live`, exit 2, naming them under `live`; nothing changes. If any step of the move fails, koto moves everything back, restoring the original headers, and refuses with `replace_failed`, exit 1. Under the cloud backend the move copies each renamed session's objects to its new prefix, pushes every changed header, and deletes the old prefixes only after all of that succeeded; an old prefix it then can't empty is reported under `undeleted_remote_prefixes` with a warning, and the new run is still created.
  - A running session is refused with `session_live`, whatever the rule says.
- `--attach-live` attaches to a running session instead of refusing the name. koto checks, in order, that the session was built from a template file with the same name as `--template` (`template_mismatch`, with `recorded` and `requested`); that its origin record equals this invocation's, meaning the same execution anchor (`--execution-dir` or the working directory) and the same session store, including the same bucket for the cloud store (`origin_mismatch`); and that every non-`rebind` variable passed explicitly equals the recorded value (`var_mismatch`, naming `var`, `recorded` and `requested`). Variables the caller doesn't pass aren't compared. A session with no origin record is refused with `origin_mismatch` and a message that says so: finish it with the koto version that started it, or remove it with `koto session cleanup <name>`. A session the local store started, attached once the host runs on the cloud backend, from the same anchor and over the same sessions directory, is the same session: koto uploads it whole to the bucket and records the cloud origin, so later attaches compare cloud to cloud. If the upload fails, the attach refuses with `persistence_error` (exit 3) and the record stays local. A session the cloud store started is still refused under the local backend. On acceptance every `rebind: true` variable is re-applied from this invocation (its value, else the declared default), recorded as one `variables_rebound` event, and the output lists the changes under `rebound`. A finished session is refused with `session_terminal`. `--intent` is not applied to an attached session. If the caller's `PATH`, `HOME` or `XDG_CONFIG_HOME` differs from the session's recorded values, the attach still succeeds: koto prints one warning on stderr and adds `environment_drift`, an array naming the differing variables (never their values), to the output. The session's commands keep running with the recorded values. A session with no record yet, or one created with `--legacy-environment`, isn't compared.
- With both `--attach-live` and `--replace-terminal`, a running session is attached, a finished one is replaced, and a missing one is created.
- `--koto-leg <request-id>:<leg>` attaches the created, attached or replacement session to a request leg in the same call, with every check `koto request attach` makes (see [request attach](#request-attach)). A value that doesn't match the request-id and leg-name grammars is a usage error. The output gains `leg`: `{"request_id", "leg", "written"}`. Once the leg is bound, the session gets a dispatched-by link naming the request's `requested_by`, as `--dispatched-by` would record it; a `requested_by` that a link can't name (one that fails the session-name rule, makes a key over 255 characters, or is the session itself) gets no link.

Without `--attach-live` or `--replace-terminal`, an existing session still gets the "already exists" message and exit 1. The three entry flags are rejected with `--from-stdin` and with `--parent` (`invalid_usage`, exit 2).

**Order of checks and writes.** Every check runs before anything is written: variables, then the existing session's template, origin and fixed variables, then the leg checks. Writes follow in a fixed order: create or replace the session, bind the leg, then re-apply the rebind variables, and last the dispatched-by links. So a refused invocation leaves the session and its variables as they were; a stale invocation that names an abandoned leg, or a leg another session holds, can't flip a rebind variable such as `MERGE`. If the leg bind loses a race after the checks passed, the invocation exits with the bind's error, removes a session it just created, and leaves an attached session's variables unchanged.

**Refusals recorded on the leg.** Under `--koto-leg`, every refusal is also written onto the named leg when that leg is open and unbound: it resolves with `result_source: refused`, status `failure`, and a payload `{"outcome": "refused", "reason", "var", "recorded", "requested"}`. `reason` is `invalid-var:<V>`, `duplicate-var:<V>`, `unknown-var:<V>`, `var-mismatch:<V>`, `template-mismatch`, `origin-mismatch`, or, for any other refusal, its error code in the same kebab form (`session-terminal`, `session-live`, `leg-abandoned`, `input-mismatch`, and so on). The other three keys are empty strings when they don't apply. A leg that is bound, resolved or abandoned, or a request that is closed or doesn't exist, gets nothing, and a refusal never binds the leg or writes a leg pointer. The exit code and the printed error are the same with and without `--koto-leg`.

```json
{"name":"scope-t1","state":"work","outcome":"attached","rebound":{"MERGE":"false"},"leg":{"request_id":"req-...","leg":"scope","written":true}}
```

### next

Returns the directive for the current state. This is the main agent-facing command -- it tells the agent what to do next, what evidence to submit, and whether any gates are blocking.

```bash
koto next <name> [--with-data @<file>] [--to <target>] [--no-cleanup]
```

**Positional argument:**
- `<name>` -- Workflow name.

**Optional flags:**
- `--with-data @<file>` -- Submit evidence as a JSON object read from `<file>` (or given inline; see below), validated against the state's `accepts` schema. On success, appends an `evidence_submitted` event and sets `advanced: true` in the response.
- `--to <target>` -- Directed transition to a named state. The target must be a valid transition from the current state. Appends a `directed_transition` event, then dispatches on the new state. Gates are skipped, except a non-overridable gate the edge to the target depends on; see [Directed transitions and non-overridable gates](#directed-transitions-and-non-overridable-gates).

The `--with-data` and `--to` flags are mutually exclusive. Passing both produces a `precondition_failed` error with exit code 2. The `--with-data` payload is capped at 1 MB.

**Agent-session anchor.** `koto next` reads `KOTO_ANCHOR` the way [init](#init) does: a held-by link to the named anchor in the ticked session when it has none, the same successor rule, and a throttled `anchor_activity` event in the anchor. A second tick under the same anchor adds no link; a tick under another anchor adds a second one. A nested tick writes nothing. `koto next` on an anchor or an anchor's close session is refused with `precondition_failed` (exit 2) and changes nothing.

**Advisories.** A check that passes while printing `warning` findings doesn't block, but its warnings still reach you: the response carries a top-level `advisories` list with one entry per such check this call evaluated, as `{"state": ..., "gate": ..., "findings": [...]}` (`gate` is `__action__` for a default action), at most 100 findings in all; when that cap leaves warnings out, the response also carries `advisories_truncated: true` (absent otherwise). A response with none has no `advisories` key. `--to` and `koto status` never carry it. To say a warning is wrong, use [`koto disputes record`](#disputes-record).

#### Directed transitions and non-overridable gates

`--to` skips a state's gates, so it stays the way out of a stuck state, with one exception: a gate declared `overridable: false` that the edge to the target depends on is evaluated first. If its result doesn't satisfy that edge, `--to` exits 1 with `gate_blocked`, names the gate in the message and in `details`, and appends nothing. A `--to` that proceeds appends exactly the `directed_transition` event it always did.

An edge depends on a gate the way `koto next` would route it:

- An edge whose `when` clause reads `gates.<gate>.*` is checked against the gate's real output. An edge that routes on the gate failing stays open while it fails, and an edge that needs a particular output is refused while the gate produces a different one, even if the gate passed.
- An unconditional edge needs the gate to pass.
- In a state that has no `accepts` and doesn't route on gate output, every edge needs the gate to pass.
- Any other edge, such as an evidence-conditioned edge in a state with `accepts`, doesn't depend on the gate.

When several edges lead to the target, `--to` is refused only if every one is blocked. A gate that reads a `capture_stdout_as` value this run hasn't produced can't be evaluated, so `--to` refuses with `capture_unset`. Overridable gates are never evaluated on this path.

The `--with-data` value can be a file reference or inline JSON. Prefix a path with `@` to read the payload from disk. Prefer the file form: agents often run koto behind a prefix that strips a token (`env -u GH_TOKEN koto ...`), and some agent harnesses refuse a quoted JSON argument after that prefix because they can't tell what the command runs. The file gives them nothing quoted to misread, and koto reads the same JSON from it. It also suits batch task lists and any payload large enough to be awkward on the command line:

```bash
# Read from a file (preferred)
koto next coord --with-data @tasks.json

# Inline JSON, also accepted
koto next task-42 --with-data '{"decision":"proceed"}'
```

The 1 MB cap applies to both forms (file size is checked before reading). Use `@-` is **not** supported; only file paths are accepted after `@`.

- `--full` -- Include the `details` field in the response regardless of delivery state. By default, `details` is delivered when the workflow arrives at a state -- entering it from a different state, or being rewound into it -- and omitted on every later tick until it arrives again. Going around a loop it is already in is not an arrival: a self-transition, and a `--to` transition into the state the workflow already occupies, both repeat nothing. What decides it is whether the entry event that landed the workflow records a different source state -- so a tick that leaves a state, passes through another, and comes back within the same tick does deliver. This flag forces inclusion every time.
- `--no-cleanup` -- Keep the session after it reaches a terminal state. A terminal declared `failure: true` is always kept, and so is a session with a link or lineage, so the flag matters only for a lone session. Keeping a session changes nothing else about the tick: the result still reaches the parent and any bound request leg. The flag applies to the tick it's passed on: a kept lone success terminal ticked again without it is removed.

**What a terminal tick does with the session.** On the tick a session arrives at a terminal state, koto records the result on the session's own log, promotes it to a bound request leg, writes the terminal-index entry and appends `ChildCompleted`, carrying the result, to the parent's log. It does each once per arrival, whether or not the session is then kept; ticking a session already standing in a terminal writes nothing. A session that leaves its terminal (through `retry_failed`, `koto rewind`, or a `--to` from one terminal to another the template declares) and reaches one again is a new arrival.

Then koto decides whether to keep the session, and says so in the response's `retention` field. It checks, in this order:

| `retention` | When |
|---|---|
| `{"retained": true, "reason": "failure_terminal"}` | The terminal is declared `failure: true`. The session is always kept, flag or not, so the parent's `retry_failed`, `koto rewind`, `koto status` and `koto context get` still work on it. |
| `{"retained": true, "reason": "pinned", "kept_by_pin": true, "pin": {...}}` | The session holds a pin (see [session pin](#session-pin)), readable or not (`pin_unreadable`). Kept with or without `--no-cleanup`. A failure terminal that is also pinned reports `failure_terminal` with the pin beside it. Under the cloud backend the bucket's pin object counts, with `source: bucket`. |
| `{"retained": true, "reason": "bucket_unchecked", "bucket_unchecked": {"error"}}` | Under the cloud backend only: the session would have been removed, and the bucket couldn't be checked for a pin (`unreachable` or `unparseable`). |
| `{"retained": true, "reason": "hierarchy"}` | The session has a link or lineage: a link of its own (`koto/links/`), a `parent_workflow`, a `chain/parent`, or another session naming it through one of those, a link's session-id hint included. Kept with or without `--no-cleanup`; `koto session prune` reclaims it once its hierarchy is released (see [session hierarchy](#session-hierarchy)). |
| `{"retained": true, "reason": "no_cleanup"}` | Any other terminal, ticked with `--no-cleanup`. |
| `{"retained": false}` | Any other terminal, ticked without the flag. koto removes the session directory after the response. |

The `hierarchy` check reads the local sessions directory in one pass that stops at the first session naming this one, and reads no retention setting, so a malformed setting never fails a tick and a lone session is removed as it always was. The pass reads each session's header line and each link body (at most 4 KiB apiece), because a link can name a session by the id it recorded rather than by its name, as it does once a re-run moves its peer aside; `koto session prune` counts that link, so the tick does too, and never removes what a prune would keep. Anything it can't read counts as an edge and keeps the session.

A parent with children always has an edge, so it is kept at its own terminal, and its children with it. The sweep that would have removed its terminal children removes nothing; the response lists them instead, beside `retention`, in `kept_children`:

```json
"kept_children": [
  {"name": "plan-7.task-1", "reason": "hierarchy",
   "detail": "kept with its hierarchy (its dispatched-by link names coord; its parent_workflow names plan-7); koto session prune removes it once the hierarchy is released"}
]
```

It walks the parent's koto descendants the way the sweep does, without going below a child that is still running, and appears only on a parent's terminal tick that keeps it for `hierarchy` (one whose template has a batch hook or whose log holds a `ChildCompleted`) and has a terminal child. `detail` names the links and lineage that keep each child, so the listing says which sessions a later prune will wait on.

`retained: false` is koto's intent for the tick. If the parent's log or the request store can't be written, koto keeps the session so the next tick can retry, and says so on stderr. A kept child's later tick re-sends its `ChildCompleted` only when the parent holds none for the child's current arrival, so a notice whose write failed still arrives, and a repeat tick otherwise writes nothing.

A kept session stays until one of these removes it:

- it is retried or rewound and then, as a lone session, reaches a success terminal without `--no-cleanup`, and is removed on that tick;
- `koto init --attach-live --replace-terminal` replaces it, or its parent, once the retention rule releases their hierarchy; the replace removes the replaced session's terminal descendants first. It never removes a descendant that is still running, one with anything running under it, one it can't read, or one whose request leg still waits for its result. While the rule keeps the hierarchy, the replace moves the old run and its subtree aside instead (see [entry flags](#entry-flags));
- `koto workspace prune --root <root>` removes the members of a finished root's subtree that the retention rule releases (see [workspace prune](#workspace-prune));
- `koto session prune` removes finished sessions as a previewed set, keeping any whose hierarchy the retention rule keeps or that something else still needs (see [session prune](#session-prune));
- `koto session cleanup <name>` removes one session, and names any live member of its hierarchy the removal broke.

A kept session costs its directory on disk, typically tens of kilobytes. A hierarchy stays until a prune finds it released past `retention.keep`.

A kept session occupies its name: `koto init` on it is refused with a message naming `koto session cleanup`, and `koto init --attach-live --replace-terminal` replaces it or moves it aside. `koto session cleanup` on a kept parent removes only that session and leaves its kept children naming a parent no session holds; they show in `koto workflows --orphaned` and are removed one at a time, or with the whole tree by pruning the finished root above them. Children moved aside by a `koto rewind` of their batch parent (renamed `<parent>~N.<task>`) are in the same position.

**Runtime variable substitution:**

Before running a `default_action` command, resolving its `working_dir`, evaluating a gate's `command`, `key`, `pattern` or `name_filter`, or serializing directives and details, `koto next` replaces two tokens: `{{SESSION_DIR}}` with the absolute path to the workflow's session directory, and `{{SESSION_NAME}}` with the name the session was created under. This lets templates reference session-local files, and address the session itself, without hard-coding either:

```markdown
## plan

Write an implementation plan to {{SESSION_DIR}}/plan.md.

**Gate**: cat {{SESSION_DIR}}/plan.md | head -1
```

`SESSION_DIR` and `SESSION_NAME` are reserved variable names and can't be overridden by template-defined variables.

A `context-matches` gate's `pattern` is a regex, so a value substituted into it is escaped and matches itself. Anchors, classes and quantifiers you write around a `{{KEY}}` still mean what they say -- what changes is that the value can no longer contribute regex syntax of its own. A session name may contain a dot, so `pattern: "^ready {{SESSION_NAME}}$"` against a session named `probe.one` matches `ready probe.one` and not `ready probeXone`.

A `children-complete` gate's `name_filter` resolves references in the plain form: it is a name prefix compared against child workflow names, not a shell word and not a regex. Scoping a gate to your own fan-out is what it is for -- children spawned as `{{SESSION_NAME}}.research.1` are matched by `name_filter: "{{SESSION_NAME}}.research."`.

A reference in any of these fields can still resolve to a value the gate cannot use, and the compiler cannot see that because the value does not exist yet. Three cases are reported as a gate error carrying the reason, rather than as a mismatch you would have to diagnose: a `key` that resolves to something the context store refuses; a `pattern` that resolves to an empty string, which would otherwise match every input and pass the gate on anything at all; and a `name_filter` that resolves to an empty string, which would not narrow the gate but remove the filter, so a gate written to watch one fan-out would silently watch every child. Declaring no `name_filter` is how you ask to watch every child, and is unaffected.

One field deliberately does not resolve a reference: a `default_action`'s `fallback`. It is spliced onto the failure response after substitution has run, so the prose reaches the agent as written -- and because a reference there would never resolve, the compiler now refuses one rather than letting it through in silence. Put the reference in the state's directive instead.

**Response variants:**

Every successful response is a JSON object with an `action` field and an `error` field set to `null`. The `action` value identifies the response type -- dispatch on it directly.

| Field | EvidenceRequired | GateBlocked | Integration | IntegrationUnavailable | Confirm | Terminal |
|-------|:---:|:---:|:---:|:---:|:---:|:---:|
| `action` | `"evidence_required"` | `"gate_blocked"` | `"integration"` | `"integration_unavailable"` | `"confirm"` | `"done"` |
| `state` | yes | yes | yes | yes | yes | yes |
| `directive` | yes | yes | yes | yes | yes | -- |
| `details` | optional | optional | optional | optional | optional | -- |
| `advanced` | yes | yes | yes | yes | yes | yes |
| `expects` | object | `null` | object or `null` | object or `null` | object or `null` | `null` |
| `blocking_conditions` | array | array | -- | -- | -- | -- |
| `action_output` | -- | -- | -- | -- | object | -- |
| `integration` | -- | -- | object | object | -- | -- |
| `unassigned_children` | array | array | array | array | array | array |
| `result` | -- | -- | -- | -- | -- | object |
| `retention` | -- | -- | -- | -- | -- | object |
| `error` | `null` | `null` | `null` | `null` | `null` | `null` |

"yes" = always present. "--" = absent from the JSON (not `null`, just missing). "object or `null`" = present as an object when the state has an `accepts` block, `null` otherwise. "optional" = present when the workflow arrives at the state (or when `--full` is passed), absent on every later tick until it arrives again and when the state has no details content. Use `koto status <name>` to retrieve `directive`/`details`/`expects` unconditionally regardless of delivery state -- see the `status` command below.

The `unassigned_children` array is present on every `NextResponse` variant (including Terminal `done` and Error) so coordinator-side consumers branch uniformly on the field rather than on the action label. Each element describes a child workflow waiting on agent dispatch with fields `child_session_id`, `role`, `template`, `inputs` (optional), `requested_by`, `created_at`, and `dispatch_epoch`. The discovery scan populates the list from headers under `~/.koto/sessions/*` whose request-store fields name the workflow being ticked as their `coordinator_of_record` and that have not yet been claimed; the list caps at `request_store.directive_batch_size` (default 50) per tick, with overflow surfaced on subsequent ticks.

The `advanced` field is a boolean indicating that at least one state transition occurred during this invocation. It's informational only -- dispatch on `action`, not on `advanced`.

**EvidenceRequired** -- the state expects the agent to do work and submit evidence:

```json
{
  "action": "evidence_required",
  "state": "review",
  "directive": "Review the code changes.",
  "advanced": false,
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "decision": {"type": "enum", "required": true, "values": ["proceed", "escalate"]}
    },
    "options": [
      {"target": "implement", "when": {"decision": "proceed"}}
    ]
  },
  "blocking_conditions": [],
  "error": null
}
```

The `expects.options` array is omitted when no transitions have `when` conditions. The `values` array on a field is omitted when empty.

The `blocking_conditions` array is always present on `evidence_required` responses. When gates fail on a state with an `accepts` block, the array is populated with the failing gates. Fix the conditions first, then call `koto next` again -- once gates pass, submit evidence normally. When no gates are blocking, the array is empty.

**GateBlocked** -- one or more command gates failed, timed out, or errored on a state without an `accepts` block:

```json
{
  "action": "gate_blocked",
  "state": "deploy",
  "directive": "Deploy to staging.",
  "advanced": false,
  "expects": null,
  "blocking_conditions": [
    {
      "name": "ci_check",
      "type": "command",
      "status": "failed",
      "agent_actionable": false,
      "output": {"exit_code": 1, "error": ""}
    }
  ],
  "error": null
}
```

Each entry in `blocking_conditions` includes structured gate output in the `output` field. The shape of `output` depends on the gate type -- see the [gate output schemas](#gate-output-schemas) in the custom skill authoring guide for details. Passing gates don't appear in the array.

`status` reflects the `GateOutcome`: `"failed"` (pass condition not met), `"timed_out"` (command exceeded its timeout), `"error"` (spawn or evaluation error).

**Why a check failed: `failure` and `attempts`.** The example above shows only the fields routing reads. A failed `command`, `context-exists` or `context-matches` gate, and a failed `default_action` (the `__action__` condition), also carry a `failure` object beside `output`, and a response with blocking conditions carries a top-level `attempts` object. Here's the second tick of a state whose lint gate prints two findings and exits 1 (the finding line in `captured.stdout` is shortened):

```json
{
  "action": "gate_blocked",
  "advanced": false,
  "attempts": {
    "rules": {
      "ruff": {
        "E501": {"session": 2, "visit": 2}
      }
    },
    "session": 2,
    "visit": 2
  },
  "blocking_conditions": [
    {
      "agent_actionable": true,
      "category": "corrective",
      "failure": {
        "captured": {
          "stderr": "",
          "stderr_truncated": false,
          "stdout": "::koto-finding::{\"rule_id\":\"E501\",...}\n::koto-finding::{\"rule_id\":\"W291\",...}\nFound 2 errors.\n",
          "stdout_truncated": false
        },
        "findings": [
          {
            "column": 89,
            "effect_landed": false,
            "level": "error",
            "line": 12,
            "message": "line too long (104 > 88)",
            "message_source": "check",
            "path": "app/views.py",
            "rule_id": "E501",
            "rule_ref": "https://docs.example.org/rules/E501"
          },
          {
            "effect_landed": false,
            "level": "warning",
            "line": 40,
            "message": "trailing whitespace",
            "message_source": "check",
            "path": "app/views.py",
            "rule_id": "W291"
          }
        ],
        "findings_truncated": false
      },
      "name": "ruff",
      "output": {"error": "", "exit_code": 1},
      "status": "failed",
      "type": "command"
    }
  ],
  "directive": "Fix the lint errors.",
  "error": null,
  "expects": null,
  "state": "lint",
  "unassigned_children": []
}
```

`output` is unchanged, so nothing in `failure` or `attempts` can be routed on or overridden, and a passing or evidence-only response has neither field. The field reference, the order an agent works through a failed check, and why `failure` is treated as data rather than instructions are in [Reading why a check failed](../../plugins/koto-skills/skills/koto-user/references/response-shapes.md#reading-why-a-check-failed); a check reports its own findings with the `::koto-finding::` line format in the [`default_action` authoring guide](default-action-authoring.md#reporting-findings).

**Integration / IntegrationUnavailable** -- the state declares an integration. When the runner is available, you get `"integration"` with the output. When unavailable, you get `"integration_unavailable"` with `available: false`:

```json
{
  "action": "integration_unavailable",
  "state": "delegate",
  "directive": "Run the integration.",
  "advanced": false,
  "expects": null,
  "integration": {"name": "code_review", "available": false},
  "error": null
}
```

**Confirm** -- a default action ran and needs review before the engine records its result:

```json
{
  "action": "confirm",
  "state": "context_injection",
  "directive": "Review the action output.",
  "advanced": false,
  "action_output": {
    "command": "extract-context.sh --issue 42",
    "exit_code": 0,
    "stdout": "...",
    "stderr": ""
  },
  "expects": {
    "event_type": "evidence_submitted",
    "fields": {
      "status": {"type": "enum", "required": true, "values": ["accepted", "rejected"]}
    }
  },
  "error": null
}
```

Review the `action_output` and submit evidence if the state accepts it.

**Terminal** -- the workflow has ended:

```json
{
  "action": "done",
  "state": "done",
  "advanced": true,
  "expects": null,
  "error": null
}
```

Terminal responses don't include `directive`, `details`, `blocking_conditions`, `action_output`, or `integration`.

**Dispatcher classification order:**

The dispatcher evaluates the current state in this order and returns the first match:

1. Terminal state -> `Terminal`
2. Any gate failed/timed_out/errored (no accepts block) -> `GateBlocked`
3. Integration declared -> `Integration` or `IntegrationUnavailable`
4. Accepts block exists -> `EvidenceRequired`
5. Gates failed but accepts block exists -> `EvidenceRequired` (with populated `blocking_conditions`)
6. Fallback -> `EvidenceRequired` with empty `expects` (auto-advance candidate)

**Error responses:**

All errors use a structured JSON format with `code`, `message`, and `details` fields:

```json
{"error": {"code": "<string>", "message": "<string>", "details": [...]}}
```

| Exit code | Error codes | Agent action |
|-----------|-------------|--------------|
| 1 | `gate_blocked`, `integration_unavailable`, `concurrent_access` | Retry after fixing or wait |
| 2 | `invalid_submission`, `precondition_failed`, `terminal_state`, `workflow_not_initialized` | Change your approach |
| 3 | `template_error`, `persistence_error` | Report to user |

`template_error` covers structural template problems: cycle detected, chain limit reached, ambiguous transition, dead-end state, unresolvable transition, unknown state. `persistence_error` covers disk I/O failures. `concurrent_access` means another `koto next` is already running on this workflow -- wait and retry.

### rewind

Rolls back the workflow to the previous state by appending a `rewound` event to the state file.

```bash
koto rewind <name>
```

**Output (JSON):**

```json
{"name":"my-workflow","state":"assess"}
```

The `rewound` event payload contains `from` (the current state) and `to` (the state being rewound to). Exits non-zero if the workflow is already at the initial state (only one state-changing event exists). Rewind is non-destructive -- it appends a new event rather than truncating history, so the full event log is preserved.

### workflows

Lists all active workflows for the current repository.

```bash
koto workflows
```

**Output (JSON):**

```json
[{"name":"my-workflow","created_at":"2026-03-15T10:00:00Z","template_hash":"a1b2c3..."},{"name":"task-42","created_at":"2026-03-15T11:30:00Z","template_hash":"d4e5f6..."}]
```

Each object contains the workflow name, creation timestamp, and template hash read from the state file header. Returns an empty array `[]` when no workflows are found. Anchors and anchor close sessions (see [session anchor](#session-anchor)) are not workflows and never appear here; `koto session list` shows them.

### status

Read-only view of a workflow's current state metadata. No gates are evaluated, no actions run, and no state changes happen -- and, unlike every other command in this reference, it takes no lock on the session at all.

```bash
koto status <name>
```

**Output (JSON):**

```json
{"name":"my-workflow","current_state":"assess","template_path":".koto/plan.template.json","template_hash":"a1b2c3...","is_terminal":false}
```

When `is_terminal` is `false`, the response also carries the current state's `directive`, `details`, and `expects` -- the same content `koto next` would show for that state, substituted through the identical pipeline so the text matches byte-for-byte. This is the retrieval to reach for when an agent has lost track of a state's instructions: it always returns them in full, independent of whatever delivery state `koto next` is tracking (see the `--full` note above), and calling it appends nothing and records no delivery. `directive`/`details`/`expects` are absent together when `is_terminal` is `true`; `details` is also absent on its own when the current state declares no details content.

Every `koto next` response whose current state declares instructions carries a short pointer to `koto status` in its `directive`, regardless of whether that response happened to include `details` -- so an agent that has lost everything else still learns the command exists.

If the template read from disk no longer matches the hash recorded in the session header, the response gains a `template_hash_mismatch` key rather than failing:

```json
{"template_hash_mismatch":{"recorded":"a1b2c3...","actual":"d4e5f6..."}}
```

`koto next` fails closed on the same mismatch; `status` reports it instead, because this command is often the only recovery path left to an agent, and failing here would deny it exactly when it's needed most.

For batch parents and sessions bound to a request leg, `koto status` also carries additional fields -- see [Batch workflows](#batch-workflows) below.

**A session koto removed.** koto removes a lone session at its success terminal (unless the tick passed `--no-cleanup`), and a replace of a released parent (`koto init --attach-live --replace-terminal`) removes its kept terminal children. Before either removal it writes a finished record, so `koto status` on that name still answers, exit 0:

```json
{"name":"deploy","current_state":"done","is_terminal":true,"result":{"status":"success","summary":"completed at done"},"removed":true,"finished_at":"2026-10-11T23:44:36.123Z","session_id":"<session-id>","execution_dir":"/work/project"}
```

`current_state`, `is_terminal` and `result` mean what they mean for a kept terminal session, so the same run kept with `--no-cleanup` answers the same three. The answer isn't a superset of a kept session's, though: with no session left to read, it has no `template_path`, `template_hash`, `batch` or `leg`. A child that had been cancelled before its parent removed it answers with its cancel state, a null `result` and `"status": "cancelled"`. A result too large for the record (over 64 KiB) keeps its `status` and `summary`, and the answer carries `"result_truncated": true`. A session removed by a person (`koto session cleanup`, `koto cancel --cleanup`) leaves no record. When those verbs remove a session, they also delete a record a crash left beside it, written just before a removal that never happened. A session whose removal failed keeps no record either, since koto deletes it again. So a name that never existed, a name a person removed, and a name whose record was pruned all answer `not found`, exit 2. Creating a session under the name deletes its record. The record keeps the session's whole result, including any payload its terminal state's result map carries, in a file only the user can read (mode 0600), until the name is reused or `koto session prune` removes it.

### session

The `session` subcommand group provides direct access to session directories. These are useful for skills that need to read or write session-local artifacts, and for manual cleanup during development.

#### session dir

Prints the absolute path to a session's directory. This is the primary way skills discover where to store artifacts.

```bash
koto session dir <name>
```

**Output (plain text):**

```
/home/user/.koto/sessions/my-workflow
```

The path is printed even if the directory doesn't exist yet (no I/O validation). This lets callers check the path before or after `koto init`.

#### session list

Lists all sessions for the current repository as a JSON array.

```bash
koto session list
```

**Output (JSON):**

```json
[
  {
    "id": "my-workflow",
    "created_at": "2026-03-15T10:00:00Z",
    "template_hash": "a1b2c3..."
  },
  {
    "id": "task-42",
    "created_at": "2026-03-15T11:30:00Z",
    "template_hash": "d4e5f6..."
  }
]
```

Each object contains the session id (same as the workflow name), creation timestamp, and template hash read from the state file header. Returns an empty array `[]` when no sessions exist. Directories without a valid state file are skipped.

Each row also carries `template_source_status`, describing whether the directory the session's template was loaded from (at `koto init` time) still resolves on this machine: `{"path": "...", "exists": true|false, "machine_id": "..."}`, or `null` when the session recorded no `template_source_dir` (or, under a cloud backend, when the row is a remote-only placeholder that has not been synced locally yet). When `exists` is `false`, the object gains a `note` field explaining the staleness. The wording is backend-aware: a local session gets a direct note ("template source directory no longer exists"), while a cloud-backed session gets a softened note, since the directory may simply be missing because the session came from another host: `koto session import` keeps the template directory the source host recorded (see "Moving a session to another host" in `docs/guides/cloud-sync-setup.md`).

Every row also carries `execution_dir`, the directory the session's ticks run in, and `session_id`, both from the session's header. Either is `null` when the header doesn't record it, and both are `null` on a remote-only placeholder row.

A row for a session koto keeps for its own bookkeeping also carries `kind`: `anchor` for an agent session's anchor and `anchor-close` for an anchor's close session (see [session anchor](#session-anchor)). Ordinary sessions have no `kind`.

**Optional flags:**
- `--state` -- Add where each session stands: `state` (the current state's name), `status` (`running`, `terminal`, `cancelled` or `unreadable`), `failure` (`true` for a terminal declared `failure: true`) and `finished_at` (the time of the last event in its log, which for a finished session is when it finished). This reads every session's log, where the plain listing reads only headers. A session is `cancelled` when its log records a cancellation that no later transition or rewind follows, and `unreadable` when its log or template can't be read; `state` is `null` for an unreadable session. All four are `null` on a remote-only placeholder row.
- `--execution-dir <dir>` -- List only the sessions whose execution directory is `<dir>` or lies under it, compared by whole path components: `/work/a` matches `/work/a` and `/work/a/b`, not `/work/ab`. `<dir>` is resolved (symlinks included) when it exists on this machine and compared as given otherwise, and a trailing slash makes no difference; `/` matches every session that records an execution directory, and a session that records none matches no filter. Implies `--state`, and the output becomes an object:

```json
{
  "sessions": [
    {
      "id": "issue_42",
      "created_at": "2026-10-11T23:44:36.123Z",
      "template_hash": "a1b2c3...",
      "parent_workflow": null,
      "template_source_status": null,
      "execution_dir": "/work/repo",
      "session_id": "<session-id>",
      "state": "done",
      "status": "terminal",
      "failure": false,
      "finished_at": "2026-10-11T23:50:02.410Z"
    }
  ],
  "excluded_remote_only": 0
}
```

`excluded_remote_only` counts the cloud backend's remote-only placeholder rows the filter left out: they have no header on this machine, so their execution directory isn't known. A teardown that wants the sessions a piece of work left runs `koto session list --execution-dir <its directory>` and reads `status` to tell the finished ones from the running ones.

#### session recover

Lists, and optionally restores, the sessions the old-layout migration moved aside because their name was already taken. See [Session storage](#session-storage) for how they get there.

```bash
koto session recover [--apply] [--session <name>]...
```

**Optional flags:**
- `--apply` -- Perform the moves. Without it the command only reports what it would do.
- `--session <name>` -- Limit to quarantined sessions with this name. Repeatable. Without it, every quarantined session is in scope.

**Output (JSON):**

```json
{
  "quarantine_dir": "/home/user/.koto/sessions/.migration-conflicts",
  "applied": true,
  "sessions": [
    {
      "repo_id": "0123456789abcdef",
      "session": "deploy",
      "path": "/home/user/.koto/sessions/.migration-conflicts/0123456789abcdef/deploy",
      "recovered_as": "r0123456789abcdef-deploy",
      "status": "recovered",
      "header_rewritten": true
    }
  ],
  "summary": {"total": 1, "recovered": 1, "skipped": 0, "failed": 0},
  "unmatched": []
}
```

A session comes back as `r<repo-id>-<name>`, where `<repo-id>` identifies the repository its old-layout directory belonged to. The leading `r` is there because a session id has to start with a letter. The repo-id leads rather than trails because a session's parent is the dotted prefix of its own name: recovering `deploy` and `deploy.stage-2` as `r<id>-deploy` and `r<id>-deploy.stage-2` keeps the child pointing at its own parent, which a trailing suffix would not.

Recovery moves; it never deletes and never writes over an existing session. If something already holds the name it would use, it takes `r<repo-id>-<name>-2` instead. In a report-only run `recovered_as` is the name recovery would prefer; the `--apply` run reports the name it actually used. A quarantined directory with no state file is not a session, so it is reported as `skipped` and left where it is. A session whose state file header will not parse is still moved back into the flat namespace -- it is no less readable there than in quarantine -- and its row carries `"header_rewritten": false` to say the `workflow` field inside still names the old id.

Names passed to `--session` that match nothing in the quarantine come back under `unmatched` rather than failing the run, so the command is safe to re-run. It exits non-zero only when a move was attempted and failed; those sessions stay in quarantine and the command can be run again.

Under a cloud backend, recovery works on the local session store. Recovered sessions are not pushed to the configured bucket by this command, and the report says so in a `note` field.

#### session link

Records that another session dispatched this one.

```bash
koto session link <session> --dispatched-by <name>
```

For example, a run started with no request leg records the coordinator that dispatched it, either afterwards or as it is created:

```bash
koto session link work-on-12 --dispatched-by deliver-a
koto init work-on-13 --template ./work-on.md --dispatched-by deliver-a
```

Writes one dispatched-by link, the context key `koto/links/dispatched-by/<name>`, into `<session>`'s own context and nowhere else. The dispatcher isn't written and need not exist. The command is meant for the session's own driver, when a dispatch had no request leg to record it; `koto init --dispatched-by` does the same as the session is created, and a leg-bound session gets its link from the request (see [init](#entry-flags) and [request attach](#request-attach)).

The link's value is one JSON object:

```json
{"v":1,"session":"deliver-a","session_id":"3f2c...","workspace":"/home/me/repo","at":"2026-10-22T18:01:02.000Z"}
```

`session_id` and `workspace` are hints taken from the dispatcher's header when it is in the local store, and `null` when it isn't. The key is written with writer `koto`, and a `context_added` event naming that writer goes into the session's log.

Prints `{"name", "kind", "peer", "written"}`. Recording a link the session already holds changes nothing, prints `"written": false` and exits 0. A dispatcher name that fails the session-name rule (`~` included), makes a key longer than 255 characters, or is `<session>` itself, exits 2 and writes nothing, as does a session that doesn't exist. A failed write exits 3.

#### session anchor

Opens, observes and closes the anchor koto keeps for one agent session. An agent session's start hook opens it and exports its name as `KOTO_ANCHOR`, so every `koto init` and `koto next` the session runs records a held-by link to it (see [init](#init)). Anchors are sessions whose header has `kind: "anchor"`, made from a one-state template embedded in koto that never finishes; `koto next` refuses them, and `koto workflows`, the dashboard and the native `/workflows` file leave them out.

The three verbs print `key=value` lines on stdout. A refusal (a malformed id, key or name, or a name held by a session that isn't an anchor) prints `{"error", "command"}` and exits 2; any other failure exits 3. A failed `open` prints no `anchor=` line.

```bash
koto session anchor open --harness-session <id> --source <startup|resume|compact|clear|other> \
  [--process <token>] [--dispatched-by <name>] [--observed-key <key>] \
  [--workspace <dir>] [--transcript-path <path>]
koto session anchor observe --anchor <name> (--key=<key> | --inherit)
koto session anchor close --key=<key> --harness-session=<id>
```

**open.** The anchor for harness session `<id>` is `anchor-<id>`. `<id>` must match `^[A-Za-z0-9_-]{1,128}$` and may not end in `-closed` or `-s<n>`. Runs under the anchor's lock and never reads `KOTO_ANCHOR`, so an anchor never holds a held-by link. Every call appends an `anchor_started` event recording the source, the harness id, a SHA-256 prefix of `--process` and of `--transcript-path` (never the token or the path), the workspace (`--workspace`, or the current directory), and the predecessor, reason and dispatcher when there are any.

- A first open creates the anchor (`opened=new`). A repeat continues it (`continued`), opening no second anchor; an idle anchor reads open again once that event lands. An anchor that was removed is created again under its name (`reopened`); koto knows it existed when its close session or a successor is still there, or when the start is a `compact`, or a `resume` whose transcript path's stem is the harness id.
- An anchor that reads closed by an observed close stays closed: the open creates the next free successor, `anchor-<id>-s2`, `-s3` and so on, holding a dispatched-by link to the one before it (`successor`). The family's current anchor is the highest suffix contiguous from `-s2` whose link names the one before. A migrated anchor (one whose log records an import) gets a successor the same way.
- `--source clear` (a `/clear` gives the agent session a new id) looks for the anchor the same process ran before: among `anchor-*` sessions whose log changed since the start time in the token `<host>:<pid>:<epoch-start>`, newest first, the first whose `anchor_started` carries the token's digest. On a match the new anchor holds a dispatched-by link to it and copies its observed marker, and the output adds `previous=<name>` and `inherit_until=<rfc3339>`, three minutes on. No match opens an unlinked anchor.
- `--source resume` with no anchor of its own looks for a predecessor through hints only: an anchor whose `anchor_started` recorded the same transcript path digest, or else the current anchor of the family whose harness id is the transcript path's stem. On a match the new anchor holds a dispatched-by link to it; it never copies the predecessor's marker. No match opens a fresh anchor. The hint never replaces the name match.
- `--dispatched-by` is written as a dispatched-by link only when the open creates an anchor; on a continue it goes into the event alone. `--observed-key` is written as the observed marker, `koto/anchor/observed`, only when the anchor has none.

Prints `anchor=<name>`, `opened=new|continued|reopened|successor` and `observed=yes|no`, plus `previous=` and `inherit_until=` after a `clear` that found its predecessor.

**observe.** Writes an observed marker into the named anchor and nowhere else: `--key` writes the key given, `--inherit` copies the marker of the predecessor the anchor's own `anchor_started` recorded (nothing when that one has none yet). The anchor must have `kind: "anchor"`; an anchor that already holds a marker keeps it, so a repeat changes nothing. A key that doesn't match `^[A-Za-z0-9_-]{1,128}$` is refused. Prints `anchor=<name>` and `observed=yes|no`.

**close.** Records as closed, at the current time, the anchor `anchor-<id>` whose `anchor_started` recorded that harness id, and the anchors that succeeded it after a `/clear` or a resume under a new id, each only when its observed marker equals the key. A child anchor that records the session as its dispatcher, and a successor opened because its predecessor already read closed, are never matched. Each record is a separate close session, `<anchor>-closed`, with `kind: "anchor-close"`, a terminal initial state, and the context keys `koto/links/closes/<anchor>` and `koto/anchor-close` (`{"v":1,"anchor","anchor_session_id","kind":"observed","key","at"}`). The anchor itself is never written. Prints `closed=<n>`, the number of anchors this call recorded; run again, or with a key or harness id that matches no anchor, it prints `closed=0`, changes nothing and exits 0. There is no caller-supplied time.

A close session counts only when its header has `kind: "anchor-close"`, its name is `<anchor>-closed`, the manifest lists its record, the record's `anchor_session_id` is the anchor's header id and its key is the anchor's marker. So a close left for an earlier anchor of the same name doesn't close a reopened one; the next close replaces it.

The close session is written whole under a reserved `.koto-staging-` entry in the sessions directory and renamed into place, so no reader sees a partial one; replacing a stale one is a single exchange of the two directories. Listing, prune and every other walk of the store ignore staging entries. A later close sweeps an entry once it is older than 10 minutes, and never a fresher one, which may be a close in progress; an entry dated in the future is reset to now so it is swept in time. A staged entry that is gone at rename time fails the run (exit 3) and leaves no close session; running the close again completes it. Two closes at once both exit 0 and leave one valid close session.

The guarded form a workspace runs when an agent session is reaped:

```bash
[ -n "$NIWA_REAP_KEY" ] || exit 0; command -v koto >/dev/null 2>&1 || exit 0
koto session anchor close --help >/dev/null 2>&1 || exit 0
exec koto session anchor close --key="$NIWA_REAP_KEY" --harness-session="$NIWA_SESSION_ID"
```

Each guard exits 0 quietly: with no reap key, with no koto on the `PATH`, and with a koto older than the anchor verbs, whose `koto session anchor close --help` fails. So the same command is safe on a host that hasn't upgraded. In niwa's workspace configuration, declare `KOTO_DISPATCHER_ANCHOR` and nothing else in the dispatch pass-through list, so a dispatched worker's start hook links its anchor to the dispatcher's, and `KOTO_ANCHOR` in the list of variables removed from the worker's environment, so a worker never acts under its dispatcher's anchor.

An agent session, a run it holds, and the close its reap would run, by hand:

```bash
koto session anchor open --harness-session demo-1 --source startup --observed-key demo-key
# anchor=anchor-demo-1
# opened=new
# observed=yes
KOTO_ANCHOR=anchor-demo-1 koto init demo-run --template ./demo.md
koto session hierarchy demo-run      # kept: anchor-demo-1 is alive
koto session anchor close --key=demo-key --harness-session=demo-1
# closed=1
```

**The koto-skills start hook.** With the koto-skills plugin, Claude Code runs `hooks/session-start.sh` at every start, resume, compaction and `/clear`. It opens `anchor-<session id>` with the agent process's token, the transcript path and the working directory, passing the anchor it inherited (`KOTO_DISPATCHER_ANCHOR`, else `KOTO_ANCHOR`) as `--dispatched-by`, and niwa's `NIWA_REAP_KEY` as `--observed-key` only when no `KOTO_ANCHOR` was inherited, so a process started from another agent session's shell never takes that session's key. It then replaces its own block, between `# >>> koto-skills` and `# <<< koto-skills`, in the hook's `CLAUDE_ENV_FILE`, which Claude Code sources before every shell command: `export KOTO_ANCHOR='<anchor>'` and `export KOTO_DISPATCHER_ANCHOR='<anchor>'`, both naming the session's own anchor, then `unset NIWA_REAP_KEY` and `unset KOTO_AUTHORITY`, so no shell of the session, and nothing it launches, carries an authority handed to the start (the open records that authority in the anchor). When the open fails, the block unsets both variables instead, so the shells never keep a dispatcher's anchor. When `CLAUDE_ENV_FILE` is unset, a symlink, or in another session's directory, nothing is written and the hook returns the `KOTO_ANCHOR` export line as `additionalContext`. On a start with no key (and a resume that opened a successor), a detached watcher reads, and never sources, the session's other env files for niwa's `export NIWA_REAP_KEY='<value>'` line for three minutes and runs `observe --key` with it; after a `/clear`, one runs `observe --inherit` until `inherit_until`. The hook always exits 0, and does nothing without koto or `jq`; with a koto that has no anchor verbs, or one that takes longer than 5 seconds and is cut off, it unsets both variables. Every koto call it makes runs with the `KOTO_SESSION_*` variables removed, so the anchor lives in the store koto's config files name: a user who chooses the cloud backend through those variables alone, and not in a config file, gets an anchor their shells' koto commands don't see, which keeps nothing, as before the hook. Set the backend in a config file instead. Don't run the anchor verbs yourself in such a session; `koto session hierarchy` says why a session is kept.

#### session hierarchy

Shows the hierarchy a session belongs to, and whether koto keeps it.

```bash
koto session hierarchy <name> [--json]
```

A hierarchy is every session joined to `<name>` through a link (`koto/links/dispatched-by/`, `held-by/` or `closes/`), koto lineage (a header's `parent_workflow`; a value of the form `<p>~<digits>` that names no session counts as lineage to `<p>`) or a well-formed `chain/parent` context key. A link whose `session_id` hint names a session other than the one its name resolves to (a predecessor moved aside under a new name) joins that one too. A name with no session behind it is listed as `absent` and joins nothing. A session another session's import copied into this store is `migrated`: links naming it resolve to the copy, and it keeps nothing itself. A session counts as migrated only when the copy's `koto/imported-from` names it with its session id, the copy's log holds the matching `session_imported` event, and its own last event is no later than that one.

The hierarchy is kept while any member is alive:

- A koto session is alive while it runs, and while its log can't be read.
- An anchor with no observed marker (one never observed, or an imported one) closes once `retention.idle_period` has passed since its last activity. Activity is what its own agent session writes to it: its `anchor_started` events, the `anchor_activity` events `init` and `next` record, the read records it logs of other sessions, and, for an anchor copied in by `koto session import`, the `session_imported` event the import appended. A read of the anchor or of a session it holds from anywhere else, a child's completion record, or a `koto context add` from a bare shell never counts.
- An observed anchor closes only through a valid observed close (`koto session anchor close`); nothing else closes it. Past `retention.stale_after` since its last activity it is reported `stale`, with the command that closes it by hand, `koto session anchor close --key='<marker>' --harness-session='<id>'`, its own values filled in. The command is printed only when both values match `^[A-Za-z0-9_-]{1,128}$`; otherwise the anchor is listed as stale with a malformed marker and no command. A stale report closes nothing.
- An anchor's close session is never alive; it stops at its record's time.

Once nothing is alive, the release instant is the latest stop among the members: the transition into a terminal state, a trailing cancel, an observed close, or an idle close. The hierarchy is kept until `retention.keep` has passed since then, and released after.

Everything fails toward keeping. A malformed link, close record or `chain/parent`, a log that can't be read, a symlink or anything other than a regular file under a session's `ctx/koto/`, a link body over 4 KiB, or a timestamp more than five minutes in the future keeps its hierarchy, and is listed under `unreadable` with the session it belongs to. A header, manifest or links directory that can't be read keeps every hierarchy in the store, since that session could name any of them. Instants in the future are clamped to now and the members they came from are listed under `clamped`.

The text form says kept or released and why, then lists the keepers (a stale anchor with its close command), the release instant and when the retention passes, anything unreadable, each member with its kind, liveness, stop and the edges that joined it, and the three settings with their origins. `--json` prints:

```json
{
  "session": "worker",
  "kept": true,
  "keepers": [{"name": "anchor-1f3c", "kind": "anchor", "observed": true,
               "stale": true, "close_command": "koto session anchor close --key='K' --harness-session='1f3c'"}],
  "retention": null,
  "unreadable": [],
  "members": [{"name": "worker", "kind": "koto_session", "alive": false,
               "stopped_at": "2026-10-22T18:01:02.000Z", "close_kind": "terminal",
               "absent": false, "migrated": false, "observed": false, "imported": false,
               "stale": false, "clamped": false,
               "joined_by": [{"holder": "worker", "via": "held-by link", "named": "anchor-1f3c", "target": "anchor-1f3c"}]}],
  "clamped": [],
  "now": "2026-10-22T19:00:00.000Z",
  "settings": {"retention.keep": {"value": "7d", "origin": "default"}, "...": {}}
}
```

When nothing is alive, `keepers` is empty and `retention` is `{"release": {"at", "member", "close_kind"}, "passes_at"}`, with `close_kind` one of `terminal`, `cancelled`, `observed` or `idle`; `kept` stays `true` until `passes_at`. A stale keeper whose marker or harness session id fails the pattern carries `"malformed_marker": true` and no `close_command`. Each anchor member also carries `last_activity`, the instant its idle close and stale report count from (`null` when it can't be read); the `hierarchy` object a prune entry carries leaves it out.

The command reads the local sessions directory directly and writes nothing; under the cloud backend it asks the bucket only about the pins the scan found, and lists and reads no other session through it. A malformed retention setting, a malformed name, or a name with no session exits 2; a sessions directory that can't be listed exits 3. koto assumes `~/.koto` is private to its user: anyone who can write the store can forge the records this command reads.

#### session cleanup

Removes a session directory and all its contents. Idempotent -- succeeds even if the session doesn't exist.

```bash
koto session cleanup <name>
```

When it removes a session, it also deletes a finished record a crash left under the name; on a name koto already removed itself it removes nothing, and the record stays the `koto status` answer. This is the manual equivalent of the removal `koto next` performs at a terminal state, and the way to remove a session koto kept: one at a failure terminal, or one ticked with `--no-cleanup`. It removes only the named session; a kept parent's kept children are left for `koto workflows --orphaned` and `koto workspace prune`.

An explicit removal still removes when the retention rule would keep the session, and says what it broke. Just before removing, it judges the session's hierarchy (see [session hierarchy](#session-hierarchy)) and lists every other live member under `live_members`, each with its `kind` (`koto_session` or `anchor`) and, for an anchor, whether it is `observed`. The removed session itself is never named, and a session nothing alive keeps gets an empty list, including one kept only because its retention hasn't passed. A non-empty list also prints a warning on stderr. When the sessions directory can't be read, the session is still removed, the list is empty, and `live_members_error` says why. A malformed retention setting falls back to the defaults here, with a warning, so it never blocks a removal.

```json
{"name": "worker", "cleaned_up": true, "children": [],
 "live_members": [{"name": "coord", "kind": "koto_session", "observed": false}]}
```

`koto cancel --cleanup <name>` does the same after appending the cancel: its output gains `live_members` (and `live_members_error` when the judgment failed), naming every other live member of the cancelled session's hierarchy, such as a dispatcher that is still running. A `koto cancel` without `--cleanup` removes nothing and carries neither field.

A pinned session is refused: both commands exit 2 with code `pinned` (or `pin_unreadable`), name the pin's reason, setter and time, and change nothing, so `koto cancel --cleanup` doesn't cancel it either. Unpin it first with `koto session unpin <name>`. A migrated source is the exception: `koto session import` carried its pin to the moved copy, so the source's pin keeps nothing and cleanup removes it, while `koto session unpin` refuses it. Under the cloud backend a bucket that can't be checked for a pin refuses with exit 3, code `bucket_unchecked`.

#### session pin

Pins a session so no removal by rule takes it, until it is unpinned. The [session retention guide](session-retention.md) covers what a pin keeps and which releases honour it.

```bash
koto session pin <name> --reason <text>
koto session unpin <name> [--reason <text>]
koto session list --pinned [--json]
```

`koto session pin` prints `{"name", "pin": {"reason", "set_by", "set_at"}, "store"}`, with `store` `"local"` or `{"cloud": "<bucket>"}`, and exits 0; a session already pinned keeps its pin and prints it. It refuses with exit 2, writing nothing, a reason that is blank, longer than 500 characters or holds a control character (`invalid_reason`), a name with no session (`no_session`), an anchor, a close record or a migrated source (`not_pinnable`), and a session whose pin can't be read (`pin_unreadable`). Under the cloud backend the pin is written to the bucket as its own object and nothing else; a write the bucket refuses exits 3 (`pin_not_pushed`) and leaves no pin anywhere, and a bucket that can't be listed exits 3 (`bucket_unchecked`).

`koto session unpin` prints `{"name", "released"}`: the release record (`released_by`, `authority`, `authority_at`, `authority_source`, `reason`, `released_at`, and the released pin's `reason`, `set_by` and `set_at`), or `null` when the session held no pin. A call with no `KOTO_ANCHOR` may unpin. Under an anchor it needs the `release-pin` authority that anchor's agent session was handed at its start, and is refused otherwise with exit 2, `authority_required`, naming the authority and the anchor. Under the cloud backend the release record and the pin's removal are the bucket's objects; either one failing exits 3 with the pin still in force.

`koto session list --pinned` lists every pinned session, sorted by name, with its pin or `pin_unreadable`, whether it is `running` or `finished`, and `source`. Under the cloud backend it also lists sessions pinned in the bucket, with `source: bucket` and `state` `unknown` when this host holds no copy; when the bucket can't be listed it prints this host's pins as `{"pinned": [...], "bucket_unchecked": {"error"}}` and exits 3.

#### session prune

Removes finished sessions as a set, after showing the set first.

```bash
koto session prune [--execution-dir <dir>] [--older-than <age>] [--all]
koto session prune --apply --plan <token>
koto session prune --apply (--execution-dir <dir> | --older-than <age> | --all)...
koto session prune --migrated-from <workspace> [--source-retired] [--apply [--plan <token>]]
```

Without `--apply` nothing is removed. The command prints the finished sessions and finished records it would remove, every session it keeps with the rule that keeps it, the stale anchors it found, and a plan token, and stores the would-remove list as `.prune-plans/<token>.json` under the sessions directory (a private file in a private directory; a preview removes stored plans older than a day).

**Optional flags:**
- `--execution-dir <dir>` -- Only sessions whose execution directory is `<dir>` or lies under it, matched the way [session list](#session-list) matches it.
- `--older-than <age>` -- Only sessions that stopped at least `<age>` ago: a whole number and a unit, `s`, `m`, `h` or `d`, such as `30d`. The age is measured from the stop instant the retention engine reports for the session (the transition into its terminal state, its trailing cancel, or an anchor's close), not from its last event, which a later read or context write moves. A session that hasn't stopped is left out.
- `--all` -- No filter: every session is judged. A preview without filters already covers them all; `--all` is what an `--apply` without `--plan` names when no other filter is given.
- `--apply` -- Remove instead of previewing.
- `--plan <token>` -- With `--apply`, remove only sessions the preview that printed `<token>` listed.

Every session is judged by the retention rule over one scan of the store, with one set of [retention settings](#config-keys-reference) and one `now`: the same judgment [`koto session hierarchy`](#session-hierarchy) prints. The filters narrow which sessions are candidates, never which sessions count as members of a hierarchy: a finished session dispatched by a session that is alive in another execution directory is kept, and named, whatever the filters select. A candidate is kept, and listed under `kept` with the first rule that applies, when:

| Rule | Keeps a session that |
|------|----------------------|
| `p` | holds a pin, readable or not (see [session pin](#session-pin)). The entry carries `kept_by_pin` and `pin` or `pin_unreadable`, and a `hierarchy` object when the hierarchy is kept as well. A pin keeps its own session only. Under the cloud backend a preview checks the bucket for each session it would remove, and the apply checks again just before each delete: a pin object there keeps the session (`source: bucket`), and a bucket that can't be checked keeps it with `bucket_unchecked` |
| `a` | is alive: a koto session that runs, or can't be read (its header, log or template is unreadable, or its directory is a symlink), or an anchor that is open |
| `h` | belongs to a hierarchy the retention rule keeps: another member is alive, `retention.keep` hasn't passed since the hierarchy's release instant, or something in it can't be read (a malformed link, close record or `chain/parent`, an unreadable log, a timestamp more than five minutes in the future, or any session's header, which could name any other) |
| `d` | has a `chain/parent` context key that isn't exactly one session name |
| `e` | is bound to a request leg that is still waiting, or whose leg pointer or request can't be read |
| `f` | is a coordinator's session (its name starts with `coordinate-`) |
| `g` | under the cloud backend, has a remote copy that may be newer: a pushed log longer than the local one, a `version.json` ahead of or diverged from the local one, or a remote copy that can't be checked |

Rule `h` replaced the earlier rules `b` and `c`, which looked one reference away: it keeps the whole hierarchy, joined through links, `parent_workflow` and `chain/parent`. Its `detail` is the rendered verdict: the live members keeping it, each with its kind and whether an anchor is observed; or the release instant, the member and close kind it came from, and when the retention passes; or what can't be read. The entry also carries a `hierarchy` object, the JSON `koto session hierarchy <name> --json` prints for the same judgment. Rule `a` carries it too. Since the engine taints a hierarchy whose `chain/parent` is malformed, rule `h` usually keeps such a session before `d` is reached. A close session whose anchor stays in the store (kept, or left out by a filter) is kept by `h`: a close session goes only with its anchor, which would otherwise read open again. A request that no longer exists releases a session bound to it. Everything the rules read is on this host (the sessions directory, and the request store for rule e) except rule g. Without a remote `version.json`, rule g compares only the pushed log's length, so a remote log that diverged without growing longer than the local one isn't caught.

**Stale anchors.** An observed anchor past `retention.stale_after` since its last activity keeps what it holds, and the prune closes nothing. Each one any judgment names is listed under `stale_anchors`, with the `close_command` that closes it by hand, `koto session anchor close --key='<marker>' --harness-session='<id>'`, its real values filled in, the same command `koto session hierarchy` prints; the anchor's own rule `a` detail carries it too. Run it once the agent session is gone, and a prune after the retention removes what the anchor held. An anchor whose marker or harness session id doesn't match `^[A-Za-z0-9_-]{1,128}$` is listed with `malformed_marker` and no command.

**Finished records.** The would-remove list also holds the finished records koto wrote for sessions it removed itself (see [status](#status)), as `"kind": "record"` entries beside the `"kind": "session"` ones. A record carries the removed session's `session_id`, `state`, `status` (`terminal` or `cancelled`), `failure`, `finished_at` and `execution_dir`, and `--execution-dir` and `--older-than` match it by those last two. No rule keeps a record: koto writes one only once the session's parent already had its result, so nothing still needs it. Removing a record makes `koto status` answer `not found` for the name. Applying reads each record again and skips one rewritten since the preview (another session of the name finished and was removed in between).

**Preview output (JSON):**

```json
{
  "mode": "preview",
  "plan": "3f6c1e0a9b2d4c58e7f1a0b3c5d7e9f1",
  "remove": [
    {"kind": "session", "name": "review-12", "session_id": "<session-id>",
     "status": "terminal", "state": "done", "failure": false,
     "finished_at": "2026-10-11T23:50:02.410Z", "execution_dir": "/work/project",
     "stopped_at": "2026-10-11T23:50:01.902Z"}
  ],
  "kept": [
    {"name": "plan-7", "rule": "h",
     "detail": "kept: alive in its hierarchy: plan-7.task-2 (koto_session)",
     "hierarchy": {"session": "plan-7", "kept": true, "keepers": [...], "...": "..."}}
  ],
  "stale_anchors": []
}
```

A removable anchor or close session also carries `session_kind` (`anchor` or `anchor_close`).

The token is the first 32 hex digits of a SHA-256 over the would-remove list's kinds, names and session ids, so it changes when a session joins or leaves the list, or a removed name is taken by a new session, and not when anything outside the list changes.

**Applying.** Every apply takes one fresh scan of the store and judges it again. `--apply --plan <token>` does so with the filters the preview ran with (the stored plan records them, and passing a filter alongside `--plan` is an error). It refuses when the fresh judgment would remove anything the plan didn't name: it removes nothing, exits 4 with `prune_plan_stale`, and lists under `added` and `dropped` how the set changed. A fresh judgment that only keeps more than the plan isn't a refusal: what the plan named and the rules now keep (a session that gained a live member since the preview, say) goes to `skipped` with its reason, and the rest is removed, exit 0. A token with no stored plan (it expired, was applied, or was previewed on another host) also exits 4, with `prune_plan_unknown`. `--apply` without `--plan` removes the set it works out now, in one step with no preview anyone read, and needs `--execution-dir`, `--older-than` or `--all` so that a bare `--apply` can't empty the store by mistake. It is the form for scheduled runs; the rules apply to it all the same, but only `--plan` guarantees that nothing goes that someone didn't see.

Sessions are removed one hierarchy at a time. Every member to be removed is locked first, then the hierarchy is judged again as a unit, reading each member afresh; a hierarchy the rule now keeps loses nothing, and its members go to `skipped` with the reason. Then each member is checked under its lock: one replaced by a new session under the same name, a koto session that is no longer finished (rewound, say), and one rules d to g now keep are skipped, as is one another koto process holds. Children go before their parents and anchors before their close sessions, and a close session whose anchor is still there (skipped, say) is skipped too, so no apply removes a close session while its anchor remains. A session created after the apply's own scan that links to a member isn't seen. Under the cloud backend a session's remote copy is deleted before its local directory, apart from a migration marker, so a failure leaves the local copy for a later prune to retry. The remote delete isn't conditional: a push from another host that lands between rule g's check and the delete is lost with it; that window can't close without conditional deletes. The prune never touches the run journal, and writes nothing into any session it doesn't remove.

**Apply output (JSON):** `{"mode": "apply", "plan": ..., "removed": [...], "skipped": [{"name", "reason"}], "kept": [...], "failed": [{"name", "error"}], "stale_anchors": [...]}`.

| Exit code | Meaning |
|-----------|---------|
| 0 | Previewed, or applied with nothing failed (skipped sessions included) |
| 2 | `invalid_arguments`: a malformed token or age, `--plan` without `--apply` or with a filter, or `--apply` with nothing naming the set; or `invalid_retention_setting`: a retention setting is malformed, and the message names it. Nothing was judged or removed |
| 3 | A session couldn't be removed; it is listed under `failed`. Or `prune_scan_failed`: the sessions directory can't be listed, so nothing was judged or removed, in preview and in apply |
| 4 | `prune_plan_stale` or `prune_plan_unknown`; nothing was removed |

**Migration markers.** `--migrated-from <workspace>` judges the migration markers `koto session import` left under that workspace's bucket prefix instead of local sessions; it needs the cloud backend (under the local backend it exits 2) and takes none of `--execution-dir`, `--older-than` or `--all`. `<workspace>` is the old workspace's absolute path as it was on its own host. Each marker is listed with `name`, `target` (`session`, `session_id`, `workspace`, `prefix`), `migrated_at`, `age_days`, `target_status` (`present`, `replaced`, `migrated`, `absent`, `unverified` or `unknown`), `removable`, `reason`, `detail` and, when the old copy could be listed and its log read, `objects` (how many objects its removal deletes, the marker included), under `remove` or `kept`. A target is `present` only when its state file names the marker's session id and its whole log reads with no error and begins with every event of the old copy's log, unchanged; a truncated, corrupt, shorter or differing log is `unverified`. A marker is removable only when its target is `present` and it is at least `session.cloud.marker_retain_days` days old (default 30; 0 makes every such marker removable at once), or `--source-retired` was given; anything else is kept, and a marker that can't be read is kept as `unreadable`. The token covers each removable marker's name and `migrated_at`, and the stored plan (`.prune-plans/markers-<prefix>-<token>.json`) records the workspace and `--source-retired`, so `--apply --plan <token>` takes `--migrated-from` with the same workspace and no `--source-retired`. Apply reads and judges each marker again, then deletes everything under the old session's prefix, the marker last (nothing locks the bucket between the two); a delete that fails leaves the marker, lists the entry under `failed` and exits 3, and a re-run finishes it. Nothing local is removed, so remove the old host's local copy (`koto session cleanup <name>` there) or retire the host first: with the marker gone, that copy would sync back as a live session. The [cloud sync guide](cloud-sync-setup.md#removing-the-old-copy-and-its-marker) has the full rule.

### context

The `context` subcommand group manages workflow content. Agents use these commands to submit artifacts, retrieve them, and check whether specific content has been produced. All content is stored opaquely by koto and keyed by session name and content key.

#### What a content key may hold

A key is `/`-separated. Each component starts with a letter or digit and continues in letters, digits, `.`, `_` and `-`; `.` and `..` components are refused, as are leading, trailing and doubled slashes, and the whole key is capped at 255 characters.

That is narrower than what a variable value may hold, and the gap is exactly four characters: a value may also carry a space, a `:`, an `@` and a `+`. The two are different on purpose and are not converging. A value is content -- it reaches a directive, a command argument, a regex -- and those four are there so a value can be a calendar title, a filter like `from:user@example.com`, or a path through a directory named like `workspace+instance`. A key is an address: it becomes a directory name under the session, an entry in the store's manifest, and an argument in the `koto context add` and `koto context get` commands templates run, where a space would split it in two.

The two meet when a context gate's `key` resolves a `{{KEY}}` reference. `key: "{{TITLE}}-note"` with a `TITLE` of `Weekly Planning` asks for `Weekly Planning-note`, which is not a usable key. koto says which character it refused, at the gate and at `koto context exists` alike, in the same words -- but the fix is to scope the key on a slug-shaped variable rather than on the prose one.

#### The reserved `koto/` area

Keys under `koto/` belong to koto, which writes its own records about a session there, such as the dispatched-by links [`koto session link`](#session-link) records. `koto context add` and `koto context remove` refuse any key whose first component is `koto`, compared case-insensitively (`KOTO/...` too), with exit 2 and an error naming the prefix, and change nothing. The refusal sits in the context store itself, so no other path for agent writes gets around it. `koto context get`, `exists` and `list` read the area like any other key.

`koto session import` treats the bucket as untrusted for this area. It leaves behind the observed marker (`koto/anchor/observed`), any close record (`koto/anchor-close`), every `koto/links/closes/*` link and any `koto/imported-from`, and refuses an anchor's close session as a source (`import_source_is_anchor_close`, exit 2). It validates every link it keeps, dropping a malformed one with a warning on stderr. Each `koto/links/held-by/<anchor>` is kept when the name resolves here to an open, unmigrated anchor (the anchor's recorded workspace is only a hint), rewritten to `koto/links/held-by/<importer's anchor>` with `"was"` holding the old name when the import runs with `KOTO_ANCHOR` naming an anchor here, and dropped otherwise. Every copy gets `koto/imported-from` (`{"from_session", "from_session_id"}`). The [cloud sync guide](cloud-sync-setup.md#what-the-import-settles-in-kotos-own-keys) has the details.

#### context add

Submits content to the store for a given session and key. Reads from stdin by default.

```bash
echo "plan contents" | koto context add <name> <key>
```

Or read from a file:

```bash
koto context add <name> <key> --from-file <path>
```

**Positional arguments:**
- `<name>` -- Workflow/session name.
- `<key>` -- Content key (e.g., `plan.md`, `spirit-greeting.txt`).

**Optional flags:**
- `--from-file` -- Read content from the specified file instead of stdin.

Exits 2 if the session has no state log (it was never initialized, it's a batch child that is still `pending` or `blocked`, or it has already finished and been cleaned up), and stores nothing. Exits 2 for a key in the reserved `koto/` area, before reading the input. Exits non-zero if the input can't be read. Overwrites any existing content for the same key.

#### context get

Retrieves content from the store. Writes to stdout by default.

```bash
koto context get <name> <key>
```

Or write to a file:

```bash
koto context get <name> <key> --to-file <path>
```

**Positional arguments:**
- `<name>` -- Workflow/session name.
- `<key>` -- Content key.

**Optional flags:**
- `--to-file` -- Write content to the specified file instead of stdout.

Exits non-zero if the session or key doesn't exist.

**Where the read is logged.** `koto context get` and `koto context exists` each log the read as a `context_read` event (key name and value hash, never the value). Where it goes depends on `KOTO_ANCHOR`, the running agent session's anchor (see [session anchor](#session-anchor)):

| `KOTO_ANCHOR` | The read is logged in |
|---|---|
| any, when the command runs inside `<name>`'s own tick (a gate script or default action reading its own session) | `<name>`'s own log |
| names an existing anchor that is `<name>`, or that holds `<name>` (`<name>` carries `koto/links/held-by/<anchor>`) | `<name>`'s own log, as before |
| names an existing anchor otherwise (a read from another agent session) | that anchor's log, with a `session` field naming `<name>`; `<name>`'s files stay byte-identical |
| unset, or names no anchor in the local store | this host's read spool, `context-reads.jsonl` beside `<name>`'s local copy; `<name>`'s log and other files stay byte-identical |

The table holds under both backends. Under the cloud backend the read may also refresh this host's sync cache beside `<name>`'s local copy (`cloud-sync.json`, `cloud-manifest.json`), which records what the host last pulled and is never pushed; "byte-identical" covers `<name>`'s log, its context and every object in the bucket. The last row used to write into the session read, which made a read from a bare shell a cross-session writer (koto#412); now it writes only the spool, which never leaves the host (the cloud backend never uploads it), counts as no session's activity, and is read back only by the run journal when it sums the visit the read fell in. A read logged in the reader's anchor counts as that anchor's activity for its idle close, never as activity of `<name>` or of an anchor holding it. An append that fails warns on stderr and never fails the read. `koto context list`, `koto status`, `koto session list`, `koto session hierarchy` and a prune preview log no read anywhere. The [session-feed contract](../reference/session-feed.md#reads-from-another-agent-session) has the event's fields and its consumers.

#### context exists

Checks whether a content key exists for a session. Produces no output.

```bash
koto context exists <name> <key>
```

**Positional arguments:**
- `<name>` -- Workflow/session name.
- `<key>` -- Content key.

Exits 0 if the key exists and 1 if it doesn't, printing nothing either way. This is the CLI equivalent of the `context-exists` gate type in templates. The read is logged exactly as [context get](#context-get)'s is (see the table there): in `<name>`'s log, in the reader's anchor when `KOTO_ANCHOR` names an anchor that doesn't hold `<name>`, or, when `KOTO_ANCHOR` is unset or names no anchor, in this host's read spool, leaving `<name>` byte-identical.

Exits 2 when the argument is not a usable context key -- see "What a content key may hold" above -- and prints a JSON error naming the offending character and the component it sits in:

```json
{"error":"context key \"Weekly Planning-note\" is not usable: context key contains invalid character ' ' in component 'Weekly Planning-note'; allowed: letters, digits, '.', '_', '-'\n  remedy: ...","command":"context exists"}
```

Exit 2 is non-zero, so `if koto context exists ...; then` behaves as it always has; what changes is that the reason is printed rather than absent.

Note that a `1` still means "not present" and nothing more precise: the check
cannot distinguish a key that was never written from a store it could not read.
Callers that need to act on that difference should not infer it from this exit
code. The one case that used to hide inside exit 1 and now does not is the
unusable key.

#### context remove

Removes a content key and its stored content from a session.

```bash
koto context remove <name> <key>
```

**Positional arguments:**
- `<name>` -- Workflow/session name.
- `<key>` -- Content key.

**Idempotent:** removing a key that is not there succeeds. That shape is
deliberate — the alternative would push every caller to probe with `context
exists` first, and per the note above that probe still cannot tell "absent" from
"unreadable", so the guard would silently skip keys it should have removed. The
session itself must exist: on a session with no state log, `context remove`
exits 2 and changes nothing. A key in the reserved `koto/` area exits 2 and is
left in place.

The removal appends a `context_removed` event to the session's event log,
mirroring the `context_added` event that `context add` writes. A workflow whose
gate is `context-exists` over the removed key will report it absent on the next
evaluation, so this is the verb that makes such a gate fail on purpose.

#### context list

Lists all content keys for a session as a JSON array.

```bash
koto context list <name>
```

Filter by prefix:

```bash
koto context list <name> --prefix "review/"
```

**Positional arguments:**
- `<name>` -- Workflow/session name.

**Optional flags:**
- `--prefix` -- Only list keys that start with this string.

**Output (JSON):**

```json
["plan.md", "review/feedback.md", "spirit-greeting.txt"]
```

Returns an empty array `[]` when no keys exist (or none match the prefix).

### template

The `template` subcommand group contains authoring tools for template development. These commands aren't needed for running workflows -- they're for people writing and debugging templates.

#### template compile

Compiles a source template to FormatVersion=1 JSON and caches the result. Outputs the compiled JSON file path on success.

```bash
koto template compile <source>
```

**Positional argument:**
- `<source>` -- Path to the YAML template source file.

**Output:** The path to the compiled JSON file.

```
/home/user/.cache/koto/abc123.json
```

Uses SHA256-based caching: if the source hasn't changed, the cached path is returned without recompiling. Exits non-zero with a JSON error on compilation failure.

#### template validate

Validates a compiled template JSON file against the expected schema.

```bash
koto template validate <path>
```

**Positional argument:**
- `<path>` -- Path to the compiled template JSON file.

Exits 0 if the file is valid. Exits non-zero with a JSON error if the schema check fails.

#### template export

Generates a visual representation of a compiled template. Supports two output formats: Mermaid text diagrams and interactive HTML.

```bash
koto template export <source> [--format mermaid|html] [--output <path>] [--check] [--open]
```

**Positional argument:**
- `<source>` -- Path to the template source file (`.md`, compiled on the fly) or pre-compiled JSON (`.json`).

**Flags:**
- `--format` -- Output format: `mermaid` (default) or `html`.
- `--output` -- Write output to a file path. Required for `--format html`. When omitted with `--format mermaid`, output goes to stdout.
- `--check` -- Compare what would be generated against the existing file at `--output` without writing. Exits 0 if fresh, 1 if stale or missing. Requires `--output`.
- `--open` -- Open the generated file in the default browser. Only valid with `--format html`.

**Flag compatibility rules:**

| Combination | Result |
|-------------|--------|
| `--format html` without `--output` | Error (exit 2) |
| `--open` without `--format html` | Error (exit 2) |
| `--open` with `--check` | Error (exit 2) |
| `--check` without `--output` | Error (exit 2) |

**Mermaid format** produces a `stateDiagram-v2` diagram showing states, transitions with condition labels, `[*]` markers for initial and terminal states, and gate annotations. GitHub renders this natively in markdown files.

```bash
# Print Mermaid to stdout
koto template export my-workflow.md

# Write to a sibling file for committing
koto template export my-workflow.md --output my-workflow.mermaid.md

# Check if committed diagram is fresh (for CI)
koto template export my-workflow.md --output my-workflow.mermaid.md --check
```

**HTML format** produces a self-contained interactive diagram using Cytoscape.js with dagre layout. Includes hover tooltips for gates and evidence schemas, click-to-highlight for tracing paths (one hop), pan/zoom, dark mode, and a `[*]` start marker. CDN scripts are loaded with SRI integrity hashes.

```bash
# Generate interactive HTML
koto template export my-workflow.md --format html --output my-workflow.html

# Generate and open in browser for local debugging
koto template export my-workflow.md --format html --output my-workflow.html --open

# Check if deployed HTML is fresh
koto template export my-workflow.md --format html --output docs/my-workflow.html --check
```

Unlike other template subcommands, export errors go to stderr as plain text (not JSON), since it's a developer-facing tool rather than an agent-consumed command.

### config

The `config` subcommand group reads and writes koto's configuration. koto looks for config in two places, merged in this order (later wins):

1. **Project config** -- `.koto/config.toml` in the current repository. Checked into version control and shared with collaborators.
2. **User config** -- `~/.koto/config.toml`. Machine-specific settings and credentials.

Project config uses an allowlist. Only non-secret keys are allowed -- credential keys like `session.cloud.access_key` and `session.cloud.secret_key` are rejected with an error if you try to set them in project config.

#### config get

Prints the resolved value of a config key.

```bash
koto config get <key>
```

**Positional argument:**
- `<key>` -- Dotted config key (e.g., `session.backend`).

Exits 0 and prints the value if set. Exits 1 if the key is unset.

#### config set

Writes a value to project config (default) or user config.

```bash
koto config set <key> <value>
koto config set --user <key> <value>
```

**Positional arguments:**
- `<key>` -- Dotted config key.
- `<value>` -- Value to set.

**Optional flags:**
- `--user` -- Write to `~/.koto/config.toml` instead of `.koto/config.toml`. Without this flag, the value is written to project config, which fails if the key isn't on the project config allowlist.

#### config unset

Removes a key from project config (default) or user config.

```bash
koto config unset <key>
koto config unset --user <key>
```

**Positional argument:**
- `<key>` -- Dotted config key to remove.

**Optional flags:**
- `--user` -- Remove from `~/.koto/config.toml` instead of `.koto/config.toml`.

#### config list

Dumps the fully resolved config as TOML. Credential values are redacted in the output.

```bash
koto config list
koto config list --json
```

**Optional flags:**
- `--json` -- Output as JSON instead of TOML.

#### Config keys reference

| Key | Values | Default | Project config |
|-----|--------|---------|:--------------:|
| `session.backend` | `"local"`, `"cloud"` | `"local"` | yes |
| `session.cloud.endpoint` | S3-compatible endpoint URL | -- | yes |
| `session.cloud.bucket` | Bucket name | `"koto-sessions"` | yes |
| `session.cloud.region` | AWS region | -- | yes |
| `session.cloud.access_key` | Access key ID | -- | no |
| `session.cloud.secret_key` | Secret access key | -- | no |
| `workflows.native` | `true`, `false` | `true` | yes |
| `retention.keep` | Duration (`90s`, `45m`, `12h`, `7d`) | `"7d"` | yes |
| `retention.idle_period` | Duration | `"7d"` | yes |
| `retention.stale_after` | Duration, longer than `retention.keep` | `"30d"` | yes |
| `decider.mode` | `"off"`, `"shadow"`, `"auto"` | `"off"` | yes (can only lower the mode) |
| `decider.api_key` | Decider API key | -- | no |
| `decider.endpoint` | `https` URL of the decision call (plain `http` only for loopback) | `"https://api.typesafe.ai/v1/systemone"` | no |
| `decider.timeout_ms` | Integer, 1 to 10000 | `2000` | no |

Credential keys (`access_key`, `secret_key`) can also be provided through environment variables `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`. Environment variables take precedence over config file values.

The `decider` keys opt a user in to consulting a decider on template fields that declare one. `KOTO_DECIDER`, `KOTO_DECIDER_API_KEY`, and `KOTO_DECIDER_ENDPOINT` override `mode`, `api_key`, and `endpoint`. Unlike other keys, a project `decider.mode` doesn't replace the user's: the lower of the two applies, so a repository can turn the decider down but never on. koto ignores `api_key`, `endpoint`, and `timeout_ms` in project config with a warning. A key is sent only to an endpoint set in the same place (both env or both user config) or to the default, and `koto config get decider.api_key` prints `<set>`, never the key. A key must be printable ASCII: one holding a control character (a newline, say) or a non-ASCII character is ignored with a warning, and `koto config set` refuses it. A config file that won't parse is reported by path, line, and column only; koto never prints its content, since it can hold a key. See [decider-authoring.md](decider-authoring.md) for opting in, what a consultation sends, and how a template value is promoted.

The `retention` keys decide how long a finished hierarchy of sessions is kept (see [session hierarchy](#session-hierarchy)): `retention.keep` is how long after its release instant, `retention.idle_period` how long an anchor with no observed marker stays open after its last activity, and `retention.stale_after` how long after its last activity an observed anchor is reported stale. Each resolves `KOTO_RETENTION_KEEP`, `KOTO_RETENTION_IDLE_PERIOD` or `KOTO_RETENTION_STALE_AFTER` first, then project config, then user config, then the default, and `koto config list` prints where each came from (`# origin: retention.keep = env KOTO_RETENTION_KEEP`, or the `origins` object with `--json`). A value is a whole number followed by `s`, `m`, `h` or `d` and longer than zero, and `retention.stale_after` must be longer than `retention.keep`, so a stale report never comes before a release. `koto config set` refuses a malformed value; one that reaches a config file or the environment anyway is refused, naming the setting and where it came from, by the commands that judge retention. Loading the configuration never fails on these keys, and no tick fails on them: whether a tick keeps its session never depends on them, and the activity throttle of `init` and `next` reads `retention.idle_period` with a fallback to the default. They are outside the `KOTO_SESSION_*` family on purpose: koto passes that family to the commands a tick runs.

```bash
koto config set --user retention.keep 3d        # keep released hierarchies three days
koto config set retention.idle_period 12h       # this project: unobserved anchors close after 12 hours
KOTO_RETENTION_STALE_AFTER=60d koto session prune   # one preview with a longer stale threshold
koto config list | grep retention                # each value with its origin
```

`workflows.native` controls rendering koto sessions in Claude Code's `/workflows` screen (see [native-workflows-verification.md](native-workflows-verification.md)); it is on by default and a participating session self-discovers its target directory from `CLAUDE_CODE_SESSION_ID`. Set it to `false` to opt out. A fully headless run (no Claude Code environment) renders nothing regardless.

#### Example: local-only config (default)

No configuration needed. Sessions are stored at `~/.koto/sessions/` and never leave the machine.

#### Example: cloud sync with user config

```toml
# ~/.koto/config.toml
[session]
backend = "cloud"

[session.cloud]
endpoint = "https://s3.us-east-1.amazonaws.com"
bucket = "my-koto-sessions"
region = "us-east-1"
access_key = "AKIAIOSFODNN7EXAMPLE"
secret_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
```

#### Example: shared project config with per-user credentials

```toml
# .koto/config.toml (committed to repo)
[session]
backend = "cloud"

[session.cloud]
endpoint = "https://my-r2-account.r2.cloudflarestorage.com"
bucket = "team-koto-sessions"
```

Each team member sets credentials in their own user config or environment:

```bash
export AWS_ACCESS_KEY_ID="AKIAIOSFODNN7EXAMPLE"
export AWS_SECRET_ACCESS_KEY="wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
```

### Cloud sync

When `session.backend` is set to `"cloud"`, koto syncs session state to S3-compatible storage. This works with AWS S3, Cloudflare R2, MinIO, and any other S3-compatible provider.

Cloud sync is built into existing commands -- `init`, `next`, and `context add` all sync automatically. There's no separate sync command to run.

Cloud sync is included in the default koto binary. To enable it, configure a backend:

```bash
koto config set session.backend cloud
koto config set session.cloud.endpoint https://<account-id>.r2.cloudflarestorage.com
koto config set session.cloud.bucket my-koto-sessions
```

#### Conflict resolution

Version conflicts are rare but can happen if two machines modify the same session simultaneously. When koto detects a conflict, it pauses and asks you to choose which version to keep:

```bash
koto session resolve <name> --keep local
koto session resolve <name> --keep remote
```

**Positional argument:**
- `<name>` -- Workflow/session name.

**Required flag:**
- `--keep` -- Which version to keep: `local` (discard the remote version) or `remote` (discard local changes and pull the remote version).

**Optional flag:**
- `--children` -- How to reconcile the parent's children during the same call. One of four modes:
  - `auto` (default) — apply the strict-prefix rule to each child's state file. When one side is a byte-exact prefix of the other, the longer side wins; any other divergence surfaces as a conflict that needs its own `koto session resolve <child>`.
  - `skip` — leave child state files untouched.
  - `accept-remote` — overwrite local child state with remote.
  - `accept-local` — overwrite remote child state with local.

After resolving, normal operations resume.

### dashboard

Opens a live TUI showing all sessions for the current repository. Unlike other koto commands, `dashboard` outputs to the terminal rather than JSON, so it's not suitable for agent use.

```bash
koto dashboard [<name>] [--once] [--interval <ms>] [--status <liveness>] [--needs-you] [--all]
```

**Positional argument:**
- `<name>` -- Optional. When provided, filters the display to the named session only.

**Optional flags:**
- `--once` -- Print a snapshot and exit. Outputs tab-separated lines, one per session, then exits 0. Exits 0 even when the session directory is empty.
- `--interval <ms>` -- Override the default 500ms poll interval. Only affects the live TUI mode.
- `--status <liveness>` -- (`--once` only) Filter to a single liveness state. One of: `needs-you-blocked`, `needs-you-failed`, `needs-you-stalled`, `active`, `idle`, `pending`, `done`.
- `--needs-you` -- (`--once` only) Show only sessions in the needs-you band (blocked, failed, or stalled).
- `--all` -- (`--once` only) Include the receded set (done and abandoned sessions), which is excluded by default.

**TUI navigation:**

| Key | Action |
|-----|--------|
| `j` / `k` | Move cursor down / up |
| `Enter` | Open detail panel for selected session |
| `Escape` | Close detail panel |
| `r` | Force refresh |
| `q` | Quit |

The TUI shows all sessions as a tree with each session's current state, elapsed time, and task counts. It polls for changes at 500ms by default.

**`--once` output format:**

Each line is tab-separated with eight fields:

```
<id>\t<current_state>\t<elapsed>\t<status_bucket>\t<intent>\t<template>\t<idle>\t<liveness>
```

The `status_bucket` column (field 4) uses one of five values: `running`, `done`, `failed`, `blocked`, or `unknown`. The `liveness` column (field 8) carries the machine-readable liveness token (e.g. `needs-you-blocked`, `active`, `idle`) used by `--status`. Fields 5-7 (`intent`, `template`, `idle`) are sanitized of tabs and newlines.

**Examples:**

```bash
# Open the live TUI for all sessions
koto dashboard

# Watch a single session
koto dashboard my-workflow

# Snapshot for scripting
koto dashboard --once

# Snapshot for a specific session, faster poll in live mode
koto dashboard --once my-workflow
koto dashboard --interval 200
```

### decider answer

Records your answer to a decider-check criterion the decider couldn't grade. When a decider check gets no verdict for a criterion (its slice was over budget, the extraction failed, the provider errored or answered something unreadable, the per-call cap was spent, or another `koto next` held the lock), `koto next` stops the state with the check `pending` and a warning finding that names the criterion, its question, what pass, fail and `unclear` mean, and this command. See [decider-authoring.md](decider-authoring.md) for how the fallback works.

```bash
koto decider answer <name> --gate <check> --criterion <criterion id> --answer <pass|fail|unclear>
```

**Required flags:**
- `--gate <check>` -- The decider check (gate) the finding names.
- `--criterion <criterion id>` -- The criterion's id.
- `--answer <pass|fail|unclear>` -- Your judgment of your own work. `unclear` is the criterion's escape.

The answer binds to the slice the check was judging when the criterion fell back, and decides that criterion on the next `koto next` of the same slice in the same visit: `pass` and `unclear` let it through, `fail` blocks in veto mode until the work changes or you answer again. If you change the work instead, the next `koto next` asks the decider about the new slice. The latest answer for a slice wins. The command never consults the decider and doesn't wait for the decider lock.

Prints `{"status":"recorded","gate":...,"criterion":...,"answer":...}` and exits 0. Refusals (`invalid_answer`, `unknown_criterion`, `no_fallback`) exit 2 with a typed code; see [error-codes.md](../reference/error-codes.md#decider-answer).

### disputes record

Records that a rule's finding at a gate is wrong. A dispute is recorded, not obeyed: it changes no gate outcome, no route and no state, and the next `koto next` responds exactly as it would have without it. A failing check still blocks; to move past one, fix the work or use `koto overrides record`. Disputes are for the record: a reader of the session's log or of the [run journal](../reference/run-journal.md#check_disputed) can see which rules agents and people disagree with.

```bash
koto disputes record <name> --gate <gate> --rule <rule-id> --reason <text> [--state <state>] [--by agent|person]
```

**Required flags:**
- `--gate <gate>` -- The gate whose finding is wrong, or `__action__` for a state's default action. The session's log must hold a settled evaluation of it in the state, one with an outcome: a polling gate that has only answered pending can't be disputed yet. The dispute is about the most recent one.
- `--rule <rule-id>` -- The finding's `rule_id`, 1 to 128 bytes. koto doesn't require a logged finding for it, since a check's logged findings may have been cut at their cap.
- `--reason <text>` -- Why the finding is wrong, 1 to 1,000 bytes. It goes through koto's known-credential redaction before it's stored, the same pass a check's output gets, and stays in the session's log; the run journal never copies it.

**Optional flags:**
- `--state <state>` -- The state the gate was evaluated in. Defaults to the current state; name an earlier state to dispute a check the session has moved past.
- `--by agent|person` -- Who disputes the finding. Defaults to `agent`.

Appends one `finding_disputed` event, prints `{"status":"recorded"}` and exits 0. A missing session exits 1. Refusals (`gate_not_evaluated`, `invalid_rule`, `invalid_reason`) exit 2 with a typed code and append nothing, as does a `--by` other than `agent` or `person`; see [error-codes.md](../reference/error-codes.md#disputes-record).

A passing check's `warning` findings reach the agent on the `koto next` response that evaluated it, under a top-level `advisories` list (see the koto-user skill's response reference). They are the findings this command is most often for: a warning doesn't block, so the dispute is how an agent says it disagrees.

### decider report

Reads the decider ledger and reports, per question and per value, how often the decider agreed with agents. With `--fixtures` it also runs a golden fixture set against the configured decider and marks each value promotion-eligible or not. The command only reads: it never changes a mode, never writes the ledger or a session log, and compiles the template in memory without touching the compile cache. The promotion workflow it supports is described in [decider-authoring.md](decider-authoring.md#promoting-a-value-to-auto).

```bash
koto decider report [--ledger <path>] [--state <state>] [--json] [--include-custom-endpoints]
koto decider report --fixtures <path> --template <path> --state <state> [--field <field>] [--json] [--include-custom-endpoints]
```

**Optional flags:**
- `--ledger <path>` -- Ledger to read. Defaults to `_decider_ledger.jsonl` in the koto home directory (`~/.koto/_decider_ledger.jsonl`, resolved from `HOME`). A missing ledger is an empty report, not an error.
- `--state <state>` -- Report only questions on this state, in the table and in JSON. With `--fixtures`, it's also the state whose declaration the fixtures exercise.
- `--json` -- Print the report as JSON instead of a table.
- `--include-custom-endpoints` -- Count consultations and fixture runs sent to a user-config or `KOTO_DECIDER_ENDPOINT` endpoint toward promotion eligibility. Without it, only the default endpoint counts, so stub and redirected runs can't reach the bar. Metrics include custom-endpoint consultations either way.
- `--fixtures <path>` -- Run this JSON Lines fixture set and judge promotion eligibility. Needs `--template` and `--state`.
- `--template <path>` -- Template source whose declaration the fixtures exercise. Only valid with `--fixtures`.
- `--field <field>` -- The declared field to exercise when the state declares more than one. Only valid with `--fixtures`.

**What the ledger report shows.** A question is one state, field, and declaration hash; changing a declaration's question, values, descriptions, or inputs starts a new question, while changing a mode or threshold doesn't. For each question the report shows consultations by outcome, paired observations (a consultation and the agent's later answer for the same visit), and per value: paired count, recall, coverage, and disagreements where the decider picked that value at or above threshold and the agent chose another. It also shows directed exits: visits the agent left with `koto next --to` instead of answering, counted per question and, per value, where the decider chose that value at or above threshold. They aren't paired observations, since the agent gave no answer, so they stay out of the paired counts, recall, and the confusion matrix. The report also shows a confusion matrix (rows are the agent's values; columns are the decider's confident value, `below_threshold`, `escape`, and `no_answer`), the disagreeing visits (directed exits included, each with the state it went to, though the per-value disagreement counts cover answers only), fallback and error rates (errors broken down by `error_class`), latency p50 and p95 (nearest rank, leaving out `input_unavailable` consultations), agent stops removed, and directive bytes not delivered. Coverage uses well-formed answers (`applied` and `not_applied`) as its denominator, and the table prints the share over all consultations beside it. A question with at least 30 consultations that had a value in `auto`, whose coverage over those is below 30%, carries a `low_coverage` flag.

The header counts ledger lines, skipped malformed lines, lines of an unknown kind, duplicate consultations, orphaned answers, directed exits and orphaned ones, consultations with no session id (counted but never paired), and consultations from a custom endpoint along with how many of those are excluded from eligibility.

**Fixture file.** One JSON object per line with `inputs` (each declared input label mapped to its text), `expected` (a declared value, the escape, or a JSON `true`/`false` for a boolean field), and an optional `id`. Every line is checked before anything is sent: an unknown key, an undeclared `expected`, a missing or extra input label, or an input over its `max_bytes` stops the run with the line number. Each case goes through the same request building, provider client, and evaluation the runtime uses, and nothing it does is recorded.

**Opt-in and network.** A fixture run needs an opted-in decider: an effective mode of `shadow` or `auto` (a project `.koto/config.toml` can only lower it), an API key, and an endpoint from the key's own layer or the default. Without that, the command says fixture runs need an opted-in decider and sends nothing. A refused connection or a rejected key stops the run. A timeout or an unusable answer on one case records that case as `no_answer` and the run continues.

**Eligibility conditions.** A value is `eligible` when all of these hold, and otherwise the output names each condition that failed:
- at least 10 fixture cases labelled with the value (the escape needs none; a boolean needs 10 each for `true` and `false`);
- at least 40 fixture cases in total, escape-labelled cases included;
- every fixture case got an answer (one `no_answer` makes every value ineligible);
- no fixture labelled otherwise is answered with the value at or above its threshold;
- macro recall exceeds always choosing the most frequent label (that baseline is 0 when the escape is the most frequent label);
- at least 30 paired observations under the current declaration hash;
- at most 1 ledger disagreement where the decider chose the value;
- no ledger directed exit from a visit where the decider chose the value;
- the fixture run used the default endpoint, unless `--include-custom-endpoints` is passed.

A value the template marks `never` is still judged and is shown with a `(template: never)` note.

**JSON output.** The top-level keys are `ledger`, `header`, `questions`, and `fixtures` (`null` without `--fixtures`). Each question carries `directed_exits`, `counted_directed_exits`, `values` (each with `directed_exits` and `counted_directed_exits`), `disagreements` (a directed exit carries `directed_to`, the state it went to, which is how to tell it from an answer, and `agent: "directed_exit"`), `confusion`, `rates`, `latency_ms`, `success_measures`, and `flags`. The `fixtures` object carries the declaration hash, the case results, `macro_recall`, `majority_baseline`, and per-value `eligible`, `status`, `conditions`, and `reasons`.

**Exit codes:**
- `0` -- The report printed, whether or not any value is eligible.
- `2` -- A caller problem: a flag combination clap rejects, a fixture run without an opted-in decider, a template that doesn't compile, a state with no declared field (or several and no `--field`), a bad fixture line, or an endpoint the run can't reach or that rejects the key.
- `3` -- The ledger exists but can't be read.

### version

Prints version information as JSON.

```bash
koto version
```

```json
{"version":"0.1.0","commit":"abc1234","built_at":"2026-03-14T00:00:00Z"}
```

### workspace prune

Reclaims a workspace tree rooted at a terminal session. The verb reads the root header, verifies the workflow has reached a terminal state (`completed` or `abandoned`), walks descendants via the session backend's `list()` filtered by `parent_workflow`, judges each member of that subtree by the retention rule (see [session hierarchy](#session-hierarchy)), and after operator confirmation removes the members the rule releases, deepest first and the root last. It is how sessions koto kept (failure terminals, `--no-cleanup`) under a kept root are reclaimed.

**Subtree scope.** The root is judged with every edge. Each descendant is judged with only the lineage between the root and its direct children dropped, so naming the root lets you dissolve its subtree: a child whose own hierarchy (its descendants, and anything linked to them) is released past `retention.keep` goes, while a child with a live dispatcher stays, and so does a grandchild under it. The root goes only when its whole hierarchy is released, which includes every child, so no kept child is left naming a removed parent. Members the rule keeps are listed with their reasons; nothing outside the subtree is removed. Before anything is removed, every released member is locked and judged again, and one now kept, held by another process, or replaced under its name is skipped, as is any member with a skipped one below it. A stale observed anchor that keeps a member is listed with its close command, as `koto session prune` lists it; the prune closes nothing. A malformed retention setting is refused (exit 2) and a sessions directory that can't be listed exits 3; neither removes anything.

```bash
koto workspace prune --root <session-id> [--dry-run] [--yes] [--force]
```

**Required:**
- `--root <session-id>` -- Root session id of the tree to prune. Must be a valid session id (the same allowlist `session start` enforces).

**Optional:**
- `--dry-run` -- Print what would be removed, in order, and what is kept and why, then exit 0 without reclaiming.
- `--yes` -- Skip the interactive confirmation prompt. Required for cron-friendly invocation.
- `--force` -- Bypass the terminal-state safety gate. Allows judging a tree whose root has NOT reached a terminal state. It never overrides the retention rule: a live root, and every member whose hierarchy has a live member, stays. Combining `--yes` with `--force` adds a second confirmation prompt that requires typing the literal string `force-prune` to proceed.

**Symlink refusal:** the verb `lstat()`s the root before any directory traversal. A root that is a symlink (pointing inside or outside `~/.koto/`) is rejected categorically with exit code 2 — this is a workspace-escape mitigation.

**Terminal-state safety gate:** without `--force`, the verb refuses to prune a tree whose root has not reached `completed` or `abandoned`. The error names the current state so the operator can decide whether to wait or force.

**JSON output (success):** printed last, after the preview text.

```json
{
  "name": "my-workflow",
  "pruned": false,
  "descendants_removed": 2,
  "removed": ["my-workflow.task-2", "my-workflow.task-3"],
  "kept": [
    {"name": "my-workflow.task-1", "reason": "kept: alive in its hierarchy: coord (koto_session)",
     "hierarchy": {"session": "my-workflow.task-1", "kept": true, "...": "..."}},
    {"name": "my-workflow", "reason": "kept: alive in its hierarchy: coord (koto_session)",
     "hierarchy": {"session": "my-workflow", "kept": true, "...": "..."}}
  ],
  "skipped": [],
  "failed": [],
  "stale_anchors": [],
  "cursors_gc": 1
}
```

`pruned` says whether the root itself was removed, and `descendants_removed` counts the descendants that were. Each `kept` entry's `reason` is the rendered verdict and its `hierarchy` the JSON `koto session hierarchy` prints for the same judgment (for a descendant, judged without the root's lineage to its children). `skipped` lists members the second judgment or a check under the lock held back, and `failed` members whose removal failed, which exits 3 after the output. `stale_anchors` lists each stale observed anchor with its `close_command`. `cursors_gc` is the count of stale `~/.koto/coordinators/<id>/scan_cursor.toml` files reclaimed as part of the prune run. See `docs/workspace-layout.md` for the full derived-file catalog and the prune-cadence sizing guide.

**Cron-friendly invocation:**

```bash
# Weekly Sunday at 02:00, suppressing the JSON output to silence cron mail.
0 2 * * 0 /usr/local/bin/koto workspace prune --root <id> --yes >/dev/null 2>&1
```

### session start

Creates a child session under a named parent. Drives two distinct on-disk shapes via a companion-flag contract.

```bash
koto session start <name> --parent <parent>
  [--needs-agent --role <r> --template <t> --inputs <json>]
  [--coordinator-of-record <coord-id>]
```

**Required positional:**
- `<name>` -- Name of the new child session. Validated against the same allowlist used by `koto init`.

**Required flag:**
- `--parent <parent>` -- Name of the parent workflow this session is a child of. Validated as a session id.

**Companion-flag contract for the request-store dispatch flow:**
- `--needs-agent` (boolean) -- Mark the session as awaiting agent dispatch. Writes `needs_agent = true` to the header.
- `--role <r>`, `--template <t>`, `--inputs <json>` -- All three are REQUIRED when `--needs-agent` is set. Any of them passed WITHOUT `--needs-agent` is rejected with a parse-time error naming the missing companion.
- `--coordinator-of-record <coord-id>` -- Optional. Defaults to the parent's recorded `coordinator_of_record`, falling back to the parent's session id when the parent is itself pre-request-store.

**Two shapes:**
1. **Plain start** — `--needs-agent` omitted and all four dispatch flags omitted. The header carries no dispatch-request marker.
2. **Dispatch-request start** — `--needs-agent` set along with `--role`, `--template`, `--inputs`. The header writes the dispatch-request fields; a coordinator can claim the session via the request-store protocol on its next `koto next` tick.

**Inputs validation:** `--inputs` must be valid JSON, ≤ 1 MiB, and nested ≤ 128 levels deep. Rejection is exit code 2.

**JSON output:**

```json
{
  "name": "task-42-child-a",
  "parent": "task-42",
  "needs_agent": true
}
```

When a `koto next` tick lands on a needs-agent child that has not yet been claimed, koto returns exit code 66 (EX_NOINPUT) with the typed error `needs_agent_not_dispatched` rather than the historical `corrupt state file` message. Route ticks through the coordinator's `koto next` on the parent root instead.

### next --redelegation-cap

The `koto next` verb accepts `--redelegation-cap <n>` to override the resolved `request_store.redelegation_cap` (default 3) for the current tick. Useful for one-off operator-driven retries when a respawn-heavy workload temporarily exceeds the steady-state cap. The override does NOT persist; subsequent ticks fall back to the resolved value.

### next --dispatch-epoch

The `koto next` verb accepts `--dispatch-epoch <n>` to write the current tick's `ChildDispatched` audit event with the supplied dispatch epoch. Used by recovery walks (Issue 11 cases 3b/3c) when a header rewrite has bumped a child's epoch and the coordinator's log needs to record the bump as a fresh dispatch.

### request attach

Attaches a session to a request leg it will answer, so the session's terminal result is recorded on the leg.

```bash
koto request attach <request-id> <leg> --session <session-id> [--issued-by <id>]
```

A **root session** (created without `--parent`) binds itself to the leg. It has no dispatch epoch, so koto admits it only when every check passes, and writes nothing otherwise:

- the request and the leg are open (`request_closed`, `leg_already_resolved`, `leg_abandoned`);
- the session is not at a terminal state and was not cancelled (`session_terminal`);
- the session was built from a template the leg names (`template_mismatch`). The leg's `template` is one file name or a list of up to eight, and an entry matches the file name of the template the session was initialized from, such as `scope.md`. A session created with `--from-stdin` has no template file and is refused;
- each key in the leg's `inputs` names a variable the template declares, and unless that variable is `rebind: true`, the session's recorded value equals the input (`input_mismatch`, naming the key and both values);
- the leg is unbound, or already bound to this session, in which case the call is a no-op with `"written": false` (`leg_bound_to_different_child` otherwise);
- the session doesn't already answer another live leg. It moves to the new leg only when its old leg was abandoned or its old request closed (`child_bound_to_different_leg` otherwise).

All refusals exit 2. On success the command prints the standard request envelope, and the leg shows `"attach": "self"` and a `bound_template` object with the template's `name`, `hash` and `source` file name. The session's leg pointer is written so its ticks know which leg they answer.

A root session that attaches itself also gets a dispatched-by link naming the request's `requested_by`, written into that session only (see [session link](#session-link)), unless the requester is a name a link can't carry or is the session itself. The attach counts as the session's own unless `--issued-by` names some other issuer: an attach issued for another session binds the leg as before and writes nothing into the session's context.

On a self-attached leg, `koto request progress`, `koto request resolve` and `koto request abandon` are refused with `self_attached_leg` whatever `--dispatch-epoch` says: the leg's result arrives only when the session reaches a terminal state, including under `koto next --no-cleanup`, which keeps the session on disk. `koto request abandon-request` and `koto request close` stay available, and abandoning the request is how a newer run releases a session for re-attachment.

A dispatched child (one `koto request bind` accepts) presented to `attach` is bound exactly as `bind` binds it. Any other child session is refused with `child_not_fenceable`.

### Leg wakes and request watch

A session parked on a `request-leg` gate learns that its leg changed when it next ticks. koto tells the harness running that session when to tick. There is one contract: a per-session wake file, which the request store rings and every subscriber reads. `koto request watch` is koto's own reader over that file, with a cursor; a harness may read the file directly instead.

**What rings.** Every write that can change what a `request-leg` gate reads rings a wake: a leg's result (a worker's promoted terminal result, `koto request resolve`, or the refusal `koto init --koto-leg` records), a leg abandonment (`koto request abandon`, or each leg of `koto request abandon-request`), and `koto request close`. Creating a request, binding or attaching a leg, and appending progress do not ring. The wake is written by the process that made the change, after the change is durable and before that command returns, so a worker's terminal `koto next` has rung its coordinator by the time it exits.

**Whom it addresses.** The request's coordinator of record and its requester, once each, or once when they name the same session. A name that is not a valid session identifier is skipped with a warning. A wake is delivered whether or not the session exists yet, so a subscriber that starts before its session is created still sees it. A wake never fails the write that caused it: if the file can't be written, the write succeeds and a warning goes to stderr. The wake-candidates pass in `koto next` rings the same file when it records `RequesterWoken`.

**What it means.** Only "look again". A wake carries no state: the session's next tick reads the leg through its gate as it always does. A lost wake costs latency, and a duplicate costs one tick that finds nothing new.

**The wake file.** `~/.koto/wakes/<session>`. Each wake appends one opaque line; the file is never renamed, so a watcher registered on the path keeps working. Once it reaches 32 KiB, the next wake truncates it in place before appending its line, so it stays small. Any change to the file is a wake. A native file watcher or `tail -F` sees each append; a poller should compare the file's size and modification time together, because comparing size alone can miss a wake that followed a truncation.

**Reading it with `koto request watch`.**

```bash
koto request watch --session <session-id> --timeout-secs <n> [--since <cursor>]
```

`watch` reads the wake file above: it blocks until the file changes, or until `--timeout-secs` passes, and exits 0 either way with one JSON line:

```json
{"session":"coord","woke":true,"cursor":"w1:27:1790000000000000000.4242.0","cli_contract":{"major":1,"minor":2}}
```

`woke` is `true` on a wake and `false` at the timeout. `cursor` is opaque. Pass it as `--since` to the next `watch`: a wake delivered between the two returns at once instead of being missed. Without `--since`, `watch` starts from the file as it is when it starts. The loop a harness runs is: take a cursor first with `koto request watch --session <id> --timeout-secs 0`, then tick the session; start `watch --since <cursor>` in the background; when it exits, tick the session and start `watch` again with the cursor it printed. Taking the cursor before the tick is what makes a wake that lands between the tick and the watch return at once rather than be missed. A harness that reacts to a background command finishing needs nothing else. Any number of watches may run for one session; each exits on the first wake after its cursor, so several wakes between two watches come back as one `woke: true`.

`--session` and `--timeout-secs` are required: without them the command exits 2 with a usage message on stderr and no JSON. An invalid session is `invalid_identifier` and an unparseable `--since` is `invalid_submission`, both exit 2 before anything is read. A wake file that can't be read is `persistence_error`, exit 3. A signal while polling is `wait_interrupted`, exit 1.

**The bound.** A `watch` that was running before the change exits within 1 second of the command that made the change returning. It polls every 100 ms, which leaves room for a loaded machine. The harness's own reaction time after `watch` exits is outside this bound.

**With no subscriber.** Nothing depends on the wake being read. Where no harness watches the file, wait on a leg with `koto request wait`, which polls the request itself and needs a timeout:

```bash
koto request wait <request-id> --leg <name> --timeout-secs <n>
```

**Local only.** The wake file, like the request store, lives on one machine. A leg resolved on one host never wakes a coordinator on another, and request records do not replicate under the cloud backend.

## Typical agent workflow

The standard loop for an AI agent dispatches on the `action` field:

```bash
# Initialize from a template
koto init task-42 --template workflow.md
EV=$(mktemp -d)

# Main loop
while true; do
  result=$(koto next task-42)
  action=$(echo "$result" | jq -r '.action')

  case "$action" in
    "done")
      # Terminal state -- workflow is done
      break
      ;;
    "gate_blocked")
      # Read .blocking_conditions, fix the issue, then re-query
      continue
      ;;
    "evidence_required")
      # Check .blocking_conditions first -- fix if non-empty
      # Do the work described in .directive
      # Submit evidence matching .expects schema, from a file
      printf '%s\n' '{"decision": "proceed"}' > "$EV/evidence.json"
      result=$(koto next task-42 --with-data @"$EV/evidence.json")
      ;;
    "integration"|"integration_unavailable")
      # Review .integration output (or proceed manually if unavailable)
      # Submit evidence if .expects is present
      ;;
    "confirm")
      # Review .action_output
      # Submit evidence if the state accepts it
      printf '%s\n' '{"status": "accepted"}' > "$EV/evidence.json"
      result=$(koto next task-42 --with-data @"$EV/evidence.json")
      ;;
  esac
done
rm -rf "$EV"
```

Use `--to` for directed transitions when the agent needs to jump to a specific state:

```bash
koto next task-42 --to feedback
```

To roll back after an unexpected result:

```bash
koto rewind task-42
```

## Batch workflows

A batch workflow has one coordinator (parent) that submits a task list, and many workers (children) that drive their own state machines independently. Templates with a `materialize_children` hook expose batch surface through existing commands.

### Batch surface on existing commands

- **`koto next <parent> --with-data @tasks.json`** — submit the task list. Responses from a batch-scoped parent carry a `scheduler` object with `materialized_children`, `spawned_this_tick`, and per-task `feedback.entries`. Dispatch workers based on `materialized_children`, not `spawned_this_tick`.
- **`koto workflows --children <parent>`** — list every child for a parent, with per-row batch metadata (short task name, outcome, waits-on dependencies).
- **`koto status <parent>`** — read-only view of the parent's current state. For batch parents, the response includes the materialized-children ledger so you can check progress without advancing state. When the session recorded a `template_source_dir` at `koto init` time and that directory no longer exists, the response also gains a `stale_template_source_dir` key (`{"path": "...", "machine_id": "...", "note": "..."}`); it's absent (not `null`) whenever there's nothing stale to report. The `note` wording is backend-aware -- direct for local sessions, softened for cloud-backed sessions since a missing directory there may just mean the session was imported from another host, which keeps the template directory the source host recorded (see `docs/guides/cloud-sync-setup.md`).

### Worked example: 3-task linear batch

Given a coordinator that declares `tasks` as an accepts field and routes on `children-complete` gate output, a minimal dependency chain (`task-1` → `task-2` → `task-3`) flows like this:

```bash
# 1. Parent is on the submission state — coordinator submits the task list
koto next coord --with-data @tasks.json
# => action: "gate_blocked" (children-complete waiting),
#    scheduler.materialized_children: [
#      {"name": "coord.task-1", "outcome": "running", "ready_to_drive": true, ...},
#      {"name": "coord.task-2", "outcome": "blocked", "ready_to_drive": false, "waits_on": ["task-1"]},
#      {"name": "coord.task-3", "outcome": "blocked", "ready_to_drive": false, "waits_on": ["task-2"]}
#    ]

# 2. For each entry where ready_to_drive == true, dispatch a worker:
koto next coord.task-1                      # worker drives the child
# ... worker submits evidence for each state until child reaches terminal ...

# 3. Coordinator re-ticks to observe progress:
koto next coord
# => materialized_children updated; task-2 is now ready_to_drive: true

# 4. Dispatch the next worker, re-tick, repeat until all children terminal.

# 5. Final coordinator tick fires the success route:
koto next coord
# => action: "evidence_required" or "done" (depending on post-batch template states)
```

Each coordinator tick re-derives the ledger from disk, so resume after a crash just means running `koto next coord` again. For the full runner surface (failure routing, `retry_failed`, typed error envelopes), see `docs/designs/current/DESIGN-batch-child-spawning.md` and the `koto-user` skill's batch references.
