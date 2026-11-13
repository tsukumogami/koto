---
schema: design/v1
status: Proposed
problem: |
  koto dashboard's session reader (log parsing, row derivation, liveness, the
  modification-time diff and the session tree) lives inside koto's one
  package, which links rust-s3, rustls, attohttpc, http, url, ratatui and
  crossterm, and the persistence module it reads logs through also appends to
  them and locks them. A second program, such as a fleet view in shirabe,
  can't use that code without taking on those dependencies and the write
  code with them, so it would have to copy the logic. The spike in koto#661
  showed the logic separates but left the split itself, the store layout a
  reader relies on and how a dependent takes the code undecided.
decision: |
  A new workspace member, koto-session-reader, owns the reader's source
  files, moved out of koto with git mv: the frozen state-file types and
  everything EventPayload embeds, the read half of engine::persistence, the
  full template types without their validators, row derivation, the leg
  pointer read, the dashboard's local data layer, a read-only listing trait
  and a four-outcome context read. koto depends on it by path and re-exports
  each moved module at its old path, so koto::engine::types and every other
  frozen name resolve unchanged. Every write, lock, fsync, hash, machine
  identity function and template validator stays in koto. A per-crate clippy
  ban list, forbid(unsafe_code), a dependency allowlist and a guard script
  run in a CI job make the crate unable to write. docs/STABILITY.md gains a
  scoped, additive-only promise of exactly the store layout the reader reads,
  guarded by a layout marker that is absent today. Dependents pin a koto
  release tag, and the crate carries koto's version. A dashboard golden test
  lands before the split, and a second workspace member, koto-session-list,
  is the minimal program that CI checks against the dashboard.
rationale: |
  A crate that owns the files is the only shape where koto dashboard and the
  second program run one copy of the logic and where "cannot write" is a
  property of a directory rather than of a build configuration; cargo
  features were rejected because a dependent's feature unification can
  compile the writers back in, and a crate of copies breaks the one-owner
  driver. Module re-exports keep the frozen surface without cutting
  EventPayload's type closure. The layout promise is written down because a
  dependent's pin can trail the user's koto, so same-commit coupling alone
  can't protect a binary already shipped. A tag pin with lockstep versions
  fits koto's release flow with no second version line, and CI keeps the
  crate packageable so a registry publish stays one step away.
---

# DESIGN: A session reader koto shares with other programs

## Status

Proposed

## Context and Problem Statement

`koto dashboard` reads every session in the store, works out each one's
current state, notices which sessions changed since the last refresh, and
classifies each as active, idle, stalled, blocked, failed, pending or done.
A fleet view being built in shirabe wants the same answers, from koto and
nothing else, and it shouldn't rebuild them: a second copy of row derivation
and liveness would drift from koto's, and koto would lose control of its own
store layout. The view's owner set one constraint over all of it. Status
comes from koto, agents write nothing new, and their koto calls cost what
they did. Whatever reads the store on the view's behalf must not write to it.

Today that code can't be shared. koto is one package, and its lock carries
rust-s3 with rustls, the attohttpc client, the http and url crates, and the
ratatui and crossterm terminal stack. A program that depends on koto links
all of it. The dashboard's data layer, `src/cli/dashboard_data.rs`, sits
inside the CLI module of that one package, and the persistence module it
reads logs through also appends to them, takes their locks and writes
supersede fences.

The spike in `docs/spikes/SPIKE-koto-shared-session-reader.md` (koto#661)
showed the logic can be separated. It compiled 22 of koto's source files a
second time in a throwaway crate with no storage or network dependency, with
fourteen validation, machine-identity and context-store functions and the
dashboard's remote overlay fenced off by `cfg` attributes. That crate
produced the same 251 default rows and 2,170 `--all` rows `koto dashboard
--once` showed on a copy of a 5,458-session store, and a hundred reads of a
cloud host's mirror under a sandbox denying writes and network left the
store unchanged. The spike didn't do the split itself. It reached koto's
source through `#[path]` attributes, which can never be packaged for a
registry; koto didn't link the crate; and the crate still contained koto's
append and lock code, so "doesn't write" held only because nothing called
it.

Its recommendation names what a real split needs: template validation and
machine identity moved off the shared types, the read paths in
`engine::persistence` separated from its write paths so the shared code
cannot write rather than merely does not, and a decision on what of the
store layout the crate promises. This design settles those, and how a
dependent takes the crate, how its releases tie to koto's, how koto
dashboard is shown unchanged, and how the boundary leaves room for koto#556,
which wants signals from outside koto recorded beside a session's events and
shown in the dashboard.

### Scope

In scope:

- The boundary of the shared code: which types, parsers, derivations and
  dashboard pieces move, and what stays in koto.
- How the moved code is made unable to write, and how a reviewer checks it.
- What of the store layout the shared code promises, how it finds the store,
  and how it reads one recorded context value.
- How a dependent takes the code and how its versions relate to koto's.
- How `koto dashboard` is shown unchanged and how the minimal second program
  is shown working, in terms a plan can take as acceptance criteria.

Out of scope:

- The build itself, which a plan and its issues carry after this design.
- Seats, holders and roles in koto, and any read verb for them.
- Any change in shirabe, its install or its release, and cutting a koto
  release.
- The remote rows other machines push to the bucket. The dashboard keeps
  showing them; the shared code doesn't read them.
- Recording koto#556's external signals. Only the room for them is in scope.

## Decision Drivers

- **One owner for the logic.** Row derivation, liveness and the
  modification-time diff `koto dashboard` uses are the code the second
  program runs, not a copy of it.
- **No storage or network dependency in the shared code.** Its dependency
  graph carries none of rust-s3, rustls, attohttpc, http, url, ratatui or
  crossterm, checked by `cargo tree`, not by reading `Cargo.toml`.
- **Cannot write, not merely does not.** The shared code has no function that
  creates, appends to, renames, removes or locks a file, and a check that
  fails CI says so; a sandbox run is evidence, not the mechanism.
- **koto dashboard unchanged.** The same sessions, states and liveness
  classes on the same store, before and after.
- **The frozen surface holds.** `docs/STABILITY.md` freezes the types under
  `koto::engine::types` and four `SessionBackend` methods. Moving their
  definitions mustn't change a path or a shape a dependent imports.
- **The agent's calls cost what they did.** The split moves code without
  changing what a tick computes, so an agent's koto calls do the same work.
  Where a read koto's commands share with the reader has to change, it
  keeps every answer for the files koto writes, and the design names it.
- **The layout gap is owned.** The session directory layout, the state file
  name, the `ctx/` store and the leg pointer are promised nowhere today. A
  reader outside koto relies on them, so whatever the shared code promises
  is written down where koto's own changes have to meet it.
- **Room for koto#556.** Unknown event types stay readable, row derivation
  takes facts from beside the log from its caller, and one bad source marks
  one session rather than failing the scan.
- **A dependent builds without koto's toolchain at install.** The second
  program compiles the shared code once, in its own release build; nothing
  at install time builds koto.

## Considered Options

### Decision 1: The shared crate's boundary and shape

The reader is spread over about 22 koto source files inside one package. The
question is what the real split looks like: which files move, how koto links
them while `koto::engine::types::*`, its `derive_state_from_log` alias and
`koto::error::Error` keep resolving (docs/STABILITY.md, checked by
`koto-stability-tests`), and how template validation, machine identity and
the one context-store function come off types that would then live in
another crate.

Reading the code shows the moved set is closed under reference once those
items stay behind. `engine::persistence` splits in one direction only: no
read function calls a write function, while the write side calls about six
private read helpers. `engine/jsonl_append.rs` is write-only. Of the files
that move, only two functions write, `leg_pointer::write_pointer` and
`SessionVersion::save`, and both stay in koto. Every `unsafe` block in the
moved files (two `libc::flock` calls and `libc::gethostname`) sits in code
that stays.

Key assumptions:

- A module re-export makes `koto::engine::types::StateFileHeader` the same
  item as the crate's, so `koto-stability-tests` and any
  `use koto::engine::types::*` compile unchanged. This needs a build; a
  failure is a compile error at the step where the re-exports go in, and a
  public-API snapshot of `koto::engine::types` and `koto::error`, taken
  before the move and diffed after, catches unintended additions.
- `rebuild_roots` is a pure function of the session map, so running koto's
  `drop_shadowed_remote` after the crate's `refresh` ends in the same tree as
  today's single pass.

#### Chosen: One workspace member crate that owns the moved files, with koto re-exporting at the old paths

A new member, `koto-session-reader/`, joins `[workspace] members` beside
`koto-stability-tests`. Files move with `git mv`, and its module tree mirrors
koto's, so every `crate::` and `super::` path inside them resolves as
written. It holds:

- `engine::types` and `engine::errors`, whole.
- `engine::persistence`, read half: header and log parsing, every `derive_*`,
  `is_terminal_state`, `is_failed_state`, the visit, epoch and window
  helpers, the merged-line detector (Decision 2), `read_fence` and
  `superseded_by_in_log`.
- `engine::leg_pointer`, read half: `read_pointer`, `read_pointer_best_effort`,
  `pointer_mtime`, `pointer_path`.
- `engine::terminal_result` without `failure_reason_for_current_run`.
- `engine::context_assign::outstanding_assignments`, the pure fold the
  context read uses (Decision 3). `write_assignments` and `reconcile` stay.
- `findings`, `redact`, and `decider::{check, evaluate, record, request,
  types}`: the closure `EventPayload` embeds, moved whole so no edge is cut
  and the frozen deserializer is the same code.
- `template::types` data definitions, serde helpers and pure methods, plus
  `template::{assignments, decider, decider_check, result_map}`.
- `session::row`, `session::validate`, and from `session::version` only
  `validate_machine_name` and its constants.
- `dashboard`: the local half of `cli/dashboard_data.rs`, meaning the
  `liveness` module, `CachedSession`, `SessionTree`, `scan_sessions`,
  `stat_and_diff`, `read_session`, `refresh`, `read_detail`,
  `rebuild_roots`, cycle detection and `compute_elapsed_since`.
- `source`: the listing trait, `LocalDirSource`, the root resolver and the
  context read (Decision 3).

koto adds `koto-session-reader = { path = "koto-session-reader" }`. A module
that moved whole becomes a module re-export, for example
`pub use koto_session_reader::engine::types;` in place of `pub mod types;`
in `src/engine/mod.rs`, so `koto::engine::types` is the crate's module
itself. The `derive_state_from_log` alias moves with the file, and
`koto::error::Error` resolves through the re-exported `engine::errors`
without an edit. A module that keeps koto items (`engine::persistence`,
`engine::leg_pointer`, `engine::terminal_result`, `engine::context_assign`,
`session::version`, `template::types`, `template::assignments`,
`cli::dashboard_data`) becomes a mixed file: its koto items plus
`pub use koto_session_reader::<same path>::*;`. One rule keeps it safe: a
mixed module defines no name the crate exports.

Validation comes off `CompiledTemplate` by changing only `impl` headers. The
public `validate` and `collect_pass_env_warnings` go under a koto trait,
`ValidateTemplate`, with today's signatures; the seven private validators go
under a private `ValidateSteps`; `ReviewVocabulary::validate` and
`impl VariableDecl` become `ValidateVocabulary` and `VariableDeclExt`. The
bodies and all their tests stay in koto's `src/template/types.rs`. Call
sites keep method syntax and add one trait import. Machine identity
(`machine_name`, `get_or_create_machine_id` and the rest), `SessionVersion`
and `failure_reason_for_current_run` stay as free functions at their
current paths, so no caller changes.

About 23 items that are crate-private today and used elsewhere in koto, such
as `parse_timestamp_to_systemtime`, `delivery_window`,
`compute_elapsed_since` and the read helpers `prepare_append` calls, become
`#[doc(hidden)] pub`. STABILITY.md promises none of them.

The dashboard keeps its TUI, its status buckets and formatting in
`cli/dashboard.rs`, and the bucket overlay, in koto. `CachedSession`'s
`imported_from` becomes the crate's import record, `SessionTree`'s
`remote_failure` moves into koto's dashboard state, and koto adds a
same-named `refresh` wrapper that calls the crate's `refresh`, then
`drop_shadowed_remote`, rebuilding roots if anything was dropped, plus
`impl SessionSource for LocalBackend`.

#### Alternatives Considered

**Same crate, `template::types` stays in koto with a lean template reader.**
About 60 lines of decider and template data re-homed and a six-field reader
for terminal state and detail, with no validation surgery. Rejected because
the `/workflows` projection that runs after each commit needs koto's
full-type `is_terminal_state`, which leaves two implementations of the
terminal rule and puts `koto dashboard` on a parser the spike never ran, and
because it splits the frozen type's decider closure across two crates.

**Two member crates, types first and the reader on top.** The frozen types
in a crate with no file I/O. Rejected because the extra property protects
against nothing a driver asks for, no dependent takes the types alone, and a
second crate doubles the release tie in Decision 4. The same layering is
kept as a source-scanning test, and splitting later is a `git mv` with no
koto path change.

**koto split behind cargo features.** A default-on `full` feature, with a
reader skeleton compiled when it's off. Cheapest, and it holds the frozen
paths by construction. Rejected because "cannot write" becomes a property of
build configuration: feature unification in a dependent's graph silently
compiles the writers and rust-s3 back in, a new edge into koto-only code
fails only in a reader CI job, and a dependent would fetch the whole package
and run koto's `build.rs`.

**A crate with its own copies, types staying in koto.** The dashboard would
run on the crate's copied decoder and rules. Rejected under one owner: the
copied rules keep changing (`epoch_slice` in #201 and #248, `SessionImported`
in #316, `producing_machine` in #572 and #580), and #201 deliberately kept
the delivery window and the blocked badge on one scan. It stays the
fallback if the split stalls.

### Decision 2: How the shared code is made unable to write

The spike mounted `persistence.rs` whole, so its crate still held
`append_event`, `AppendLock`, `acquire_state_flock`, `write_backup`,
`rewrite_tail` and `write_pointer`. "Doesn't write" held only because
nothing called them. Every write in `persistence.rs` sits in the append,
idempotent-append, fence-append, fsync and merged-line repair paths; the
read half calls only `read_to_string`, `File::open`, `fs::read` and
metadata. One read needs care: `read_pointer` opens with
`OpenOptions::read` plus `custom_flags(libc::O_NOFOLLOW)`, so `OpenOptions`
can't be banned as a type and libc can't be banned as a dependency. And
`custom_flags` is itself a write route, since `O_CREAT` or `O_TRUNC` passed
through it creates or truncates even on a read-only open.

koto already enforces rules this way: the root `clippy.toml` bans
`dirs::home_dir` with one sanctioned `#[allow]` in `src/koto_home.rs`, the
Decider Checks job bans crates from the graph with `cargo tree`, and
`scripts/check-decider-purity.sh` covers structural rules the compiler
can't.

Key assumptions:

- Clippy reads the first `clippy.toml` walking up from the crate's manifest
  directory, so the member's own file replaces the root's for that crate.
- `disallowed-methods` resolves trait-method paths such as
  `OpenOptionsExt::custom_flags`. Not built here; the plan's first guard
  issue proves it with a deliberate violation.

#### Chosen: A per-crate clippy ban list, forbid(unsafe_code), a dependency allowlist and a guard script, run by a Reader Write Guard CI job

1. `koto-session-reader/clippy.toml` bans by resolved path, so aliases, glob
   imports and method chains are caught. `disallowed-methods` lists the
   `std::fs` write, copy, rename, remove, create, link and `set_permissions`
   functions; `File::create`, `create_new`, `set_len`, `set_times`,
   `sync_*` and the `lock*` family; the `OpenOptions` modes `write`,
   `append`, `create`, `create_new` and `truncate`; `OpenOptionsExt::mode`
   and `custom_flags`; the unix `symlink` and `chown` family;
   `ToSocketAddrs::to_socket_addrs`; `std::env::set_var` and `remove_var`;
   and the root file's two `home_dir` entries. `disallowed-types` lists
   `DirBuilder`, `std::process::Command` and the TCP, UDP and unix socket
   types. The read calls themselves (`File::open`, `fs::read`,
   `fs::read_to_string`, `OpenOptions::new`) are banned too, outside one
   module.
2. The crate's `lib.rs` carries `#![forbid(unsafe_code)]`, which makes every
   libc function and any FFI call uncallable, so libc stays only for
   constants, and `#![deny(clippy::disallowed_methods,
   clippy::disallowed_types)]`. Every file open in the crate lives in one
   module, `fs_read.rs`, which holds the only allows and offers two
   functions. `open_plain(path)` is today's `File::open`, kept for the
   reads koto's own commands make of their own session's log through
   `read_events` and `read_header`, so those read exactly as they do now.
   The guard script allows it to be called only from those log readers in
   `persistence.rs`; the dashboard scan and the read surface can't reach
   it. `read_store_file(path, cap, mode)` is the hardened read. It
   takes no flag argument: `mode` is `InStore` or `Template`, and each maps
   to a fixed constant, `IN_STORE_OPEN_FLAGS` (`O_NOFOLLOW | O_NONBLOCK`)
   or `TEMPLATE_OPEN_FLAGS` (`O_NONBLOCK`, since a template given with
   `--template` may legitimately be a symlink outside the store). It checks
   with `fstat` that the opened file is a regular file, reads at most `cap`
   bytes, and in `InStore` mode rechecks after the open that the canonical
   path is still inside the store, as `koto context get` does. It is the
   crate's only `custom_flags` call. The context read, the layout
   marker and the dashboard's store scan (Decision 3) use it. Two reads
   switch to it for every caller, koto's commands included, and these are
   the only places the split changes what a koto command runs:
   `read_pointer`, which already opens with `O_NOFOLLOW`, and
   `is_terminal_state`, which today calls `fs::read` on the template path
   and is also what the `/workflows` projection calls after each commit.
   For a regular file under the cap neither changes an answer; only a FIFO,
   device or over-cap file, which blocks or reads without bound today, is
   refused.
3. `cargo tree -p koto-session-reader --depth 1` must list only an
   allowlist of direct dependencies: anyhow, serde, serde_json, thiserror,
   regex, aho-corasick, toml, libc, sha2, hex (the template declaration
   hash) and dirs (the root resolver, Decision 3). The full graph must
   contain none of rust-s3, rustls, attohttpc, http, url, ratatui,
   crossterm, tempfile, fs2, fs4, fd-lock, nix or rustix. A new direct
   dependency fails CI until the allowlist changes in a reviewed diff.
4. `scripts/check-reader-readonly.sh`, in the style of
   `check-decider-purity.sh`, checks what the lint can't: both attributes
   are still in `lib.rs`; no file under the crate's `src/` carries an
   `allow`, `expect` or `cfg_attr` naming the two lints or `clippy::all`
   outside the two sanctioned sites (`fs_read.rs` and the moved `home_dir`
   call); `custom_flags` appears exactly once, passed one of the two mode
   constants, whose definitions read exactly `O_NOFOLLOW | O_NONBLOCK` and
   `O_NONBLOCK`;
   `clippy.toml` still holds every required entry; the crate has no
   `build.rs`; neither the crate's manifest nor the workspace root's
   carries `[patch]` or `[replace]`; every dependency of the crate in
   `cargo metadata` comes from the crates.io registry; `open_plain` is
   called only from the two log readers; and the two `cargo tree`
   checks pass. A `--self-test` plants each violation in a temporary copy
   and expects a failure.
5. validate.yml gains a `reader-write-guard` job, "Reader Write Guard":
   `cargo clippy -p koto-session-reader --all-targets --locked -- -D
   warnings`, then the script.

Because `--all-targets` lints the crate's own `tests/` under the same ban,
those tests read committed fixture files only. Tests that need a store
written at run time, including the dashboard data layer's current unit
tests that build fixtures with `append_event` and `tempfile`, move to
koto's `tests/`, which reaches the crate through koto's re-exports. The
graph checks use normal and build edges, so dev-dependencies don't count
against the allowlist.

What this guarantees is a property of the crate's own source, enforced in
koto's CI at every tag. It isn't a process sandbox: a dependency's code
runs with the caller's privileges, which is why the dependency set is an
allowlist checked for name and source, and a dependent's build doesn't run
these lints.

`persistence.rs` divides so that the shared crate gets a read module with
no write vocabulary in it. The pure merged-line detection both halves
already share (`split_concatenated`, `consecutive_from`, `join_seqs`,
`last_event_line`, `split_last_line`, `last_line_would_split`) moves to the
crate, with a new pure `plan_append(content) -> Result<AppendPlan>` that
holds `prepare_append`'s header check, next sequence number and merged-line
classification without I/O. koto keeps `prepare_append` as the writer's
step (read under the append lock, call `plan_append`, and on a split run
`split_merged_last_line`, `write_backup` and `rewrite_tail`), so the
torn-tail-tolerant read and the repairing write share one detector. The
fence readers go to the crate; `refuse_if_fenced`, `append_supersede_fence`
and every append, lock, fsync and `idempotency_hash` stay in koto.

#### Alternatives Considered

**The clippy ban list alone.** Rejected as sufficient because it can't see
inside dependencies or stop unsafe FFI, and a local or group `#[allow]`
defeats it. It's kept as the primary layer.

**A CI grep or symbol scan as the mechanism.** Rejected because grep misses
aliases, glob imports and chains on values whose type is never named, and a
symbol scan can't tell a read open from a write open. Grep stays only for
the structural rules.

**A dependency rule alone ("no libc, no tempfile").** Rejected because it
says nothing about the crate's own `std::fs` calls, and "no libc" can't hold
while `read_pointer` needs `O_NOFOLLOW`.

**A test-time sandbox run.** Rejected as the mechanism because it covers
only executed paths and is platform-specific. It stays as evidence under
Decision 5.

**A read-only handle type.** Rejected as enforcement because nothing stops a
direct `std::fs` call without the lint; the listing trait and the one
nofollow helper keep the useful part.

### Decision 3: What of the store layout the reader promises, how it finds the store, and how it reads a context value

The reader reads files koto writes: `<base>/<id>/koto-<id>.state.jsonl`, the
`ctx/<key>` content files, the `request-leg.toml` sidecar, and the compiled
template that `workflow_initialized.template_path` names. No document
promises any of them. A dependent ships a prebuilt binary pinned to one
crate revision, and that pin can trail the koto release a user runs, so
keeping layout and crate in one commit protects koto's build but not a
binary already shipped.

Three facts from the code shape the answer. A full parse of
`CompiledTemplate` is fragile for an older reader: `ReviewVocabulary` and
`MaterializeChildrenSpec` carry `deny_unknown_fields`, six deserialized
enums have no catch-all, and `is_terminal_state` turns any parse failure
into "not terminal", so an additive template change in a newer koto would
show finished sessions as live. The store root differs by backend: the
local backend uses `KOTO_SESSIONS_BASE` when set, else `<koto root>/sessions`,
while the cloud backend's local mirror, which the dashboard reads, is always
`<koto root>/sessions`; and `koto session dir --json` prints the koto root,
not the sessions directory. And `koto context get` doesn't always read the
store: when the latest event touching a key is a transition's assignment it
answers from the log (`outstanding_assignments`), it always records the
read, and under the cloud backend it can pull and push.

Key assumptions:

- Sessions still in the old per-repo layout, which `LocalBackend::new`
  migrates, aren't seen until koto has opened the store once. This is a
  documented limit.
- The crate may depend on `dirs`, which has no network or write code, for
  the home directory, as koto does.

#### Chosen: A scoped, additive-only read promise in STABILITY.md, single-sourced in the crate and guarded by a layout marker; an explicit-path API with a pure root resolver koto also calls; a four-outcome context read that matches `koto context get`

*What is promised.* A new STABILITY.md section, "Session store read
surface", promises exactly what the reader reads:

1. A session is a directory directly under the sessions directory that holds
   `koto-<id>.state.jsonl`. Directories starting with `.koto-staging-` and
   names containing `~` aren't sessions; any other entry without that file
   is ignored, so koto can add files and dot-directories freely.
2. Context content lives at `<session dir>/ctx/<key>` under the key grammar
   of `validate_context_key`, hierarchical keys as subdirectories, the
   content as raw bytes.
3. The leg pointer is `<session dir>/request-leg.toml` with its three fields,
   and a bound leg is a pointer that parses.
4. The compiled template is found by the session's path rule, and a reader
   uses only `states.<name>.terminal` from it.
5. Exactly four events set or supersede a context key: a `transitioned`
   event's `context_assignments`, `context_added`, `context_removed` and
   `context_cleared`.
6. The layout marker, below.

"Additive" means an older reader stays correct, not just that it still
parses. A fifth event that sets a key, or a template fallback an older
reader would miss, is a breaking change. The manifest's shape, lock files,
the run journal, the read spool and finished records stay unpromised and are
described in `docs/workspace-layout.md`.

*How it's kept.* Each name and rule is defined once in the crate
(`state_file_name`, the staging prefix, the `~` rule, the ctx path,
`POINTER_FILENAME`, `resolve_template_path_in_session`, the terminal
projection, the set of superseding events, the marker name), and koto's
write side imports them. `koto-stability-tests` pins each one as a literal,
so a rename that would move koto and the crate together, orphaning every
older reader, fails CI and points at the section. Terminal detection, in
the crate and so in koto, reads a minimal projection,
`{states: {<name>: {terminal}}}`, that accepts unknown fields. On every
template a current koto parses, its answer is the same as today's.

*Breaking changes.* The marker is `<sessions dir>/.koto-store-layout`, and a
missing file means layout 1, so nothing is written today. The check sits in
the crate's `scan_sessions`, so `koto dashboard` makes it too and the two
can't diverge. The reader checks it before listing and refuses the whole store with a named error, giving
the marker's path and the found and supported numbers, when the number is
higher than it knows, as the log refuses a higher `schema_version`. A marker
that isn't a regular file, is over 64 bytes or doesn't parse as a number is
refused the same way rather than guessed at. A breaking change to the
promised surface goes through STABILITY.md's major-bump protocol, with a
crate release that reads both layouts shipped first; only then does a koto
release's migration write the marker. The marker sits in a directory any
session's commands can write, so a writer can plant a high number and make
every dependent refuse the store. That availability risk is accepted: the
same writer can already delete or rewrite every session, and the refusal
is loud and names the file to remove.

*How the store is read.* The dashboard's scan, in the crate and so in
`koto dashboard` too, reads each state file, compiled template, leg pointer,
context value and the marker through `read_store_file` with a fixed cap per
kind: 512 MiB for a state log, 32 MiB for a template, 64 KiB for a leg
pointer or the marker, 16 MiB for a context value. The bytes then go to the
same parsers koto's commands use (`parse_state_log`, the terminal
projection, the pointer parser). A symlinked, non-regular or over-cap file
marks that one session or value unreadable with its reason, and the scan
goes on. The template read uses `Template` mode, which follows a symlink
and skips the containment check, since a template given with `--template`
lives outside the store, but keeps the non-blocking open, the regular-file
check and the cap; the detail view's template read goes the same way.
Listing skips a symlinked session directory, as `LocalBackend::list` already does.
On every store koto itself writes, none of this changes a row; the
reviewer run in Decision 5 records the largest state log in the reference
store, which must sit well under its cap.

*Finding the root.* The crate's source constructor takes an explicit
sessions directory, and nothing reads the environment implicitly. Beside it
the crate exports `sessions_dir(backend: Option<BackendKind>)`, koto's own
rule moved in: the koto root is `$KOTO_HOME` when non-empty, else `~/.koto`;
`Some(Cloud)` gives `<root>/sessions`; `Some(Local)` or `None` gives
`KOTO_SESSIONS_BASE` when set, else `<root>/sessions`. koto's backend
constructors call it with `Some`, so the rule has one owner. A dependent
passes `None`, which is exact inside every tick and outside one except when
the cloud backend is configured and someone exported `KOTO_SESSIONS_BASE` by
hand, a variable koto documents as for testing. `koto session dir --json`
gains a `sessions` field from the same function, for a caller that wants
certainty and has koto on PATH. It still builds no backend and writes
nothing.

*Reading one context value.* `read_context(session_dir, key, events:
Option<&[Event]>) -> ContextRead` takes no lock, records nothing, calls no
backend and never fails the scan. It returns one of:

- `Unusable(reason)`: the key fails the grammar, or is `manifest.json` or
  `manifest.lock` directly under `ctx/`, which only ever hold koto's own
  files.
- `Present { bytes, from: Log }`: given `events`, `outstanding_assignments`
  shows a transition assigned the key, so the value comes from the log
  exactly as `koto context get` answers. This covers the lost store write.
- `Absent`: `ctx/`, an intermediate directory or the file doesn't exist.
- `Unreadable(reason)`: a symlink at any component (checked with
  `symlink_metadata` on each, then an `O_NOFOLLOW` open of the leaf and a
  containment recheck of the opened path), a non-regular leaf (opened
  non-blocking and re-checked with `fstat`, so a FIFO can't hang the scan),
  a value over 16 MiB, or any I/O error. Refusing symlinks is a deliberate
  difference from `koto context get`; koto never writes one.
- `Present { bytes, from: Store }`: a regular file, returned whole.

A value from the log comes from what a transition assigned, not from
`ctx/`, so a dependent that reasons "I only read `ctx/`" is wrong in the
same way it would be about `koto context get`.

The host-local key `workflows/publish-location` reads like any other key and
matches `koto context get` on this host. Two limits are documented: a read
racing koto's unlocked content write can see a torn value that the next
refresh corrects, as `koto context get` can, and under the cloud backend the
read returns this host's mirror, matching the dashboard's local rows.

#### Alternatives Considered

**The API is the contract and nothing is promised.** Layout changes land in
the same commit as the crate. Rejected because same-commit coupling doesn't
reach a binary pinned to an older crate revision: a rename makes it misread
silently, and nothing written obliges koto. Its single-sourcing is kept.

**Freeze the whole layout in STABILITY.md.** Rejected because it promises
the manifest, lock files and the full compiled-template shape the reader
never reads, and a full template parse breaks on `deny_unknown_fields`. Its
additive rule and protocol survive, narrowed.

**A descriptive reference doc, with the caller asking `koto session dir
--json` for the root.** Rejected because a description obliges nothing, the
command prints the koto root rather than the sessions directory and ignores
`KOTO_SESSIONS_BASE`, and it needs koto on PATH. Its new `sessions` field
and the reference doc for unpromised files are kept as complements.

**A layout number in `koto version --json`.** Rejected because it describes
a binary, not the store on disk, which another koto may have written.

### Decision 4: How a dependent takes the crate and how versions tie

koto publishes nothing to a registry. A release runs `.release/set-version.sh`,
which rewrites the root `Cargo.toml`'s version lines, runs `cargo update
--workspace`, checks the `koto` lock entry and tags `vX.Y.Z`; main then
advances to the next `-dev` version. The log carries its own version:
`CURRENT_SCHEMA_VERSION` is 1, additive changes never bump it, and a bump
happens only in a major release with a 6-week window.

Key assumptions:

- `cargo package -p koto-session-reader` runs in CI on the crate as
  declared. If cargo refuses a `publish = false` crate, `publish` stays
  unset, as koto's own package does, and nothing publishes it because no
  token exists.
- The dependent keeps its existing runtime koto floor check for the koto
  commands it runs.

#### Chosen: A git dependency pinned to a koto release tag, the crate versioned in lockstep with koto, kept registry-ready by CI

A dependent writes `koto-session-reader = { git =
"https://github.com/tsukumogami/koto", tag = "vX.Y.Z" }`. Its `Cargo.lock`
records the tag's commit. A revision pin is for testing unreleased work
only; at a revision the crate reads `X.Y.Z-dev`, which says it isn't a
release. Nothing is vendored.

The root manifest gains `[workspace.package] version`, and koto and the
crate both say `version.workspace = true`. The existing `sed` in
`set-version.sh` stamps that one line and `cargo update --workspace` moves
both lock entries, so the crate at tag `vX.Y.Z` is version `X.Y.Z`, with no
second version line and no mapping table. The log schema stays separate on
purpose: STABILITY.md gains a dependent's promise that the reader from koto
release X reads every log whose `schema_version` is at most its own, that
unknown event types come back as `EventPayload::Unknown` and unknown header
fields are ignored, and that a higher `schema_version` marks that one
session unreadable rather than failing the scan. A pin may trail koto and
stay correct for what it knows; row parity with the installed koto's
dashboard holds exactly when the pin equals the installed release.

The dependent doesn't gate reading on the installed koto's version, because
the per-file header check and the layout marker are the real gates and a
store holds logs from several releases. The crate exports the koto version
it was built from, so a dependent may show "reader from koto vA, installed
koto vB" as a note, never a refusal.

Two things keep the versions from drifting. `set-version.sh`'s lock check
extends to the crate's entry, and a validate.yml job reads `cargo metadata
--no-deps` and fails when the crate's version differs from koto's or the
crate declares a path dependency, then runs `cargo package -p
koto-session-reader`, which fails if the crate reaches a file outside its
directory or a dependency not on crates.io. A later registry publish is then
a token and one step after the tag, with the version still koto's.

#### Alternatives Considered

**A git dependency pinned to a revision, the crate with its own version.**
The spike's form. Rejected because a revision on main can match no koto
anyone installed, and an independent version needs its own cadence and a
mapping nothing in the release flow maintains.

**Publish to crates.io now, with its own semver.** Rejected for now: it adds
a token, a publish step and a name and version that can't be withdrawn, for
one dependent that builds fine from a tag, and its own semver brings the
mapping back.

**Vendored source in the dependent.** Rejected because it's the copy the
design exists to prevent.

**The crate keeps its own version under a tag pin.** Rejected because
`set-version.sh` stamps only the root manifest, so that version would never
move or would need a second bump path.

### Decision 5: How dashboard parity and the second program are shown

The milestone asks for three pieces of evidence: `koto dashboard` shows the
same sessions, states and liveness classes on the same store before and
after; a minimal program lists the sessions under one root with their
current states and one context value each, builds without cloud storage,
TLS or HTTP dependencies and reports the dashboard's rows; and a hundred
reads on a cloud host leave the store unchanged with nothing pushed. The
spike produced all three once, by hand. Two facts shape the answer: the
elapsed and idle columns of `--once` tick with the clock and liveness
depends on it through its 5-minute, 2-hour and 7-day thresholds, so a
fixed-timestamp fixture drifts between classes; and koto's CI already has
`tests/support/fake_s3.rs`, an in-process bucket that counts requests and
that `tests/dashboard_cross_machine_test.rs` drives the binary against.

Key assumptions:

- A Linux release build of the program imports no `connect`, `socket` or
  `getaddrinfo` symbol when it doesn't use `std::net`, as the macOS trial
  binary didn't. If std pulls one in anyway, that step becomes an allowlist.

#### Chosen: CI for every property that must keep holding, one reviewer run on the build PR for real scale and a real cloud host

The program is a second workspace member, `koto-session-list/`, `publish =
false`, version 0.0.0, depending on the reader crate by path, modelled on
`koto-stability-tests`. It has two modes: `once [--all]` prints the
dashboard's columns 1 to 8 in the dashboard's order, and `list --root
<name> --key <key>` prints each session under the root with its parent,
current state, liveness and one context value.

CI checks four things:

1. *Dashboard golden, landed on main before the split.* A test builds a
   store at run time with one session per liveness class, an abandoned
   session, a parent with two children, a session with an unknown event
   type, a corrupt state file and a staging directory, with timestamps as
   offsets from the run's clock, each at least 4 minutes from every
   threshold. It compares columns 1, 2, 4, 5, 6 and 8 of `--once` and
   `--once --all`, in order, with a committed expected file, plus the
   unreadable-session note. The split PR passes it without touching the
   expected file or the builder.
2. *Program equals dashboard.* On the same generated store, the program's
   `once` output equals the dashboard's columns row for row; `list` lists
   exactly the root and its descendants with the dashboard's states and
   liveness; and each value equals what `koto context get` prints, run after
   the comparisons because it records the read.
3. *Builds without cloud, TLS or HTTP.* `cargo tree -p koto-session-list -e
   normal,build` names none of rust-s3, rustls*, attohttpc, http, url,
   ratatui or crossterm, and the release binary imports no `connect`,
   `socket` or `getaddrinfo`.
4. *A hundred reads on a cloud-configured host change nothing and push
   nothing.* Sessions are created with koto on the cloud backend against
   FakeS3; the koto root is snapshotted, then the program runs a hundred
   times with the cloud environment still set. Every run exits 0 with empty
   stderr, the snapshot is unchanged, and FakeS3 records zero requests.

Checks 2 and 4 drive the program's binary, so they're `#[ignore]` tests run
by a new validate.yml job after the release build, reading the binary path
from a variable the job sets and failing when it's missing.

The reviewer runs three things once and records them on the build PR: the
spike's three-way diff of `koto dashboard --once` from main, from the PR and
the program's `once`, on a copy of a real cloud host's mirror, both default
and `--all`; the program's `list` on a real root of that copy; and a hundred
sandboxed runs on the live mirror plus a hundred unsandboxed runs on a copy
with a full file manifest before and after.

#### Alternatives Considered

**A reviewer real-store run only.** Rejected because nothing guards the
result after merge, and the drivers ask for a `cargo tree` check that
fails.

**Committed fixed-timestamp golden fixtures only.** Rejected because such
fixtures drift between liveness classes as the calendar moves, and CI has no
real store history or OS sandbox. A clock-relative version survives as
checks 1 and 2.

**The program as a cargo example.** Rejected because an example builds with
its crate's dev-dependencies and `cargo tree` can't select it.

**The program as an integration test.** Rejected because it isn't a program
a reviewer builds and it says nothing about what a dependent builds.

## Decision Outcome

The split is one new workspace member, `koto-session-reader`, that owns the
code `koto dashboard` reads sessions with, and a guard that makes that
member unable to write. Files move with `git mv`; koto depends on the member
by path and re-exports each moved module at its old path, so
`koto::engine::types`, `derive_state_from_log`, `koto::error::Error` and
every internal caller resolve as they do today. koto's commands run the same
function bodies, except that two reads (`read_pointer` and
`is_terminal_state`) gain the hardened open, which changes no answer for a
regular file. Everything that writes stays in koto: the append and
lock half of `engine::persistence`, `jsonl_append`, `write_pointer`,
`SessionVersion::save`, machine identity, template validation (moved onto
koto-local traits by changing only `impl` headers) and the context store's
write paths. The one piece of logic both sides share across that line, the
merged-last-line detector, becomes a pure `plan_append` in the crate, so
koto's repairing write and the crate's tolerant read can't drift.

"Cannot write" then has three layers that CI runs: a per-crate clippy ban
list over the `std::fs` write surface, process and socket types; a
`forbid(unsafe_code)` that makes libc calls impossible; and a dependency
allowlist plus a guard script that closes the `#[allow]` escapes and keeps
every file open in one module. That module's hardened read, which refuses
symlinks inside the store, never blocks on a FIFO, accepts only regular
files and caps what it reads, is how the dashboard scan, the leg pointer,
the context read and the layout marker touch a store any session's
commands can write to. koto's own commands keep reading their logs and
templates as they do now.

A second program uses the crate by giving it a sessions directory, usually
from `sessions_dir(None)`, which is koto's own root rule moved into the
crate. It lists sessions through a three-method `SessionSource`, derives
rows, liveness and the tree with the dashboard's code, and reads one context
value per session with `read_context`, which answers from the log when a
transition assigned the key and from `ctx/<key>` otherwise, refusing
symlinks and never locking or recording. What it relies on about the store
is written into a new STABILITY.md section that promises only the read
surface: the session directory and state file rule, `ctx/<key>`, the leg
pointer, the template path rule with only `terminal` read, and the four
events that set a context key. A layout marker that is absent today lets a
future breaking change make old readers refuse instead of misread.
Terminal detection reads a minimal template projection that tolerates
unknown fields, which removes the one way an older reader would show a
finished session as live.

Dependents pin a koto release tag. The crate carries koto's version through
`[workspace.package]`, so the tag says which reader it is, and the log
schema keeps its own version under the existing bump protocol. CI keeps the
crate packageable so a registry publish stays a later one-step change.

The work is shown by a dashboard golden test that lands on main before the
split, three more CI checks against a second workspace member,
`koto-session-list` (program equals dashboard, no cloud, TLS or HTTP in its
graph, and a hundred reads on a FakeS3-backed cloud host change nothing and
send no request), and one reviewer run on a real cloud host's store recorded
on the build PR.

The pieces reinforce each other. Owning the files is what lets the write
guard scan one directory and lets the layout constants be defined once
where koto's writer imports them. Writing the layout promise down is what
makes a trailing tag pin safe, and the lockstep version is what lets a
dependent tell which promise its pin carries. The trade-offs accepted are a
two-package workspace with about 23 newly `#[doc(hidden)] pub` helpers, a
reader version on every koto release whether the reader changed or not, and
a narrow set of store names koto can no longer change without the major
protocol.

## Solution Architecture

### Overview

After the split the repository is a three-member workspace: koto, the new
`koto-session-reader`, and `koto-session-list`, beside the existing
`koto-stability-tests`. `koto dashboard` and any outside program call the
same crate functions to list, read, derive and classify. koto alone writes.

### Components

```
koto-session-reader/          (new member; no writer, no network)
  Cargo.toml                  version.workspace = true; deps on the allowlist
  clippy.toml                 write, process and socket ban list
  src/lib.rs                  forbid(unsafe_code); deny(disallowed_*)
  src/engine/types.rs         frozen types, EventPayload (moved whole)
  src/engine/errors.rs
  src/engine/persistence.rs   read half + plan_append + fence readers
  src/engine/leg_pointer.rs   read_pointer and friends
  src/engine/terminal_result.rs
  src/engine/context_assign.rs  outstanding_assignments only
  src/decider/{check,evaluate,record,request,types}.rs
  src/findings.rs, src/redact.rs
  src/template/{types,assignments,decider,decider_check,result_map}.rs
  src/session/{row,validate,version}.rs   version: validate_machine_name only
  src/dashboard.rs            liveness, CachedSession, SessionTree, refresh
  src/source.rs               SessionSource, LocalDirSource, sessions_dir,
                              read_context, layout constants and marker
  src/fs_read.rs              every file open: open_plain, read_store_file
                              (the one custom_flags call), read caps

koto (root package)
  src/engine/mod.rs           pub use koto_session_reader::engine::types; ...
  src/engine/persistence.rs   mixed: write half + pub use of the read half
  src/template/types.rs       mixed: ValidateTemplate and friends + tests
  src/session/version.rs      mixed: machine identity, SessionVersion
  src/cli/dashboard_data.rs   mixed: bucket overlay, refresh wrapper,
                              impl SessionSource for LocalBackend
  src/cli/dashboard*.rs       TUI and formatting, unchanged

koto-session-list/            (new member; publish = false; the minimal program)
koto-stability-tests/         gains glob-import and layout-literal tests
scripts/check-reader-readonly.sh
```

### Key Interfaces

```rust
// koto-session-reader::source
pub trait SessionSource {
    fn list(&self) -> anyhow::Result<Vec<SessionInfo>>;
    fn session_dir(&self, id: &str) -> PathBuf;
    fn count_unreadable(&self) -> usize;
}
pub struct LocalDirSource { /* sessions directory */ }
impl LocalDirSource {
    pub fn new(sessions_dir: impl Into<PathBuf>) -> Result<Self, LayoutError>;
}
pub enum BackendKind { Local, Cloud }
pub fn sessions_dir(backend: Option<BackendKind>) -> anyhow::Result<PathBuf>;

// Debug prints the variant, the source and the byte length, never the bytes.
pub enum ContextRead {
    Unusable(String),
    Absent,
    Unreadable(String),
    Present { bytes: Vec<u8>, from: ContextSource },
}
pub enum ContextSource { Log, Store }
pub fn read_context(session_dir: &Path, key: &str, events: Option<&[Event]>)
    -> ContextRead;

pub const STORE_LAYOUT_MARKER: &str = ".koto-store-layout";
pub const STORE_LAYOUT_VERSION: u32 = 1;
pub const KOTO_VERSION: &str = env!("CARGO_PKG_VERSION");
```

`LocalDirSource::new` reads the marker and returns a `LayoutError` naming
the found and supported numbers when the store is newer than the crate. The
dashboard functions (`scan_sessions`, `refresh`, `classify_liveness`,
`attention_key`, `read_detail`) take any `&dyn SessionSource`; koto passes
its `LocalBackend` (or the cloud backend's local mirror), a dependent passes
a `LocalDirSource`. `derive_row` keeps taking facts from beside the log,
such as the request-leg flag, as arguments from its caller.

### Data Flow

For `koto dashboard`: koto builds its backend from config as today, hands
`cloud.local()` or the `LocalBackend` to the crate's `refresh` as a
`SessionSource`, and then applies its bucket overlay (`drop_shadowed_remote`,
remote rows) in its wrapper before rendering. The crate stats each session's
state file, re-reads only those whose modification time changed through
`read_store_file`, parses header and events with `parse_state_log`,
resolves the template path, reads `states.<name>.terminal` through the
minimal projection, reads the leg pointer, and derives rows, liveness and
the tree.

For a dependent: it calls `sessions_dir(None)`, constructs a
`LocalDirSource` (which checks the marker), and calls the same `refresh`.
For each session it wants a value from, it calls `read_context` with the
session's events, which it already holds. Nothing in that flow opens a file
for writing, takes a lock, starts a process or opens a socket. A session
whose log is newer than the reader, or whose source is malformed, comes back
marked unreadable with its reason, and every other row stays intact.

### Room for koto#556

The boundary leaves room for external signals without deciding them.
`EventPayload::Unknown` stays the tolerant fallback, so a signal recorded as
a new event type reads on older readers. `derive_row` takes facts from
beside the log as caller-supplied inputs, so a signal kept in a sidecar
reaches a row the way the request-leg flag does today. A bad source marks
one session through the same unreadable path rather than failing the scan.
If a signal sidecar becomes something readers rely on, it joins the promised
read surface through the additive rule, and the guard keeps its reading in
the crate read-only.

## Implementation Approach

Each phase is one pull request, and each keeps `koto dashboard` unchanged.
The first two land before any file moves.

### Phase 1: Dashboard golden test

The clock-relative golden test (Decision 5, check 1) lands on main as a
test-only change, with its expected file produced by main's own dashboard.

Deliverables:
- `tests/dashboard_golden_test.rs` and its store builder
- the committed expected output for `--once` and `--once --all`

### Phase 2: Split in place

Inside koto, with no new crate: the persistence write half moves into its
own file and `plan_append` is extracted; validation moves onto the
`ValidateTemplate`, `ValidateSteps`, `ValidateVocabulary` and
`VariableDeclExt` trait headers; the dashboard's bucket overlay becomes free
functions with the `refresh` wrapper; `SessionSource` is introduced and
`LocalBackend` implements it. The golden test passes unchanged.

Deliverables:
- the write-half file under `src/engine/`
- trait headers in `src/template/types.rs` and `src/template/variables.rs`
- `SessionSource` and the overlay wrapper in `src/cli/dashboard_data.rs`

### Phase 3: Create the crate and move the files

The member is created, files move with `git mv`, the re-exports and mixed
modules go in, and the crate's write guard lands in the same pull request:
`clippy.toml`, the `lib.rs` attributes, `scripts/check-reader-readonly.sh`
with its self-test, and the Reader Write Guard job. CI adds `-p
koto-session-reader` test, clippy and coverage steps, because the root
`cargo test` runs only the root package, and `koto-stability-tests` gains a
glob-import test. The public-API snapshot taken before the move is diffed
after.

Deliverables:
- `koto-session-reader/` with its manifest, `clippy.toml` and moved sources
- `koto-session-reader/src/fs_read.rs`, with every existing open routed
  through `open_plain` and `read_pointer` switched to `read_store_file`
- the dashboard data layer's fixture-building unit tests moved to koto's
  `tests/`
- `[workspace.package] version` and `version.workspace = true` on both
- the guard script and the validate.yml jobs

### Phase 4: The read surface

`LocalDirSource`, `sessions_dir`, `read_context`, the layout constants and
the marker check land in the crate, and the dashboard scan reads through
`read_store_file` with its caps; koto's backends call `sessions_dir` and
import the constants; `koto session dir --json` gains `sessions`.
docs/STABILITY.md gains the "Session store read surface" section and the
dependent's compatibility promise, `docs/workspace-layout.md` gains the
descriptive entries, and `koto-stability-tests` gains the layout literal
pins. `set-version.sh` checks the crate's lock entry and the version and
packaging job lands.

Deliverables:
- `koto-session-reader/src/source.rs` and the capped scan in
  `src/dashboard.rs`
- tests in koto's `tests/` that build a store at run time with a FIFO, a
  symlink, an over-cap file and a planted marker, each marking one session
  or refusing the store as described, and the golden test unchanged
- protected `v*` tags on the koto repository, a repository setting the
  release owner turns on before the first dependent pins a tag
- the STABILITY.md and workspace-layout.md sections
- the version-equality and `cargo package` CI job

### Phase 5: Terminal projection

Terminal detection switches to the minimal projection that tolerates
unknown fields, and `is_terminal_state` reads the template through
`read_store_file` in `Template` mode for every caller. This is its own
change because the `/workflows` projection that runs after each commit
calls `is_terminal_state`; it changes no answer on any template a current
koto parses, which the golden test and a template-corpus test show.

Deliverables:
- the projection and the hardened read in
  `koto-session-reader/src/engine/persistence.rs`
- a test over every template under `plugins/` and the test fixtures

### Phase 6: The minimal program and its checks

`koto-session-list` lands with its two modes and the CI checks 2 to 4 from
Decision 5. The build pull request carries the reviewer's real-store run.

Deliverables:
- `koto-session-list/` with `once` and `list`
- the `#[ignore]` parity and hundred-read tests and their validate.yml job
- the reviewer evidence recorded on the pull request

## Security Considerations

**Trust model.** The crate reads a sessions directory that other processes
can write to, including agents running arbitrary commands under the same
user. Everything it reads (state logs, compiled templates, leg pointers,
context values, the layout marker) is untrusted input, and what it reports
is what those writers recorded. A program built on it displays
agent-written state; it isn't a monitor of agents, and "status comes from
koto" means it comes from koto's records, not that koto vouches for them.
An agent that wants a session to look finished can already write that log.

**Untrusted files.** Every read of a path inside the store, and of the
compiled template a log names, goes through `read_store_file`: a
non-blocking open, a regular-file check with `fstat`, a byte cap per kind
of file, and for paths inside the store `O_NOFOLLOW` plus a containment
recheck of the opened path. A symlink, FIFO, device or over-cap file marks
that one session or value unreadable with a reason, and the scan goes on;
nothing a writer plants can hang the scan, and each file it reads is capped
(the caps are per file, so a store of many large files still costs time in
proportion). koto's commands reading their own session's log through
`read_events` keep today's plain read; a dependent that calls `read_events`
on a path itself gets that behavior, and the supported way to read a store
is the scan and `read_context`. A
crafted `template_path`, absolute or not, goes through the same read, so it
can at worst mark one session unreadable or set that session's terminal
flag. The template isn't checked against `template_hash`, as `koto
dashboard` doesn't check it today; under the trust model that is
acceptable, because the writer who could plant a template could equally
write the log. The containment recheck after an open leaves a narrow race
in which a swapped directory makes the reader return another file the user
can read, capped, as one value. That grants a writer nothing it lacks when
it runs as the same user as the reader, which is the case this design
assumes, since it could copy the same file into `ctx/` directly. An agent
confined by a sandbox that can't read a file the reader can is the case
where the race matters; a dependent running a reader for such agents
should treat values from `ctx/` as able to hold any file the reader can
see. The layout marker
is a store-wide switch a writer can flip; that availability risk is
accepted for the same reason (it can already delete every session), and
the refusal names the file.

**What "cannot write" covers.** The crate's own source can't write, lock,
spawn a process, resolve a host name or open a socket, and koto's CI
enforces that at every tag (Decision 2). It isn't a process sandbox: a
dependency's code and the dependent's own code run with the caller's
privileges, which is why the dependency set is an allowlist checked for
name and registry source.

**Supply chain.** A dependent takes the crate from koto's repository at a
release tag, and its `Cargo.lock` records the commit, so a tag moved later
can't change a locked build. The first resolve does trust the tag, so koto
protects its release tags and a dependent builds its release with
`--locked`. The crate has no `build.rs`, so fetching koto's repository runs
nothing. A dependent resolves the allowlisted crates at versions of its own
choosing and audits them as it does any dependency.

**Data exposure.** Context values come back as raw bytes, up to 16 MiB,
from `ctx/` or from what a transition assigned in the log, unredacted. A
dependent that prints, stores or forwards them escapes terminal control
sequences and applies its own redaction; a fleet view does better to show
that a key exists than its value by default. The host-local key
`workflows/publish-location` names a folder on this host and doesn't
belong in anything shared across hosts. Error reasons carry absolute paths,
which reveal the home directory and user name if forwarded, and never any
of a file's contents; `ContextRead`'s `Debug` prints a value's length, not
its bytes. `sessions_dir`
reads `KOTO_HOME` and `KOTO_SESSIONS_BASE`, so a process that controls the
reader's environment chooses its store, as it does for koto; a dependent
running where the environment isn't trusted passes an explicit directory.

## Consequences

### Positive

- `koto dashboard` and the fleet view run one copy of row derivation,
  liveness, the diff and the tree, so they can't disagree about a session.
- A program can read the store without linking any storage, TLS, HTTP or
  terminal crate, and a reviewer audits "cannot write" by reading one
  directory and one CI job.
- A new dependency from reader code on koto-only code fails a local build
  rather than a late CI job.
- The store layout outside readers rely on is written down for the first
  time, with a guard that turns a future breaking change into a refusal.
- Terminal detection stops reporting finished sessions as live when an older
  binary meets a newer template.
- koto's writer and reader share one merged-line detector instead of two.
- A registry publish, if a second outside consumer appears, is one step.

### Negative

- koto becomes a multi-package workspace, and CI has to name the new member
  in its test, clippy and coverage steps or the moved tests quietly stop
  running.
- About 23 helpers become `#[doc(hidden)] pub` on koto's paths.
- Validation call sites need a trait import, and the mixed modules carry a
  shadowing hazard.
- koto can no longer rename the state file, the `ctx/` layout, the leg
  pointer file, the template path rule or the set of context-setting events
  without the major protocol.
- Every koto release produces a reader version whether or not the reader
  changed, and a reader fix needs a koto release.
- A dependent's build fetches koto's whole repository at the tag.
- The ban list follows std: a write API added in a future Rust release isn't
  banned until someone lists it.
- Dashboard data-layer unit tests that build fixtures with `append_event`
  and `tempfile` move to koto's test tree; the crate's own tests read
  committed fixtures only.
- On a pathological store (a FIFO, a symlink or an over-cap file where a
  state file, pointer or context value belongs) `koto dashboard` now marks
  that session unreadable where today it would hang or read through, and a
  planted or unparseable layout marker makes it refuse the whole store with
  the marker's path. koto's commands that read a leg pointer or check a
  terminal state refuse the same pathological files. Those are the only
  places behavior changes, and no store koto writes has them.

### Mitigations

- The Reader Write Guard job and the per-member CI steps land in the same
  pull request that creates the crate, so there's no window where the moved
  tests don't run.
- The `#[doc(hidden)]` helpers stay outside STABILITY.md, which says so.
- The rule that a mixed module defines no name the crate exports is checked
  by the compiler's ambiguity errors and named in the module headers.
- The promised read surface is narrow on purpose; everything else in a
  session directory stays free to change, and a breaking change has a
  dual-layout crate release and the marker to carry it.
- Lockstep versions mean a dependent never needs a mapping table, and the
  tag pin keeps its build reproducible.
- `clippy.toml`'s `File::lock*` entries carry `allow-invalid` so the MSRV
  build doesn't error, and the guard's self-test plants each violation, so a
  ban that stops matching is found.
