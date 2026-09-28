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
  A session's commands resolve tools and git and gh configuration the way
  they did when the session started, from whichever shell ticks it. Shell and
  config injection never reaches them, the variables commands legitimately
  need keep coming from the live environment by name, and no credential is
  written into session state. A stale record is named with a way out, drift
  on resume is warned about rather than refused, and sessions in flight when
  koto is upgraded keep advancing.
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
function in the ticking shell changes nothing, a stale or drifted environment
is named rather than silent, and credentials still come from the live
environment without being written into the session. A template author can rely
on a non-overridable gate against a one-tick environment change and knows
which channels stay open; a user upgrading mid-run finds sessions still advance.

**The journeys.** An eval author's stand-in tool stops leaking between ticks;
an agent can't pass a non-overridable gate by prefixing `PATH` or exporting a
function; a developer resuming from a shell whose tools moved gets a named
failure or warning instead of silent drift; a shirabe user upgrades koto with
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

- **Determinism.** A session's gates and actions find the tools and the `git`
  and `gh` configuration they found when the session started, whichever shell
  ticks it. A verdict changes because the repository changed, not because the
  terminal did.
- **The reproduced bypasses are closed.** Changing `PATH`, `HOME` or
  `XDG_CONFIG_HOME`, or exporting a shell function, `BASH_ENV`, `ENV` or a
  `git`/`gh` config-injection variable, for one `koto next` no longer changes a
  gate's verdict. The documentation names the channels that stay open.
- **Credentials keep working and stay out of the session.** Every other
  variable a command needs is read live at each run by name; koto writes no
  credential into session state.
- **Nothing strands, and a stale record says so.** A session created by an
  older koto keeps advancing after the upgrade and is told once what changed.
  A recorded value that stops resolving produces a named error with a way out,
  never a silent failure. Resuming from a shell whose environment differs is
  warned about, not refused.

## User Stories

### Story 1: An eval author's stand-in tool stops leaking between ticks

As an author of a koto-backed workflow eval, I want the stand-in `gh` I put on
`PATH` before `koto init` to be what every tick reads, whichever shell ticks
it, so that the eval's result stops depending on who called `koto next`.

### Story 2: A one-tick environment change can't pass a non-overridable gate

As a template author, I want a gate marked `overridable: false` to give the
same verdict when a caller prefixes `PATH`, exports a shell function under a
tool's name, or points `HOME` at a directory with a crafted `.gitconfig` for a
single `koto next`, so that the gate isn't negotiable through the environment
of one tick. (What a caller can still do when it also creates the session is
stated in the threat model, not promised away.)

### Story 3: A developer resumes from a shell whose tools have moved

As a developer resuming a session from a new terminal, I want to be told when
my shell's `PATH` or `HOME` differs from the session's record, and, when a
command fails because a recorded value no longer resolves, to be told which
value is stale and how to start a new session, so that a change of tools never
fails silently or blocks my resume.

### Story 4: A shirabe user upgrades koto with sessions in flight

As a shirabe user who upgrades koto half-way through a `/work-on` run, I want
the run's next tick to advance, tell me once that the session now runs with a
fixed environment, and keep finding `gh`, `jq`, `git`, `koto` and `shirabe`, so
that the upgrade doesn't strand the run.

### Story 5: An operator shares a session log

As an operator sending a failing session's log to a maintainer, I want the log
to show which `PATH`, `HOME` and `XDG_CONFIG_HOME` the session's commands ran
with and which variable names they could see, and no credential, so that I can
share it without scrubbing secrets first.

### Story 6: A harness keeps working through the first release

As the maintainer of a test harness whose stand-in tools read variables such as
`GH_DB` or `EVAL_SCENARIO`, I want a documented way to keep my harness's
sessions on the old behaviour for the first release that enforces the fixed
environment, recorded at `koto init` so no tick can change it, so that my
suite keeps passing while the long-term route to add names is decided.

## Requirements

### Functional: what a session records

- **R1. Three values are fixed at creation.** Every session koto creates
  records the values of `PATH`, `HOME` and `XDG_CONFIG_HOME` as they are in
  the creating invocation's environment; a variable unset at creation is
  recorded as unset. The recorded `PATH` has empty and relative entries
  removed, since those resolve inside whatever directory a command starts in.
  A fixed value that contains the value of any set variable on the default
  live list is recorded as unset, and the `koto init` response says so. This
  covers `koto init` in every form (plain, `--vars-file`, `--replace-terminal`,
  `--koto-leg`, `--from-stdin`) and `koto session start`.
- **R2. Everything else is a name read live.** Besides the fixed values, a
  session's commands see only the variables on its pass list, read from the
  ticking process at each run. koto writes no value of a pass-list variable
  into its header, its event log, or any other file in its session directory.
  (A command that prints a value itself has its output recorded as evidence,
  as today; that's the command's doing, not koto's.)
- **R3. The default pass list.** Every session's pass list contains a default
  set of names fixed by the koto release that created it and published in the
  documentation as an exact list: user and locale, time zone, temporary
  directory, terminal and colour settings, `CI`, the XDG cache, data, state
  and runtime directories, CA-bundle paths, the ssh agent, the Linux keyring's
  D-Bus session, `gh` and `git` authentication tokens, `GH_HOST`, and HTTP
  proxy settings. It does not include `GH_REPO` or any `GIT_*` name.
- **R4. Templates can add names, and that is the only way.** A template can
  declare additional names its commands need; a session created from it
  passes them. There is no `koto init` flag and no environment variable that
  adds names in this release.
- **R5. Names that are never passed.** These can't reach a command: a template
  declaring one is a compile error. `BASH_FUNC_*`, `BASH_ENV`, `ENV`,
  `GIT_CONFIG*`, `GIT_SSH_COMMAND`, `GIT_ASKPASS`, `GH_CONFIG_DIR`. Nothing
  else is refused. A name must match `^[A-Za-z_][A-Za-z0-9_]*$`. A declared
  name that koto sets itself (a fixed value, `KOTO_TICK_SESSION`,
  `KOTO_SESSIONS_BASE`) has no effect, and the compiler warns.
- **R6. The legacy opt-out is recorded at creation.** `koto init
  --legacy-environment` creates a session whose commands run with the ticking
  process's whole environment, as they did before this change. The choice is
  recorded in the session; no tick, attach or other verb can set or clear it,
  and children inherit it. It is documented as removed in the next release,
  once shirabe's harnesses have migrated off it.
- **R7. The record is readable.** The record lives in the session header, the
  first line of its state file. Adopting a record on an older session (R16)
  appends an event carrying it.
- **R8. Children inherit.** A child session -- batch spawn, retry, skip
  marker, `koto init --parent`, `koto session start` under a parent -- copies
  its parent's record (fixed values and the legacy flag) rather than
  recording its own from the ticking process. Its own template's declared
  names apply to it.

### Functional: how commands run

- **R9. A cleared environment rebuilt from the record.** Every command koto
  runs for a session without the legacy flag -- command gates and default
  actions, including each polling attempt -- starts with an empty environment
  to which koto adds exactly: each fixed variable recorded as set, with its
  recorded value; each default and template-declared name that the ticking
  process has set, with its live value; `KOTO_TICK_SESSION` set to the name of
  the session being ticked; and `KOTO_SESSIONS_BASE` set to the base of the
  session store the tick is operating on. When the recorded `PATH` is unset,
  koto sets `/usr/bin:/bin`. A command's standard input is empty.
- **R10. Shell and config injection doesn't reach commands.** As a consequence
  of R5 and R9, an exported shell function, `BASH_ENV`, `ENV` and the `git`
  and `gh` config-injection names set in the ticking process are absent from a
  command's environment.
- **R11. The shell is started by absolute path.** koto starts `/bin/sh`, never
  a shell found by searching any `PATH`.
- **R12. The nested-tick refusal survives.** A `koto next` started from inside
  a command a tick is running is still refused with `nested_invocation`, with
  or without the legacy flag.
- **R13. A stale record is named, never silent.** On every tick koto checks
  that each recorded `PATH` directory, the recorded `HOME` and the recorded
  `XDG_CONFIG_HOME` still exist. When a gate or action fails and either the
  check found a missing value or the command reported a command not found,
  the `koto next` response names the failing gate or action, which recorded
  value is stale (or that the command ran under the recorded `PATH`, given),
  and how to start a new session. A missing value found with no failure is
  reported as a notice on the response. A gate's recorded evidence is
  unchanged, and the note is never written into captured output.

### Functional: attach and rebind

- **R14. Attach warns on drift and refuses nothing.** `koto init
  --attach-live` compares the caller's normalized `PATH`, `HOME` and
  `XDG_CONFIG_HOME` with the record. A difference attaches as before; the
  response carries each differing variable with its recorded and caller
  values and says commands run with the recorded ones. No environment
  difference refuses an attach.
- **R15. No verb changes the record.** `koto session rebind` moves the
  execution anchor and leaves the record unchanged. Nothing rewrites a record
  once written. The remedy for a stale record is a new session.

### Functional: older sessions

- **R16. Older sessions adopt once.** A session with no record -- created by
  an earlier koto -- records the ticking process's fixed values (with the same
  `PATH` normalization) on its first tick under this version, appends the R7
  event, and prefixes a one-time notice to that tick's directive showing the
  recorded values and the dropped entries. Later ticks take the ordinary path.
  An older session never adopts the legacy flag.

### Functional: documentation

- **R17. The threat model is stated.** The documentation for gates, default
  actions and the session lifecycle says which adversary this serves (an
  accidental environment; a ticking caller changing one tick) and which it
  doesn't (a caller who also creates the session and chooses its values or
  the legacy flag; edits to files, the session directory, the configuration
  under the recorded `HOME`, or the binaries on the recorded `PATH`; live
  values of pass-list variables).
- **R18. The behaviour change is announced.** The CHANGELOG's Unreleased
  section calls out the behaviour change for release notes, the default pass
  list, `pass_env:`, `--legacy-environment` and its planned removal, and what a
  project whose gates read other variables must do.
- **R19. What shirabe must do is written down.** The design lists what a
  shirabe session sees on upgrade and which shirabe files must change; the
  change itself is shirabe's.
- **R20. Deferred work keeps its reproductions.** The design lists each
  deferred follow-up with the reproduction that would justify it.

### Non-functional

- **R21. Additive on disk.** The record is an additive header field and an
  additive event. `CURRENT_SCHEMA_VERSION` doesn't change, and a state file
  written by this version parses with the header and event types of the
  previous release.

## Acceptance Criteria

### The reproductions

- [ ] With a template whose only gate is `overridable: false` and checks that
      `whoami` prints `nobody`: a session created from an ordinary shell and
      ticked with a directory holding a fake `whoami` prefixed on `PATH` stays
      at its first state.
- [ ] The same session, on a host whose `/bin/sh` is bash, ticked with an
      exported function `whoami` that prints `nobody`, stays at its first
      state. This runs in CI.
- [ ] A session ticked with `HOME` pointing at a directory whose `.gitconfig`
      defines an alias that a gate calls runs its gates with the `HOME`
      recorded at creation, so the gate fails; the same holds for an
      `XDG_CONFIG_HOME` whose git config defines it.
- [ ] A session created with a stand-in tool first on `PATH` reads the
      stand-in on a tick whose `PATH` doesn't carry it, and a session created
      without it doesn't read it on a tick whose `PATH` does.

### Recording and secrecy

- [ ] After each of plain `koto init`, `--vars-file`, `--replace-terminal`,
      `--koto-leg`, `--from-stdin` and `koto session start`, the header
      carries a record with the normalized `PATH`, `HOME` and
      `XDG_CONFIG_HOME`.
- [ ] A `PATH` holding empty, `.`, relative and `~` entries is recorded
      without them, and a `PATH` with no surviving entry is recorded unset.
- [ ] With credential-shaped variables (`GH_TOKEN`, `GITHUB_TOKEN`, names
      ending `_TOKEN`, `_SECRET`, `_KEY`, `_PASSWORD`) set to unique markers
      at `koto init` and on ticks that run a gate, a default action and a
      polled default action, no file under the session directory contains a
      marker.
- [ ] A `HOME` whose value contains a set token's value is recorded unset and
      the `koto init` response says so.
- [ ] A template-declared name delivers its live tick value to a gate
      command, and a value changed between two ticks reaches the second.
- [ ] Each R5 name, and a name failing the pattern, is a compile error in
      `pass_env:`; declaring `PATH` compiles with a warning and has no effect.
- [ ] A child created by a batch spawn, a retry, a skip marker,
      `koto init --parent` and `koto session start` under a parent carries its
      parent's record and legacy flag, even when created from a process with a
      different `PATH`.
- [ ] `CURRENT_SCHEMA_VERSION` is unchanged, and a header and a log written by
      this version parse with the previous release's types.

### Running commands

- [ ] Inside a gate, a default action and a polled default action, the
      environment holds exactly the fixed values set at init, the default and
      declared names set on the tick, `KOTO_TICK_SESSION` and
      `KOTO_SESSIONS_BASE`; an unlisted variable, `BASH_ENV`, `ENV`,
      `GIT_CONFIG_COUNT`, `GIT_SSH_COMMAND` and any `BASH_FUNC_` name set on
      the tick are absent. The test names variables and never prints the
      environment wholesale.
- [ ] `TMPDIR` changed between init and a tick reaches the gate with its tick
      value; a `TMPDIR` that existed at init and is gone at the tick doesn't
      affect the session's record.
- [ ] Inside a gate `KOTO_TICK_SESSION` equals the session name and a nested
      `koto next` is refused with `nested_invocation`; a gate calling
      `koto context get` reaches the ticking process's store.
- [ ] A gate reading standard input sees end of file when `koto next` has
      input piped to it.
- [ ] A tick whose `PATH` lacks `sh`, and one whose `PATH` puts a decoy `sh`
      first, both run commands with `/bin/sh`.
- [ ] A session with the legacy flag runs its gates with the ticking
      process's whole environment, and no tick, attach or rebind changes the
      flag.

### Stale records

- [ ] A gate whose tool isn't on the recorded `PATH` fails with its evidence
      unchanged, and the response names the gate, the recorded `PATH`, and how
      to start a new session.
- [ ] A gate that calls a script whose inner command isn't found (the script
      exits with its own status) gets the same note.
- [ ] A recorded `PATH` directory, `HOME` or `XDG_CONFIG_HOME` removed after
      init is named in a notice on the next tick, and in the failure note of a
      gate that then fails.
- [ ] No note text appears in an action's captured stdout or stderr.

### Attach

- [ ] `koto init --attach-live` with a different `PATH`, `HOME` or
      `XDG_CONFIG_HOME` attaches, and its response names each differing
      variable with both values; a `PATH` differing only in dropped entries
      reports no difference.
- [ ] `koto session rebind` moves the anchor and leaves the record unchanged.

### Older sessions

- [ ] A session file with no record advances on its first tick, records the
      tick's fixed values, appends the record event once, and the response
      carries the one-time notice with the recorded values and dropped
      entries; the second tick carries no notice.
- [ ] A batch parent with no record adopts before it spawns, and its new
      children copy the adopted record; a child spawned before the upgrade
      adopts on its own first tick.

### Documentation

- [ ] The CHANGELOG's Unreleased section has the entry R18 describes.
- [ ] The default-action and gate guides and the session lifecycle docs
      publish the exact default pass list and refused list, `pass_env:`,
      `--legacy-environment`, the stale-record note, and the threat model.
- [ ] The DESIGN has the shirabe section and the deferred follow-ups R19 and
      R20 describe.

## Out of Scope

- **File, session-directory, repository and binary tampering.** None of it
  goes through the environment.
- **A caller who also creates the session.** It chooses the recorded values
  and, in this release, the legacy flag; see the threat model.
- **`koto next --to` and overrides.** Other routes around a gate have their
  own treatment.
- **Sandboxing.** No restriction on what a running command reads, writes or
  executes beyond the environment it starts with.
- **The deferred follow-ups** listed in the design: more values fixed, a wider
  refused list, caller routes to add names, an attach refusal, removing the
  legacy flag, re-adoption.
- **Changing shirabe.** The design proposes what shirabe must change; shirabe
  makes the change.

## Decisions and Trade-offs

### D1. On by default, with a temporary init-time opt-out

The fixed environment applies to every new session. A template opt-in would
leave the accidental case open wherever nobody opted in. The first release
also ships `--legacy-environment`: recorded at creation, never changeable by a
tick, and documented as removed later. It exists because this changes
behaviour for every user and a named escape between the release that breaks
someone and the release that fixes them is cheaper than pinning an old koto.
The end state was ruled on 2026-09-28: the next release removes the flag once
shirabe's harnesses have migrated, leaving no opt-out.

### D2. Three values fixed; everything else read live by name

koto issue #261 proposed fixing `PATH` alone. A live `HOME` is the same
channel for `git` and `gh` gates: a crafted `.gitconfig` alias or
`core.hooksPath` runs arbitrary code inside an ordinary `git` call, and
`XDG_CONFIG_HOME` is the same for tools that honour it. None of the three
holds a secret, so fixing them keeps the issue's constraint. Fixing more
values was considered and deferred: once the environment is cleared, a
variable that isn't passed is absent, which already closes it, so recording a
value is a compatibility choice, and each extra fixed value strands sessions
when its path disappears (a per-shell `TMPDIR` is the common case). A value is
promoted to fixed when a test shows a verdict flipping on it.

### D3. A short refused list

The refused list is the names the issue and the `.gitconfig` reproduction
justify. Wider lists refuse names to the template author, who can delete the
gate anyway, and break legitimate uses (deploy keys, corporate CAs) with no
remedy short of a release.

### D4. Older sessions adopt on first tick

Adoption keeps runs in flight moving and closes the channel for them from the
next tick on, following the execution anchor's adoption. Its cost is that the
first tick's environment is kept; the notice shows it.

### D5. No verb changes the record; a stale value is named

A rebind flag that re-recorded `PATH` would be the same bypass with a log line.
So a record that goes stale must say so loudly: R13 names the value and the
remedy, a new session.

### D6. Attach warns, never refuses

Attach writes no record, and a tick from the same shell runs with the recorded
values anyway, so refusing an attach on drift protected nothing while blocking
shirabe's resume, which attaches on every entry.

### D7. One way to add names

Only a template adds names in this release. A creation-time flag or variable
would give the creator a way to widen a session that a template author never
agreed to, and an ambient variable breaks resumes whenever a profile changes.
Both are deferred until a named user needs them; harnesses use the legacy flag
meanwhile.

### D8. Requirements state the obligation; the design picks the mechanism

The header field, event, template syntax, default list and stale-check
details are design decisions.

## Known Limitations

- **The creator chooses.** Whoever runs `koto init` chooses the recorded values
  and, in this release, can opt out with `--legacy-environment`.
- **Pass-list values are caller-controlled.** A caller who changes `GH_HOST`,
  a token, `TMPDIR`, `SSL_CERT_FILE` or a template-declared name still changes
  what a command sees.
- **An older koto ignores the record.** A session created by this version and
  ticked by an earlier koto runs with the caller's environment.
- **A moved tool strands a session.** The remedy is a new session; the error
  says so.
- **Harnesses need the legacy flag** in the first release when their stand-in
  tools read variables outside the default list.

## Downstream Artifacts

- `docs/designs/DESIGN-koto-fixed-environment.md` (planned) -- the technical
  approach, alternatives and compatibility story.
