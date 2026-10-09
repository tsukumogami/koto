---
schema: design/v1
status: Current
upstream: docs/prds/PRD-session-migration.md
decision_provenance: inline-resolved
problem: |
  A koto session on the cloud backend lives under a prefix derived from the
  workspace that created it, so nothing on another host or workspace can
  continue it. `rebind` writes its header outside the backend and is undone
  by the next pull, context keys never move, the first event points at a
  compiled template in the origin host's cache, and nothing stops two hosts
  from advancing one session. The PRD asks for a migration: an import into a
  fresh local session and a marker on the old object.
decision: |
  Add `koto session import <name> --from <workspace-path> [--as <new-name>]`.
  It reads the source's remote object (state file, manifest, every context
  key), takes the compiled template from this host's own cache by the hash
  the session recorded (or, with `--trust-template`, from a copy the cloud
  backend now pushes at init), stages
  a new local session anchored where it runs with a new header and the
  source's events plus a `session_imported` event, pushes it under this
  workspace's prefix, moves it into place, and only then writes
  `migrated.json` beside the source. Every cloud read of state or context
  checks that object once per process and refuses with `session_migrated`.
  Header rewrites move behind a backend method that pushes (koto#310), a
  `session.cloud.path_style` key makes IP endpoints work, and an integration
  test with an in-process S3-compatible endpoint measures the whole move.
rationale: |
  A sibling marker object is the one form a source host's own pushes can't
  erase, and checking it once per process keeps the cost at one request per
  command. Staging the target and writing the marker last means every
  failure before the marker leaves the source untouched and every failure
  after it is completed by re-running the same command. Taking the
  template from the importer's own cache keeps a bucket writer from
  supplying the commands the imported session runs, and resolving it from
  the session directory when the recorded path is missing keeps the copied
  log byte-identical. An in-process endpoint lets the harness run on every pull
  request without secrets. koto#239's lock is deferred behind a
  stopped-source rule, because the import never writes the source's log.
---

# DESIGN: Session migration

## Status

Current

**Note (2026-10-09), departures in the implementation.** The import's
move into place takes no lock: review found that the staging directory is
private and complete before the move, and the rename never replaces an
existing directory (`atomic_rename_dir`), so nothing can open the target
before it exists and a lock would guard nothing. Decision 8 and step 10 of
the import sequence first said the move ran under the target's local lock.
And
`SessionBackend::rewrite_header` has a default (rewrite the local file, then
confirm the push with `ensure_pushed`) instead of a `LocalBackend` override,
so a future syncing backend pushes header rewrites without overriding it;
`CloudBackend` overrides it to push the way it pushes events. Three smaller
changes followed review: the marker check lists instead of GETting, to avoid
rust-s3's retry sleep on the expected 404; an import re-run that finds its
own marker on the source reports `template: unchanged` instead of refusing;
and the sections below now say so.

## Context and Problem Statement

The requirements are in `docs/prds/PRD-session-migration.md`; this design
cites them by number. What follows is the technical picture they land on.

`CloudBackend` (`src/session/cloud.rs`) wraps `LocalBackend` and mirrors a
session to `<prefix>/<name>/` in the bucket, where `<prefix>` is the first 16
hex characters of the SHA-256 of the working directory's canonical path
(`repo_id` in `src/session/local.rs`). The state file is pushed whole after
every append and pulled whole on every `read_header` and `read_events`,
overwriting the local file. Context keys go per key through
`src/session/sync.rs`, with `ctx/manifest.json` and a `version.json` counter
that guards context writes only. Under the cloud backend the local store is
always `$HOME/.koto/sessions`: `KOTO_SESSIONS_BASE` is honored only by the local
backend.

Six facts in the code shape the design:

1. **The header pins a session to its host.** `execution_dir` is checked on
   every tick (`check_execution_anchor`); a path that doesn't resolve refuses
   with `execution_anchor_unresolvable`. `origin` is compared by
   `koto init --attach-live`. `command_environment` records the creating
   host's `PATH`, `HOME` and `XDG_CONFIG_HOME`, and commands run with those
   values.
2. **The template is a host path.** `WorkflowInitialized.template_path`
   records `$HOME/.cache/koto/<sha256>.json` as an absolute path for file
   templates (a session-relative `<sha256>.json` for `--from-stdin` ones).
   `koto next` reads it and checks its SHA-256 against the header's
   `template_hash`; a missing file is a template error. The cache is not
   synced.
3. **Header rewrites bypass the backend.** `handle_rebind`
   (`src/cli/session.rs`), the first-tick anchor adoption in `handle_next`
   (`src/cli/mod.rs`) and `init_child.rs` call `rewrite_header_atomically` on
   the local file after an event has been pushed, so the remote keeps the old
   header and the next pull restores it. This is koto#310.
4. **Terminal cleanup deletes the prefix.** `finish_terminal_tick` calls
   `backend.cleanup` unless the outcome is a failure or `--no-cleanup` was
   given; the cloud `cleanup` deletes every object under the session prefix.
5. **Errors from rust-s3 are typed by status.** The crate is built with
   `fail-on-err`, so a 404 comes back as an `Err` carrying the status, not
   as an `Ok` with a 404 code. Telling "absent" from "unreachable" means
   matching that status.
6. **Buckets are virtual-host addressed.** `create_bucket` never calls
   `with_path_style()`, so `http://127.0.0.1:9000` fails with
   `url parse: invalid IPv4 address`.

## Decision Drivers

- The source's log and keys are never written by the import (PRD R7, R12):
  the marker is the only object it adds under the source's prefix.
- No failure strands a half-migrated session (R9, R11): either nothing
  changed, or re-running the same command finishes the job.
- The copied state log stays byte-identical to the source's events (R3).
- One extra remote request per command at most, none on the local backend
  (R19).
- The harness runs on every pull request, without secrets (R15).
- Nothing new for agents to learn beyond one verb and one error code.

## Considered Options

Every question below was settled inline while the feature was scoped, as an
ordinary choice; none needed a separate decision record.

### Decision 1: The verb and how it names the source

The source lives under a prefix computed from a path on another host.

- **Chosen: `koto session import <name> --from <workspace-path> [--as
  <new-name>]`.** `--from` takes the source workspace's absolute path as it
  was on the creating host (the origin anchor its header records). A new
  helper, `prefix_for_workspace` in `src/session/local.rs`, hashes the
  canonical form when the path resolves here, exactly as `repo_id` does, and
  otherwise hashes the literal absolute path, which equals the origin host's
  `repo_id` whenever the user passes the canonical path. `repo_id` itself
  keeps refusing a path that doesn't exist. The
  verb sits under `koto session` next to `rebind` and `resolve`.
- **Alternative: find the session by name across the bucket.** List every
  prefix, HEAD `<prefix>/<name>/koto-<name>.state.jsonl` in each, and import
  the single live match. Rejected for now: it costs one request per
  workspace that ever used the bucket, and two workspaces holding the same
  name, which machine-wide names make common, force a second disambiguating
  flag anyway. It can be added later as the behavior when `--from` is
  omitted.
- **Alternative: `--prefix <16-hex>`.** Exact, but the prefix is a hash no
  user can read off anything they have. Rejected.

### Decision 2: The marker's form

- **Chosen: a sibling object, `<prefix>/<name>/migrated.json`.** It holds
  `{schema: 1, target: {session, session_id, workspace, prefix},
  machine_id, migrated_at}`. Reads check for it with one listing request
  keyed on its name; absent means not migrated, and a failed listing means
  "can't tell" and the command proceeds with a warning (PRD's fail-open
  decision). Only a listed marker is fetched, and a listed marker refuses
  even when its body can't be read. The source host's own pushes
  only ever PUT the state file, context keys, manifest and version record,
  so nothing it does overwrites the marker.
- **Alternative: object metadata on the state file.** A header such as
  `x-amz-meta-koto-migrated-to` costs nothing extra on a pull, since the GET
  already returns headers. Rejected: the next state push from the source host
  replaces the object and drops the metadata, so the source erases its own
  lock with the very write the lock should have stopped. That push can be
  in flight while the import runs.
- **Alternative: append a `session_migrated` event to the source's remote
  log.** Readers would refuse while deriving state, with no extra request.
  Rejected: the import would write the source's state file (PRD R7, R12
  forbid it), and the source's next push, built from its local log, removes
  the event the same way it would remove metadata.

### Decision 3: Carrying the compiled template

The compiled template is what a session executes: its states' gates and
actions are shell commands. Today a pull replaces the state file but the
template always comes from the host's own cache, so a bucket writer can't
choose the commands a host runs. The import must not change that by default.

- **Chosen: the importer's own cache by default; the bucket's copy only on
  request.** The import looks for `<template_hash>.json` in this host's
  template cache, which `koto template compile <source>` on the importer's
  own checkout fills. When it isn't there, the import refuses
  `import_template_unavailable`, naming the hash and the template file name
  the header records. With `--trust-template`, the import instead takes
  `<prefix>/<name>/template.json`, which `CloudBackend::init_state_file` now
  pushes at init, after checking its SHA-256 against `template_hash` and
  parsing it as a compiled template; the output says the template came from
  the bucket. Either way the import writes it into the new session directory
  as `<template_hash>.json`. `resolve_template_path_in_session`
  (`src/engine/persistence.rs`), the one seam every state reader goes
  through, gains a fallback: an absolute `template_path` that doesn't exist,
  and whose file name is exactly 64 hex digits plus `.json`, resolves to that
  file in the session directory when it exists. The log keeps the origin's
  path byte for byte.
- **Alternative: always take the bucket's `template.json`.** Smoothest for
  the operator, and the hash check proves the bytes weren't altered after
  the source pushed them. Rejected as the default: the hash comes from the
  same bucket, so it proves integrity, not origin, and anyone who can write
  the bucket could have an importing host run arbitrary commands.
- **Alternative: rewrite `template_path` in the copied
  `WorkflowInitialized` event to the session-relative form.** The resolver
  already handles that, with no fallback. Rejected: it edits a source
  event, which breaks the byte-identical carry (R3), and a reader comparing
  the two logs can no longer tell what the source recorded.
- **Alternative: recompile from `template_source_dir` on the new host.**
  Rejected: that directory is a path on the origin host, often absent on
  the new one, and the import can't tell which checkout the operator means
  to trust. Asking the operator to compile names the checkout explicitly.

### Decision 4: Ordering the import's writes

- **Chosen: stage, push, move, mark.** The import builds the target in a
  staging directory beside the session store, pushes it under this
  workspace's prefix, renames the staging directory into place, and writes
  the marker last. A failure before the rename deletes the staging directory
  and whatever it pushed (`import_push_failed`, or the refusal that stopped
  it); a failed marker PUT leaves a complete target and exits
  `import_unmarked`. Re-running the same import finds a target whose
  `session_imported` event names the same source session id and writes only
  the marker.
- **Alternative: mark first, then import.** Claims the source before doing
  any work, which looks safer against two importers. Rejected: a failure
  after the marker strands the session with a lock pointing at nothing,
  which is worse than the race it prevents, and the stopped-source rule
  already makes concurrent imports an operator error (PRD Known
  Limitations).

### Decision 5: Making header rewrites persist (koto#310)

- **Chosen: a backend method.** `SessionBackend::rewrite_header(id, f)`
  runs `rewrite_header_atomically` on the local file; the cloud backend then
  pushes the state file the way `append_event` does. `handle_rebind`, the
  anchor adoption in `handle_next` and the rewrite in `init_child.rs` call
  it instead of the free function, which stops being reachable from
  `src/cli/`.
- **Alternative: push from each call site.** Three `ensure_pushed` calls
  fix today's bug. Rejected: the fourth rewrite someone adds repeats it, and
  `DESIGN-backend-state-persistence.md` already says state I/O goes through
  the backend; this closes the gap it left.

### Decision 6: Path-style addressing

- **Chosen: `session.cloud.path_style` (bool, default false), allowed in
  project config.** `create_bucket` calls `with_path_style()` when it is
  true.
- **Alternative: switch to path style automatically when the endpoint host
  is an IP literal.** Rejected as the only mechanism: a self-hosted MinIO
  behind a hostname still needs path style, so the key is needed anyway.
  Auto-detection on top would make two ways to get one behavior.

### Decision 7: The harness's endpoint

- **Chosen: an in-process S3-compatible endpoint inside the integration
  test.** `tests/support/fake_s3.rs` serves path-style GET, PUT, HEAD and
  DELETE and ListObjectsV2 (prefix and delimiter) from memory on a
  `127.0.0.1` port, records every request, and can be told to fail a PUT to
  a given key. The test runs in the CI unit-test job. With the
  `cloud-integration-tests` feature and the existing bucket secrets, the same
  steps run against the real bucket.
- **Alternative: a MinIO service container in CI.** Real S3 semantics, but
  it adds a container to every run and can't inject a single failed PUT,
  which R11's test needs. Rejected as the default; the real-bucket run covers
  fidelity.

### Decision 8: The state log under the carrier (koto#239)

- **Chosen: defer the lock, with a stopped-source rule.** An import is
  defined for a source no process is advancing and whose last write reached
  the remote; the help text and the guide say so, and how to force a final
  push (`koto session resolve <name> --keep local` on the source host). The
  import builds the new session in a private staging directory and moves it
  into place with a rename that never replaces, so no other process can open
  it before it is complete.
  koto#239 stays open as its own work: it fixes concurrent writers on one
  host, which the import never adds.
- **Alternative: take koto#239's lock in this feature.** Rejected: the lock
  has to span every writer and needs its reproduction harness rebuilt
  first, the import's own writes go to a session no other process has heard
  of yet, and a version check on state pulls is the documented corrective
  if a migrated log corrupts in use.

## Decision Outcome

Migration is a new verb on the existing cloud backend plus three small
changes underneath it. The verb reads the source's remote object, builds a
target anchored where it runs, pushes it under its own prefix and marks the
source. Underneath: the backend pushes the compiled template and checks for
markers, header rewrites go through the backend, and the bucket can be
addressed path-style. The harness proves the whole move on every pull
request.

The answers to the questions the PRD left open:

- **The verb:** `koto session import <name> --from <workspace-path>
  [--as <new-name>]`.
- **The marker:** `migrated.json` beside the source's state file; never
  deleted by koto.
- **A name collision:** refused with `import_name_taken`; `--as` imports
  under a name the user picks. A target from the same source session id is a
  retry, not a collision.
- **The template path:** the template is found by content hash in the
  importer's own cache (or, opted into, the bucket's copy), stored in the
  session directory, and resolved from there; the log is not edited.

And the three behaviors the PRD names from the code:

- **A non-failure terminal tick deletes the remote prefix.** Cloud cleanup
  now skips `migrated.json`, so neither a terminal tick nor
  `koto session cleanup` removes a marker. A source that finished before it
  was imported has no state file and is refused as `import_source_not_found`;
  the target's own terminal tick deletes only the target's prefix.
- **Session names are machine-wide.** Handled by the collision rule and
  `--as`; the import checks both the local store and its own prefix.
- **The compiled template's absolute path.** Handled by Decision 3.

## Solution Architecture

### Remote layout

```
<prefix>/<name>/koto-<name>.state.jsonl   state file (unchanged)
<prefix>/<name>/ctx/<key>                  context keys (unchanged)
<prefix>/<name>/ctx/manifest.json          manifest (unchanged)
<prefix>/<name>/version.json               version record (unchanged)
<prefix>/<name>/template.json              compiled template (new, pushed at init)
<prefix>/<name>/migrated.json              marker (new, written by an import)
```

### The verb

```
koto session import <name> --from <workspace-path> [--as <new-name>] [--trust-template]
```

Run from the directory the target should be anchored in. On success it
prints one JSON object:

```json
{"name": "coord", "imported": true,
 "from": {"workspace": "/srv/ws-a", "session": "coord"},
 "keys": 5, "template": "local-cache", "marked": true}
```

`template` is `local-cache`, `bucket` (under `--trust-template`), or
`unchanged` when a re-run finds the session already built;
`marked` is always true on success, since a marker that couldn't be written
exits `import_unmarked` instead. Refusals print `{"error": {"code": ..., "message": ...}}`
and exit 2 for the caller-actionable codes (`import_requires_cloud`,
`import_source_not_found`, `import_source_migrated`,
`import_source_is_child`, `import_name_taken`,
`import_template_unavailable`) and 1 for the rest (`import_source_unreadable`,
`import_push_failed`, `import_unmarked`).

### Import sequence

`handle_import` in `src/cli/session.rs`, with the remote work in
`CloudBackend::import_session` (`src/session/cloud.rs`):

1. Refuse `import_requires_cloud` unless the backend is cloud. Validate both
   names as `ValidatedSessionId`.
2. Derive the source prefix from `--from` (Decision 1).
3. Read the source's `migrated.json`: a marker naming another target refuses
   `import_source_migrated` with that target; a marker naming this import's
   own target means an earlier run's marker PUT landed: when the local target
   exists with the marker's session id the import is already complete and
   nothing is written, and otherwise it refuses `import_source_migrated` with
   a hint naming the conflict; a failed read refuses
   `import_source_unreadable`.
4. GET the source's state file: 404 refuses `import_source_not_found`. Parse
   the header; a `schema_version` other than 1, or a `template_hash` that
   isn't exactly 64 lowercase hex digits, refuses
   `import_source_unreadable` (the hash becomes a file name in steps 6 and
   8); `parent_workflow` set refuses
   `import_source_is_child`. Parse every event line, so a corrupt log is
   refused rather than carried.
5. Resolve the target name (`--as` or `<name>`). If a local session or an
   object under this workspace's prefix already has it, read that log
   (local, else remote). When its `session_imported` event names the
   source's `session_id`, this is a retry: with a local copy present, skip
   to step 11; with only the remote copy (a crash between steps 9 and 10),
   run steps 6 to 10 again over it, which rewrites the same objects. Any
   other owner refuses `import_name_taken`, naming `--as`.
6. Find the template (Decision 3): this host's
   `<cache>/<template_hash>.json`, or under `--trust-template` the source's
   `template.json`. Check its SHA-256 against `template_hash` and parse it
   as a compiled template; refuse `import_template_unavailable` naming the
   hash when no acceptable copy exists.
7. GET the source's manifest and every key it lists. Every key name must
   pass `validate_context_key` (`src/session/validate.rs`), and each key's
   SHA-256 and size must match its manifest entry; a bad name, a mismatch or
   a missing object refuses `import_source_unreadable`.
8. Build the target in `<sessions>/.import-<target>-<random>/`, created
   exclusively with mode 0700:
   - the header: the source's, with `workflow` set to the target name, a new
     `session_id`, `execution_dir` and `origin.anchor` set to the canonical
     current directory, `origin.store` set to this backend's store identity,
     and `command_environment` taken with `command_env::record_from_process`
     the way `koto init` takes it;
   - every source event line, copied verbatim;
   - one `session_imported` event (next `seq`), a new
     `EventPayload::SessionImported { from_workspace, from_session,
     from_session_id, machine_id }`, where `machine_id` is the random id
     koto keeps in user config (`get_or_create_machine_id`). Like every
     additive event, an older koto reads it as `EventPayload::Unknown` and
     keeps reading the log, and state derivation ignores it;
   - `ctx/` with every key's bytes and the source manifest, verbatim;
   - `<template_hash>.json`;
   - a fresh `version.json` for this machine.
9. Push the staged target under this workspace's prefix: state file,
   `template.json`, each key, manifest, version record, using the strict
   (error-returning) push. On any failure, delete exactly the keys this run
   pushed and the staging directory, and refuse `import_push_failed`.
10. Rename the staging directory to `<sessions>/<target>/` with a rename that
    never replaces an existing directory, and rename its state file to match.
    If the target appeared meanwhile, take back what step 9 pushed and refuse
    `import_name_taken`.
11. GET the source's `migrated.json` again; if another import marked it
    meanwhile, keep the target, exit `import_source_migrated` naming the
    other target, and leave the operator to remove one of the two. Otherwise
    PUT it. On failure exit `import_unmarked`, keeping the target; a re-run
    lands in step 5's retry branch. The re-check narrows, but doesn't close,
    the window two simultaneous importers share; the stopped-source rule is
    what rules that case out.

The import issues a constant number of requests plus one GET and one PUT per
context key (R20).

### The marker check

`CloudBackend` gains `check_not_migrated(id) -> anyhow::Result<()>` and a
`migration_checks` cache field. The first call for an id in a process lists
the session's prefix for `migrated.json` (a ListObjectsV2 request, since
rust-s3 retries a GET's 404 after a one-second sleep); later calls return
the cached result. A listed marker is fetched and returns `SessionMigrated
{ name, target, workspace }`, and still refuses if the fetch fails; an
absent one records "not migrated"; a failed listing prints
`warning: cloud sync: migration check failed: ...` and proceeds. It runs at
the top of `read_header`, `read_events` and every `ContextStore` method on
the cloud backend (`add`, `add_with_writer`, `get`, `ctx_exists`, `remove`,
`list_keys`, `meta`). `ctx_exists` and `meta` have no error return, so for a
migrated session they answer `false` and `None`; callers that must say why
call `check_not_migrated` themselves. `read_events_local`, `exists`, `list`, `cleanup` and
the `session resolve` paths don't call it.

`SessionMigrated` is a typed error in `src/session/mod.rs`. Its message is
`session_migrated: session '<name>' was migrated to '<target>' in
<workspace>; continue it there`, so every command's existing error path
carries the code. `handle_next` maps errors explicitly, so its
`read_events` error branch downcasts to `SessionMigrated` before falling
back to `PersistenceError`, and emits a new
`NextErrorCode::SessionMigrated` (`"session_migrated"`, exit 2).
`handle_status` does the same downcast for its exit code.

### Template push and resolution

`CloudBackend::init_state_file` reads the `WorkflowInitialized` event from
`initial_events`, resolves its `template_path` against the session
directory, and PUTs the file as `template.json`, best effort like every
other push. `resolve_template_path_in_session` gains the fallback in
Decision 3. Child sessions (`init_child`) push theirs the same way; the
import still refuses children. `src/cli/retry.rs` reads a child's raw
`template_path` to classify its outcome; children aren't imported, so it is
unaffected.

### Header rewrites

```rust
trait SessionBackend {
    fn rewrite_header(
        &self,
        id: &str,
        f: &dyn Fn(StateFileHeader) -> StateFileHeader,
    ) -> anyhow::Result<()>;
}
```

The trait's default runs `rewrite_header_atomically` and then confirms the
push with `ensure_pushed`; `CloudBackend` overrides it to rewrite locally and
then `sync_push_state`, the push its event appends use. The three CLI call
sites move to it. Two more rewrites live in `src/engine/claim.rs` (the
assignment-claim write and the re-delegation epoch bump); they take a
state-file path rather than a backend, and only tests reach them, so they
stay on the free function, as the koto#310 pull request records with the
reason.

### Cleanup keeps the marker

`sync_delete_session` skips any key ending in `/migrated.json`. Nothing else
in koto deletes one.

### Configuration

`CloudConfig` gains `path_style: Option<bool>`; `koto config set
session.cloud.path_style true` is accepted in project and user config.

### The harness

`tests/session_migration_test.rs`, with `tests/support/fake_s3.rs`. Two
simulated hosts are two sets of `HOME`, `XDG_CACHE_HOME` and working
directory, sharing one endpoint and bucket; each workspace's
`.koto/config.toml` sets the backend, endpoint, bucket, region and
`path_style`, and credentials come from `AWS_ACCESS_KEY_ID` and
`AWS_SECRET_ACCESS_KEY`. The carrier test's steps, each failing with its
name:

1. `init-a`: `koto init` a session in workspace A from a template with a
   waiting state.
2. `keys-a`: `koto context add` five keys of different sizes, binary
   included.
3. `import-b`: `koto template compile` the same template in B, then
   `koto session import <name> --from <A>` in workspace B, timed (R20).
4. `keys-b`: `koto context get` each key in B; compare SHA-256 with what A
   stored.
5. `advance-b`: `koto next` in B, with evidence, advances past the waiting
   state.
6. `refuse-a`: `koto next` in A exits 2 with `session_migrated` naming B;
   A's local state file is unchanged.
7. `reimport-c`: import from B into workspace C succeeds (R6).

The same file holds the refusal, retry, rename, cleanup and request-count
tests the PRD's criteria name, using the endpoint's recording and
fault-injection. Under `--features cloud-integration-tests` with the bucket
secrets, steps 1 to 7 also run against the real bucket, with a unique
session name per run and a cleanup of every prefix they touched.

### Documentation

- `docs/guides/cloud-sync-setup.md`: a "Moving a session to another host"
  section built on the import and the stopped-source rule; the koto#312
  fixes; the remote-reaching command list; the path-style key; the error
  codes.
- `DESIGN-config-and-cloud-sync.md` and
  `DESIGN-backend-state-persistence.md`: dated notes pointing here.
- The koto-user skill (`SKILL.md`, `references/command-reference.md`,
  `references/error-handling.md`): the import verb, `session_migrated`, and
  "the session moved machines" routed to the import instead of `rebind`.

## Implementation Approach

Four pieces. The first is the thin slice the harness measures; the other
three don't depend on each other once it lands, except that the docs
describe what the slice and the hardening ship.

1. **Thin slice: import, marker and harness.** `session.cloud.path_style`;
   the resolver fallback; `koto session import` with steps 1 to 11 in their
   simplest form (the template from the local cache only, a plain
   `import_name_taken` refusal with no `--as` and no retry branch, cleanup
   on failure best effort); the marker check on `read_header` and
   `read_events`; cleanup keeping the marker; `SessionMigrated` in `next`
   and `status`; the fake
   endpoint and the carrier test's seven steps. Its pull request reports
   the harness result.
2. **Import hardening.** The full refusal table with no-trace tests;
   `--as` and the retry branch; the template push at init and
   `--trust-template`; staging, rollback and `import_unmarked`;
   the marker check on every `ContextStore` method and its per-process
   cache; the request-count, timing and
   credential tests.
3. **Header rewrites through the backend (koto#310).** `rewrite_header` on
   the trait, the three call sites, and a cloud test that rebinds and adopts
   an anchor and asserts the remote header.
4. **Docs and skills.** The guide (koto#312 included), the two design
   notes, and the koto-user skill.

## Security Considerations

**Credentials.** The import reuses the backend's credential handling; no
new place reads or stores a credential. The marker, the import output and
every new error carry session names, workspace paths, a machine id and a
timestamp, never a key or an endpoint secret. A test seeds sentinel
credentials and greps all of them (R21).

**Trust in the bucket.** Anyone who can write the bucket can already
rewrite any session's state, since a pull replaces the local state file.
What they can't do today is choose the commands a host runs: the compiled
template, which holds every gate and action command, always comes from the
host's own cache. The import keeps that true by default (Decision 3). Only
`--trust-template` takes the template from the bucket, and its output says
so; an operator who passes it is trusting everyone with write access to the
bucket to the same degree as the person who started the session. The rest
of what the import reads it treats as untrusted input: names are validated as `ValidatedSessionId`; every key
name from the manifest goes through the same validation the context store
applies, (`validate_context_key` in `src/session/validate.rs`), so a key such as
`../../x` can't escape `ctx/`; every key and the
template are checked against the manifest's and the header's SHA-256; the
header's schema version and every event line are parsed before anything is
written. A forged marker can make a session refuse, which is a denial of
service by someone who could already delete the session.

**Paths.** `--from` is only hashed, never opened when it doesn't resolve.
The staging directory sits inside the session store and is created
exclusively, mode 0700. The resolver fallback accepts only a 64-hex-digit
`.json` file name, so a crafted `template_path` can't point the fallback at
an arbitrary file in the session directory.

**Bucket strings in output.** Names, paths and marker fields read from the
bucket reach the output only inside JSON-encoded strings, so they can't
inject terminal control sequences or forge fields. The header fields the
import doesn't rewrite (`template_source_dir`, `template_source_file`, the
request-store fields) are carried as recorded and never opened by the
import; `koto next` already verifies the template it loads against
`template_hash`, whichever path the resolver returns.

**Size.** The import adds no new limit on object sizes or key counts; a
bucket writer can already fill a host's store through ordinary pulls. The
import reads each object once and holds at most one key in memory at a time
beyond the state file.

**What stays behind.** The source's context keys stay in the bucket after
the import; the marker stops koto from using them, not anyone with bucket
read access from reading them. The guide says so.

**The migrated workspace path appears in errors.** It's a path the user's
own team chose; it already appears in `execution_anchor_*` errors. Errors
that name the endpoint print it with any URL userinfo removed.

## Consequences

### Positive

- A session moves between hosts with one command, carrying its context
  keys, and the old copy refuses instead of forking quietly.
- `rebind` works under the cloud backend, and no header rewrite can bypass
  the remote again.
- Self-hosted S3 on an IP endpoint works, which also makes local testing of
  the cloud backend possible.
- The carrier has a measurement that runs on every pull request.

### Negative

- Every cloud command pays one more request (a listing for the marker) on
  its first read of a session.
- An import needs the template compiled on the new host first, from a
  checkout whose template matches the session's hash, unless the operator
  passes `--trust-template`.
- `migrated.json` objects accumulate; koto never deletes them.
- The marker is a flag. An offline host, or one whose remote is
  unreachable, keeps advancing its copy with a warning.

### Mitigations

- The marker check is cached per process, so a command's many reads cost
  one request.
- `import_template_unavailable` names the hash, the template's file name
  and the compile command that supplies it.
- Markers are a few hundred bytes; removing them is a bucket operation an
  operator can do deliberately.
- The fail-open path warns on every command, and the guide's stopped-source
  rule tells operators to retire the source before importing.
