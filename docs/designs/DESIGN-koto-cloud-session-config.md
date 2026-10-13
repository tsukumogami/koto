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
  TBD
rationale: |
  TBD
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
