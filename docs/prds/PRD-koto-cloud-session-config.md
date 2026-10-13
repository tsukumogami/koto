---
schema: prd/v1
status: Accepted
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

Accepted

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
Hosts that set none of the new variables see no change in where their
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
5. As a workspace provisioning tool, I want to enable the cloud backend by
   setting environment variables for the workspace's processes, so that I
   never write a file under the user's home directory.
6. As an operator of a host that keeps its cloud settings in
   `~/.koto/config.toml` and its key in the AWS pair, and sets none of the new
   variables, I want upgrading koto to change nothing about where my sessions
   live or which key they use.

## Terms

- **Stopped source.** No process advances the session or writes its context,
  and its last write reached the bucket. This is the session-migration
  design's precondition for any import, unchanged here.
- **The source's own local copy.** The session directory holding `<name>` in
  `$HOME/.koto/sessions` whose state-file header carries the same, non-empty
  `session_id` as the source's state file in the bucket under the `--from`
  workspace's prefix.
- **Ahead of the bucket.** The local state log's last event `seq` is greater
  than the last `seq` in the bucket's copy.
- **Legacy-environment session.** A session whose recorded command
  environment is marked legacy, so its template commands inherit the calling
  process's whole environment rather than koto's minimal one.
- **Live session.** A session that accepts `koto next` without refusing with
  `session_migrated`.
- **Builds the backend.** A command that opens any session's state or context
  through the session backend: among others `koto init`, `koto next`,
  `koto status`, every `koto context` subcommand and every `koto session`
  subcommand. `koto config get`, `koto config list` and `koto version` don't.

## Requirements

### Functional

**Session settings from the environment**

- **R1.** koto reads these settings from these environment variables. The
  name is `KOTO_` followed by the setting's dotted name, upper-cased, with
  dots replaced by underscores.

  | Setting | Variable | Default | Valid values |
  |---------|----------|---------|--------------|
  | `session.backend` | `KOTO_SESSION_BACKEND` | `local` | `local`, `cloud` |
  | `session.cloud.endpoint` | `KOTO_SESSION_CLOUD_ENDPOINT` | none | any string |
  | `session.cloud.bucket` | `KOTO_SESSION_CLOUD_BUCKET` | none | any string |
  | `session.cloud.region` | `KOTO_SESSION_CLOUD_REGION` | none | any string |
  | `session.cloud.path_style` | `KOTO_SESSION_CLOUD_PATH_STYLE` | `false` | `true`, `false` |
  | `session.cloud.access_key` | `KOTO_SESSION_CLOUD_ACCESS_KEY` | none | any string |
  | `session.cloud.secret_key` | `KOTO_SESSION_CLOUD_SECRET_KEY` | none | any string |

  Values are compared after trimming surrounding whitespace, and case
  matters: `Cloud` and `TRUE` are not valid.
- **R2.** For each of the first five settings, a value from its variable wins
  over the project file, which wins over the user file, which wins over the
  default. Each setting resolves on its own: a variable that sets one leaves
  the others to the file layers. Each command resolves the settings from its
  own process environment when it runs.
- **R3.** A variable from the table that is unset, empty, or only whitespace
  counts as unset, and the setting falls through to the next layer. This
  holds for the two credential variables too.
- **R4.** A value for `KOTO_SESSION_BACKEND` or
  `KOTO_SESSION_CLOUD_PATH_STYLE` outside its valid values never falls back to
  another layer or to the local backend. Every command that builds the
  backend exits non-zero with an error naming the variable and the value, and
  creates or changes no session. `koto config list` and `koto config get`
  still exit 0 and show the value with its origin. File-layer values are
  handled exactly as today.
- **R5.** `session.cloud.marker_retain_days` has no environment variable: a
  variable named `KOTO_SESSION_CLOUD_MARKER_RETAIN_DAYS` has no effect.

**Credentials**

- **R6.** When `KOTO_SESSION_CLOUD_ACCESS_KEY` or
  `KOTO_SESSION_CLOUD_SECRET_KEY` is set (R3), both credentials come from
  those two variables, and neither the AWS pair nor any file supplies either
  one.
- **R7.** When exactly one of the two is set, every command that builds the
  cloud backend exits non-zero with an error naming the missing variable,
  before any request reaches the endpoint. A command that builds the local
  backend is unaffected.
- **R8.** With neither set, credentials resolve exactly as today:
  `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` over the user file, with
  today's handling of each, an exported empty AWS variable included.
- **R9.** A value from either koto credential variable is redacted on every
  surface that redacts credentials today, labeled with that variable's name:
  `koto config list` prints `<set>`; command output and failure reports koto
  captures show `[REDACTED:<variable>]`, also when a config file fails to
  load. Where the koto and AWS values are equal, the koto variable's name is
  the label.
- **R10.** Credentials stay barred from the project file, as today.

**Origins in `koto config list`**

- **R11.** `koto config list` reports an origin for each of the eight session
  settings: the seven in R1's table and `session.cloud.marker_retain_days`.
  An origin is one of: `env` with the variable that supplied the value (a
  koto variable, or `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` for
  credentials from the AWS pair); `project`; `user`; `default` (no layer set
  it and the setting has a default: `session.backend`,
  `session.cloud.path_style`, `session.cloud.marker_retain_days`); or `unset`
  (no layer set it and it has no default).
- **R12.** The report is additive. `koto config list --json` gains one
  top-level field, `origins`, an object keyed by the dotted setting name whose
  values are `{"source": "<origin>"}` plus `"variable": "<name>"` when the
  source is `env`. The TOML form appends one comment line per setting,
  `# origin: <setting> = <origin>` or `# origin: <setting> = env <variable>`,
  after the tables, so it parses to exactly the tables it parses to today.
  Every existing field keeps its path, type and value, and credential values
  stay `<set>`.
- **R13.** A file counts as a setting's origin only when the file names the
  key, in either TOML spelling (a `[session]` table entry or a dotted key);
  a `[session]` table without the key does not. A key named with any value
  counts.

**Same-host import**

- **R14.** `koto session import <name> --from <workspace>` with no `--as`,
  run from a different workspace on the same host, succeeds and keeps
  `<name>` when the local session holding `<name>` is the source's own local
  copy and is not ahead of the bucket.
- **R15.** After it, every command run from the source workspace against
  `<name>` that reads the session's state or context exits 2 with
  `session_migrated` naming the new session's workspace, as after a move
  between hosts. The imported session's log is the bucket's events plus one
  `session_imported` event; nothing from the source's local files is written
  into it after the import reads the bucket.
- **R16.** A same-host import refuses with exit 2, and leaves the source's
  local files, its prefix in the bucket and its marker unchanged, when (checked
  in this order, the first that applies wins):
  - another process holds the session's state-file lock (a non-blocking
    attempt to take it fails): code `import_source_running`, and the error
    says the source is still running;
  - the source's own local copy is ahead of the bucket: code
    `import_source_unpushed`, and the error says to run `koto session resolve
    <name> --keep local` from the source workspace first;
  - a session in the local store names `<name>` as its parent: code
    `import_source_has_children`, and the error names the child.
  A local copy behind the bucket is not a refusal: the bucket's copy is what
  the import reads. A local copy whose `session_id` is empty is not the
  source's own local copy (R17).
- **R17.** A local session holding `<name>` that is not the source's own
  local copy keeps refusing as `import_name_taken`, naming `--as`, and
  nothing local or remote changes. `--as` keeps today's behavior in every
  case.
- **R18.** The import's writes are: the new session's objects under the
  importing workspace's prefix, the marker on the source's prefix, and the
  local directory that holds `<name>`. The failures in scope are a refused or
  failed remote request, a failed local rename, and the process exiting
  between any two of those writes. After any of them, either the source's local files,
  prefix and marker are unchanged, or running the same command again
  completes the import. At every point the name in the local store is held by
  at most one session directory, and the local store never holds the new
  session while the source's prefix lacks its marker.
- **R19.** A same-host import prints the same JSON fields as a cross-host
  import, with `name` equal to the kept name.
- **R20.** The takeover in R14 applies only when `--from` names a workspace
  other than the one the import runs in. An import whose `--from` resolves to
  the importing workspace's own prefix behaves as today: with no `--as` it
  refuses as `import_name_taken`. A cross-host import, with or without
  `--as`, behaves as today.

**Documentation**

- **R21.** `docs/guides/cloud-sync-setup.md` lists the seven variables with
  their settings, states the precedence of R2, the credential rule of R6 and
  R7, shows an example of the origin report, and shows a same-host import
  without `--as`. `DESIGN-config-and-cloud-sync.md` and
  `DESIGN-session-migration.md` each gain a dated status note naming this
  feature's design. `cargo test --test doc_names` passes.

### Non-functional

- **R22.** A host that sets none of the seven variables resolves every
  setting to the value it resolves to today, builds the same backend, and
  uses the same credentials; the origin report is the only visible
  difference.
- **R23.** The existing session-migration carrier test sees the same request
  sequence at the fake endpoint as before. A same-host import makes no more
  remote requests than a cross-host import of the same session.
- **R24.** Every acceptance criterion below is a test in the default `cargo
  test` job, which runs on every pull request without secrets.

## Acceptance Criteria

**Settings and precedence**

- [ ] With a disposable `HOME`, no config file, and only the seven variables
      set, `koto init` and `koto next` sync a session to a fake bucket (R1,
      R6).
- [ ] The project file sets `session.backend = "local"`: with
      `KOTO_SESSION_BACKEND=cloud` the backend is cloud; unset, `""` and
      `"  "` each give local (R2, R3).
- [ ] For each of endpoint, bucket, region and path style: the variable wins
      over project and user files that also set it; unset, the project value
      wins over the user value; with neither file, the default applies (R2).
- [ ] In one run, the backend comes from the variable, the bucket from the
      project file and the region from the user file, and `config list`
      reports those three origins (R2, R11).
- [ ] `KOTO_SESSION_BACKEND=s3`: `koto init` and `koto next` exit non-zero
      naming the variable and the value, no session directory is created, and
      `koto config list` exits 0 showing `s3` with origin `env
      KOTO_SESSION_BACKEND`. `KOTO_SESSION_CLOUD_PATH_STYLE=yes` and
      `KOTO_SESSION_BACKEND=Cloud` behave the same, and `koto config get
      session.backend` exits 0 printing `s3`. `KOTO_SESSION_BACKEND=" cloud "`
      selects the cloud backend (R1, R4).
- [ ] Two processes sharing one `HOME`, one with `KOTO_SESSION_BACKEND=cloud`
      and one without, each build their own backend from their own
      environment (R2).
- [ ] `KOTO_SESSION_CLOUD_MARKER_RETAIN_DAYS=1` leaves
      `marker_retain_days` at its user-file or default value and origin (R5).

**Credentials**

- [ ] Both koto credential variables and both AWS variables are set to
      different values, and the user file sets credentials too: the request
      the fake endpoint receives is signed with the koto access key (R6).
- [ ] Only `KOTO_SESSION_CLOUD_ACCESS_KEY` set, with the AWS pair set: a
      cloud command exits non-zero naming `KOTO_SESSION_CLOUD_SECRET_KEY`,
      and the fake endpoint receives no request. The mirror case names
      `KOTO_SESSION_CLOUD_ACCESS_KEY`. With the backend local, both cases
      succeed (R7).
- [ ] Koto credential variables set to `""` with the AWS pair set: the AWS
      access key signs the request (R3, R8).
- [ ] No koto credential variable: the AWS pair wins over user-file
      credentials, and `AWS_ACCESS_KEY_ID=""` overrides a user-file key as it
      does today (R8).
- [ ] `koto config list` prints `<set>` for credentials from the koto pair;
      a legacy-environment session's command that prints both koto values
      shows `[REDACTED:KOTO_SESSION_CLOUD_ACCESS_KEY]` and
      `[REDACTED:KOTO_SESSION_CLOUD_SECRET_KEY]`, including with a user file
      that fails to parse; with the koto and AWS values equal, the label is
      the koto variable's name (R9).
- [ ] `koto config set session.cloud.access_key x` without `--user` is
      refused as today (R10).

**Origins**

- [ ] `--json` output holds `origins` with all eight settings; tests cover
      each source: `env` with a koto variable, `env` with `AWS_ACCESS_KEY_ID`,
      `project`, `user`, `default` and `unset` (R11, R12).
- [ ] The TOML output, parsed, equals the parse of the same configuration's
      output from the current release, and holds one `# origin:` line per
      setting (R12).
- [ ] A user file and a project file each with a `[session]` table and no
      `backend` key report `session.backend` as `default`; a file with the
      dotted key `session.backend = "local"` at top level reports that file
      (R13).

**Same-host import**

All of these share one `HOME` and a fake bucket, with the backend enabled
through the variables only. The source session is created in workspace A
from a template whose second state waits for evidence, advanced into that
state, given three context keys, and then left alone (stopped).

- [ ] `koto session import <name> --from <A>` in workspace B, with no `--as`,
      exits 0 and prints `name` equal to `<name>`, with the same fields as a
      cross-host import (R14, R19).
- [ ] Each context key reads back in B byte for byte (R14).
- [ ] `koto next <name>` in B with the waited-for evidence moves the session
      to its next state (R14).
- [ ] `koto next <name>`, `koto status <name>` and `koto context get <name>
      <key>` run from A each exit 2 with `session_migrated` naming B (R15).
- [ ] B's state log equals the bucket's source events plus one
      `session_imported` event (R15).
- [ ] With an event appended to A's local log and not pushed, the import
      exits 2 with `import_source_unpushed` and the `koto session resolve`
      hint (R16).
- [ ] With the session's lock held by another process, the import exits 2
      with `import_source_running` (R16).
- [ ] With a child session naming `<name>` as parent, the import exits 2
      with `import_source_has_children`, naming the child (R16).
- [ ] With the bucket's copy one event ahead of A's local copy, the import
      succeeds and B's log holds the bucket's events (R16).
- [ ] In each of the three refusals, A's local files, A's prefix and the
      absence of a marker are unchanged (R16).
- [ ] A different session named `<name>` in the local store makes the import
      exit 2 with `import_name_taken` naming `--as`, with nothing changed
      (R17).
- [ ] For each injected failure (the push of the new session, the marker
      write, the local rename, and a process exit after each write), the
      state afterwards has the source unchanged or is completed by a re-run,
      and the local store never holds two directories for `<name>` nor the new
      session without the marker (R18).
- [ ] `koto session import <name> --from <A>` run in A itself, with no
      `--as`, exits 2 with `import_name_taken` and changes nothing; the
      existing cross-host carrier test passes unchanged (R20).

**Documentation and compatibility**

- [ ] The guide holds the seven variable names, the precedence statement,
      the credential-pair rule, an origin report example and a same-host
      import example; both designs carry a dated note; `cargo test --test
      doc_names` passes (R21).
- [ ] With none of the seven variables set, the existing config, credential,
      redaction and session-migration tests pass; the only changes to existing
      tests add the `origins` field to an exact-key-set assertion (R22).
- [ ] The carrier test's recorded request sequence is unchanged, and a
      same-host import's request count is at most a cross-host import's
      (R23).

## Out of Scope

- A move between hosts. It works today with `--from` and `--as`; its
  acceptance is a separate decision.
- Writing any workspace's environment, overlays or secrets. This feature adds
  the names koto reads, not the configuration that sets them.
- Environment variables for `session.cloud.marker_retain_days` (R5) or for
  settings outside `[session]`.
- Per-workspace local session stores. The same-host import works with
  machine-wide names.
- Changes to shirabe's import step. With R14, its call without `--as` works
  as written.
- Re-pointing `workflows/publish-location` on import; the key is carried as
  today (koto#361).
- Passing the new variables through to template commands. Non-legacy ticks
  keep their minimal environment.
- Two imports of the same source started at once from different
  workspaces. The stopped-source rule already excludes it, as it does for a
  cross-host import.

## Known Limitations

- A process that was already running in workspace A before the import and
  keeps running afterwards can hold a cached "not migrated" answer for the
  session. The stopped-source rule covers it, as it does for a cross-host
  import; the lock check (R16) catches a process holding the lock at import
  time.
- `koto config set` writes files only. After it, `koto config get` can still
  show an environment value that wins over the file; the origin report says
  so.

## Decisions and Trade-offs

**Variable names derive from the setting name.** `KOTO_` plus the dotted name,
upper-cased, dots to underscores. Alternative: shorter names
(`KOTO_SESSION_ENDPOINT`). Rejected because one rule is easier to remember and
to check, and the credential names then read as koto's own
(`KOTO_SESSION_CLOUD_ACCESS_KEY`) rather than as AWS look-alikes.

**Environment above the project file.** The variables win over both files.
Alternative: put them between the project and user files. Rejected because a
workspace's environment is the most specific signal an operator gives, a
committed project file shouldn't override it, and the request store and
decider variables already win over files.

**Credentials resolve as a pair.** When either koto credential variable is
set, both come from the koto pair, and a half-set pair refuses. Alternative:
resolve each variable independently, as the AWS pair does today. Rejected
because it can mix one key's id with another key's secret, and because a
missing half would otherwise fall through to the S3 client's own AWS
lookup, which reaches the very variables the operator meant to keep away.

**Unusable values refuse; they don't fall back.** Alternative: ignore a
malformed value, as the request-store variables do. Rejected because a
backend setting that silently falls back writes sessions somewhere the
operator didn't intend, and several callers already treat a failed config
load as "use defaults", which would mean the local backend.

**Origins are additive.** A JSON field and TOML comment lines, rather than a
new table or a reshaped value. Alternative: an `--origins` flag. Rejected
because the point is that the answer is there when someone looks, and the
additive shape keeps every existing reader working.

**Same-host import keeps the name; koto handles it.** Alternatives: koto
picks a free suffixed name; shirabe passes a derived `--as`; local stores
become per-workspace. The first two break every by-name lookup, which is the
reason the import exists; the third touches the run journal, finished
records and live attach, far more than this feature needs. So the fix is in
koto and shirabe's import step needs no change.

**Empty means unset for the new variables only.** The AWS pair keeps today's
handling, where an exported empty value overrides a file, because R22
promises no change for hosts that don't use the new names.
