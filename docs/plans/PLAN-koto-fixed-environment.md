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
records the non-secret variables that locate its tools and a list of names
read live, and every command gate and default action runs in a cleared
environment rebuilt from that record, with attach refusal, older-session
adoption, the reproduction tests and the documentation, in one pull request.

## Decomposition Strategy

**Horizontal, in the design's commit order.** The design fixes the order of
landing so that no intermediate commit runs an unrecorded session with an
empty environment: the record and its creation paths first, then attach and
adoption so every ticking session has a record, then the runner switch, then
the documentation. Each layer has a stable interface to the next
(`CommandEnvironment` in the header, then `CommandEnv` into the runner), and a
walking skeleton would have to switch the runner early, which is the one
ordering the design rules out.

The work lands as a single PR: the repository declares no delivery
preference, so the consolidated default applies, and no split branch fires --
the runner change without the record has nothing to rebuild from, and the
record without the runner change does nothing observable, so no unit is
independently useful.

## Issue Outlines

### Issue 1: feat(engine): record a session's command environment at creation

**Complexity**: testable

**Goal**: Every session koto creates carries a `CommandEnvironment` record --
the fixed variables' values (with `PATH` normalized), the release's default
live names, and the creator's added names -- in its header and an
`environment_recorded` event, and children inherit it. Commands still run as
today.

**Acceptance Criteria**:
- `StateFileHeader.command_environment` and `EventPayload::EnvironmentRecorded`
  exist as additive, skip-when-empty fields; `CURRENT_SCHEMA_VERSION` is
  unchanged and a header and log written by this change parse with the
  previous release's types (round-trip test).
- A new `src/engine/command_env.rs` holds the fixed, default-live and refused
  lists exactly as the design's Decision 4 lists them, the name validator
  (pattern, refused list incl. `GIT_*` minus the allowed six and `KOTO_*`,
  value-equals-a-set-variable refusal), and the `PATH` normalization
  (empty/relative/`~` dropped, in-anchor flagged, nothing-left recorded unset,
  relative path-valued fixed values recorded unset), each unit-tested.
- Template `pass_env:` compiles into `CompiledTemplate.pass_env`; a template
  that declares none has an unchanged compiled hash; a refused or malformed
  name is a compile error; `--from-stdin` templates accept it.
- `koto init --pass-env NAME` (repeatable) and `KOTO_PASS_ENV` (comma list,
  trimmed, empties ignored) add names on every top-level form; `--pass-env`
  with `--parent` is refused; a bad item is refused as `invalid_pass_env`
  (exit 2) naming its position and source, never its text, with no session
  created.
- Both header writers (`init_child_core`, `init_inline_into_session`) record
  through an `EnvironmentSource`. A test per creation form -- plain,
  `--vars-file`, `--replace-terminal`, `--koto-leg`, `--from-stdin` and
  `koto session start` -- asserts exactly one `environment_recorded` event and
  a header record equal to it. The `koto init` response lists dropped and
  flagged `PATH` entries.
- Batch, retry, skip-marker, `--parent` and `koto session start` children copy
  the parent's record even when created from a process with a different
  `PATH` (tested for each). Only `--parent` and `koto session start` against a
  parent with no record record from their own process; a batch, retry or
  skip-marker child whose parent has no record is an internal error, since the
  parent's tick adopts first.
- The record survives every header rewrite: a test runs `koto session
  rebind`, a rename, `koto session recover` and a claim write, and asserts the
  record is byte-identical afterwards.
- A test with credential-shaped variables (`GH_TOKEN`, `*_TOKEN`, `*_SECRET`,
  `*_KEY`, `*_PASSWORD`) set to markers at `koto init` finds no marker in any
  file under the session directory, and the event carries values only for the
  fixed names.
- `cargo test` for the touched modules and `cargo clippy --all-targets -- -D
  warnings` pass.

**Dependencies**: None

### Issue 2: feat(cli): refuse a mismatched attach and adopt a record on an older session's first tick

**Complexity**: critical

**Goal**: `koto init --attach-live` refuses a session whose recorded `PATH`,
`HOME` or `XDG_CONFIG_HOME`, or whose creator-added names, differ from the
caller's; a session with no record adopts one on its first tick with a
one-time notice. After this issue every session that ticks has a record.

**Acceptance Criteria**:
- Attach with a different normalized `PATH`, `HOME` or `XDG_CONFIG_HOME`
  exits 2 with `environment_mismatch`, printing the variable and both values,
  and leaves the log and header unchanged; a `PATH` differing only in dropped
  entries attaches.
- Under `--koto-leg` the refusal is recorded on the leg with reason
  `environment-mismatch:<VARIABLE>` and no values.
- With matching fixed values, an attach adding a superset, a subset or a
  disjoint set of names is refused with `environment-mismatch:pass-list`; the
  same set, or none, attaches.
- Attach on a session with no record attaches with any `PATH`.
- A session file with no record advances on its first tick, records the
  tick's fixed variables and default names, appends one
  `environment_recorded` event with `adopted: true`, and the response carries
  the notice (recorded values, dropped and flagged entries, what changed, the
  remedy); the second tick carries no notice.
- Adoption runs after the anchor check and the dispatch-epoch fence, under the
  state-file lock, re-reading the header first. A deterministic test calls the
  adoption step twice against the same session (the second call finding the
  record the first wrote under the lock) and asserts exactly one record event.
- `koto session rebind` moves the anchor and leaves the record unchanged.

**Dependencies**: <<ISSUE:1>>

### Issue 3: fix(action): run every command in the recorded environment

**Complexity**: critical

**Goal**: `run_shell_command` runs `/bin/sh -c` with a cleared environment
built once per tick from the record (fixed values, live names from the
default list, the template's `pass_env` and the creator's names, plus
`KOTO_TICK_SESSION` and `KOTO_SESSIONS_BASE`), empty standard input, and no
`PATH` fallback -- closing the reproductions in koto issue #261 and the
`HOME` channel the design added.

**Acceptance Criteria**:
- `CommandEnv` lives in `src/action.rs`; `run_shell_command` and
  `evaluate_gates_with_request_store` take it, so every caller (`TickGates`,
  the polling loop's gate closure, the `--to` guard, both default-action
  sites) passes the same per-tick value; an unset recorded `PATH` becomes
  `/usr/bin:/bin`; refused names are filtered again when it is built.
- `PATH`-prefix reproduction: a non-overridable `whoami` gate stays at its
  first state when ticked with a fake `whoami` first on `PATH`; it reached the
  terminal state before this change.
- Exported-function reproduction, run in a Debian container with `/bin/sh`
  linked to bash: the same gate stays at its first state when ticked with an
  exported `whoami` function. The test is marked ignored in the default suite
  and fails rather than skips when run without a container runtime; it is run
  explicitly (on the rootless daemon locally) and its result is reported in
  the PR.
- Only listed variables get through: inside a gate, a default action and a
  polled default action, the environment (read with `env` by name only, never
  printed wholesale) holds exactly the fixed names that were set at init, the
  live names set on the tick, `KOTO_TICK_SESSION` and `KOTO_SESSIONS_BASE`;
  an arbitrary unlisted variable set on the tick, `BASH_ENV`, `ENV` and any
  `BASH_FUNC_` name are absent; `KOTO_PASS_ENV` set only on the tick adds
  nothing.
- A template `pass_env` name, a `--pass-env` name and a `KOTO_PASS_ENV` name
  each deliver their live tick value to a gate command.
- Secrecy after ticks: with credential-shaped variables and a pass-list
  variable set to unique markers at init and on ticks that run a gate, a
  default action and a polled default action, no file under the session
  directory contains a marker.
- Accidental shim: a session created with a stand-in `gh` on `PATH` reads it
  from a tick whose `PATH` lacks it, and one created without it doesn't read it
  from a tick whose `PATH` has it.
- `HOME` and `XDG_CONFIG_HOME` channel: a gate running an undefined git alias
  fails when ticked with `HOME` (or `XDG_CONFIG_HOME`) pointing at a config
  that defines it.
- `TMPDIR`, `XDG_CACHE_HOME` and `SSL_CERT_FILE` changed between init and a
  tick reach a gate with their init values; a live name changed between two
  ticks reaches the second tick's command.
- Inside a gate and an action `KOTO_TICK_SESSION` equals the session name and
  a nested `koto next` is refused with `nested_invocation`; a gate calling
  `koto context get` reaches the ticking process's store.
- A gate reading standard input sees end of file when `koto next` has input
  piped to it.
- A tick whose `PATH` lacks `sh`, and one whose `PATH` puts a decoy `sh` first,
  both run commands with `/bin/sh`.
- An exit 127 leaves gate evidence byte-identical to today and adds the note
  (recorded `PATH`, can't be changed, remedy) to the `koto next` response, and
  to an action's captured stderr.
- A session whose `PATH` held only relative entries doesn't find a tool file
  placed in the execution anchor by bare name.
- Existing tests updated for the new argument and field;
  `tests/command_output_limits.rs` sets its `PATH` at init; the full
  `cargo test -- --test-threads=1` and `cargo clippy --all-targets -- -D
  warnings` pass.

**Dependencies**: <<ISSUE:1>>, <<ISSUE:2>>

### Issue 4: docs: document the fixed command environment

**Complexity**: simple

**Goal**: Users, template authors and agents can find what a command's
environment is, how to add a name, what `environment_mismatch` means, and what
the change does not close.

**Acceptance Criteria**:
- CHANGELOG Unreleased entry names the behaviour change for release notes, the
  fixed and default live lists (or a link to them), `pass_env:`,
  `--pass-env`, `KOTO_PASS_ENV`, `environment_mismatch`, `invalid_pass_env`,
  and what a `GH_HOST` user or a harness must do.
- `docs/reference/error-codes.md` lists `environment_mismatch` and
  `invalid_pass_env` with their fields; `docs/reference/session-feed.md`
  covers `environment_recorded`.
- `docs/guides/default-action-authoring.md` no longer says a command inherits
  the `koto next` environment, and it, the template-format and session
  lifecycle docs publish the exact fixed-variable list and the exact default
  live-name list, both ways to add names, the refused list, and the channels
  that stay open (files, session directory, binaries on
  the recorded `PATH`, live values, umask and limits, the creator's
  environment).
- The koto-user, koto-author and koto-adhoc skills are assessed per
  `CLAUDE.md`; koto-adhoc's advice to read secrets as `$VAR` at gate time
  becomes "declare the name".
- `cargo test --test doc_names` passes.

**Dependencies**: <<ISSUE:3>>

## Dependency Graph

_(omitted in single-pr mode -- each outline lists its dependencies: I1, then I2, then I3, then I4)_

## Implementation Sequence

The critical path is I1, I2, I3, I4, strictly in order: I3's runner switch
must not land before I2's adoption, or an older session would tick with an
empty environment between commits. There is no parallelism worth taking inside
one PR; documentation (I4) can be drafted alongside I3 but lands after it so
it describes shipped behaviour.
