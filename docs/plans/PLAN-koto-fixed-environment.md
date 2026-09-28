---
schema: plan/v1
status: Active
execution_mode: single-pr
split_mode_source: none
upstream: docs/designs/DESIGN-koto-fixed-environment.md
milestone: "Fixed command environment"
issue_count: 4
---

# PLAN: gates and actions run in an environment fixed at init

## Status

Active

## Scope Summary

Implement `docs/designs/DESIGN-koto-fixed-environment.md`: every session
records `PATH`, `HOME` and `XDG_CONFIG_HOME` at creation, every command gate
and default action runs in a cleared environment rebuilt from that record plus
a published list of names read live, a stale record is named with its remedy,
attach warns on drift, older sessions adopt on their first tick, and
`--legacy-environment` offers a creation-time opt-out, in one pull request.

## Decomposition Strategy

**Horizontal, in the design's commit order.** No intermediate commit may run
an unrecorded session with an empty environment, so the record and its
creation paths land first, then adoption so every ticking session has a
record, then the runner switch with the reproductions, then the documentation.
Each layer hands the next a stable interface (`CommandEnvironment` in the
header, then `CommandEnv` into the runner).

The work lands as a single PR: the repository declares no delivery preference,
so the consolidated default applies. The record could ship alone (it is
readable in the header), but on its own it changes no verdict, and the
reproductions are what the issue asks to close; the order inside the branch
keeps each commit safe to bisect.

## Issue Outlines

### Issue 1: feat(engine): record a session's command environment at creation

**Complexity**: testable

**Goal**: Every session koto creates carries a `CommandEnvironment` record --
normalized `PATH`, `HOME`, `XDG_CONFIG_HOME`, the release's default live
names, and the legacy flag -- in its header, and children inherit it.
Commands still run as today.

**Acceptance Criteria**:
- `StateFileHeader.command_environment` is an additive, skip-when-empty field;
  `CURRENT_SCHEMA_VERSION` is unchanged and a header written by this change
  parses with the previous release's type (round-trip test).
- `src/engine/command_env.rs` holds the default live list and the seven
  refused names exactly as the design's Decision 4 lists them, the name
  validator, the `PATH` normalization (empty, `.`, relative and `~` entries
  dropped; nothing left recorded unset; a relative `HOME` or
  `XDG_CONFIG_HOME` recorded unset), and the credential check (a fixed value
  containing a set default-live value is recorded unset), each unit-tested.
- Template `pass_env:` compiles into `CompiledTemplate.pass_env`; a template
  that declares none has an unchanged compiled hash; a refused or malformed
  name is a compile error; a koto-set name compiles with a warning;
  `--from-stdin` templates accept it.
- `koto init --legacy-environment` records `legacy: true` on every top-level
  form and is refused with `--parent`.
- A test per creation form -- plain, `--vars-file`, `--replace-terminal`,
  `--koto-leg`, `--from-stdin`, `koto session start` -- asserts the header
  record. The `koto init` response lists dropped `PATH` entries and any value
  recorded unset by the credential check.
- Batch, retry, skip-marker, `--parent` and `koto session start` children
  copy the parent's record, legacy flag included, even when created from a
  process with a different `PATH` (tested for each). A child whose parent has
  no record gets no record in this issue; Issue 2 makes the parent adopt
  first.
- The record survives `koto session rebind`, a rename, `koto session recover`
  and a claim write byte-identical (tested).
- Credential-shaped log test: with `GH_TOKEN`, `GITHUB_TOKEN` and variables
  named `*_TOKEN`, `*_SECRET`, `*_KEY`, `*_PASSWORD` set to unique markers at
  `koto init` (each creation form), no file under the session directory
  contains a marker, and the header's record carries values only under
  `path`, `home` and `xdg_config_home`. The test names variables and never
  prints an environment.
- `cargo test` for the touched modules and `cargo clippy --all-targets -- -D
  warnings` pass.

**Dependencies**: None

### Issue 2: feat(cli): adopt a record on an older session's first tick and report drift at attach

**Complexity**: critical

**Goal**: A session with no record adopts one on its first tick with a
one-time notice, so every session that ticks has a record; `koto init
--attach-live` reports drift in `PATH`, `HOME` or `XDG_CONFIG_HOME` and
refuses nothing.

**Acceptance Criteria**:
- A session file with no record advances on its first tick, records the
  tick's normalized fixed values and default list (never the legacy flag),
  appends one `environment_adopted` event with the dropped entries, and the
  response carries the notice with the recorded values; the second tick
  carries no notice.
- Adoption runs after the anchor check and the dispatch-epoch fence, under the
  state-file lock, re-reading the header first. A deterministic test calls the
  adoption step twice against one session and asserts one event.
- A batch parent with no record adopts before it spawns, and its new children
  copy the adopted record; a child spawned before the upgrade adopts on its
  own first tick.
- Attach with a different `PATH`, `HOME` or `XDG_CONFIG_HOME` attaches, its
  response carries `environment_drift` entries with both values, and one line
  goes to stderr; a `PATH` differing only in dropped entries reports no drift;
  nothing is written to a request leg; an unrecorded session is not compared.
- `koto session rebind` moves the anchor and leaves the record unchanged.

**Dependencies**: <<ISSUE:1>>

### Issue 3: fix(action): run every command in the recorded environment

**Complexity**: critical

**Goal**: `run_shell_command` runs `/bin/sh -c` with a cleared environment
built once per tick from the record, empty stdin and no `PATH` fallback;
stale records are named; the issue's reproductions and the `.gitconfig`
channel are closed.

**Acceptance Criteria**:
- `CommandEnv` lives in `src/action.rs`; `run_shell_command`,
  `evaluate_gates` and `evaluate_gates_with_request_store` take it, so every
  caller (`TickGates`, the polling loop's gate closure, the `--to` guard, both
  default-action sites) passes the same per-tick value. An unset recorded
  `PATH` becomes `/usr/bin:/bin`; refused names are filtered; fixed values and
  koto's two variables are applied last. A legacy session gets the ticking
  process's environment plus `KOTO_TICK_SESSION`.
- `PATH`-prefix reproduction: a non-overridable `whoami` gate stays at its
  first state when ticked with a fake `whoami` first on `PATH`; it reached the
  terminal state before this change.
- Exported-function reproduction on a bash `/bin/sh`: the same gate stays at
  its first state when ticked with an exported `whoami` function. The test is
  ignored in the default suite; a new CI job links `/bin/sh` to bash on the
  runner and runs it; locally it runs in a Debian container on the rootless
  daemon. The result is reported in the PR.
- `.gitconfig` reproduction: a gate calling an undefined git alias fails when
  ticked with `HOME`, or `XDG_CONFIG_HOME`, pointing at config that defines it.
- Accidental shim: a session created with a stand-in `gh` on `PATH` reads it
  from a tick whose `PATH` lacks it, and one created without it doesn't read
  it from a tick whose `PATH` has it.
- Only listed variables get through: inside a gate, a one-shot action and a
  polled action, the environment holds exactly the fixed values set at init,
  the default and declared names set on the tick, `KOTO_TICK_SESSION` and
  `KOTO_SESSIONS_BASE`; an unlisted variable, `BASH_ENV`, `ENV`,
  `GIT_CONFIG_COUNT`, `GIT_SSH_COMMAND` and any `BASH_FUNC_` name set on the
  tick are absent. Variables are checked by name; the environment is never
  printed wholesale.
- A declared name and `TMPDIR` deliver their tick values, and a value changed
  between two ticks reaches the second.
- Secrecy after ticks: with credential-shaped variables set to unique markers
  at init and on ticks running a gate, a one-shot action and a polled action,
  no file under the session directory contains a marker.
- Inside a gate `KOTO_TICK_SESSION` equals the session name and a nested
  `koto next` is refused with `nested_invocation`; a gate calling
  `koto context get` reaches the ticking process's store.
- A gate reading standard input sees end of file with input piped to
  `koto next`; a tick whose `PATH` lacks `sh`, and one with a decoy `sh`
  first, both run `/bin/sh`.
- Stale records: a gate whose tool isn't on the recorded `PATH`, and a gate
  script whose inner command isn't found, fail with unchanged evidence and a
  response note naming the gate, the recorded `PATH` and the remedy; a
  recorded `PATH` directory, `HOME` or `XDG_CONFIG_HOME` removed after init is
  named in a notice on the next tick and in the note of a gate that then
  fails; no note text appears in an action's captured output.
- Existing tests updated for the new argument and field;
  `tests/command_output_limits.rs` sets its `PATH` at init; the full
  `cargo test -- --test-threads=1` and `cargo clippy --all-targets -- -D
  warnings` pass.

**Dependencies**: <<ISSUE:1>>, <<ISSUE:2>>

### Issue 4: docs: document the fixed command environment

**Complexity**: simple

**Goal**: Users, template authors and agents can find what a command's
environment is, how to declare a name, what the legacy flag does and when it
goes away, what the stale-record note means, and what the change does not
close.

**Acceptance Criteria**:
- CHANGELOG Unreleased entry names the behaviour change for release notes, the
  default live list (or a link to it), `pass_env:`, `--legacy-environment` and
  its planned removal, the stale-record note, the attach drift report, and
  what a project whose gates read other variables must do.
- `docs/reference/session-feed.md` covers `environment_adopted`.
- `docs/guides/default-action-authoring.md` no longer says a command inherits
  the `koto next` environment, and it, the template-format and session
  lifecycle docs publish the exact default live list and refused list,
  `pass_env:`, `--legacy-environment`, the stale-record note, and the threat
  model table from the design.
- The koto-user, koto-author and koto-adhoc skills are assessed per
  `CLAUDE.md`; their advice to read `$VAR` at gate time becomes "declare the
  name".
- `cargo test --test doc_names` passes.

**Dependencies**: <<ISSUE:3>>

## Dependency Graph

_(omitted in single-pr mode -- each outline lists its dependencies: I1, then I2, then I3, then I4)_

## Implementation Sequence

The critical path is I1, I2, I3, I4, strictly in order: I3's runner switch
must not land before I2's adoption, or an older session would tick with an
empty environment between commits. The documentation (I4) can be drafted
alongside I3 but lands after it so it describes shipped behaviour.
