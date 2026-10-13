---
status: Proposed
upstream: docs/prds/PRD-koto-cloud-session-config.md
problem: |
  koto resolves its session backend, cloud location and bucket credentials in
  load_config from two config files and the AWS_* pair only, records no origin
  for any of them, and koto config list serializes the merged struct. Under
  the cloud backend the local store is machine-wide, so import_session's
  existing_target refuses a same-host import because the source's own local
  copy holds the name, and the import's write order (marker last) would leave
  a window where a command in the source workspace pulls the source's state
  over the new session's local file.
decision: |
  load_config gains an environment layer above both files for five session
  settings (KOTO_SESSION_BACKEND, KOTO_SESSION_CLOUD_ENDPOINT, _BUCKET,
  _REGION, _PATH_STYLE), a koto credential pair (KOTO_SESSION_CLOUD_ACCESS_KEY,
  _SECRET_KEY) that replaces the AWS pair as a whole when either is set, and a
  per-setting origin record; the layer never fails the load, and
  build_backend refuses unusable values and half pairs. koto config list
  appends the origins (an origins object in JSON, comment lines in TOML). A
  same-host import whose local holder is the source's own copy writes the
  marker on the source's prefix first and then swaps the name with one atomic
  directory exchange through a fixed .takeover-<name> directory, with three
  local guards that keep source-workspace commands off the new copy.
rationale: |
  The decider's env layer is the proven shape, and deferring refusals to
  build_backend keeps config list working while no caller can fall back to
  the local backend. Resolving credentials as a pair is the only rule that
  can't mix halves or reach aws-creds' own AWS lookup. For the import, the
  PRD requires that the local store never hold the new session while the
  source is unmarked, which forces marker-first; it is recoverable on the
  same host because the import never deletes anything under the source's
  prefix and the marker names the new session id. An exchange leaves the
  name held at every instant, which two renames can't.
---

# DESIGN: Cloud session config and same-host import

## Status

Proposed

## Context and Problem Statement

The requirements are in `docs/prds/PRD-koto-cloud-session-config.md`. This
section restates them in the terms of the code they change.

**Config resolution.** `load_config` (`src/config/resolve.rs`) starts from
`KotoConfig::default()` with `session.backend = "local"`, merges the user file
`~/.koto/config.toml`, then the project file `.koto/config.toml` in the
current directory, and then lets `AWS_ACCESS_KEY_ID` and
`AWS_SECRET_ACCESS_KEY` replace the two credentials. `merge_config` overlays a
file's value whenever it is non-empty or `Some`, and it can't tell "the file
named this key" from "serde filled a default": any file with a `[session]`
table deserializes `backend` to `"local"`. Nothing records which layer a value
came from. The request store's `KOTO_REQUEST_STORE_*` and the decider's
`KOTO_DECIDER*` variables already form an environment layer above the files;
the decider's (`apply_decider_env`) takes a lookup closure, treats an empty or
whitespace value as unset, and records a `SettingOrigin` per field. The
session settings have no such layer.

The feature adds one. Five settings (`session.backend`,
`session.cloud.endpoint`, `bucket`, `region`, `path_style`) take a
`KOTO_SESSION_*` variable named by a fixed rule (`KOTO_` plus the dotted name,
upper-cased, dots to underscores), and that variable wins over both files,
per setting (PRD R1, R2). Empty or whitespace-only counts as unset (R3). A
backend other than `local`/`cloud`, or a path style other than `true`/`false`,
must never fall back to another layer or to the local backend: every command
that builds the backend refuses naming the variable, while `koto config list`
and `koto config get` keep working (R4). This last rule collides with the
code as it is. `build_backend` (`src/cli/mod.rs`) is the only place a backend
value is checked, with a bare `unknown backend: X`, and several callers
(`koto next`'s tick, `cli/workspace.rs`, the run journal's rotation,
`workflows_surface/materialize.rs`) call `load_config().unwrap_or_default()`,
so making `load_config` fail on a bad value would quietly give them the
default, which is the local backend. `session.cloud.marker_retain_days` gets
no variable (R5).

**Credentials.** The cloud backend builds its client in `create_bucket`
(`src/session/cloud.rs`) with `Credentials::new(access_key, secret_key, ...)`
from the pinned aws-creds 0.39.1. When the access key is `Some`, that call
uses the two values as given; when it is `None`, it looks up `AWS_*` and the
AWS profile itself. The feature adds `KOTO_SESSION_CLOUD_ACCESS_KEY` and
`KOTO_SESSION_CLOUD_SECRET_KEY`. When either is set, both credentials come
from that pair and nothing else (R6); exactly one set refuses, naming the
missing one, before any request (R7); neither set keeps today's resolution
exactly, including an exported empty AWS variable overriding a file (R8).
Their values must be redacted on every surface that redacts credentials, under
their own names, including when a config file fails to load (R9), which means
the list `ENV_SECRET_NAMES` that `known_credentials` scans directly, the
`SecretSources` flags and the labels `redaction_keys` produces. Credentials
stay out of the project file (R10).

**The origin report.** `koto config list` (`handle_config`) serializes the
redacted `KotoConfig` as TOML or JSON. It must report, for eight settings
(the seven with variables plus `marker_retain_days`), an origin: `env` with
the variable (`AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` included), `project`,
`user`, `default` (for the three settings with a built-in value: backend,
path style, marker retention) or `unset` (R11). It must be additive: JSON
gains one top-level `origins` object keyed by dotted name with
`{"source": ...}` plus `"variable"` for `env`; TOML gains trailing
`# origin: <setting> = <origin>` comment lines and parses to exactly today's
tables (R12). A file is an origin only when it names the key, in either TOML
spelling (R13). `tests/decider_config.rs` asserts the exact top-level JSON key
set and is the one existing assertion that changes.

**Same-host import.** `CloudBackend::import_session` reads the source from the
`--from` workspace's prefix, then asks `existing_target` whether the target
name is free. `existing_target` accepts a local directory at the name only
when its log carries a `session_imported` event for this same source (an
earlier run of this import); A's own plain copy has none, so it refuses
`import_name_taken`. Under the cloud backend the local store is always
`$HOME/.koto/sessions`, so on one host A's copy always sits at the name. The
import must instead succeed and keep the name when the holder is the source's
own local copy (same non-empty `session_id` as the source's state file in the
bucket) and that copy is not ahead of the bucket (R14). Afterwards every
command in A that reads the session's state or context refuses
`session_migrated` naming B, and B's log is the bucket's events plus one
`session_imported` event (R15). It refuses, with A's local files, prefix and
marker untouched, when another process holds the session's state-file lock
(`import_source_running`), when the local copy is ahead
(`import_source_unpushed`, with the `koto session resolve --keep local`
hint), or when a local session names `<name>` as its parent
(`import_source_has_children`), checked in that order (R16). Any other holder
keeps refusing `import_name_taken` (R17), and `--from` naming the importing
workspace itself keeps today's behavior (R20).

Two facts in the code make the write order the hard part (R18). The cloud
backend's `sync_pull_state` writes the remote state file over the local one
on every read, blind, and the only thing that stops a command in A from doing
that is the migration marker, checked before the pull. And
`atomic_rename_dir` never replaces an existing directory, so A's copy has to
leave the name before the new session can take it. Today the import moves the
new session into place and writes the marker last; on the same host that
order leaves a window, and after an `import_unmarked` failure an indefinite
one, in which a command run in A pulls A's state over B's new local file. R18
rules that out: the local store must never hold the new session while A's
prefix lacks its marker, the name must be held by at most one directory at
every point, and every failure must either leave A untouched or be completed
by re-running the same command.

The output keeps its fields, with `name` the kept name (R19). A same-host
import makes no more remote requests than a cross-host one, and the existing
carrier test's request sequence doesn't change (R23).

**Documentation and compatibility.** `docs/guides/cloud-sync-setup.md`
documents the variables, precedence, credential rule, origin report and
same-host import, and both amended designs get a dated note (R21). A host that
sets none of the seven variables resolves, builds and authenticates exactly as
today (R22). Every acceptance criterion runs in the default `cargo test` job
without secrets (R24).

## Decision Drivers

- **No change without the new variables** (R22). Every existing test of
  config, credentials, redaction and import passes, apart from the one
  exact-key-set assertion on `config list --json`.
- **Never a silent fallback to the local backend** (R4), given callers that
  turn a failed `load_config` into defaults.
- **The diagnosis command keeps working** when a value is bad (R4):
  `config list` is how an operator finds the problem.
- **No mixed credentials** (R6, R7), and no path where a missing half reaches
  the S3 client's own AWS lookup.
- **Redaction knows the new names everywhere** (R9), including the
  load-failure path.
- **The source is never damaged by a failed import** (R16, R18), and the
  local store never holds the new session while the source is unmarked (R18).
- **No change to the cross-host path** (R20, R23): the takeover is a narrow
  branch keyed on the holder's identity, not a change to the general flow.
- **Reuse koto's existing primitives**: the decider's env-layer shape, the
  `LoadedConfig` key-presence tracking, `lock_state_file`,
  `atomic_rename_dir`, the import's staging and take-back.
- **Testable without secrets** (R24): the fake S3 endpoint in
  `tests/support/fake_s3.rs`, which records requests and injects failures.

## Considered Options

### Decision 1: The environment layer, credentials and origins in `load_config`

Resolved inline.

The five settings need a layer above both files, the credential pair needs a
rule that can't mix halves, every setting needs an origin, and a bad value
must refuse without breaking `config list` and without letting a caller that
swallows a failed load fall back to the local backend.

Key assumptions:

- No command builds a cloud backend except through `build_backend`; a grep
  finds `CloudBackend::new` only there.
- The redactor labels a value with the first source that names it
  (`Redactor::new` skips a spelling it already holds), so insertion order
  decides the label.

#### Chosen: An infallible env layer with a side-channel origin record; refusals in `build_backend`

`apply_session_env(&mut KotoConfig, lookup)` runs inside `load_config` after
the user and project files, in the shape of `apply_decider_env`: a lookup
closure, values trimmed, empty means unset. A backend value is stored as given
(the field is already a free `String`). A path style that isn't `true` or
`false` can't live in `Option<bool>`, so it goes into a skip-serialized
`invalid_env` list as (setting, variable, raw value) and `path_style` is set
to `None`, so no lower layer's value shows through. The load never fails on
these values. `build_backend` calls a new check before it matches on the
backend; the check refuses any `invalid_env` entry and any backend that came
from the environment and isn't `local` or `cloud`, naming the variable and
the value. The callers that turn a failed load into defaults never build a
backend, so none of them can reach the local backend by this route, and
`config list` and `config get` keep working.

Origins live in a skip-serialized `session_origins` record on `KotoConfig`,
one entry per setting. `LoadedConfig` gains `session_keys`, the dotted keys
the file's raw TOML names under `session` (both TOML spellings parse to the
same table), in the pattern of `request_store_keys`. A layer sets a setting's
origin only when it names the key. Values merge exactly as today: a file with
a `[session]` table and no `backend` still deserializes to `"local"` and
still replaces an earlier layer's backend, as it does now; the origin then
reads `default`, which is what the value is.

The credential pair runs after the env layer. When either
`KOTO_SESSION_CLOUD_ACCESS_KEY` or `KOTO_SESSION_CLOUD_SECRET_KEY` is set,
both fields are replaced from the pair (a missing half becomes `None`), their
origins name the koto variables, and `credential_pair_missing` names the
missing one. With neither set, the AWS block runs exactly as today.
`build_backend` refuses a half pair only when the backend is cloud, before
`CloudBackend::new`, so no request is made and aws-creds never receives a
`None` access key from the pair (it would otherwise look up `AWS_*` itself).

`ENV_SECRET_NAMES` lists the two koto names before the AWS pair, so
`known_credentials` finds and labels them even when the load fails;
`SecretSources` replaces its two booleans with the name of the variable that
supplied each credential, and `redaction_keys` labels with it.

#### Alternatives Considered

- **Fail `load_config` on a bad value.** Rejected: it breaks `config list`,
  the command an operator diagnoses with, and every caller that does
  `load_config().unwrap_or_default()` would silently run on defaults, which
  means the local backend.
- **Ignore a bad value, as `KOTO_REQUEST_STORE_*` does.** Rejected: a backend
  setting that silently falls through writes sessions where the operator
  didn't intend.
- **Resolve each credential variable on its own.** Rejected: a koto access
  key could pair with an AWS secret, and a missing half would reach aws-creds,
  whose `Credentials::new(None, ...)` reads `AWS_*` and the AWS profile.
- **Fix the `[session]`-table backend reset while here.** Rejected for this
  feature: it changes resolution for hosts that set none of the new
  variables. Proposed as its own issue.
- **An `--origins` flag, or a new `[origins]` TOML table.** Rejected: the
  report has to be there when someone looks, and a new table changes what the
  TOML parses to.

### Decision 2: The same-host takeover

Delegated to /decision.

On one host every workspace shares `$HOME/.koto/sessions`, keyed by name
alone, so the new session has to take over the directory the source's own
copy sits in. Today's order (stage, push, move into place, journal, mark
last) leaves the local store holding the new session while the source's
prefix is unmarked, which the PRD forbids. Three more holes surfaced that any
order must close. Once B's prefix holds the pushed copy but the name is still
A's directory, a B command finds no marker on B's prefix and pulls B's log
into A's directory. `check_not_migrated` fails open when the listing fails,
so after a takeover an A command on a flaky network can read or append to B's
copy. And `koto session cleanup` and `koto session resolve` never check the
marker, so from A they would delete or overwrite B's copy. The state-file
lock the PRD's refusal relies on is taken today only by prune and by
batch-parent ticks; ordinary commands in A stay out only by the stopped-source
rule, as they do for a cross-host import.

Key assumptions:

- The stopped-source rule remains the contract for A processes that don't
  hold the state-file lock; no order can quiesce a writer already in flight.
- The import never deletes anything under A's prefix, so A's bucket objects
  plus the marker's target session id can always rebuild the target.
- Atomic directory exchange is available on the stores koto runs on (ext4,
  xfs, btrfs, tmpfs, f2fs, current overlayfs, OpenZFS 2.2 and APFS); CIFS/SMB,
  FAT and hfsplus refuse it, and NFS refuses every `renameat2` flag, which
  already fails imports there. The pinned libc exposes `RENAME_EXCHANGE`
  (Linux) and `renamex_np` with `RENAME_SWAP` (Apple).
- `koto session resolve` keeps skipping the marker check, so its
  `--keep local` hint still works on A after a marker lands.

#### Chosen: Mark before swap, one atomic exchange, a fixed `.takeover-<name>` directory

The branch runs only when `--as` wasn't passed, a local directory holds the
name, and `prefix_for_workspace(--from)` differs from this backend's prefix.
Every other case, a same-prefix import included, runs exactly as today.

Checks, before any write: read the source as today; classify the holder (a
non-empty header `session_id` equal to the source's is A's own copy; an
earlier run of this import goes to the re-run branches; anything else is
`import_name_taken`); then, in the PRD's order, take A's state-file lock and
`ctx/manifest.lock` non-blocking (either held: `import_source_running`), check
whether A's copy is ahead (`import_source_unpushed`), and look for local
children (`import_source_has_children`). "Ahead" means A's local event lines
are not a prefix of the bucket's (longer, or forked at the same seq), or A's
local manifest holds a key the bucket's manifest lacks or one with a different
hash and a later `created_at`. Then check B's prefix for the name as the
cross-host import does, and check that the swap can work: the name is a real
directory on the same device as the store, and an exchange of two empty
scratch directories in the store succeeds. A failed probe refuses
`import_push_failed` with an `--as` hint, with nothing written.

Writes: stage the new session (new id S') in `<sessions>/.takeover-<name>`
(hidden from `list`, created exclusively, mode 0700); push under B's prefix
with the strict push (failure: take back, remove the directory, A
unchanged); re-verify the holder under the held locks; write the marker (a
definite failure takes back the push and leaves A unchanged; an unknown
outcome keeps everything and exits `import_unmarked`); exchange
`.takeover-<name>` and `<name>` in one call; release the locks; remove the
name's `.finished` record; journal the import; delete `.takeover-<name>`
(now A's old copy) only if its header still holds A's id and its last seq
hasn't grown, otherwise keep it and warn naming the path.

Three guards on the cloud backend, each a local read:

- **Local migrated check.** In `check_not_migrated` before the listing, and in
  `cleanup` and `resolve`: when the local log's latest `session_imported`
  names this backend's prefix and this session name as its source, the
  command refuses `session_migrated`. R15 then holds for A even when the
  marker listing fails, and A's `cleanup` and `resolve` can't touch B's copy.
- **Takeover pending.** Reads and writes of `<name>` refuse while
  `<sessions>/.takeover-<name>` exists, naming the import to re-run. This
  closes the B-side window between the push and the exchange, and the time
  after a crash.
- **Identity-checked pull.** `sync_pull_state`, the context pull and the
  force pull refuse to write over a local copy whose header session id
  differs from the bucket's when one log is an import of the other.

Every failure is recovered by running the same command again:

| Point | State left | What a re-run does |
|---|---|---|
| Staging, push, or a definite marker failure | No marker; name = A; maybe a partial copy on B's prefix and a staged `.takeover-<name>` | Removes the staged leftover, recognizes B's copy as this import's, runs the full takeover. A's files are unchanged. |
| After the marker, before the exchange | Own marker; name = A (S); B's prefix holds S' | Re-takes the locks and re-runs the checks against A's bucket; refuses if B's prefix copy moved past its `session_imported` or no longer starts with A's events; otherwise re-stages from A's bucket reusing S', re-pushes, exchanges, journals, deletes. |
| After the exchange, before the delete | Own marker; name = S'; `.takeover-<name>` holds S | Journals again (duplicate journal records possible, missing ones not) and deletes after the identity and no-growth check. Prints `template: "unchanged"`. |
| Complete | Own marker; name = S'; no takeover directory | Writes nothing and succeeds, as today. |

The name is held by exactly one directory at every point, and the local store
holds S' only after the marker PUT succeeded or was confirmed. The new checks
make no remote request, so a same-host import makes the same requests as a
cross-host one, and the cross-host order itself (move into place, mark last)
doesn't change.

#### Alternatives Considered

- **Mark first, then two no-replace renames through an aside.** Rejected:
  between the renames the name is empty, and `SessionVersion::save` and
  `LocalBackend::add_as` call `create_dir_all`, so a concurrent command can
  re-create `<name>/`; after the marker both the rename in and the rename back
  then collide, and recovery needs extra code for a path that rarely runs.
- **Exchange with a compiled two-rename fallback.** Rejected for now: the
  fallback would run only after the marker, only on stores CI never covers,
  and it brings the empty-name gap back. It buys the takeover on CIFS/SMB,
  FAT and hfsplus stores, where `--as` already works.
- **Keep the marker last and guard the window locally.** Rejected: it breaks
  the PRD's rule as written, and after `import_unmarked` other hosts reading
  A's prefix keep advancing A while this host holds B, so the two copies fork.
  Its local guards are kept above.
- **Rewrite A's directory in place.** Rejected: the new session holds nothing
  of A's only if every file type a session directory can hold, now and later,
  is listed and removed; it destroys A's copy as it goes, and a re-run has to
  read a half-rewritten tree.
- **After the marker, re-run from B's prefix copy when A is unchanged.**
  Rejected for one staging path: always re-staging from A's bucket with S'
  covers both that case and a `resolve --keep local` run after the marker.

### Decision 3: Measuring the same-host acceptance

Resolved inline.

The acceptance has to run without secrets and without touching the host's
live store, and it has to exercise the release binary end to end.

Key assumptions:

- `CARGO_BIN_EXE_koto` is the binary under test in CI; the measured run uses
  the release build.

#### Chosen: The fake S3 endpoint, in CI and in a measured run

`tests/support/fake_s3.rs` already serves path-style GET, PUT, HEAD, DELETE and
ListObjectsV2 from memory, records requests and injects PUT failures, and the
migration tests run it in the default job. A new test file drives the same-host
sequence with one `HOME` and two workspaces, no config file, and only the
`KOTO_SESSION_*` variables; every command first removes `KOTO_SESSION_*` and
`AWS_*` from its environment so the ambient environment never leaks in. For
the measured run, an ignored test starts the endpoint, prints its address and
bucket, and blocks until a stop file appears; a shell run with a disposable
`HOME` and two workspace directories drives the release binary through init,
advance, stop, import without `--as`, continue in B and refuse in A, recording
each exit code and timing. To koto, a fresh niwa instance is a different
working directory, which is what the two directories are.

#### Alternatives Considered

- **MinIO or moto.** Rejected: neither is installed, and a network install
  adds an unpinned dependency for one run.
- **The real bucket under `cloud-integration-tests`.** Rejected: it needs
  secrets and touches shared state.
- **Provisioning real niwa instances.** Rejected: heavy, touches the
  workspace, and adds nothing koto can observe.

## Decision Outcome

The three decisions meet at `build_backend` and at the cloud backend's read
path. Decision 1 makes the environment a full layer for the five settings and
the credential pair, records where every value came from, and moves every
refusal to the one function that builds a backend, so a bad value is visible
in `config list` and fatal to any command that would use it. That is also
what lets an operator turn the cloud backend on for one workspace without a
file, which is the precondition for the import. Decision 2 then makes the
import keep the name on the same host by marking the source before the new
session can appear locally and exchanging the directories in one call; its
three guards are cheap local reads that only fire for the two sides of an
import, so hosts that never import see no change. Decision 3 proves the pair
together the way the acceptance states it: environment only, one host, a
stopped session, an import without `--as`, and the replacement continuing.

## Solution Architecture

### Overview

Two independent changes share one integration test. The config change lives
in `src/config/` and `handle_config`; the import change lives in
`src/session/cloud.rs`, `src/engine/atomic_fs.rs` and `handle_import`. The
import change needs the config change only for its tests (environment-only
setup), not for its code.

### Components

**`src/config/resolve.rs`**

- `apply_session_env(config: &mut KotoConfig, lookup: impl Fn(&str) ->
  Option<String>)`: for each of the five settings, a trimmed non-empty value
  replaces the field and sets its origin to `Env(<variable>)`. Backend: stored
  as given (trimmed). Path style: `true`/`false` parsed; anything else pushed
  to `invalid_env` and the field set to `None`. Endpoint, bucket, region:
  stored as given.
- `apply_credential_env(config, lookup)`: the koto pair as a whole when either
  is set; otherwise today's AWS block, unchanged. Records the source variable
  per credential in `SecretSources` and the origin in `session_origins`.
- `LoadedConfig.session_keys: Vec<String>` from the raw TOML's `session`
  table (and its `cloud` subtable), used only to mark origins.
- `load_config` order: defaults, user file (origins `user`), project file
  (origins `project`), `apply_session_env`, `apply_credential_env`, then the
  existing request-store and decider layers.
- `ENV_SECRET_NAMES` becomes `[KOTO_DECIDER_API_KEY,
  KOTO_SESSION_CLOUD_ACCESS_KEY, KOTO_SESSION_CLOUD_SECRET_KEY,
  AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY]`.
- Constants for the seven variable names, next to `ENV_DECIDER_*`.

**`src/config/mod.rs`**

- `KotoConfig` gains `#[serde(skip)] session_origins: SessionOrigins` and
  `#[serde(skip)] session_env: SessionEnvFindings` (the `invalid_env` list and
  `credential_pair_missing`).
- `SessionOrigins` holds one `Origin` per setting for the eight settings, in a
  fixed order, with `Origin` = `Env(&'static str) | Project | User | Default |
  Unset`. The initial value is `Default` for `backend`, `path_style` and
  `marker_retain_days`, and `Unset` for the rest.
- `SecretSources.access_key_from_env` and `secret_key_from_env` become
  `Option<&'static str>` naming the variable; `redaction_keys` uses it as the
  label.
- `get_value` returns an `invalid_env` raw value for its setting before the
  field's value.
- `pub fn check_session_env(&KotoConfig) -> anyhow::Result<()>`: the
  refusals for `build_backend`.

**`src/cli/mod.rs`**

- `build_backend`: `config::check_session_env(&config)?` first; for
  `"cloud"`, a half pair refuses naming the missing variable before
  `CloudBackend::new`. The unknown-backend error from a file keeps today's
  text.
- `handle_config` `List`: JSON is `serde_json::to_value(&redacted)` with an
  `origins` object inserted; TOML is today's string followed by one `# origin:`
  line per setting.

**`src/engine/atomic_fs.rs`**

- `exchange_dirs(a: &Path, b: &Path) -> Result<(), AtomicCreateError>`:
  `renameat2(AT_FDCWD, a, AT_FDCWD, b, RENAME_EXCHANGE)` on Linux,
  `renamex_np(a, b, RENAME_SWAP)` on Apple, `Unsupported` elsewhere.
- `exchange_supported(dir: &Path) -> bool`: creates two empty scratch
  directories in `dir`, exchanges them, removes them.

**`src/session/cloud.rs`**

- `ImportRequest` gains `explicit_as: bool`.
- `ExistingTarget` gains `SourceCopy { local_last_seq, .. }`; `existing_target`
  returns it when the local header's non-empty `session_id` equals the
  source's, `explicit_as` is false, and the prefixes differ.
- New `ImportErrorCode` values: `SourceRunning` (`import_source_running`),
  `SourceUnpushed` (`import_source_unpushed`), `SourceHasChildren`
  (`import_source_has_children`), all exit 2.
- `takeover_session(req, &source)`: the checks and writes of Decision 2,
  holding the two lock guards from the check through the exchange.
- `stage_import` takes an optional session id (reused on a post-marker re-run)
  and a target staging directory.
- The own-marker branch gains the two takeover states (A's copy still at the
  name; `.takeover-<name>` still present).
- The three guards: a local migrated check called from `check_not_migrated`,
  `cleanup` and `resolve`; a takeover-pending stat in the same places and on
  every write path; and the identity check in `sync_pull_state`, the context
  pull and the force pull.

**`src/cli/session.rs`**

- `handle_import` passes `explicit_as = as_name.is_some()` and prints the same
  JSON.

**Tests**

- `tests/config_precedence.rs` (or a new `tests/session_env_config.rs`): every
  config and credential criterion, with a disposable `HOME`, and every
  `Command` first removing `KOTO_SESSION_*` and `AWS_*`.
- Unit tests in `resolve.rs` through the lookup closure.
- `tests/same_host_import_test.rs`: the same-host sequence, the three
  refusals, `import_name_taken`, the same-prefix case, and fault injection at
  the push, the marker, the exchange (through a test seam) and a process exit
  after each write; the request-count comparison with a cross-host import.
- `tests/decider_config.rs`: the exact top-level key set gains `origins`.
- `tests/redaction_test.rs`: the koto credential labels, including a user file
  that fails to parse.

### Key Interfaces

Environment variables:

| Variable | Setting |
|----------|---------|
| `KOTO_SESSION_BACKEND` | `session.backend` |
| `KOTO_SESSION_CLOUD_ENDPOINT` | `session.cloud.endpoint` |
| `KOTO_SESSION_CLOUD_BUCKET` | `session.cloud.bucket` |
| `KOTO_SESSION_CLOUD_REGION` | `session.cloud.region` |
| `KOTO_SESSION_CLOUD_PATH_STYLE` | `session.cloud.path_style` |
| `KOTO_SESSION_CLOUD_ACCESS_KEY` | `session.cloud.access_key` |
| `KOTO_SESSION_CLOUD_SECRET_KEY` | `session.cloud.secret_key` |

Precedence per setting: variable, project file, user file, default. For
credentials: the koto pair as a whole, else the AWS pair as today, else the
user file.

`koto config list --json` (excerpt):

```json
{
  "session": { "backend": "cloud", "cloud": { "bucket": "sessions", "access_key": "<set>" } },
  "origins": {
    "session.backend": { "source": "env", "variable": "KOTO_SESSION_BACKEND" },
    "session.cloud.endpoint": { "source": "user" },
    "session.cloud.bucket": { "source": "project" },
    "session.cloud.region": { "source": "unset" },
    "session.cloud.path_style": { "source": "default" },
    "session.cloud.access_key": { "source": "env", "variable": "KOTO_SESSION_CLOUD_ACCESS_KEY" },
    "session.cloud.secret_key": { "source": "env", "variable": "KOTO_SESSION_CLOUD_SECRET_KEY" },
    "session.cloud.marker_retain_days": { "source": "default" }
  }
}
```

TOML form: today's text, then:

```toml
# origin: session.backend = env KOTO_SESSION_BACKEND
# origin: session.cloud.endpoint = user
# origin: session.cloud.path_style = env KOTO_SESSION_CLOUD_PATH_STYLE (invalid: "yes")
```

An invalid path style also appears in JSON as `"invalid": "yes"` beside its
`source` and `variable`.

Errors from `build_backend`:

- `KOTO_SESSION_BACKEND is "s3"; set it to local or cloud, or unset it`
- `KOTO_SESSION_CLOUD_PATH_STYLE is "yes"; set it to true or false, or unset it`
- `KOTO_SESSION_CLOUD_ACCESS_KEY is set but KOTO_SESSION_CLOUD_SECRET_KEY is
  not; set both or neither`

Import codes: `import_source_running`, `import_source_unpushed`,
`import_source_has_children` (exit 2), and the existing codes unchanged.

### Data Flow

Config: files are read into `LoadedConfig` with their key-presence lists;
merged as today while origins are marked; the env layer and the credential
pair overwrite and mark; `build_backend` checks the findings, then builds.
`config list` reads the same config and prints values plus origins.

Same-host import, in order: read the source (marker, state, parse); classify
the local holder; lock A's state file and manifest; check ahead and children;
check B's prefix; probe the exchange; stage S' in `.takeover-<name>`; push
under B's prefix; re-verify under the locks; mark A's prefix; exchange;
release locks; drop the `.finished` record; journal; delete A's old copy;
print. After the import, a command in A reads its local copy first, finds a
`session_imported` that names A's prefix and the same name, and refuses
`session_migrated` before any request or pull.

## Implementation Approach

### Phase 1: Session settings and credentials from the environment

The env layer, the credential pair, origins, the refusals in
`build_backend`, redaction of the new names, and the origin report in
`config list`, with their tests and the guide's config sections. It stands
alone: a host can enable the cloud backend through its environment once this
lands.

Deliverables:
- `src/config/resolve.rs`, `src/config/mod.rs`, `src/cli/mod.rs` changes
- unit tests and `tests/` coverage for every config and credential criterion
- `tests/decider_config.rs` and `tests/redaction_test.rs` updates
- `docs/guides/cloud-sync-setup.md`: variables, precedence, credential pair,
  origin report; dated note in `DESIGN-config-and-cloud-sync.md`

### Phase 2: Same-host import

The exchange primitive, the takeover branch and its re-run states, the three
guards, and their tests, including fault injection. Its tests set the backend
through the environment, so they build on Phase 1.

Deliverables:
- `src/engine/atomic_fs.rs`: `exchange_dirs`, `exchange_supported`
- `src/session/cloud.rs`: takeover branch, new codes, guards
- `src/cli/session.rs`: `explicit_as`
- `tests/same_host_import_test.rs`
- `docs/guides/cloud-sync-setup.md`: same-host import; dated note in
  `DESIGN-session-migration.md`

### Phase 3: Measured acceptance

The ignored endpoint-serving test and the measured run against a release
build, reported with the last pull request. No product code.

Deliverables:
- an ignored test that serves the fake endpoint until told to stop
- the measured run's exit codes and timings in the pull request body

## Consequences

### Positive

- One workspace's sessions go to the bucket by setting its environment; no
  file under the home directory changes, so other workspaces' running
  sessions keep their backend.
- `koto config list` answers "where did this come from" for every session
  setting, and a bad value shows up there instead of failing the command an
  operator diagnoses with.
- The bucket key can stay out of the AWS names, so AWS tools a session runs
  don't inherit it.
- A coordinator moves to a fresh workspace on its own host under its own
  name, and every by-name lookup keeps working with no caller change.
- A command in the source workspace can no longer touch the imported copy,
  even when the marker listing fails, and `cleanup` and `resolve` from the
  source can't delete or overwrite it.

### Negative

- The import gains a second write order (marker first) beside the
  cross-host one (marker last), with its own re-run states. It is the most
  intricate code in the session store.
- The takeover depends on atomic directory exchange. On CIFS/SMB, FAT and
  hfsplus stores it refuses and the operator falls back to `--as`, which
  breaks by-name lookup there.
- After a crash mid-takeover, `<name>` refuses commands from both workspaces
  until the import runs again.
- The macOS exchange arm isn't exercised by CI, which runs on Linux only.
- `KotoConfig` carries two more skip-serialized side channels.
- A file with a `[session]` table and no `backend` still resets an earlier
  layer's backend to `local`, as today; the origin report now shows it as
  `default`, which may surprise.

### Mitigations

- The takeover branch is narrow: it runs only for a local holder with the
  source's session id, no `--as`, and a different workspace, so the
  cross-host path and its tests are untouched; fault injection covers every
  row of the re-run table.
- The exchange probe runs before any write, so an unsupported store refuses
  with A unchanged and an `--as` hint.
- The takeover-pending refusal names the import to re-run.
- The exchange primitive is a thin wrapper next to `atomic_rename_dir`, whose
  macOS arm uses the same `renamex_np` call with a different flag.
- The `[session]`-table reset is proposed as its own issue.
