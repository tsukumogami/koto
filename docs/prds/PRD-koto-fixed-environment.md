---
schema: prd/v1
status: In Progress
absorbed: docs/briefs/BRIEF-koto-fixed-environment.md
source_issue: 261
problem: |
  Every command gate and default action runs as `sh -c` with the whole
  environment of the process that called `koto next`. A shim first on `PATH`
  is read silently, an exported shell function replaces a tool where
  `/bin/sh` is bash, and the same session gives different verdicts from
  different shells. The same gap lets a caller pass a gate marked
  `overridable: false` by changing the environment of one tick.
goals: |
  A session's commands resolve tools the way they did when the session
  started, from whichever shell ticks it. Shell-injected behaviour never
  reaches them, a move onto a different `PATH` or `HOME` is refused by name,
  and the variables commands legitimately need keep coming from the live
  environment without any secret being written into session state. Sessions in flight
  when koto is upgraded keep advancing, and shirabe knows what changes for it.
---

# PRD: gates and actions run in an environment fixed at init

## Status

In Progress

Absorbed [BRIEF](docs/briefs/BRIEF-koto-fixed-environment.md); carried in Absorbed Brief.

## Absorbed Brief

**Why this exists.** A koto session binds its template, variables and
starting directory at `koto init` but not the environment its commands run
with, so every gate and default action reads whatever tools and shell state
the ticking process carries. A shim first on `PATH` is read silently, an
exported function replaces a tool where `/bin/sh` is bash, and one session
gives different verdicts from different shells. The same gap lets a caller
pass a gate marked `overridable: false` by changing the environment of one
tick. Commands still need part of the environment, much of it secret, and
every session in flight was started by a koto that recorded nothing.

**The outcome.** Whoever ticks a session gets the verdict it would give from
any shell. Its commands find the tools they found at start, a shim or
function in the ticking shell changes nothing, a move onto different tools is
refused by name, and credentials still come from the live environment without
being written into the session. A template author can rely on a
non-overridable gate at least as far as the environment goes and knows which
channels stay open; a user upgrading mid-run finds sessions still advance.

**The journeys.** An eval author's stand-in tool stops leaking between ticks;
an agent can't pass a non-overridable gate by prefixing `PATH` or exporting a
function; a developer resuming from a shell whose tools moved gets a named
failure or refusal instead of silent drift; a shirabe user upgrades koto with
runs in flight and they keep advancing; an operator shares a session log
without scrubbing secrets.

**The boundary.** In: the environment of every command koto runs for a
session, what the session records at init, the variables read live, removing
shell-injected behaviour, attach and rebind, older sessions, and stating the
limit. Out: file, session-directory, repository and binary tampering;
`koto next --to` and overrides; sandboxing; and changes to shirabe itself.

## Problem Statement

A koto session binds its template, its variables and the directory its
commands start in at `koto init`. It doesn't bind the environment those
commands run with. `run_shell_command` starts every command gate and every
default action -- one-shot or polled -- as `sh -c <command>` with the full
environment of whichever process called `koto next`. What a check reads
therefore depends on the shell that ticked it.

The accidental case is the common one. A shell with a shim first on `PATH`
makes every gate in that tick read the shim: a stand-in `gh` in an eval
fixture, a wrapper installed last week, a tool manager that rewrites `PATH` per
directory. Nothing reports it. Where `/bin/sh` is bash (macOS, Fedora), an
exported shell function replaces a tool inside every command koto runs, with no
file on disk. The same session and template give one verdict from one terminal
and another from the next. That's a determinism defect with no bad intent
anywhere in it.

The deliberate case follows. A template marks a gate `overridable: false` so
that neither evidence nor an override record gets past it, yet a caller who
prefixes `PATH` for a single `koto next` changes what the gate's command sees.
Both reproductions are confirmed on koto 0.13.0 and recorded in koto issue #261:
a `PATH` prefix passes a non-overridable gate on any host, and an exported
function passes it where `sh` is bash (dash drops `BASH_FUNC_*` entries, so it
fails there).

Three constraints shape any fix. Commands legitimately need part of the
environment: `gh` needs a token or a keyring, `git` needs `HOME`, a gate that
calls `koto` needs the same session store. Many of those values are secrets,
and a session's log is plain, append-only and readable, so the fix can't copy
the environment into the session. And every session in flight today was
created by a koto that recorded nothing about its environment, driven by
templates written assuming the caller's. shirabe, koto's largest consumer, is
about to move its minimum koto to 0.14.1, so the behaviour change is best
settled before shirabe pins past it.

## Goals

- **Determinism.** A session's gates and actions find the tools they found
  when the session started, whichever shell ticks it. A verdict changes
  because the repository changed, not because the terminal did.
- **A non-overridable gate holds against the environment.** Changing the
  environment of one `koto next` no longer changes a gate's verdict. The
  documentation says which other channels stay open (see Known Limitations).
- **Credentials keep working and stay out of the session.** Variables commands
  need are read live at each run; no value that could hold a secret is ever
  written into session state.
- **No stranded sessions.** A session created by an older koto keeps
  advancing after the upgrade and is told once what changed. shirabe's
  templates keep working, and shirabe's harnesses have a documented way to
  pass the variables their stand-in tools read.

## User Stories

### Story 1: An eval author's stand-in tool stops leaking between ticks

As an author of a koto-backed workflow eval, I want the stand-in `gh` I put on
`PATH` before `koto init` to be what every tick reads, whichever shell ticks
it, so that the eval's result stops depending on who called `koto next`.

### Story 2: An agent can't talk its way past a non-overridable gate

As a template author, I want a gate marked `overridable: false` to give the
same verdict when the ticking caller prefixes `PATH` or exports a shell
function under a tool's name, so that the gate means what I declared.

### Story 3: A developer resumes from a shell whose tools have moved

As a developer resuming a session from a new terminal, I want a command whose
tool isn't on the session's recorded `PATH` to fail with a message naming the
recorded `PATH`, and an attempt to re-attach with a different `PATH` to be
refused by name, so that a change of tools is a decision rather than a side
effect of opening a terminal.

### Story 4: A shirabe user upgrades koto with sessions in flight

As a shirabe user who upgrades koto half-way through a `/work-on` run, I want
the run's next tick to advance, tell me once that the session now runs with a
fixed environment, and keep finding `gh`, `jq`, `git`, `koto` and `shirabe`, so
that the upgrade doesn't strand the run.

### Story 5: An operator shares a session log

As an operator sending a failing session's log to a maintainer, I want the log
to show which `PATH`, `HOME` and variable names the session's commands ran
with, and no other variable values, so that I can share it without scrubbing
secrets first.

### Story 6: A harness passes the variables its stubs read

As the maintainer of a test harness whose stand-in tools read variables such as
`GH_DB` or `EVAL_SCENARIO`, I want to add those names to a session at
`koto init` without editing the template or the `koto init` command line a
skill script builds, so that my stubs keep receiving them and their values can
still change between ticks.

## Requirements

### Functional: what a session records

- **R1. Values fixed at creation.** Every session koto creates records the
  values, as they are in the creating invocation's environment, of the fixed
  variables: `PATH`, `HOME`, `XDG_CONFIG_HOME`, and a documented set of other
  variables that locate tools, configuration or caches and can't hold a
  secret -- user name, locale, time zone, temporary directory, terminal, the
  XDG cache directory, and the CA-bundle paths. A variable unset at creation is
  recorded as unset. The recorded `PATH` has empty and relative entries
  removed, since those resolve inside whatever directory a command starts in;
  the design specifies the normalization. This covers `koto init` in every form
  (plain, `--vars-file`, `--replace-terminal`, `--koto-leg`, `--from-stdin`)
  and `koto session start`.
- **R2. Names, never values, for everything else.** Besides the fixed
  variables, a session records a set of variable names (the pass list) whose
  values are read live. koto writes no value of any pass-list variable, in its
  header, its event log, or any other file in its session directory. (A
  command that prints a value itself has its output recorded as evidence, as
  today; that's the command's doing, not koto's.)
- **R3. The default pass list.** Every session's pass list contains a default
  set of names fixed by the koto release that created it and published in the
  documentation as an exact list. It holds the variables that can carry a
  secret or name a live socket: `gh` and `git` authentication tokens, the
  Linux keyring's D-Bus session and runtime directory, the ssh agent, and HTTP
  proxy settings. It does not include `GH_REPO`, `GH_HOST`, or any `GIT_*`
  name.
- **R4. Templates can add names.** A template can declare additional names its
  commands need. A session created from it has them in its pass list.
- **R5. Callers can add names at creation, two ways.** A caller can add names
  to a session's pass list when creating it (a) on the `koto init` command line
  and (b) through a mechanism read from the creating invocation's environment,
  so a harness can supply names to a `koto init` that a skill script builds
  without editing that script. Both mechanisms are read only when a session is
  created or attached, never on a tick.
- **R6. Names that are never passed.** koto refuses, at template compile time
  and at `koto init`, any attempt to add one of these names to a pass list:
  every fixed variable (R1); the shell start-up and injection names
  `BASH_ENV`, `ENV`, `BASH_FUNC_*`, `SHELLOPTS`, `BASHOPTS`, `PS4`, `CDPATH`;
  the dynamic-loader and locale-path names `LD_*`, `DYLD_*`, `GCONV_PATH`,
  `LOCPATH`, `NLSPATH`, `TZDIR`; the names that make `git`, `gh` or `ssh`
  load configuration, read other data, or run a program of the caller's
  choosing -- every `GIT_*` name except `GIT_CEILING_DIRECTORIES`,
  `GIT_TERMINAL_PROMPT` and the four `GIT_AUTHOR_*`/`GIT_COMMITTER_*` name and
  email variables, plus `SSH_ASKPASS`, `GH_CONFIG_DIR`, `GH_EDITOR`,
  `GH_PAGER`, `GH_BROWSER`, `EDITOR`, `VISUAL`, `PAGER`, `BROWSER`,
  `XDG_DATA_HOME`; and every `KOTO_*` name, since koto sets
  `KOTO_TICK_SESSION` and `KOTO_SESSIONS_BASE` itself. A caller-added item
  equal to the value of a variable set in the creating process is refused
  too, so a pasted token can't be recorded as a name, and a refusal never
  echoes the rejected text. This list is closed for this release. A name must also match `^[A-Za-z_][A-Za-z0-9_]*$`; a pattern or
  wildcard is not a name.
- **R7. The record is readable in the log.** Creating a session, or adopting a
  record under R17, appends one event to its log carrying the fixed variables'
  values and the pass list, so a reader of the log sees what the session's
  commands run with.
- **R8. Children inherit.** A child session -- batch spawn, retry, skip
  marker, `koto init --parent`, `koto session start` under a parent -- copies
  its parent's recorded fixed variables and caller-added names rather than
  recording its own from the ticking process, and adds the names its own
  template declares. A caller can't add names when creating a child.

### Functional: how commands run

- **R9. A cleared environment rebuilt from the record.** Every command koto
  runs for a session -- command gates and default actions, including each
  polling attempt -- starts with an empty environment to which koto adds
  exactly: each fixed variable recorded as set, with its recorded value; each
  pass-list name set in the ticking process, with its live value; and
  `KOTO_TICK_SESSION` set to the name of the session being ticked, and
  `KOTO_SESSIONS_BASE` set to the base of the session store the tick is
  operating on, so a `koto` a command runs reaches the same store. When the
  recorded `PATH` is unset, koto sets a fixed default that contains no
  relative entry. A command's standard input is empty.
- **R10. Shell-injected behaviour doesn't reach commands.** As a consequence
  of R6 and R9, an exported shell function, `BASH_ENV`, `ENV` and every other
  R6 name set in the ticking process are absent from a command's environment.
- **R11. The shell is started by absolute path.** koto starts the shell that
  runs a command by an absolute path, never by searching any `PATH`, so neither
  the ticking process's `PATH` nor the recorded one decides which shell runs.
- **R12. The nested-tick refusal survives.** A `koto next` started from inside
  a command a tick is running is still refused with `nested_invocation`.
- **R13. A missing tool is named.** When a command exits 127, the `koto next`
  response carries a note stating that the command ran under the session's
  recorded `PATH`, giving that value, and saying that the recorded `PATH` can't
  be changed: the tool must be made available on it, or the session replaced
  by a new one. The note is built from the exit status alone; koto doesn't
  search for the tool, and a gate's recorded evidence is unchanged.

### Functional: attach and rebind

- **R14. Attach refuses a different tool location.** `koto init
  --attach-live` on a session whose recorded `PATH` (compared after the same
  normalization), `HOME` or `XDG_CONFIG_HOME` differs from the caller's
  refuses with code `environment_mismatch`, exit 2, printing the variable and
  its recorded and requested values, and changes nothing. Under `--koto-leg`
  the refusal is recorded on the leg with reason
  `environment-mismatch:<VARIABLE>`, naming the variable without its values.
  The other fixed variables are not compared; they simply stay as recorded.
- **R15. Attach doesn't change the pass list.** When an attaching caller adds
  names by either R5 mechanism, the set of names it adds must equal the set
  the session recorded from its creator's R5 input; otherwise attach refuses
  with `environment_mismatch` and reason `environment-mismatch:pass-list`. A
  caller that adds no names is not compared.
- **R16. No verb changes the record.** `koto session rebind` moves the
  execution anchor and leaves the recorded environment unchanged. Nothing
  rewrites a record once written. A session that needs a different fixed
  variable is replaced by a new one.

### Functional: older sessions and templates

- **R17. Older sessions adopt once.** A session with no recorded environment
  -- created by an earlier koto -- records the ticking process's fixed
  variables and the default pass list on its first tick under this version,
  appends the R7 event, and prefixes a one-time notice to that tick's
  directive. Later ticks take the ordinary path.
- **R18. Attach on an older session.** `koto init --attach-live` on a session
  with no recorded environment is not refused under R14 or R15; the next tick
  adopts under R17.
- **R19. No opt-out.** No template field, `koto init` flag, or environment
  variable restores the caller's whole environment for a session's commands.
  R4 and R5 add named variables only.

### Functional: documentation

- **R20. The limit is stated.** The documentation for gates, default actions
  and the session lifecycle states that this closes the environment channel
  only, and names what stays open: an agent that edits the files a gate reads,
  the session directory, or the binaries on the recorded `PATH`; the live
  values of pass-list variables, which remain caller-controlled; and process
  attributes other than the environment (umask, resource limits).
- **R21. The behaviour change is announced.** The CHANGELOG's Unreleased
  section calls out the behaviour change for release notes, names
  `environment_mismatch`, and says what a template or user relying on a
  variable outside the default pass list (including `GH_HOST`) must do.
- **R22. What shirabe must do is written down.** The design records what a
  shirabe session sees on upgrade and what shirabe's harnesses must change;
  the change itself is shirabe's.

### Non-functional

- **R23. Additive on disk.** The record is an additive header field and an
  additive event. `CURRENT_SCHEMA_VERSION` doesn't change, and a state file
  written by this version parses with the header and event types of the
  previous release.

## Acceptance Criteria

### The reproductions

- [ ] With a template whose only gate is `overridable: false` and checks that
      `whoami` prints `nobody`: a session created from an ordinary shell and
      ticked with a directory holding a fake `whoami` prefixed on `PATH` stays
      at its first state.
- [ ] The same session, on a host whose `/bin/sh` is bash (a container with
      `/bin/sh` linked to bash), ticked with an exported function `whoami`
      that prints `nobody`, stays at its first state.
- [ ] A session created with a stand-in tool first on `PATH` reads the
      stand-in on a tick whose `PATH` doesn't carry it, and a session created
      without it doesn't read it on a tick whose `PATH` does.
- [ ] A session ticked with `HOME` pointing at a directory whose `.gitconfig`
      defines an alias that prints a pass sentinel runs its gates with the
      `HOME` recorded at creation, so a gate calling that alias fails; the
      same holds for an `XDG_CONFIG_HOME` whose git config defines it.

### Recording and secrecy

- [ ] After each of plain `koto init`, `--vars-file`, `--replace-terminal`,
      `--koto-leg`, `--from-stdin` and `koto session start`, the session's log
      holds exactly one record event naming the fixed variables' values and
      the pass list, and the header carries the same record.
- [ ] A fresh session's recorded pass list equals the documented default list,
      and contains none of `GH_REPO`, `GH_HOST`, or any `GIT_*` name.
- [ ] With a pass-list variable set to a unique marker value at `koto init`
      and at every tick, no file in the session directory contains the marker
      after a tick that runs a gate, a default action and a polled default
      action.
- [ ] A name declared by the template, a name added on the `koto init` command
      line, and a name added through the environment-read mechanism of R5
      each appear in the recorded pass list, and a gate command sees each
      one's live value.
- [ ] A pass-list value changed between two ticks reaches the second tick's
      command.
- [ ] Each R6 name, and a name failing the name pattern (a leading digit, a
      dash, a `*`), is a compile error when declared by a template and a
      refusal with no session created when added through either R5
      mechanism.
- [ ] With variables named like credentials (`GH_TOKEN`, `GITHUB_TOKEN`, a
      name ending `_TOKEN`, `_SECRET`, `_KEY` or `_PASSWORD`) set to unique
      marker values at `koto init` and at a tick, the session's log and
      header contain none of the markers, and the record event carries values
      only for the documented fixed variables.
- [ ] No template field, `koto init` flag or environment variable documented
      for this feature causes a variable outside the fixed variables, the
      pass list and `KOTO_TICK_SESSION` to reach a command; the feature's
      tests assert this for every such input.
- [ ] A variable set in the ticking process but not on the pass list, and each
      of `BASH_ENV`, `ENV` and an exported function, is absent inside a gate
      command, a default action and a polled default action.
- [ ] A child created by a batch spawn, a retry, a skip marker,
      `koto init --parent` and `koto session start` under a parent each
      carries its parent's record, even when created from a process with a
      different `PATH`.
- [ ] `CURRENT_SCHEMA_VERSION` is unchanged, and a header and a log written by
      this version parse with the previous release's header and event types.

### Running commands

- [ ] Inside a gate command and a default action, `KOTO_TICK_SESSION` equals
      the ticked session's name, and a `koto next` run from there is refused
      with `nested_invocation`.
- [ ] A gate command that calls `koto context get` reaches the same session
      store as the ticking process.
- [ ] A gate whose command isn't found under the recorded `PATH` fails with
      the same evidence shape as before, and the `koto next` response carries
      the R13 note: the recorded `PATH` value and the statement that it can't
      be changed.
- [ ] A gate command reading standard input sees end of file even when
      `koto next` is run with input piped to it.
- [ ] A session created with a `PATH` holding only empty and relative entries
      records `PATH` as unset, and its commands run with koto's fixed default
      `PATH`, so a tool file in the execution anchor is not found by bare
      name.
- [ ] With `TMPDIR`, `XDG_CACHE_HOME` or `SSL_CERT_FILE` changed between
      `koto init` and a tick, a gate command sees the value from `koto init`.
- [ ] A tick whose own `PATH` has no `sh` on it, and a tick whose `PATH` puts a
      decoy `sh` that exits 0 first, both run the session's commands with the
      real shell.

### Attach

- [ ] `koto init --attach-live` with a `PATH`, a `HOME` or an `XDG_CONFIG_HOME`
      different from the recorded one exits 2 with code
      `environment_mismatch`, names the variable and both values, and leaves
      the session's log and header unchanged.
- [ ] Under `--koto-leg`, the same refusal is recorded on the leg with reason
      `environment-mismatch:<VARIABLE>` for whichever variable differs.
- [ ] With matching fixed variables, an attach adding a set of names different
      from the creator's (a superset, a subset, a disjoint set) is refused with
      reason `environment-mismatch:pass-list`; an attach adding the same set,
      or none, attaches.
- [ ] `koto session rebind` moves the anchor and leaves the recorded
      environment unchanged.

### Older sessions

- [ ] A session file with no recorded environment advances on its first tick,
      records the tick's fixed variables and the default pass list, appends
      the record event once, and the response carries the one-time notice;
      the second tick carries no notice.
- [ ] `koto init --attach-live` on such a session, with any `PATH`, attaches.

### Documentation

- [ ] The CHANGELOG's Unreleased section has an entry naming the fixed
      environment, `environment_mismatch`, and how to add a name.
- [ ] `docs/reference/error-codes.md` lists `environment_mismatch` with its
      fields.
- [ ] The default-action and gate guides and the session lifecycle docs list
      the fixed variables, the default pass list, both ways to add names, the
      refused names, and the channels R20 names as still open.
- [ ] The DESIGN has a section stating what a shirabe session sees on upgrade
      and what shirabe's harnesses must change.

## Out of Scope

- **File, session-directory, repository and binary tampering.** None of it
  goes through the environment; this PRD closes one channel and says so.
- **`koto next --to` and overrides.** Other routes around a gate have their
  own treatment.
- **Sandboxing.** No restriction on what a running command reads, writes or
  executes beyond the environment it starts with.
- **Tool-specific variables beyond R6.** Variables that change what some other
  tool does (a language runtime's options, a package manager's config) aren't
  refused; a template or caller that passes one takes that on.
- **Making koto reachable.** A session created by running koto through a path
  not on `PATH` gets no special handling; gates that call `koto` find it on the
  recorded `PATH` or not at all, as today.
- **Changing shirabe.** The design proposes what shirabe must change; shirabe
  makes the change.
- **The batch view and the reserved-actions routing.** Separate open issues
  touch those engine paths; this work doesn't.

## Decisions and Trade-offs

### D1. On by default, with no opt-out

Chosen over a template opt-in and over a per-invocation escape hatch. The
brief's two open questions both land here and in D4. An opt-in leaves the
accidental case -- the stronger argument -- unfixed wherever an author didn't
opt in, and makes a non-overridable gate's strength depend on a second
declaration. An opt-out any caller can reach is the bypass under another name.
The compatibility cost is carried by the default pass list (R3) and the two
ways to add names (R4, R5), which research showed cover shirabe's templates and
harnesses.

### D2. Values that locate tools are fixed; names that can hold secrets are read live

koto issue #261 proposed fixing `PATH` alone. Research showed that a live
`HOME` is as strong a channel as a live `PATH` for the gates that matter: `git`
and `gh` read their configuration from it, and a crafted `.gitconfig` alias or
`core.hooksPath` runs arbitrary code inside an ordinary `git` call.
`XDG_CONFIG_HOME` is the same channel for tools that honour it. None of the
three holds a secret, so fixing their values keeps the issue's agreed
constraint -- no secret value in session state -- while closing the channel.
Every harness studied sets `HOME` before its first `koto init` and keeps it,
so fixing it breaks none of them.

The design's security review extended the same reasoning to the other
variables that locate a tool's inputs and can't hold a secret -- the cache and
CA-bundle paths, locale, time zone, temporary directory, terminal, user name.
A live `XDG_CACHE_HOME` can hand a tool a prepared cache (Go's test cache
lives under it) and a live `SSL_CERT_FILE` plus a proxy can forge API
responses, so those values are recorded too. Only names that can carry a
secret or a live socket are read live. `HOME` and the two tool-locating
variables are the only fixed values an attach compares, because a difference
in them changes which tools and configuration run; a difference in, say,
`TERM` doesn't, so it isn't a reason to refuse.

Snapshotting the remaining values was rejected: it writes secrets into a
readable log, and shirabe's engine tests change stub variables' values between
ticks.

### D3. Two lists: excluded by default, and refused outright

`GH_REPO` and `GH_HOST` redirect which repository or host `gh` reads, so they
stay out of the default set; a template or caller that needs one (a GitHub
Enterprise user's `GH_HOST`) adds it. Names that inject code, redirect what
`git` reads, or change how the shell and loader start (R6) can't be added at
all, because adding one would reopen the channel this PRD closes. `git` has
too many such variables to list one by one, so R6 refuses its whole prefix
and allows back the few that only set identity or prompting.
R6's list is closed so that two implementations refuse the same names.

### D4. Older sessions adopt on first tick

Chosen over leaving them on the caller's environment until they finish (the
precedent of the origin record) and over refusing them. Adoption keeps shirabe
runs in flight moving, closes the channel for them from the next tick on, and
follows the execution anchor's adoption, which users already know. Its cost is
that an older session's first tick loses variables outside the default pass
list without having opted into it; the one-time notice says so.

### D5. No verb changes the record

A rebind flag that re-recorded `PATH` would be the same bypass with a log line.
The remedy for a session whose tools moved is a new session, which is also
what `var_mismatch` asks of a changed fixed variable today.

### D6. A missing tool is explained, not predicted

koto can't know which tools a command will call, so it doesn't pre-resolve
them. It explains an exit 127 after the fact from the exit status alone, which
costs nothing on the path every command takes.

### D7. Attach from another host or shell has no rule of its own

The origin check already refuses a session from another worktree or store. What
remains -- a different shell, or a different host sharing a store -- differs in
exactly the fixed variables and added names, so R14 and R15 decide it. A
host-specific rule would add a second way to reach the same answer.

### D8. Requirements state the obligation; the design picks the mechanism

The header field's name, the event's name, the template syntax for adding
names, the R5 mechanisms, the exact default list and the shell's absolute path
are design decisions.

## Known Limitations

- **Pass-list values are caller-controlled.** A caller who changes a token, a
  locale, or a name a template or caller added still changes what a command
  sees. That's by design: those values are the ones that can be secrets.
- **An older koto ignores the record.** A session created by this version and
  then ticked by an earlier koto runs with the caller's environment, because
  the record is additive (R23).
- **A moved tool strands a session.** If a tool leaves every directory on the
  recorded `PATH`, the session's commands can't find it until the tool comes
  back or the session is replaced.
- **Harnesses and some users must name their variables.** shirabe's engine
  tests and evals whose stand-in tools read variables such as `GH_DB`, `GH_FIX`
  or `EVAL_SCENARIO`, and users whose `gh` depends on `GH_HOST`, must add those
  names (R5) once they run against this version.

## Downstream Artifacts

- `docs/designs/DESIGN-koto-fixed-environment.md` (planned) -- the technical
  approach, alternatives and compatibility story.
