---
schema: design/v1
status: Proposed
problem: |
  `run_shell_command` starts every command gate and default action as `sh -c`
  with the whole environment of the process that called `koto next`. A shim
  first on `PATH`, an exported shell function where `/bin/sh` is bash, or a
  `HOME` whose `.gitconfig` defines an alias changes what a check reads, so one
  session gives different verdicts from different shells and a gate marked
  `overridable: false` bends to the caller of a single tick. Any fix must keep
  secrets out of the session log and must not strand sessions created by an
  earlier koto.
decision: |
  Every session records, at creation, the values of the variables that locate
  tools, configuration and caches and can't hold a secret (`PATH`, `HOME`,
  `XDG_CONFIG_HOME`, locale, `TMPDIR`, cache and CA-bundle paths and a few
  more) plus a list of names read live, in a new header field and a new
  `EnvironmentRecorded` log event. Every command then runs as `/bin/sh -c`,
  with empty stdin, in a cleared environment holding the recorded values, the
  named variables' live values, and `KOTO_TICK_SESSION` and
  `KOTO_SESSIONS_BASE` set by koto. koto ships a default live-name list
  (tokens, sockets, proxies); templates add names with a
  top-level `pass_env:` list, callers with `koto init --pass-env` or
  `KOTO_PASS_ENV`; a closed list of shell, loader, git and gh injection names
  can never be added. The recorded `PATH` drops empty and relative entries and
  flags absolute ones inside the execution anchor. Attach refuses a differing
  record as `environment_mismatch`; nothing rewrites a record. A session from
  an older koto adopts one on its first tick with a notice showing what was
  recorded. It is on for every session, with no opt-out.
rationale: |
  On-by-default is the only posture that closes the accidental case for users
  who don't control the template; an authoring-time opt-out would let
  redirecting and code-running variables reach strict gates live, and a
  widening added later breaks nothing while a withdrawn opt-out would. Fixing
  every non-secret variable that locates a tool's inputs, not just `PATH`,
  closes the git and gh config channel and its cousins at no secrecy cost. Names read live keep credentials working and out
  of the log. The record reuses the execution anchor's shape -- header field,
  event, adoption with a notice, inheritance by children, refusal on attach --
  so it adds one concept, not a new mechanism. Normalizing `PATH` removes the
  one poison that can be told from legitimate setups, resolution into the
  repository under review, without refusing any real `PATH`.
upstream: docs/prds/PRD-koto-fixed-environment.md
---

# DESIGN: gates and actions run in an environment fixed at init

## Status

Proposed

## Context and Problem Statement

Every command koto runs for a session goes through one function,
`run_shell_command` in `src/action.rs`. It builds `Command::new("sh")`, sets
the working directory to the session's execution anchor, pipes the output, and
puts the child in its own process group. It never touches the environment, so
the child inherits every variable of the `koto next` process: whatever `PATH`
that shell has, any `BASH_FUNC_*` exported functions (which bash imports even
when started as `sh`), `BASH_ENV`, the dynamic-loader variables, and `HOME`,
under which `git` and `gh` read their configuration. Its callers are the
command gate evaluator (`src/gate.rs`), the one-shot default action and the
polled default action (`src/cli/mod.rs`). No other code path starts a process
on a session's behalf: the decider, the batch scheduler, the request store and
the dashboard all work in-process.

The technical problem is to make that function's environment a property of the
session instead of the caller, without writing secrets into the session and
without stranding sessions and templates that exist today. The PRD
(`docs/prds/PRD-koto-fixed-environment.md`) states what must hold. In its
terms:

- The values of `PATH`, `HOME` and `XDG_CONFIG_HOME` are fixed when a session
  is created and used for every command it runs; a variable unset at creation
  stays unset (R1). koto issue #261 proposed fixing `PATH` alone. The scope
  widened because a live `HOME` is the same channel: `HOME` pointed at a
  directory whose `.gitconfig` defines `alias.st = !<command>`, or sets
  `core.hooksPath`, runs arbitrary code inside an ordinary `git st` or
  `git commit` a gate calls. None of the three values is a secret, so the
  issue's constraint -- no secret value in session state -- still holds. The
  design's security review applied the same test to the rest of the
  environment: every variable that locates a tool's inputs and can't hold a
  secret (cache and CA-bundle paths, locale, time zone, temporary directory,
  terminal, user name) is fixed by value too (PRD R1 as revised).
- Every other variable a command may see is named on a pass list whose values
  are read live at each run and never written anywhere (R2, R9). koto ships a
  default list (R3); templates (R4) and callers creating a session (R5, by
  flag and by an init-time environment variable) add names; a closed list of
  names that inject code into the shell, the loader, `git` or `gh` can never be
  added (R6).
- The shell is started by absolute path (R11), `KOTO_TICK_SESSION` is set by
  koto on every command so the nested-tick refusal survives (R12), and an exit
  127 carries a note naming the recorded `PATH` (R13).
- An attaching `koto init` whose fixed variables or added names differ from
  the record is refused as `environment_mismatch` (R14, R15), and nothing
  rewrites a record once written (R16).
- A session created by an older koto adopts a record on its first tick, with a
  one-time notice (R17), and there is no opt-out (R19).
- The on-disk change is additive: `CURRENT_SCHEMA_VERSION` stays at 1 (R23).

Three things in the existing code shape the answer. First, koto already
solved a close cousin of this problem: the execution anchor
(`DESIGN-koto-runs-commands.md`, Decisions 6 and 7) records a directory in
the header at init, adopts one on an older session's first tick with an
`ExecutionAnchorAdopted` event and a directive notice, copies it into
children, and refuses a tick from the wrong tree. Second, the only thing
propagating `KOTO_TICK_SESSION` to a command today is inheritance
(`src/engine/reentrancy.rs`), so clearing the environment would silently
disable the guard against nested ticks (koto#208). Third, the header has two
writers -- `init_child_core` and `init_inline_into_session` in
`src/cli/init_child.rs` -- while attach (`src/cli/init_entry.rs`) writes no
header and checks template, origin and variables before any write.

## Decision Drivers

- **Close the channel for the accidental case first.** Determinism across
  shells is the stronger argument; any posture that leaves some sessions on
  the caller's environment by default leaves the accidental case open there.
- **No secret value in session state.** Values are recorded only for
  variables that can't hold a secret; everything else is a name.
- **The agent must not be able to take the escape hatch.** Whatever can widen
  a session's environment must be decided before the agent is ticking it --
  at authoring time or at creation -- never at tick time.
- **Nothing in flight strands.** shirabe's runs, created by an older koto,
  must keep advancing through the upgrade.
- **Reuse the anchor's shape.** Header field plus log event, adoption with a
  notice, inheritance by children, refusal on attach. Users and maintainers
  already know it.
- **Additive on disk.** Older koto builds keep reading newer sessions and
  templates; no schema bump.
- **Cheap on the hot path.** Building the environment reads the header koto
  already loaded and the process environment; no extra spawn or filesystem
  search per command.
- **Testable reproductions.** The `PATH` prefix, the exported function on a
  bash `sh`, the accidental shim and the `.gitconfig` alias must each be a
  test that fails before the change and passes after.

## Considered Options

### Decision 1: default posture

Every template in use today was written assuming the caller's environment, so
the first question is who gets the fixed environment. The answer decides
whether the accidental case is closed everywhere or only where someone asked,
and whether the engine's guarantee reads the same in every session log.

The question has a second half the posture must answer: what does a template
author do whose gate legitimately needs a variable the caller sets -- a token,
an enterprise host, a harness's stub variable?

Key assumptions:

- Projects whose tools depend on a dev-shell environment (direnv, `nix
  develop`) are rare among koto-run workflows at release; none of shirabe's
  templates runs a project's test suite from a gate.
- An agent can write or edit a template on disk. That gives it the power to
  remove a gate outright, which is strictly more than any field inside the
  template could give it.

#### Chosen: on for every session, no opt-out

Every session koto creates, and every older session once it adopts a record,
runs its commands in the cleared environment described in Decision 3. No
template field, `koto init` flag or environment variable restores the caller's
whole environment.

A template author whose gate needs a caller-set variable declares the name in
the template (`pass_env:`, Decision 4). Sessions created from it pass that
name; koto reads its value live at each run, never writes it down, and lets it
change between ticks. The declaration is the author's statement that the gate
depends on a caller-controlled input, and it is visible in the session's
record event. When the need belongs to one installation rather than to the
template -- a GitHub Enterprise user's `GH_HOST`, a harness's `GH_DB` -- the
creator of the session adds the name at `koto init`. No route can add a name
on the refused list.

The only real cost is the dev-shell gap: a project whose tools only resolve
inside direnv or a nix shell must list the names those tools need, or record
the session from inside that shell. It fails loudly (exit 127 with a note, or a
tool's own error) rather than silently, and the party who can fix it is the
session's creator, which is where the fix lives.

#### Alternatives Considered

**On by default, with an authoring-time template opt-out.** A frontmatter
field, compiled into the template and covered by its hash, so the ticking
agent can't set it. Rejected, though not because an agent can reach it: an
agent that can edit a template can delete the gate instead. It loses on three
other counts. A plugin author can't know whether a given user's project needs
the caller's environment, so the field is either never set or set blindly,
and set blindly it reopens the accidental case for every user of that
template. Even narrowed to widen only the pass list while keeping the fixed
values and refusals, it lets names nobody chose -- `GIT_DIR`, `GH_REPO`,
`NODE_OPTIONS`, `PYTHONPATH`, `JAVA_TOOL_OPTIONS` -- flow live into that
template's `overridable: false` gates at tick time, which is the bypass this
work closes; the set is open-ended, so no closed refusal list fixes it. And the
change is asymmetric over time: a widening added after this ships breaks
nothing, while an opt-out shipped now and later withdrawn breaks every
template that used it.

**Template opt-in**, including the variant where any `overridable: false`
gate counts as opting in. Rejected: the accidental case stays open wherever
no author opted in, and the determinism stories belong to users who don't
control the template (an eval author, a developer resuming a session). The
implicit variant ties determinism to a security annotation, so a template
silently returns to the caller's environment when its last strict gate is
removed.

### Decision 2: older sessions and what gets recorded

A session created by an older koto has no record. Whatever it records instead
is permanent, because nothing rewrites a record (R16), and the first tick under
the new koto may come from a poisoned environment. The same is true at `koto
init`, which the agent often runs itself. So the question is two-sided: what
an unrecorded session does on its first tick, and what hygiene the recorded
`PATH` gets at every point it's recorded.

Research separated two meanings of "poison". A directory the caller can write
to can't be told apart from a legitimate one: the agent runs as the user, and
`~/.local/bin`, `~/.cargo/bin`, `~/.tsuku/bin`, Homebrew's prefix and every
eval fixture directory are user-writable by design. On one sampled developer
`PATH`, 11 of the roughly 30 existing directories were user-writable and 12
weren't owned by root. Resolution into the execution anchor is different.
Commands start at the anchor, so an empty entry, `.`, or any relative entry
resolves inside the repository the agent edits and a reviewer checks out; a
`gh` committed to a branch becomes the `gh` every gate runs. An experiment
confirmed that `.`, a leading `:` and a trailing `:` all resolve a file in the
working directory. That case is visible from the string alone, costs nothing
to detect, and legitimate setups rarely depend on it.

Key assumptions:

- The anchor check runs before environment adoption in the same tick, so the
  anchor is known when the record is made.
- Few gates rely on a relative `PATH` entry to find a tool by bare name. One
  that does gets exit 127 with the R13 note until it spells the path.
- shirabe's harnesses put stub directories on `PATH` as absolute paths outside
  the anchor (confirmed for its engine tests and eval runner).

#### Chosen: adopt on first tick, with one `PATH` normalization at every recording point

**Adoption.** On its first tick under the new koto, a session with no record
records the ticking process's fixed variables (Decision 4; `PATH` normalized
as below) and the default live names. It appends the record event,
rewrites the header atomically, and splices a one-time notice onto the
directive -- the anchor's order, so a crash repeats a visible adoption instead
of leaving a silent one. The notice shows the recorded `PATH`, `HOME` and
`XDG_CONFIG_HOME` (none is a secret), lists dropped and flagged entries, says
that variables outside the default list no longer reach commands, and says the
remedy for a wrong record is a new session. Adoption never refuses and reads
no caller-added names.

**Normalization**, one function used by both header writers, by adoption, and
on the caller's `PATH` before the attach comparison:

- **Dropped:** empty entries (leading, trailing or doubled `:`) and every
  relative entry, including `.`, `bin` and `node_modules/.bin`. Decided from
  the string, with no filesystem access. A relative entry names no fixed
  location, so keeping it would contradict the point of fixing `PATH`.
- **Kept and flagged:** absolute entries that resolve inside the execution
  anchor (canonicalized at record time when the directory exists, compared
  after lexically resolving `.` and `..` when it doesn't). These are usually
  deliberate per-project setups (direnv's `PATH_add`, mise), and dropping them
  would change which tool a gate runs. A `HOME` or `XDG_CONFIG_HOME` inside
  the anchor is flagged the same way.
- **Everything else** is kept verbatim, in order. No writability, ownership or
  existence check. A literal `~` isn't expanded by `execvp` or the shell's
  lookup, so an entry starting with `~` is relative and dropped.
- **Nothing left:** a `PATH` with no surviving entry is recorded as unset, and
  a relative `HOME` or `XDG_CONFIG_HOME` is recorded as unset. At run time an
  unset `PATH` becomes `/usr/bin:/bin` (Decision 3), never the shell's
  built-in default.

The anchor-inside flags are computed against the anchor at record time; a
later `koto session rebind` doesn't recompute them, and the adoption notice
and docs say so.

The `koto init` response, the adoption notice and the record event list
dropped and flagged entries separately, each with its reason. Nothing is
refused, at init or at adoption. Children copy the parent's record without
re-normalizing.

What adoption blesses, stated plainly: a poisoned absolute directory outside
the anchor, present in the environment of an older session's first tick, is
recorded and stays. The window is one tick per pre-existing session. It is
visible in the notice and the log, and the remedy is a new session. The same
holds for `koto init` and is the same limit the PRD states for the environment
at creation: whoever creates a session chooses its tools.

This normalization amends the PRD's R1 ("records the values ... as they are")
for `PATH` only: the recorded value is the normalized one.

#### Alternatives Considered

**Plain adoption**, recording `PATH` verbatim with a notice. Rejected because
it freezes resolution into the repository for the session's life, and "add
hygiene later" doesn't help sessions recorded in the meantime, since records
are never rewritten. It remains the fallback if normalization proves wrong in
practice: verbatim record plus the same flags.

**Permission hygiene**, refusing or warning on caller-writable or non-root
directories and on relative or empty entries. The writability and ownership
part was rejected: it can't tell `~/.cargo/bin`, Homebrew or an eval fixture
from an attacker's directory and would fire on nearly every real `PATH`. Its
relative and empty part survives in the chosen option, as a drop rather than a
warning, because a warning leaves the entry recorded.

**Keep inheriting** until the session finishes, as the origin record does for
older sessions. Rejected: every tick of a run that may last weeks stays open to
the whole environment, the log never carries a record, and it breaks parity
with the anchor users already know.

**Refuse until an explicit bind.** Rejected: it strands every run in flight at
the upgrade, and the agent would run the bind itself, so it records whatever
environment the agent has then. It moves the poison without closing it.

### Decision 3: record shape, storage and propagation

The record must live where the header's other creation-time facts live, be
readable in the log (R7), survive every header rewrite (rebind, rename,
recover, claim), reach children, and get from the header to
`run_shell_command` without each call site rebuilding it.

Key assumptions:

- `koto session start` already fails to record an anchor and an origin; only
  the environment record is added to it here.
- No header or event type uses `deny_unknown_fields`, so an older koto reads a
  newer header and skips an unknown event.

#### Chosen: a nested header struct, a dedicated event, and a runtime `CommandEnv`

`StateFileHeader.command_environment: Option<CommandEnvironment>`, following
the `origin: Option<SessionOrigin>` pattern (`#[serde(default,
skip_serializing_if = "Option::is_none")]`):

```rust
pub struct CommandEnvironment {
    /// Fixed variables, by name: PATH (normalized), HOME, XDG_CONFIG_HOME and
    /// the other fixed names of Decision 4. A name absent from the map was
    /// unset at creation and stays unset.
    pub fixed: BTreeMap<String, String>,
    /// The default live names of the koto release that made the record.
    pub pass: Vec<String>,
    /// The creator's caller-added names (flag and KOTO_PASS_ENV), sorted.
    pub added: Vec<String>,
}
```

The template's own `pass_env:` names are deliberately not stored. A session's
compiled template can't change for its lifetime, so each tick takes the live
names as `pass` ∪ the template's `pass_env` ∪ `added`. That keeps a child
correct when its template differs from its parent's -- a batch child copies
the parent's `fixed`, `pass` and `added` and still gets its own template's
names -- and it means a record never needs rewriting to follow a template.
`added` is kept apart from `pass` because the attach comparison (R15) is
against what the creator added, which an attaching caller can supply again.
Storing the default list rather than re-reading it each tick keeps an older
session's environment stable across a koto upgrade that changes the default.

A new `EventPayload::EnvironmentRecorded { environment, template, dropped,
flagged, adopted }` (wire name `environment_recorded`) is appended once at
creation, on every creation path including children, and once at adoption.
`template` lists the template's `pass_env:` names at that moment, so the log
alone shows every live name; `dropped` and `flagged` carry the
normalization's findings so a log reader doesn't diff strings. The header is
authoritative for what commands run with; the event is its audit copy, and
the two are written in the anchor's order (event, then header) so they can't
disagree except after a crash between the two, which adoption repeats. When
the environment is built, names on the refused list are filtered out again,
so a record edited by hand can't reintroduce one. It's a dedicated event, not an extension of `WorkflowInitialized`,
because several child paths don't emit that event the same way and because
adoption needs the same record later in the log. Unlike the anchor, the record
is emitted at creation too: R7 requires the log alone to say what a session's
commands run with, and cloud sync copies logs without re-validating headers.

**Creation paths.** `koto init` reaches a header writer three ways:
`handle_init` (plain and `--parent`), `init_entry::run` (entry flags) and
`handle_init_inline` (`--from-stdin`). The first two end in `init_child_core`,
the third in `init_inline_into_session`. Both writers take an
`EnvironmentSource`: either `Record { added }`, built from the invoking
process (normalize, read the fixed names, the default list, the caller's
names), or `Inherit(parent)`. `--pass-env` and `KOTO_PASS_ENV` are accepted
on every top-level form, including `--from-stdin`. On `--parent` the flag is
refused (`invalid_pass_env`: a child takes its parent's names) and
`KOTO_PASS_ENV` is not read, so a value exported in a shell profile can't make
child creation fail.

**Children** -- batch spawn, retry, skip marker, `koto init --parent`, `koto
session start` -- use `Inherit(parent)` through a
`resolve_command_environment` helper beside `resolve_execution_dir`. The
spawning process's environment is never consulted: a child is created inside
the parent's tick, whose process environment belongs to whoever ticked the
parent. A parent without a record (an older session) can only be spawning
children during its own tick, which adopts a record first (below), so batch,
retry and skip-marker children always find one. For `koto init --parent` and
`koto session start` against an unrecorded parent, the child records from its
own invoking process instead, exactly as a new session would.

**Attach**, beside `check_origin` in `src/cli/init_entry.rs` and before any
write, compares the normalized caller `PATH`, `HOME` and `XDG_CONFIG_HOME`
with the record (R14) and, when the caller supplied names by either route, the
caller's names with `added` (R15). A session with no record is not compared.
The refusal prints the variable and both values to the caller; the leg record
carries only `environment-mismatch:<VARIABLE>`, so a caller's `PATH` or `HOME`
isn't copied into a request store other sessions read.

**Adoption** runs in `koto next` after the reentrancy check, the anchor check
and the dispatch-epoch fence, and under the state-file lock, re-reading the
header to confirm no record exists before writing one. A displaced writer
therefore can't set the permanent record, and two racing ticks can't both
adopt.

**At run time**, a `CommandEnv` -- ordered name-value pairs, never serialized
-- is built once per tick: each fixed variable present in the record; each
live name the ticking process has set, with its value; `KOTO_TICK_SESSION`;
and `KOTO_SESSIONS_BASE`. When the record has no `PATH` (every entry was
dropped, or it was unset), `PATH` is set to `/usr/bin:/bin`, never left to the
shell's built-in default, which in upstream bash ends in `.`. The type lives
in `src/action.rs`, which imports nothing else from the crate; its builder
lives in the engine. `run_shell_command` takes `&CommandEnv`, calls
`env_clear()`, applies it, sets standard input to null, and spawns
`/bin/sh`; a failure to spawn `/bin/sh` is reported as the spawn failure it
is, with no fallback to a `PATH` search. `evaluate_gates_with_request_store`
takes the `CommandEnv` as a parameter, so the compiler finds every caller --
`TickGates`, the polling loop's own gate closure, and the `--to` guard -- and
the one-shot and polled default-action sites pass the same value.

**The exit-127 note** is not added to gate evidence. `command_gate_result`
keeps the failing evidence byte-identical so `when:` conditions and override
records keep matching. Instead the tick collects "a command exited 127" as a
flag and splices the note onto the `koto next` response with
`with_directive_prefix`, and for a default action also appends it to the
captured stderr that the action-failure path already shows.

#### Alternatives Considered

**Flat header fields and an extended `WorkflowInitialized`.** Rejected: it
breaks the one-optional-struct convention the origin record set, ties an
unrelated payload to every reader of the initialization event, and leaves
adoption without a place to record.

**Rebuilding the environment at each call site.** Rejected: three
independent builders (gate, one-shot action, polled action) can drift in one
tick, and each would have to inject `KOTO_TICK_SESSION` exactly right, where a
miss silently disables the nested-tick refusal.

### Decision 4: how names are added, the defaults, and the shell

The pass list needs a template surface (R4), two creator surfaces (R5: a flag,
and something a harness can set without editing a skill's `koto init` line),
an exact default list (R3), a closed refusal list (R6), and a shell path that
no `PATH` decides (R11).

Key assumptions:

- An older koto that meets the new template key ignores it, as it ignores any
  unknown top-level frontmatter key today; a gate that then misses a variable
  fails loudly.
- `AWS_*` and `KOTO_DECIDER*` are read by koto's own in-process
  configuration. A command that runs a nested `koto` against the cloud
  backend does need `AWS_*` to sync; such a user adds those names with
  `KOTO_PASS_ENV` (they are credentials, so they stay live). No shipped
  template does this, so they aren't defaults.

#### Chosen: `pass_env:`, `--pass-env`, `KOTO_PASS_ENV`, an explicit default list, `/bin/sh`

- **Template:** a flat top-level frontmatter list, `pass_env: [NAME, ...]`,
  beside `variables:` and `states:`. It compiles to `CompiledTemplate.pass_env:
  Vec<String>` with `skip_serializing_if = "Vec::is_empty"`, so a template that
  declares none keeps its compiled JSON and its hash. It works on the
  `--from-stdin` path, which compiles the same way.
- **Flag:** repeatable `koto init --pass-env NAME`, the shape of `--var`,
  accepted with `--vars-file`, `--attach-live`, `--replace-terminal` and
  `--koto-leg`.
- **Init-time variable:** `KOTO_PASS_ENV=NAME1,NAME2`, comma-separated, read
  only in the `koto init` entry path (creation and attach), never on a tick.
  The flag's names and the variable's names are unioned into the creator's
  `added`.
- **Validation:** every added name matches `^[A-Za-z_][A-Za-z0-9_]*$` and is not
  on the refused list; one validator serves the compiler and `koto init`. A bad
  template name is a compile error; a bad caller name is a refusal
  (`invalid_pass_env`, exit 2) with no session created.
- **Fixed names (values recorded, exact):** `PATH`, `HOME`,
  `XDG_CONFIG_HOME`, `USER`, `LOGNAME`, `LANG`, `LC_ALL`, `LC_CTYPE`,
  `LC_COLLATE`, `LC_MESSAGES`, `LC_NUMERIC`, `LC_TIME`, `LC_MONETARY`,
  `LANGUAGE`, `TZ`, `TMPDIR`, `TERM`, `NO_COLOR`, `CI`, `XDG_CACHE_HOME`,
  `SSL_CERT_FILE`, `SSL_CERT_DIR`. A path-valued one (`HOME`,
  `XDG_CONFIG_HOME`, `TMPDIR`, `XDG_CACHE_HOME`, `SSL_CERT_FILE`,
  `SSL_CERT_DIR`) whose value is relative is recorded as unset, as `PATH`'s
  relative entries are dropped. Each locates a tool, its configuration, a cache or a
  certificate store, or sets presentation, and none holds a secret; a live
  value of any of them could change what a command reads (a prepared Go test
  cache under `XDG_CACHE_HOME`, a forged CA bundle behind a proxy, a locale
  whose files the caller wrote).
- **Default live names (exact):** `SSH_AUTH_SOCK`, `XDG_RUNTIME_DIR`,
  `DBUS_SESSION_BUS_ADDRESS`, `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN`,
  `GITHUB_ENTERPRISE_TOKEN`, `HTTP_PROXY`, `http_proxy`, `HTTPS_PROXY`,
  `https_proxy`, `NO_PROXY`, `no_proxy`, `ALL_PROXY`, `all_proxy`. These can
  carry a credential (a proxy URL can embed one) or name a socket that exists
  only while the caller's session does. Not included: `GH_REPO`, `GH_HOST`,
  any `GIT_*`, `COLUMNS`, `AWS_*`, `KOTO_DECIDER*`.
- **Refused list (closed for this release):** every fixed name; `BASH_ENV`,
  `ENV`, `BASH_FUNC_*`, `SHELLOPTS`, `BASHOPTS`, `PS4`, `CDPATH`; `LD_*`,
  `DYLD_*`, `GCONV_PATH`, `LOCPATH`, `NLSPATH`, `TZDIR`; every `GIT_*` name
  except `GIT_CEILING_DIRECTORIES`, `GIT_TERMINAL_PROMPT`, `GIT_AUTHOR_NAME`,
  `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_NAME` and `GIT_COMMITTER_EMAIL` (git has
  dozens of variables that redirect its config, index, object store, TLS
  checks or helper programs, and a prefix rule doesn't go stale when git adds
  one); `SSH_ASKPASS`; `GH_CONFIG_DIR`, `GH_EDITOR`, `GH_PAGER`, `GH_BROWSER`;
  `EDITOR`, `VISUAL`, `PAGER`, `BROWSER`, `XDG_DATA_HOME` (gh extensions live
  under it); every `KOTO_*` name (koto sets `KOTO_TICK_SESSION` and
  `KOTO_SESSIONS_BASE` itself, and its other variables configure koto, not a
  command). Matching is case-sensitive, as the environment is.
- **A pasted value is not a name.** A caller-added item equal to the value of
  any variable set in the invoking process is refused: a GitHub token or an
  AWS key id matches the name pattern, so `--pass-env "$GH_TOKEN"` would
  otherwise write the token into the header, the event and any cloud copy.
  `invalid_pass_env` reports a rejected item by its position and source (the
  flag, or `KOTO_PASS_ENV`), never by its text.
- **`KOTO_PASS_ENV` parsing:** split on commas, trim surrounding whitespace,
  ignore empty items; each item then goes through the same validation as the
  flag.
- **Set by koto on every command:** `KOTO_TICK_SESSION` (the ticked session)
  and `KOTO_SESSIONS_BASE` (the base of the store the tick is operating on), so
  a nested `koto` finds the same store and a nested `koto next` is still
  refused. Reading `KOTO_SESSIONS_BASE` live instead would let a caller point a
  gate's `koto context get` at a different store.
- **Shell:** `/bin/sh`, by absolute path, on every platform koto ships (Linux
  and macOS; NixOS provides `/bin/sh`).

#### Alternatives Considered

**A nested `environment: {pass: [...]}` key.** Rejected: it adds a struct
that needs its own unknown-field policy, and with no opt-out there is nothing
to nest beside `pass`.

**Per-state name lists.** Rejected: the need is the template's, and building
the environment per state adds cost no requirement asks for.

**Rejecting unknown top-level keys, so an older koto refuses a template that
uses `pass_env:`.** Rejected: it would turn every new-template, old-koto
pairing into a hard failure, against the additive rule, when the failure it
prevents already surfaces as a failing command.

**Resolving `sh` through the recorded `PATH`.** Rejected: a recorded `PATH`
that lacks `sh`, or leads with a stand-in `sh`, would decide how every command
is parsed. A fixed path removes the question.

## Decision Outcome

Every session carries a record of the environment its commands run with, made
once and never rewritten. At `koto init` -- and in every child, copied from its
parent -- koto records the values of the fixed variables (`PATH` with empty
and relative entries dropped, `HOME`, `XDG_CONFIG_HOME`, and the other
non-secret variables that locate a tool's inputs), the release's default live
names, and whatever names the creator added with `--pass-env` or
`KOTO_PASS_ENV`. The record goes into a new header field and an
`environment_recorded` event, which also lists dropped and flagged `PATH`
entries. koto writes no value of a live name anywhere.

Every command a tick runs -- gate, one-shot action, each polling attempt --
starts as `/bin/sh -c`, with empty standard input, in an environment built
once per tick: the recorded fixed values (a variable unset at creation stays
unset, and an unset `PATH` becomes `/usr/bin:/bin`), the live value of each
default, template-declared and creator-added name that the ticking process
has set, and `KOTO_TICK_SESSION` and `KOTO_SESSIONS_BASE` set by koto. Nothing
else from the ticking process reaches the command, which removes exported
functions, `BASH_ENV`, `ENV`, loader variables and git and gh config and
program redirection in one step. An exit 127 adds a note to the response
naming the recorded `PATH` and saying it can't be changed; gate evidence is
unchanged.

`koto init --attach-live` compares the caller's normalized `PATH`, `HOME` and
`XDG_CONFIG_HOME`, and any names it adds, with the record, and refuses a
difference as `environment_mismatch` (exit 2), recorded on the leg under
`--koto-leg` by variable name only. `koto session rebind` moves only the anchor. A session created
by an older koto adopts a record on its first tick and says so once; attach
doesn't refuse it before then. There is no opt-out.

The combination works because each piece covers a gap another leaves. Fixed
values close the channels that don't hold secrets; names read live keep the
ones that do out of the log; the refused list stops a template or caller from
reopening a closed channel by name; normalization stops the recorded `PATH`
from pointing into the tree under review; and attach refusal plus
never-rewritten records mean the only way to change a session's environment is
to start a new session, which is visible.

## Solution Architecture

### Components

| Component | Where | Change |
|---|---|---|
| `CommandEnvironment` | `src/engine/types.rs` | New struct; `StateFileHeader.command_environment`; `EnvironmentRecorded` event and wire name; its entry in `docs/reference/session-feed.md` |
| `CommandEnv` | `src/action.rs` | Runtime type: ordered name-value pairs, never serialized |
| Environment module | new `src/engine/command_env.rs` | Fixed, default-live and refused lists; name validation; `PATH` normalization; record building; `CommandEnv` builder |
| Template compiler | `src/template/compile.rs`, `types.rs` | `pass_env:` parsing, validation, compiled field |
| Header writers | `src/cli/init_child.rs` | `EnvironmentSource` into `init_child_core` and `init_inline_into_session`; `resolve_command_environment` for children |
| Entry flags | `src/cli/mod.rs`, `src/cli/init_entry.rs` | `--pass-env` on all top-level forms, refused with `--parent`; `KOTO_PASS_ENV`; `invalid_pass_env`; `environment_mismatch` beside `check_origin` |
| Session start | `src/cli/session.rs` | Inherit from the parent, or record from the process when the parent has none |
| Tick | `src/cli/mod.rs` | Adoption after the anchor check and epoch fence, under the lock; build `CommandEnv` once; pass it to every gate evaluation and both action sites; the adoption and exit-127 notes |
| Runner | `src/action.rs` | `run_shell_command(command, dir, timeout, &CommandEnv)`: `env_clear`, apply, null stdin, `/bin/sh`, no fallback |
| Gate evaluation | `src/gate.rs` | `evaluate_gates_with_request_store` and `evaluate_command_gate` take `&CommandEnv`; evidence shape unchanged |

### Data flow

```
koto init (top-level form)
  compile template (pass_env names are validated, not recorded)
  read --pass-env + KOTO_PASS_ENV -> added   (validate; refuse bad names)
  normalize PATH; read the other fixed names
  record = {fixed, pass = release default live names, added}
  write header (command_environment) + EnvironmentRecorded
  print dropped/flagged entries in the init response

child creation (batch, retry, skip marker, --parent, session start)
  record = parent's record (copied), or from the process if the parent has none

koto init --attach-live
  ... template, origin checks ...
  if record present: compare normalized PATH, HOME, XDG_CONFIG_HOME, then added
     differ -> environment_mismatch (exit 2); leg gets the variable name only
  ... variables, leg bind, rebind vars ...

koto next
  reentrancy check -> anchor check -> epoch fence
  record absent -> under lock, re-read; build from process (normalized,
                   default names); append event; rewrite header; remember notice
  CommandEnv = fixed values (PATH defaulted if unset)
             + live values of (pass ∪ template pass_env ∪ added)
             + KOTO_TICK_SESSION + KOTO_SESSIONS_BASE
  every gate / action / polling attempt -> run_shell_command(.., &CommandEnv)
  any exit 127 -> note spliced onto the response; action stderr gets it too
  response carries the adoption notice once
```

### The reproductions, as tests

The PRD's acceptance criteria become integration tests; the four that define
the channel:

- **`PATH` prefix.** A template whose only gate is `overridable: false` and
  runs `test "$(whoami)" = nobody`. Create the session normally; tick with a
  directory holding a fake `whoami` first on `PATH`. Before: the session
  reaches `passed`. After: it stays at `check`.
- **Exported function.** The same template, in a container whose `/bin/sh` is
  bash (Debian with `/bin/sh` linked to bash), run through the rootless daemon.
  Tick with `whoami() { echo nobody; }; export -f whoami`. Before: `passed`.
  After: `check`. On dash the test asserts `check` both before and after, which
  documents why the container is needed.
- **Accidental shim.** A session created with a stand-in `gh` first on `PATH`
  reads it from a tick whose `PATH` lacks it; a session created without it
  doesn't read it from a tick whose `PATH` has it.
- **`.gitconfig` alias (the widened scope).** A gate runs `git koto-check`,
  where no alias exists. Tick with `HOME` pointing at a directory whose
  `.gitconfig` sets `alias.koto-check = !true` (and, separately, with
  `XDG_CONFIG_HOME` pointing at one whose `git/config` does). Before: the gate
  passes. After: the recorded `HOME` is used, the alias doesn't exist, the gate
  fails. A variant with `core.hooksPath` on a gate that commits covers hooks.

Two further tests pin the secrecy promise: with `GH_TOKEN`, `GITHUB_TOKEN` and
variables named `*_TOKEN`, `*_SECRET`, `*_KEY`, `*_PASSWORD` set to unique
markers at `koto init` and at a tick that runs a gate and an action, no file
under the session directory contains a marker, and every `environment_recorded`
event carries values only for the fixed names of Decision 4. A third passes a
token's value as `--pass-env` and asserts the refusal, the absence of the value
from its output, and that no session was created.

### What a shirabe session sees on upgrade

shirabe's templates call plugin scripts by absolute path (`{{PLUGIN_ROOT}}`)
and `gh`, `jq`, `git`, `koto` and `shirabe` by name, and read no environment
variable directly (its CI rejects `$NAME` in a gate command). So:

- **A run in flight** created by an older koto adopts a record on its next
  tick: the agent's fixed variables at that moment and the default live names.
  The response carries the one-time notice showing what was recorded. Tools
  keep resolving because the agent's `PATH` is the one they resolved from
  before. `gh` keeps its token (`GH_TOKEN`/`GITHUB_TOKEN`, or the keyring via
  `DBUS_SESSION_BUS_ADDRESS` and `XDG_RUNTIME_DIR`), `git fetch` keeps
  `SSH_AUTH_SOCK` and the proxy variables, nested `koto` calls reach the same
  store, and the scripts that read `KOTO_TICK_SESSION` still find it.
- **A new run** records at its `koto init`, which shirabe's `koto-open.sh`
  runs from the agent's shell; the same variables flow.
- **`--attach-live` on resume** compares the agent's current `PATH`, `HOME`
  and `XDG_CONFIG_HOME` with the record. The agent's shell is normally stable
  between invocations, so this passes. It is refused if the user changed tool
  managers or resumed from a different machine sharing a store; the refusal
  names the variable and both values, and the leg records the variable name.
- **A GitHub Enterprise user** whose `gh` depends on `GH_HOST` must add it.
  `export KOTO_PASS_ENV=GH_HOST` in a shell profile does that for new
  sessions; for a session created before the export, an attach that now adds
  `GH_HOST` is refused as a pass-list mismatch, so the user either unsets it
  for that resume or starts a new session.

What shirabe must change (proposed here; shirabe does the work):

1. **Harness variables.** Its engine tests and evals must add the names their
   stand-in tools read: `GH_DB`, `GH_FIX`, `GH_BOARD_DIR`, `COORD_TESTDATA`,
   `BT_STATE`, `MERGE_CONFIRM_WAIT_SECS`, `GIT_CEILING_DIRECTORIES` in the
   engine tests; `EVAL_SCENARIO`, `EVAL_SCENARIO_DIR`, `GH_CALL_LOG` in the
   passthrough evals. The least intrusive form is to export `KOTO_PASS_ENV`
   in each harness before its first `koto init`, which reaches the `koto init`
   that `koto-open.sh` builds without changing it. Their values may keep
   changing between ticks.
2. **Harnesses that change a fixed variable between init and a tick** --
   `TMPDIR`, `HOME`, `LANG` -- must set it before `koto init` instead. The
   engine tests studied already do.
3. **The minimum koto.** Once shirabe's harnesses set `KOTO_PASS_ENV`, they
   work with both older and newer koto (older koto ignores it), so the floor
   can move independently.
4. **Nothing in templates.** No template needs `pass_env:` for production use.
   `KOTO_BIN`/`SHIRABE_BIN` overrides used only by tests fall under item 1.

## Implementation Approach

1. **Record, names and creation.** `CommandEnvironment`, the event, the
   environment module (lists, validation, normalization, the `CommandEnv`
   builder), `pass_env:` in the compiler, `--pass-env` and `KOTO_PASS_ENV`,
   `invalid_pass_env`, recording through `EnvironmentSource` in both header
   writers and in children and `koto session start`. Commands still run as
   today; sessions start carrying a record.
2. **Attach and adoption.** `environment_mismatch` beside `check_origin`,
   the leg record, adoption in `koto next` after the epoch fence and under the
   lock, with its notice. After this step every session that ticks has a
   record.
3. **Runner switch, last.** `run_shell_command` takes `&CommandEnv`:
   `env_clear`, null stdin, `/bin/sh`; `evaluate_gates_with_request_store`
   takes it too, so every caller is updated; `KOTO_TICK_SESSION` and
   `KOTO_SESSIONS_BASE` injection; the exit-127 note. Existing tests that call
   `run_shell_command` or build headers get the new argument or field;
   `tests/command_output_limits.rs` sets its `PATH` at init. Doing this last
   means no intermediate commit runs an unrecorded session with an empty
   environment.
4. **Tests.** The reproductions above -- the exported-function one in a
   container on the rootless daemon, plus a cheaper assertion that no
   `BASH_FUNC_` name is present inside a gate -- the secrecy tests, attach and
   adoption, children, refused names, stdin and the empty-`PATH` default.
5. **Documentation.** CHANGELOG under Unreleased with the behaviour change for
   release notes; `docs/reference/error-codes.md`; `docs/reference/session-feed.md`
   for the new event; the default-action guide (which today says a command
   inherits the environment of `koto next`), the template format and session
   lifecycle docs; the koto-user, koto-author and koto-adhoc skills
   (koto-adhoc tells authors to read secrets as `$VAR` at gate time, which
   becomes "declare the name").

The steps land as one pull request, in that commit order.

## Security Considerations

**What this closes.** The environment of a single `koto next` no longer
decides which binaries a session's commands run (`PATH`), which shell
functions they see (`BASH_FUNC_*`), what the shell sources at start-up
(`BASH_ENV`, `ENV`), what the loader injects (`LD_*`, `DYLD_*`,
`GCONV_PATH`), which configuration `git` and `gh` load (`HOME`,
`XDG_CONFIG_HOME`, `GIT_CONFIG*`, `GH_CONFIG_DIR`), which programs they start
as editor, pager, diff, proxy or ssh helper, which repository `git` treats as
current or what index and objects it reads (every `GIT_*` name but a few identity and prompt ones), which cache or CA bundle a tool trusts
(`XDG_CACHE_HOME`, `SSL_CERT_*`), or what a command reads on standard input.
Those were the environment-shaped ways a caller could change a
non-overridable gate's verdict.

**What stays open, stated in the docs (R20).**

- **Files, the session directory, the repository, and the binaries on the
  recorded `PATH`.** An agent with a shell can edit what a gate reads, edit the
  session's log or header, or replace a binary in a directory on the recorded
  `PATH` that it can write. None of that goes through the environment. A
  user-writable directory on the recorded `PATH` (`~/.local/bin`) is the most
  direct of these; koto doesn't judge writability because it can't tell such a
  directory from a legitimate one.
- **Live values of named variables.** They stay caller-controlled by design,
  because they can be secrets. A token for a different account, or a proxy
  that the recorded CA bundle already trusts, still changes what a command
  sees. A template or creator that adds a redirecting name (`GH_REPO`,
  `GH_HOST`) accepts that its value is the caller's.
- **Tool-specific code-running variables** outside the refused list
  (`NODE_OPTIONS`, `PYTHONPATH`, `PERL5OPT`) reach a command only if a template
  or creator names them.
- **Process attributes other than the environment.** The umask, resource
  limits and open file descriptors other than the standard three are still
  inherited from the ticking process.
- **The creator's environment.** Whoever runs `koto init` chooses the tools. A
  poisoned absolute directory at creation, or at an older session's first
  tick, is recorded and stays, visible in the notice and the log.

So the claim is narrower than "one tick's environment can't pass a strict
gate": it can no longer do so through the variables koto controls, and the
remaining ways are listed above.

**Secrets.** koto records values only for the fixed variables, none of which
holds a secret. Live names, including `GH_TOKEN`, appear in the header and the
event as names only. The `environment_mismatch` refusal prints `PATH`, `HOME`
or `XDG_CONFIG_HOME` values to the caller's own terminal; the leg record, which
other sessions read, carries only the variable name. `invalid_pass_env` never
echoes text after an `=`. What a command itself prints is still captured as
evidence, as today -- a gate that runs `env` records its output -- and that is
the command's doing, not koto's; the secrecy tests cover everything koto
writes.

**Nested ticks.** `env_clear` would silently remove `KOTO_TICK_SESSION`,
which is how a nested `koto next` is detected; koto now sets it explicitly on
every command, and a test runs `koto next` from a gate.

**The shell.** `/bin/sh` is started by absolute path on Linux and macOS
(where it is bash in POSIX mode; the cleared environment removes the
exported-function and `BASH_ENV` hooks it would otherwise honour). A spawn
failure is reported, never retried through a `PATH` search.

**Older koto.** An earlier koto ticking a session created by this one ignores
the record and uses the caller's environment. The record doesn't protect a
session from a downgrade.

## Consequences

### Positive

- A session gives the same verdicts from any shell; eval stand-ins and
  developer shims stop leaking between ticks.
- A non-overridable gate can't be passed by changing one tick's environment
  through any variable koto controls; the channels that remain are listed in
  Security Considerations.
- The session log says exactly which tools and configuration its commands used,
  with no secret in it.
- One concept added, in the anchor's familiar shape.

### Negative

- Every template now runs with a restricted environment. A gate that read a
  variable outside the default list breaks until its name is declared or added.
- Projects whose tools live only in a direnv or nix shell must record sessions
  from inside that shell or add names.
- A tool that moves off the recorded `PATH` strands a session's gates until it
  comes back or the session is replaced.
- Gates that relied on `.` or a relative `PATH` entry must spell the path.
- shirabe's harnesses need a one-line change before they run against this
  version.
- Attach from a shell with a different `PATH` is now refused where it used to
  work.
- A caller who changes `TMPDIR`, `LANG` or another fixed variable between
  ticks no longer affects commands; a harness that relied on that must set it
  before `koto init`.
- A nested `koto` against the cloud backend needs its `AWS_*` names added.

### Mitigations

- The CHANGELOG entry and the guides name the default list and both ways to add
  a name; the adoption notice says what changed on the first tick.
- The exit-127 note says the command ran under the recorded `PATH` and what the
  remedy is, so a missing tool isn't a bare "command not found".
- `KOTO_PASS_ENV` in a shell profile covers per-user needs (`GH_HOST`) without
  touching any template.
- `environment_mismatch` names the variable and both values.
