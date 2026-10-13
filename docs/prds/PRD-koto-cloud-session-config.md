---
schema: prd/v1
status: Draft
problem: |
  koto takes its session backend and cloud location only from the user file,
  which every running session re-reads, or a project file in the exact working
  directory; it takes the bucket key only from the AWS_* pair every AWS tool
  reads; and under the cloud backend an import into a fresh workspace on the
  same host is refused as import_name_taken by the source's own local copy,
  while the callers that find sessions by name never pass --as.
goals: |
  An operator puts one workspace's sessions on the cloud backend through that
  workspace's environment alone, can see which layer every session setting
  came from, keeps the bucket key under koto's own names, and moves a stopped
  session into a fresh workspace on the same host under its own name. A host
  that sets none of the new variables keeps today's behavior.
upstream: docs/briefs/BRIEF-koto-cloud-session-config.md
---

# PRD: Cloud session config and same-host import

## Status

Draft

## Problem Statement

`koto session import` moves a stopped session from one workspace's copy in
the bucket into a new local session, and it runs only on the cloud backend.
Three gaps keep a host from using it.

koto reads `session.backend` and the cloud location (`session.cloud.endpoint`,
`bucket`, `region`, `path_style`) from `~/.koto/config.toml` and from
`.koto/config.toml` in the directory a command runs from, and from nowhere
else. koto re-reads the user file on every command, so changing it on a host
with running sessions moves every one of them to the new backend mid-run. A
project file has to sit in each command's working directory, and a tool that
provisions workspaces writes neither file. The request store's limits and the
decider already take environment variables a workspace can set for its own
processes; the session backend doesn't.

The bucket key comes only from `AWS_ACCESS_KEY_ID` and
`AWS_SECRET_ACCESS_KEY`, or from the user file. Exporting the AWS pair to a
session's processes hands the same key to every AWS-aware tool they run.

Under the cloud backend the local store is always `$HOME/.koto/sessions`, so
session names are machine-wide while each workspace has its own prefix in
the bucket. Importing a session from workspace A into a fresh workspace B on
the same host is refused as `import_name_taken`: A's own local copy holds the
name. `--as <new-name>` gets past the refusal and breaks every caller that
finds the session by name; shirabe's session convention says an import under
another name is outside it, and its import step never passes `--as`.

Why now: a coordinator's planned handoff starts from an imported session, on
a host whose other sessions must stay on the local backend while it happens.

## Goals

An operator who wants one workspace's sessions in the bucket sets that
workspace's environment and edits no config file; sessions in other
workspaces on the host keep their backend. `koto config list` says, for every
session setting, whether its value came from the environment, the project
file, the user file or koto's default, which answers "why is this session on
that backend" without reading files. The bucket key travels under koto's own
variable names. A stopped session moves to a fresh workspace on the same host
under the name it has, so every caller that finds it by name still finds it.
Hosts that use the files or the AWS pair today see no change in where their
sessions live or which key they use.

## User Stories

1. As an operator running several workspaces on one host, I want to turn on
   the cloud backend for one workspace through its environment, so that the
   other workspaces' running sessions don't switch backend under me.
2. As an operator diagnosing a session that synced (or didn't) unexpectedly,
   I want `koto config list` to show where each session setting came from,
   so that I fix the right layer.
3. As an operator handing a bucket key to koto, I want to pass it under
   koto-prefixed names, so that the AWS tools a session runs don't inherit
   it.
4. As a replacement coordinator in a fresh workspace on the same host, I want
   `koto session import <name> --from <old workspace>` to succeed without
   `--as`, so that the session keeps the name every caller looks it up by and
   I continue from where the stopped coordinator left off.
5. As an operator of a host that keeps its cloud settings in
   `~/.koto/config.toml` and its key in the AWS pair, I want upgrading koto
   to change nothing about where my sessions live or which key they use.

## Requirements

### Functional

**Session settings from the environment**

- **R1.** koto reads `session.backend`, `session.cloud.endpoint`,
  `session.cloud.bucket`, `session.cloud.region` and
  `session.cloud.path_style` from environment variables, one per setting,
  named under the `KOTO_SESSION_` prefix by a single rule from the setting's
  dotted name. The design fixes the names.
- **R2.** For each of those settings, a value from its environment variable
  wins over the project file, which wins over the user file, which wins over
  koto's default. The setting is resolved per key: a variable that sets one
  setting leaves the others to the file layers.
- **R3.** An environment variable that is unset, empty, or only whitespace
  counts as unset for every variable this feature adds, and the setting falls
  through to the next layer.
- **R4.** An environment value that koto can't use (a backend other than
  `local` or `cloud`; a path style other than `true` or `false`) never falls
  back silently to another layer or to the local backend. Every command that
  needs the session backend refuses with an error naming the variable, and
  `koto config list` still runs and shows the value with its origin.
- **R5.** `session.cloud.marker_retain_days` gets no environment variable; it
  stays a user-file setting.

**Credentials**

- **R6.** koto reads the bucket's access key and secret key from a
  koto-prefixed pair of environment variables. When either variable of the
  pair is set (R3), both credentials come from that pair, and the AWS pair is
  not consulted for either.
- **R7.** When exactly one variable of the koto-prefixed pair is set, every
  command that builds the cloud backend refuses with an error naming the
  missing variable. koto never combines one half from the koto pair with one
  from the AWS pair or from a file.
- **R8.** With neither koto-prefixed credential variable set, credentials
  resolve exactly as today: `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`
  over the user file, with today's handling of each.
- **R9.** A value from either koto-prefixed credential variable is redacted
  wherever koto redacts credentials today (`koto config list`, command
  output, failure reports), under that variable's name, including when a
  config file fails to load.
- **R10.** Credentials stay barred from the project file, as today.

**Origins in `koto config list`**

- **R11.** `koto config list` reports an origin for each of the session
  settings `session.backend`, `session.cloud.endpoint`, `bucket`, `region`,
  `path_style`, `access_key`, `secret_key` and `marker_retain_days`: `env`
  with the variable's name, `project`, `user`, `default` (koto's built-in
  value applies), or `unset` (no layer set it and koto has no default).
- **R12.** The origins are additive in both output forms. The existing
  tables and fields keep their names, shapes and values; the TOML form stays
  valid TOML that parses to the same tables; credential values stay redacted.
- **R13.** A file layer counts as the origin of a setting only when that file
  names the key, not when a file merely has a `[session]` table.

**Same-host import**

- **R14.** `koto session import <name> --from <workspace>` with no `--as`
  succeeds on the host the source was created on, in a different workspace,
  when the session holding `<name>` in the local store is the source itself:
  the same session, whose local log holds no event the source's copy in the
  bucket lacks. The imported session keeps `<name>`.
- **R15.** After a same-host import, the source workspace's copy refuses
  every later command with `session_migrated` naming the new session, exactly
  as after a move between hosts, and the source's local log is never written
  into the imported session.
- **R16.** A same-host import refuses, and leaves the source's local copy and
  its prefix in the bucket untouched, when: the local copy holds events the
  bucket lacks (the error says to push it with `koto session resolve <name>
  --keep local` from the source workspace first); a process holds the
  session's lock; or a local session names the source as its parent.
- **R17.** A local session holding `<name>` that is not the source keeps
  refusing as `import_name_taken`, naming `--as`, as today.
- **R18.** Every failure of a same-host import either leaves the source
  untouched (local copy, prefix and marker) or is completed by running the
  same command again. No failure strands the name with neither session in
  place, and none leaves the old and new sessions both live under it.
- **R19.** The import's JSON output keeps its fields and meanings; a
  same-host import reports `name` as the kept name.

**Documentation**

- **R20.** `docs/guides/cloud-sync-setup.md` documents the environment
  variables, their precedence, the credential pair, the origin report and the
  same-host import. The config-and-cloud-sync and session-migration designs
  record the change.

### Non-functional

- **R21.** A host that sets none of the new variables resolves every setting
  to the value it resolves to today, builds the same backend, and uses the
  same credentials. The only visible difference is the origin report.
- **R22.** The same-host import adds no remote request to a cross-host
  import, and at most a constant number of local file operations to a
  same-host one.
- **R23.** Every requirement above is covered by a test that runs in CI on
  every pull request without secrets, including an end-to-end same-host
  import with the cloud backend enabled through the environment only.

## Acceptance Criteria

- [ ] With a disposable `HOME`, no config file, and the backend, endpoint,
      bucket, region, path style and koto credential pair set only through
      environment variables, `koto init` and `koto next` sync a session to
      the bucket (R1, R6).
- [ ] With the project file setting `session.backend = "local"` and the
      backend variable set to `cloud`, the backend is cloud; with the
      variable unset, it's local; with the variable set to the empty string,
      it's local (R2, R3).
- [ ] Each of `session.cloud.endpoint`, `bucket`, `region` and `path_style`
      resolves to its variable's value over a project and a user file that
      also set it, and to the project file's value when the variable is unset
      (R2).
- [ ] The backend variable set to `s3` makes `koto next` exit non-zero with
      an error naming the variable, and `koto config list` exits 0 showing
      `s3` with origin `env`. The path-style variable set to `yes` behaves
      the same way (R4).
- [ ] With both koto credential variables and both AWS variables set to
      different values, the cloud backend is built with the koto pair (R6).
- [ ] With only the koto access-key variable set and the AWS pair set, a
      command needing the cloud backend refuses naming the koto secret-key
      variable, and no request reaches the endpoint (R7).
- [ ] With no koto credential variable, the AWS pair wins over user-file
      credentials, and an exported empty `AWS_ACCESS_KEY_ID` behaves as it
      does today (R8).
- [ ] A template command that prints the koto credential values in a session
      whose command environment carries them shows each as
      `[REDACTED:<variable name>]`, also when the user file fails to parse
      (R9).
- [ ] `koto config set session.cloud.access_key x` without `--user` is
      refused as today (R10).
- [ ] `koto config list` and `koto config list --json` report, for each of
      the eight session settings, the origin `env` (with the variable name),
      `project`, `user`, `default` or `unset`, each case covered by a test
      (R11).
- [ ] The TOML output parses to the same `session`, `request_store`,
      `run_journal`, `workflows` and `decider` tables as before; the JSON
      output keeps every existing field at its path; `access_key` and
      `secret_key` print `<set>` (R12).
- [ ] A user file with a `[session]` table and no `backend` key reports
      `session.backend` as `default`, not `user` (R13).
- [ ] With one `HOME`, cloud enabled through the environment only: a session
      created in workspace A and advanced to a waiting state, then stopped,
      is imported in workspace B with `koto session import <name> --from <A>`
      and no `--as`; the output reports `name` as `<name>`; every context key
      reads back byte for byte in B; `koto next` in B advances it; `koto
      next <name>` run from A exits 2 with `session_migrated` naming B (R14,
      R15, R19).
- [ ] The same-host import refuses, with the source's local files, prefix and
      marker unchanged, when the local log has an event the bucket lacks, when
      the session's lock is held, and when a local child names the source as
      its parent (R16).
- [ ] A local session named `<name>` with a different session identity still
      refuses `import_name_taken` naming `--as` (R17).
- [ ] With a fault injected on each step of a same-host import (the push, the
      move into place, the marker), either the source is unchanged or a re-run
      of the same command completes the import, and the name is always held
      by exactly one session (R18).
- [ ] The guide documents the variables, precedence, credential pair, origin
      report and same-host import, and the two designs carry a dated note
      (R20); the doc-names check passes.
- [ ] The existing config, credential, redaction and session-migration tests
      pass unchanged apart from tests that assert on the exact set of
      `config list` fields (R21).
- [ ] The new tests run in the default `cargo test` job (R23).

## Out of Scope

- A move between hosts. It works today with `--from` and `--as`; its
  acceptance waits for a separate decision.
- Writing any workspace's environment, overlays or secrets. This feature adds
  the names koto reads, not the configuration that sets them.
- Environment variables for `session.cloud.marker_retain_days` (R5) or for
  settings outside `[session]`.
- Per-workspace local session stores. The same-host import works with
  machine-wide names.
- Changes to shirabe's import step. With R14, its call without `--as` works
  as written.
- Re-pointing `workflows/publish-location` on import; the key is carried as
  today (tracked in koto#361).
- Passing the new variables through to template commands. Non-legacy ticks
  keep their minimal environment.

## Known Limitations

- A process that was already running in workspace A before the import and
  keeps running afterwards can have cached "not migrated" for the session.
  The stopped-source rule covers it, as it does for a cross-host import; the
  lock check (R16) catches a process that holds the lock at import time.
- `koto config set` writes files only. After it, `koto config get` can still
  show an environment value that wins over the file; the origin report says
  so.

## Decisions and Trade-offs

**Environment above the project file.** The variables win over both files.
Alternative: put them between the project and user files. Rejected because a
workspace's environment is the most specific signal an operator gives, a
committed project file shouldn't override it, and the request store and
decider variables already win over files.

**Credentials resolve as a pair.** When either koto credential variable is
set, both come from the koto pair, and a half-set pair refuses. Alternative:
resolve each variable independently, as the AWS pair does today. Rejected
because it can mix one key's id with another key's secret, and because a
missing half would otherwise fall through to the S3 client's own AWS lookup,
which reaches the very variables the operator meant to keep away.

**Unusable values refuse; they don't fall back.** Alternative: ignore a
malformed value, as the request-store variables do. Rejected because a
backend setting that silently falls back writes sessions somewhere the
operator didn't intend, and several callers already treat a failed config
load as "use defaults", which would mean the local backend.

**Same-host import keeps the name; koto handles it.** Alternatives: koto
picks a free suffixed name; shirabe passes a derived `--as`; local stores
become per-workspace. The first two break every by-name lookup, which is the
reason the import exists; the third touches the run journal, finished
records and live attach, far more than this feature needs. So the fix is in
koto and shirabe's import step needs no change.

**Empty means unset for the new variables only.** The AWS pair keeps today's
handling, where an exported empty value overrides a file, because R21 promises
no change for hosts that don't use the new names.
