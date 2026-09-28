---
schema: design/v1
status: Planned
problem: |
  `run_shell_command` starts every command gate and default action as `sh -c`
  with the whole environment of the process that called `koto next`. A shim
  first on `PATH`, an exported shell function where `/bin/sh` is bash, or a
  `HOME` whose `.gitconfig` defines an alias changes what a check reads, so one
  session gives different verdicts from different shells and a gate marked
  `overridable: false` bends to the caller of a single tick. Any fix must keep
  credentials out of the session log, must not strand sessions created by an
  earlier koto, and changes behaviour for every template.
decision: |
  Every session records at creation the values of `PATH` (empty and relative
  entries dropped), `HOME` and `XDG_CONFIG_HOME` in a new header field. Every
  command then runs as `/bin/sh -c`, with empty stdin, in a cleared
  environment holding those three values, the live values of a published
  default name list and of names the template declares in `pass_env:`, and
  `KOTO_TICK_SESSION` and `KOTO_SESSIONS_BASE` set by koto. Seven names that
  inject shell or git and gh configuration can never be passed. A recorded
  value that stops resolving produces a named error with the remedy (a new
  session); attach warns on drift and refuses nothing; an older session adopts
  a record on its first tick with a notice. The first release enforces this by
  default and offers `koto init --legacy-environment`, recorded at creation and
  removed in the next release. More fixed values, a wider refused list, caller routes to add
  names and an attach refusal are deferred with their reproductions kept.
rationale: |
  The three fixed values are the ones with demonstrated bypasses, and none is a
  secret. Once the environment is cleared a variable that isn't passed is
  absent, so recording further values is a compatibility cost (a path that
  disappears strands the session) with no security gain until a reproduction
  shows one. Refusing an attach on drift protected nothing, because a tick
  from the same shell runs with the recorded values anyway, while it blocked
  shirabe's resume. The record reuses the execution anchor's shape so it adds
  one concept, and the staged opt-out gives users a named escape during the
  release that changes behaviour for all of them.
upstream: docs/prds/PRD-koto-fixed-environment.md
---

# DESIGN: gates and actions run in an environment fixed at init

## Status

Planned

## Context and Problem Statement

Every command koto runs for a session goes through one function,
`run_shell_command` in `src/action.rs`. It builds `Command::new("sh")`, sets
the working directory to the session's execution anchor, pipes the output, and
puts the child in its own process group. It never touches the environment, so
the child inherits every variable of the `koto next` process: whatever `PATH`
that shell has, any `BASH_FUNC_*` exported functions (which bash imports even
when started as `sh`), `BASH_ENV`, and `HOME`, under which `git` and `gh`
read their configuration. Its callers are the command gate evaluator
(`src/gate.rs`), the one-shot default action and the polled default action
(`src/cli/mod.rs`). No other code path starts a process on a session's
behalf on the platforms koto ships.

The technical problem is to make that function's environment a property of the
session instead of the caller, without writing credentials into the session
and without stranding sessions and templates that exist today. The PRD
(`docs/prds/PRD-koto-fixed-environment.md`) states what must hold:

- The values of `PATH`, `HOME` and `XDG_CONFIG_HOME` are fixed when a session
  is created (R1). koto issue #261 proposed fixing `PATH` alone. The scope
  widened by two values because a live `HOME` is the same channel: `HOME`
  pointed at a directory whose `.gitconfig` defines `alias.st = !<command>`,
  or sets `core.hooksPath`, runs arbitrary code inside an ordinary `git st` or
  `git commit` that a gate calls. `XDG_CONFIG_HOME` is the same for tools that
  honour it. None of the three is a secret, so the issue's constraint -- no
  secret value in session state -- holds.
- Every other variable a command may see is on a pass list of names read live
  (R2): koto's default list (R3) plus names the template declares (R4). Seven
  names can never be passed (R5).
- A `--legacy-environment` opt-out is recorded at creation and can't be set by
  a tick (R6).
- The shell is `/bin/sh` by absolute path (R11), koto sets `KOTO_TICK_SESSION`
  so the nested-tick refusal survives (R12), and a stale record is named,
  never silent (R13).
- Attach warns on drift and refuses nothing (R14); nothing rewrites a record
  (R15); an older session adopts one on its first tick (R16).
- The on-disk change is additive (R21).

Three things in the existing code shape the answer. koto already solved a
close cousin: the execution anchor (`DESIGN-koto-runs-commands.md`, Decisions
6 and 7) records a directory in the header at init, adopts one on an older
session's first tick with an event and a directive notice, and copies it into
children. The only thing propagating `KOTO_TICK_SESSION` to a command today is
inheritance (`src/engine/reentrancy.rs`), so clearing the environment would
silently disable the guard against nested ticks (koto#208). And the header has
two writers -- `init_child_core` and `init_inline_into_session` in
`src/cli/init_child.rs` -- while attach (`src/cli/init_entry.rs`) writes no
header.

An earlier revision of this design fixed twenty-two values, refused dozens of
names, added two creation-time routes to add names, and refused attach on
drift. Two reviews found that a recorded path which disappears strands the
session (a per-shell `TMPDIR` does this routinely) and that the attach refusal
blocked ordinary resume while protecting nothing. This revision keeps what the
reproductions demand and defers the rest, each with the reproduction that
would justify it (see Deferred Follow-ups).

## Decision Drivers

- **Close the reproduced channels.** The `PATH` prefix, the exported function
  on a bash `sh`, and the `.gitconfig` alias through `HOME` each become a test
  that fails before the change and passes after.
- **Cost every fixed value.** After the environment is cleared, an unpassed
  variable is absent, which already closes it. Recording a value is a
  compatibility choice that strands the session when the value goes stale, so
  each one needs a reproduction.
- **No credential in session state.** Values are recorded only for the three
  fixed variables; everything else is a name.
- **Never fail silently, never block a resume.** A stale record is named with
  its remedy; drift at attach is reported, not refused.
- **Nothing in flight strands.** shirabe's runs created by an older koto keep
  advancing through the upgrade.
- **Reuse the anchor's shape.** Header field, adoption with an event and a
  notice, inheritance by children.
- **Additive on disk; cheap on the hot path.** No schema bump; a tick adds a
  handful of `stat` calls and no spawn.
- **A named escape for the release that changes behaviour for everyone**,
  decided at creation so the ticking agent can't take it.

## Considered Options

### Decision 1: posture and the first release

Every template in use was written assuming the caller's environment. The
question is who gets the fixed environment, and whether there is a way out.

Key assumptions:

- Projects whose gates depend on a dev-shell environment (direnv, `nix
  develop`) are rare among koto-run workflows at release. This is unmeasured;
  a count of `--legacy-environment` uses or of issues filed against the
  release would revise it. None of shirabe's templates runs a project's test
  suite from a gate (checked).
- An agent that can write or edit a template can remove a gate outright.

#### Chosen: on by default, with a creation-time opt-out that a later release removes

Every new session, and every older session once it adopts a record, runs its
commands in the cleared environment of Decision 3. `koto init
--legacy-environment` creates a session whose commands run with the ticking
process's whole environment, as before this change. The flag is recorded in
the session header, children inherit it, and no tick, attach or rebind can set
or clear it, so the choice is made by the creator before any gate is ticked.

**Ruling on the end state (2026-09-28).** The maintainers accepted the staged
end state: the first release enforces the fixed environment by default and
ships `--legacy-environment`, settable only at `koto init` and recorded in the
session; the next release removes the flag, once shirabe's harnesses have
migrated off it, leaving no opt-out. The documentation announces the removal
from the first release on.

A template author whose gate needs a caller-set variable declares its name in
`pass_env:` (Decision 4). The strongest argument for this shape over a wider
widening is enumerability: a declared name is listed in the template, still
passes the refused list, and nobody has to guess what a gate can see. A
"pass everything" switch lets names nobody chose (`GIT_DIR`, `GH_REPO`,
`NODE_OPTIONS`) flow into strict gates. And the asymmetry over time favours
starting narrow: a widening added later breaks nothing, while one shipped and
withdrawn breaks every template that used it.

#### Alternatives Considered

**On by default with no opt-out at all, from the first release.** Rejected
for the first release; it is the ruled end state for the next one. It is the stronger guarantee, but every user whose gates read
a variable outside the default list would have no remedy short of pinning an
old koto, and the default list is a set of guesses about what real projects
need. The opt-out bounds the cost of a wrong guess to one flag.

**An authoring-time template opt-out.** A frontmatter field that makes a
template's sessions inherit the caller's environment. Rejected: it widens the
environment for every user of that template, including those who never needed
it, and lets unchosen names reach strict gates. The creator of a session is
the party who knows whether their project needs its dev shell, so the escape
belongs at creation.

**Template opt-in.** Rejected: the accidental case stays open wherever no
author opted in, and the determinism stories belong to users who don't control
the template.

### Decision 2: which values are fixed, and what happens when one goes stale

Fixing a value closes a channel only if the value would otherwise be read
live. Once the environment is cleared, the alternative to "fixed" is "absent"
unless the name is on the pass list. So the question per variable is: does a
command need it, and if so, is reading it live a reproduced bypass?

Key assumptions:

- `HOME` is needed (git, gh and koto locate their configuration and stores
  under it), and a live `HOME` is a reproduced bypass. `XDG_CONFIG_HOME` is
  the same channel. `PATH` is needed and is the issue's bypass.
- A per-shell temporary directory is common: nix shells export `TMPDIR` and
  remove it on exit, and sandboxed agent harnesses do the same. So `TMPDIR`
  recorded by value would strand sessions routinely.

#### Chosen: fix `PATH`, `HOME`, `XDG_CONFIG_HOME`; name a stale one loudly

**The fixed values.** `PATH`, `HOME` and `XDG_CONFIG_HOME`, recorded at
creation from the creating process; unset stays unset.

**`PATH` normalization**, from the string alone, at creation, adoption and the
attach comparison: empty entries (leading, trailing or doubled `:`) and every
relative entry (`.`, `bin`, `node_modules/.bin`, and a literal `~`, which
neither `execvp` nor the shell expands during lookup) are dropped. Commands
start at the execution anchor, so such an entry resolves inside the
repository under review, where a `gh` committed to a branch would become the
`gh` every gate runs. A `PATH` with no surviving entry is recorded unset; at
run time an unset `PATH` becomes `/usr/bin:/bin`, never the shell's built-in
default, which in upstream bash ends in `.`. A relative `HOME` or
`XDG_CONFIG_HOME` is recorded unset.

**A fixed value carrying a credential.** A fixed value that contains the value
of a set credential-carrying default name -- the `gh`/GitHub token variables
and the proxy URLs, which can embed a password -- is recorded unset, and the
`koto init` response names the variable, never the value. It is a substring
test at creation against those few values, skipping any shorter than eight
characters, which would match ordinary path text by accident. Names like
`CI` or `TZ`, whose short values carry nothing secret, aren't compared.

**Stale values.** On every tick, before commands run, koto stats each recorded
`PATH` directory, the recorded `HOME` and the recorded `XDG_CONFIG_HOME`: a few
`stat` calls, no spawn. The missing ones form the tick's stale list. Then:

- If a gate or action fails and either the stale list is non-empty or the
  command reported a command not found, the `koto next` response carries a
  note naming the failing gate or action, the stale values (or, when none is
  missing, that it ran under the recorded `PATH`, with its value), and the
  remedy: the record can't be changed, so start a new session (`koto cancel
  --cleanup <name>`, then `koto init` again; a skill re-invoked after the
  cleanup does this). "Command not found" is detected from exit status 127
  *or* the shell's not-found message on the command's stderr (`not found` from
  dash, `command not found` from bash), so a shirabe gate script whose inner
  `gh` is missing, and which exits with its own status, still gets the note.
- If the stale list is non-empty and nothing failed, the response carries a
  notice naming the stale values.
- The note goes on the response only (`with_directive_prefix`). Gate evidence
  stays byte-identical, so `when:` conditions and override records keep
  matching, and nothing is appended to an action's captured output, which is
  persisted in the log as the command's own.

The note carries only the recorded `PATH`, `HOME` or `XDG_CONFIG_HOME` and the
gate's name, never a live value.

#### Alternatives Considered

**Fix every non-secret variable that locates a tool's inputs** (`TMPDIR`,
`XDG_CACHE_HOME`, `SSL_CERT_*`, locale, and more; this design's previous
revision). Rejected: none of them had a reproduced bypass, and each recorded
path is a way to strand a session when it disappears. `TMPDIR` is the worst
case, since per-shell temporary directories are removed routinely and dozens
of shirabe gate scripts call `mktemp`. They are read live by name instead
(Decision 4), and each is a deferred follow-up that a reproduction would
promote.

**Fix values but fall back when one is gone** (run with `/tmp` when the
recorded `TMPDIR` is missing). Rejected for now as unnecessary: with `TMPDIR`
live, only `PATH`, `HOME` and `XDG_CONFIG_HOME` can go stale, and silently
substituting another `HOME` would reopen the channel it closes.

**A verb that re-records a stale value.** Rejected: the same bypass with a log
line. A new session is the remedy, and the note says so.

### Decision 3: the record, children, adoption and attach

The record must live where the header's other creation-time facts live,
survive every header rewrite (rebind, rename, recover, claim all read-modify-
write the header), reach children, and get to `run_shell_command` without each
call site rebuilding it.

Key assumptions:

- No header or event type uses `deny_unknown_fields`, so an older koto reads a
  newer header and skips an unknown event.
- A batch parent adopts in its own tick before it can spawn a child, because
  adoption runs before gates and the scheduler.

#### Chosen: a nested header struct, an adoption event, and a runtime `CommandEnv`

`StateFileHeader.command_environment: Option<CommandEnvironment>`, following
the `origin: Option<SessionOrigin>` pattern:

```rust
pub struct CommandEnvironment {
    pub path: Option<String>,             // normalized
    pub home: Option<String>,
    pub xdg_config_home: Option<String>,
    pub pass: Vec<String>,                // the release's default live names
    pub legacy: bool,                     // --legacy-environment
}
```

The template's `pass_env:` names are not stored: a session's compiled template
can't change, so each tick resolves the live names as `pass` ∪ the template's
`pass_env`. A child whose template differs from its parent's copies the
parent's record and still gets its own template's names. Storing the default
list keeps a session's environment stable across a koto upgrade that changes
the default.

The header is the first line of the state file, so the record is readable in
the log at creation. A new `EventPayload::EnvironmentAdopted { environment,
dropped }` (wire name `environment_adopted`) is appended when an older session
adopts, so the log shows when a record arrived after the fact and what was
dropped. No event is emitted at creation; it would duplicate the header line.

**Creation paths.** Both header writers call one helper,
`resolve_command_environment`, which copies the parent's record when there is
a parent that has one, and otherwise records from the invoking process (with
`legacy` from the flag). It returns the recording report only when it
recorded, and the `koto init` response carries that report, so a response
never describes a record other than the one written. A parent whose header
can't be read is an error rather than "no record". `--legacy-environment` is
accepted on every top-level `koto init` form, has no effect when attaching to
an existing session, and is refused with `--parent`, since a child takes its
parent's record.

**Children** -- batch spawn, retry, skip marker, `koto init --parent`, `koto
session start` -- copy the parent's record through a
`resolve_command_environment` helper beside `resolve_execution_dir`. A child is
created inside the parent's tick, whose process environment belongs to
whoever ticked the parent, so it is never consulted. `koto init --parent` or
`koto session start` against a parent with no record records from its own
process. Children spawned before the upgrade adopt independently, from
whatever process ticks each first; a batch can therefore end up with records
that differ, and the adoption notice on each says what it recorded.

**Adoption** runs in `koto next` after the reentrancy check, the anchor check
and the dispatch-epoch fence (moved up for this; it reads only the header, and
`--to` excludes `--with-data`), and before variables, the template, the `--to`
guard or any command. It takes an exclusive lock on the session's own
`environment.lock` -- the state-file lock is non-blocking and held across a
batch parent's scheduling, so it can't serialize two adopters -- and re-reads
the header locally under it, so two racing ticks can't both adopt and a pulling
read on the cloud backend can't discard header changes this tick already made.
It records the ticking process's fixed values (normalized) and the default
list, never the legacy flag, appends `environment_adopted`, rewrites the
header, pushes the state file strictly, and prefixes a one-time notice showing
the recorded values, the dropped entries, and that variables outside the
default list no longer reach commands. The notice shows values because they
are the three fixed variables, recorded only after the credential check; attach
drift names variables only because the caller's own values haven't been
through that check.

**Attach**, beside `check_origin` in `src/cli/init_entry.rs`, compares the
caller's normalized `PATH`, `HOME` and `XDG_CONFIG_HOME` with the record. A
difference attaches as before; the attach response gains an
`environment_drift` array naming the differing variables, and one line on
stderr names them and says commands run with the recorded values. Neither
names a value, recorded or caller's, the same rule as the `koto init`
response. Nothing is written to a request leg. An unrecorded session isn't
compared; its next tick adopts a record.

**At run time**, a `CommandEnv` -- ordered name-value pairs, never serialized,
defined in `src/action.rs`, built by the engine -- is made once per tick. For a
legacy session it is the ticking process's environment plus
`KOTO_TICK_SESSION`, which is today's behaviour. Otherwise it is the recorded
fixed values, the live value of each name in `pass` ∪ `pass_env` that the
ticking process has set, minus the refused names, then `KOTO_TICK_SESSION` and
`KOTO_SESSIONS_BASE` last so a declared name can't override them. The fixed
values are also applied after the pass list for the same reason.
`run_shell_command` takes `&CommandEnv`, calls `env_clear()`, applies it, sets
standard input to null, and spawns `/bin/sh`, reporting a spawn failure as
such with no fallback to a `PATH` search. `evaluate_gates_with_request_store`
takes it as a parameter, so the compiler finds every caller: `TickGates`, the
polling loop's gate closure, and the `--to` guard; both default-action sites
pass the same value.

#### Alternatives Considered

**Refuse attach when `PATH`, `HOME` or `XDG_CONFIG_HOME` differ** (the previous
revision). Rejected: attach writes no record, and `koto next` from the same
caller runs with the recorded values, so the refusal protected nothing. It
did block resume for any shell whose `PATH` differs for ordinary reasons (an
activated virtualenv, an IDE terminal, a version manager), and shirabe's
entry scripts attach on every invocation, so a refusal meant discarding the
run.

**An event at creation as well as adoption.** Rejected as duplication: the
header already sits at the top of the same file.

**Rebuilding the environment at each call site.** Rejected: three builders
(gate, one-shot action, polled action) can drift within one tick, and each
would have to inject `KOTO_TICK_SESSION` exactly right.

### Decision 4: names, the defaults, and the shell

The pass list needs a template surface (R4), an exact default list (R3), a
short refused list (R5), and a shell path no `PATH` decides (R11).

Key assumptions:

- An older koto that meets the new template key ignores it, as it ignores any
  unknown top-level frontmatter key today; a gate that then misses a variable
  fails loudly.
- No named user today needs a creation-time route to add names; shirabe's
  harnesses can use the legacy flag in the first release.

#### Chosen: `pass_env:`, a published default list, seven refused names, `/bin/sh`

- **Template:** a flat top-level frontmatter list, `pass_env: [NAME, ...]`. It
  compiles to `CompiledTemplate.pass_env: Vec<String>` with
  `skip_serializing_if = "Vec::is_empty"`, so a template that declares none
  keeps its compiled JSON and hash. It works on the `--from-stdin` path. A
  name must match `^[A-Za-z_][A-Za-z0-9_]*$`; a refused name is a compile
  error; a name koto sets itself (`PATH`, `HOME`, `XDG_CONFIG_HOME`,
  `KOTO_TICK_SESSION`, `KOTO_SESSIONS_BASE`) compiles with a warning, since
  koto's value wins. Template names are authored text, not caller input.
- **Default live names (exact):** `USER`, `LOGNAME`, `LANG`, `LANGUAGE`,
  `LC_ALL`, `LC_CTYPE`, `LC_COLLATE`, `LC_MESSAGES`, `LC_NUMERIC`, `LC_TIME`,
  `LC_MONETARY`, `TZ`, `TMPDIR`, `TERM`, `NO_COLOR`, `CI`, `XDG_CACHE_HOME`,
  `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_RUNTIME_DIR`, `SSL_CERT_FILE`,
  `SSL_CERT_DIR`, `SSH_AUTH_SOCK`, `DBUS_SESSION_BUS_ADDRESS`, `GH_TOKEN`,
  `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN`, `GH_HOST`,
  `HTTP_PROXY`, `http_proxy`, `HTTPS_PROXY`, `https_proxy`, `NO_PROXY`,
  `no_proxy`, `ALL_PROXY`, `all_proxy`. Not included: `GH_REPO`, any `GIT_*`,
  `AWS_*`, `KOTO_*` (koto sets the two its commands need).
- **Refused (exact, and nothing else):** `BASH_FUNC_*`, `BASH_ENV`, `ENV`,
  `GIT_CONFIG*`, `GIT_SSH_COMMAND`, `GIT_ASKPASS`, `GH_CONFIG_DIR`. These are
  the names the issue and the `.gitconfig` reproduction justify. They can only
  reach a command through `pass_env:` once the environment is cleared, so the
  refusal is a compile-time guard for template authors, and the builder
  filters them again.
- **Set by koto on every command:** `KOTO_TICK_SESSION` and
  `KOTO_SESSIONS_BASE` (the base of the store the tick is operating on), so a
  nested `koto` finds the same store and a nested `koto next` is refused.
- **Shell:** `/bin/sh` by absolute path on Linux and macOS.

A command can still set any variable for itself (`GIT_DIR=... git ...`): the
lists govern what koto passes, not what a command may use.

#### Alternatives Considered

**Creation-time routes to add names** (`koto init --pass-env NAME`, and a
`KOTO_PASS_ENV` variable read at init). Rejected for this release. They give
the creator a way to widen a strict template's sessions that its author never
agreed to. The ambient variable breaks resumes whenever a shell profile
changes, and a pasted token can land in the record as a "name". Harnesses use
the legacy flag meanwhile. Deferred with its use case.

**A long refused list** (every `GIT_*`, editor and pager names, loader
variables, every `KOTO_*`). Rejected: once the environment is cleared those
names reach a command only when a template author declares them, and the
author can delete the gate anyway. The list broke legitimate uses (deploy
keys through `GIT_SSH_COMMAND` aside, `GIT_DIR` layouts, `KOTO_BIN` in
shirabe's scripts) with no remedy short of a release. Deferred.

**Resolving `sh` through the recorded `PATH`.** Rejected: a stand-in `sh` on
the recorded `PATH` would decide how every command is parsed.

## Decision Outcome

Every session carries a record of three values, made once and never
rewritten: at `koto init`, or in a child copied from its parent, koto records
`PATH` (empty and relative entries dropped), `HOME` and `XDG_CONFIG_HOME`, the
release's default live names, and whether the creator chose
`--legacy-environment`. The record is the header's new `command_environment`
field. A session created by an earlier koto adopts one on its first tick,
under the lock, with an `environment_adopted` event and a one-time notice.

Every command a tick runs -- gate, one-shot action, each polling attempt --
starts as `/bin/sh -c` with empty standard input, in an environment built once
per tick: the three recorded values (unset stays unset; an unset `PATH`
becomes `/usr/bin:/bin`), the live value of each default and
template-declared name the ticking process has set, minus seven refused
names, and `KOTO_TICK_SESSION` and `KOTO_SESSIONS_BASE`. Nothing else reaches
the command. A legacy session runs as before.

Each tick stats the recorded values. When a gate or action fails and a
recorded value is missing or the command reported "not found", the response
names the gate, the stale value and the remedy, a new session. A missing value
with no failure is a notice. Gate evidence and captured output are untouched.
Attach reports drift in its response and refuses nothing. There is no verb
that rewrites the record.

The pieces fit because each covers a gap another leaves open. The three fixed
values close the reproduced channels. Names read live keep credentials
working and out of the log. The short refused list stops a template from
reopening a closed channel by name. Normalization stops the recorded `PATH`
from pointing into the tree under review. And the stale-record note makes the
cost of a permanent record visible instead of silent.

## Solution Architecture

### Components

| Component | Where | Change |
|---|---|---|
| `CommandEnvironment` | `src/engine/types.rs` | New struct; `StateFileHeader.command_environment`; `EnvironmentAdopted` event and wire name; its entry in `docs/reference/session-feed.md` |
| `CommandEnv` | `src/action.rs` | Runtime type; `run_shell_command` takes it |
| Environment module | new `src/engine/command_env.rs` | Default and refused lists, name validation, `PATH` normalization, credential check, record building, stale check, `CommandEnv` builder |
| Template compiler | `src/template/compile.rs`, `types.rs` | `pass_env:` parsing, validation and warning, compiled field |
| Header writers | `src/cli/init_child.rs` | `resolve_command_environment` (parent's record, else this process's) in `init_child_core` and `init_inline_into_session`, returning the recording report |
| Entry flags | `src/cli/mod.rs`, `src/cli/init_entry.rs` | `--legacy-environment` on top-level forms, refused with `--parent`; attach drift report beside `check_origin` |
| Session start | `src/cli/session.rs` | Inherit from the parent, or record from the process when the parent has none |
| Tick | `src/cli/mod.rs` | Adoption after the anchor check and epoch fence, under the lock; stale check; build `CommandEnv` once; pass it to every gate evaluation and both action sites; adoption, stale and not-found notes |
| Gate evaluation | `src/gate.rs` | `evaluate_gates`, `evaluate_gates_with_request_store` and `evaluate_command_gate` take `&CommandEnv`; evidence unchanged |
| Reentrancy | `src/engine/reentrancy.rs` | Module comment: the marker now reaches commands because koto sets it, not by inheritance |

### Data flow

```
koto init (top-level form)
  compile template (pass_env names validated, not recorded)
  normalize PATH; read HOME, XDG_CONFIG_HOME; unset any that carries a token
  record = {path, home, xdg_config_home, pass = default live names, legacy}
  write header; report dropped entries and unset values in the response

child creation (batch, retry, skip marker, --parent, session start)
  record = parent's record, or from the process if the parent has none

koto init --attach-live
  ... template, origin checks ...
  if record present: compare normalized PATH, HOME, XDG_CONFIG_HOME
     differ -> attach anyway; response.environment_drift; one stderr line

koto next
  reentrancy -> anchor -> epoch fence
  record absent -> under lock, re-read; build from process; append
                   environment_adopted; rewrite header; remember notice
  stale = recorded PATH dirs, HOME, XDG_CONFIG_HOME that don't exist
  CommandEnv = legacy ? process env + KOTO_TICK_SESSION
             : live (pass ∪ pass_env) - refused, then fixed values,
               then KOTO_TICK_SESSION, KOTO_SESSIONS_BASE
  every gate / action / polling attempt -> run_shell_command(.., &CommandEnv)
  failure with (stale or 127 or not-found on stderr) -> note on the response
  stale without failure -> notice on the response
```

### The reproductions, as tests

- **`PATH` prefix.** A template whose only gate is `overridable: false` and
  runs `test "$(whoami)" = nobody`. Create normally; tick with a directory
  holding a fake `whoami` first on `PATH`. Before: `passed`. After: `check`.
- **Exported function.** The same template on a host whose `/bin/sh` is bash;
  tick with `whoami() { echo nobody; }; export -f whoami`. Before: `passed`.
  After: `check`. It runs in CI in a job that links `/bin/sh` to bash on the
  runner before running this one test, and locally in a Debian container with
  `/bin/sh` linked to bash. An in-suite assertion that no `BASH_FUNC_` name
  reaches a command covers dash hosts.
- **`.gitconfig` alias.** A gate runs `git koto-check`, where no alias exists.
  Tick with `HOME` pointing at a directory whose `.gitconfig` sets
  `alias.koto-check = !true`, and separately with `XDG_CONFIG_HOME` pointing at
  one whose `git/config` does. Before: the gate passes. After: it fails.
- **Accidental shim.** A session created with a stand-in `gh` first on `PATH`
  reads it from a tick whose `PATH` lacks it, and one created without it
  doesn't read it from a tick whose `PATH` has it.

Secrecy: with `GH_TOKEN`, `GITHUB_TOKEN` and names ending `_TOKEN`,
`_SECRET`, `_KEY`, `_PASSWORD` set to unique markers at `koto init` and on
ticks that run a gate, a one-shot action and a polled action, no file under
the session directory contains a marker. Stale: a recorded `PATH` directory,
`HOME` and `XDG_CONFIG_HOME` each removed after init produce the notice, and
the note when a gate then fails; a gate script whose inner command isn't
found gets the note.

### What a shirabe session sees on upgrade

shirabe's templates call plugin scripts by absolute path (`{{PLUGIN_ROOT}}`)
and `gh`, `jq`, `git`, `koto` and `shirabe` by name, and read no environment
variable directly (its CI rejects `$NAME` in a gate command). So:

- **A run in flight** adopts a record on its next tick: the agent's `PATH`,
  `HOME` and `XDG_CONFIG_HOME` and the default list, with the notice. Tools
  keep resolving from the same `PATH`; `gh` keeps its token or keyring, `git
  fetch` its ssh agent and proxies, `mktemp` its live `TMPDIR`; nested `koto`
  calls reach the same store; scripts reading `KOTO_TICK_SESSION` find it.
- **A new run** records at its `koto init`, which `koto-open.sh` runs from the
  agent's shell.
- **`--attach-live` on resume** always attaches; a drifted `PATH` is reported
  in the response, which shirabe's scripts can ignore or surface.
- **A GitHub Enterprise user's `GH_HOST`** is on the default list.

What shirabe must change (proposed; shirabe does the work):

1. **Harnesses whose stand-in tools read other variables.** Eleven test
   scripts tick a real koto and set stand-in variables through the
   environment: `work-on/terminal-retention`, `work-on-open`, `scope-open`,
   `execute/terminal-retention`, `execute-open`, `execute-coordinated-engine`,
   `deliver-absent`, `deliver-engine`, `deliver-open`, `coordinate_engine`,
   `board-land_engine` (each `*_test.sh` under `skills/*/scripts/`), plus the
   eval runner and the four skills' `evals/fixtures/bin/gh` stubs. Their
   stand-ins read `GH_DB`, `GH_FIX`, `GH_BOARD_DIR`, `COORD_TESTDATA`,
   `BT_STATE`, `MERGE_CONFIRM_WAIT_SECS`, `GIT_CEILING_DIRECTORIES`,
   `EVAL_SCENARIO`, `EVAL_SCENARIO_DIR`, `GH_CALL_LOG`, and `KOTO_BIN`,
   `KOTO_STORE`, `KOTO_FAIL_ADD`, `KOTO_BOARD_DIR`. In the first release they
   create their sessions with `--legacy-environment`; the entry scripts build
   the `koto init` line, so shirabe adds a harness-only knob to
   `scripts/koto-open.sh` that appends the flag. The other forty-odd test
   scripts call scripts directly and are untouched. A long-term route is a
   deferred follow-up here.
2. **Harnesses that change a variable between ticks** keep working: live
   values may change (`GH_DB` is re-exported per topic after init).
3. **Templates:** no change for production use.
4. **Verification:** run shirabe's engine suites against the built koto with
   the knob set, and report the result on this PR.

## Implementation Approach

1. **The record.** `CommandEnvironment`, the environment module (lists,
   validation, normalization, credential check, builder), `pass_env:` in the
   compiler, `--legacy-environment`, recording through
   `resolve_command_environment` in both header writers, children and
   `koto session start`. Commands still run
   as today; sessions start carrying a record.
2. **Adoption and attach.** Adoption in `koto next` after the epoch fence and
   under the lock, with `environment_adopted` and its notice; the attach
   drift report. After this step every session that ticks has a record.
3. **Runner switch, last.** `run_shell_command` and gate evaluation take
   `&CommandEnv`: `env_clear`, null stdin, `/bin/sh`, the two `KOTO_*` values;
   the stale check and the stale and not-found notes; the reproduction tests;
   a CI job that links `/bin/sh` to bash and runs the exported-function test.
   Existing tests that call `run_shell_command` or build headers get the new
   argument or field; `tests/command_output_limits.rs` sets its `PATH` at
   init; `tests/support/decider_session.rs` is checked for per-command `PATH`.
   Doing this last means no intermediate commit runs an unrecorded session
   with an empty environment.
4. **Documentation.** CHANGELOG under Unreleased; `docs/reference/session-feed.md`
   for the event; the default-action guide (which today says a command
   inherits the environment of `koto next`), the template format and session
   lifecycle docs; the koto-user, koto-author and koto-adhoc skills
   (koto-adhoc and koto-user tell authors to read `$VAR` at gate time, which
   becomes "declare the name").

The steps land as one pull request, in that commit order.

### Deferred Follow-ups

Each is out of this release; the reproduction named is what would justify
building it.

| Follow-up | Reproduction that would justify it |
|---|---|
| Fix `XDG_CACHE_HOME` by value | A gate running `go test` passes on a prepared Go test cache the caller points `XDG_CACHE_HOME` at, and fails without it. |
| Fix `SSL_CERT_FILE`/`SSL_CERT_DIR` by value | A `gh` gate's verdict flips when the caller points `SSL_CERT_FILE` at a bundle trusting a local proxy set through `HTTPS_PROXY`. |
| Fix `TMPDIR`, locale, `TZ` by value | None known; each needs a verdict that flips on it, and `TMPDIR` also needs a fallback when the recorded directory is gone. |
| Refuse more `GIT_*` names (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`, `GIT_SSL_NO_VERIFY`, ...) and editor, pager and loader names | A shipped template that declares one and a gate whose verdict flips on its value. Until then only an author can declare them. |
| A creation-time route to add names (`--pass-env`, or an init-time variable) | A named user who can't use `pass_env:` or the legacy flag. Must settle how an ambient value interacts with resume, and refuse credential-shaped items. |
| Refuse attach on drift | A measured case where a drifted attach caused a wrong verdict that a tick with the recorded values would not. |
| Remove `--legacy-environment` (no opt-out) | Ruled 2026-09-28 for the next release; the trigger is shirabe's harnesses no longer passing the flag. |
| Let an adopted record be re-adopted once | A report of an adoption from an atypical first tick (a monitor or a cron job) that stranded a run. |
| Let a template forbid `--legacy-environment` for its sessions | A template author relying on a strict gate who needs the guarantee against the creator. |
| Inject the request-store root for nested koto | A nested `koto request` that reads a different store because the tick's `HOME` differs from the recorded one. |

## Security Considerations

### Threat model

Two adversaries, named because they get different answers.

| | An accidental environment | The ticking caller (usually also the creator) |
|---|---|---|
| A shim or version manager earlier on one shell's `PATH` | Served: the recorded `PATH` is used from every shell | Served for a one-tick change |
| An exported function, `BASH_ENV`, `ENV` in the ticking shell | Served | Served |
| `HOME`/`XDG_CONFIG_HOME` pointed at crafted git config for one tick | Served | Served |
| `GIT_CONFIG*`, `GIT_SSH_COMMAND`, `GIT_ASKPASS`, `GH_CONFIG_DIR` | Served | Served, including through `pass_env:` |
| A shim already on `PATH` at creation | Frozen in and still read; consistency, not detection | Not served |
| Choosing `PATH`, `HOME` or `--legacy-environment` at creation | -- | Not served: the creator chooses |
| Changing a live value (`GH_HOST`, a token, `TMPDIR`, `SSL_CERT_FILE`, a declared name) | -- | Not served by design: these carry credentials or have no reproduced bypass |
| Writing a binary into a user-writable directory on the recorded `PATH`, or editing `~/.gitconfig` under the recorded `HOME` in place | -- | Not served: that's file tampering |
| Editing the session header or log | -- | Not served |

The property this design gives is: *a session's `PATH`, `HOME` and
`XDG_CONFIG_HOME` can't change after creation, and shell and git/gh config
injection names never reach its commands.* It doesn't promise that an agent
which also creates the session can't widen it. The documentation (R17) states
this table.

### Secrets

koto records values only for `PATH`, `HOME` and `XDG_CONFIG_HOME`, and records
any of them unset when it contains a set token's value. Pass-list names appear
nowhere in the record except the default list, which is fixed text.
Template-declared names are authored text. The stale and not-found notes carry
only the recorded fixed values and a gate name, go on the response only, and
never enter captured output. What a command prints is still captured as
evidence, as today; the secrecy tests cover everything koto writes.

The credential check governs the environment record only. koto already
records paths elsewhere in a session -- the execution anchor, and the compiled
template's cache path, which sits under `HOME` -- and those are unchanged. A
token whose value is literally part of a directory name (`HOME=/home/ghp_...`)
is kept out of the record but still appears in those paths. That is a
pre-existing property of those fields, not something this change introduces,
and it is out of scope here.

### Other

- **Nested ticks.** koto sets `KOTO_TICK_SESSION` explicitly on every command,
  legacy or not, and a test runs `koto next` from a gate.
- **The shell.** `/bin/sh` by absolute path; on macOS it is bash in POSIX
  mode, whose exported-function and `BASH_ENV` hooks the cleared environment
  removes. A spawn failure is reported, never retried through a `PATH` search.
- **Standard input** is null, so a gate can't read what a caller pipes into
  `koto next`.
- **Process attributes** other than the environment (umask, resource limits)
  are still inherited.
- **Older koto.** An earlier koto ticking a session created by this one
  ignores the record.

## Consequences

### Positive

- A session's verdicts no longer depend on which shell ticks it, for tools
  and git and gh configuration.
- The issue's two reproductions and the `.gitconfig` channel are closed for a
  one-tick change.
- A stale record fails loudly with the remedy; resume is never blocked.
- One concept added, in the anchor's familiar shape.

### Negative

- Every template now runs with a restricted environment. A gate that read a
  variable outside the default list breaks until its name is declared, or its
  session is created with the legacy flag.
- A tool that moves off the recorded `PATH` strands a session's gates until
  it comes back or the session is replaced.
- Gates that relied on `.` or a relative `PATH` entry must spell the path.
- shirabe's harnesses need a knob to create legacy sessions.
- In the first release the creator can opt out, so a strict gate is only as
  strong as the creator's choice.

### Mitigations

- The CHANGELOG entry and the guides publish the default list, `pass_env:`
  and the legacy flag; the adoption notice says what changed on the first
  tick.
- The stale and not-found notes name the value and the remedy.
- The deferred follow-ups keep their reproductions, so each can be revisited
  on evidence.
