---
schema: design/v1
status: Planned
problem: |
  koto's removals and reads aren't safe on a host where many workflows share
  one store and one bucket. A scratch KOTO_HOME still reaches a bucket the
  environment configures; prune keeps sessions by a `coordinate-` name prefix;
  under the cloud backend every session-scoped bucket key is built from the
  caller's working directory, so a read from another directory refuses and a
  write from a subdirectory forks a copy; a read can write into the session
  it reads; and `origin_mismatch` advises `koto session cleanup`.
decision: |
  Keep koto#470's guard as shipped and add the tests it lacks. Under
  KOTO_HOME, select the cloud backend only from the root's own config file,
  and add `koto session dir --json` to report root, backend, its source and
  bucket. Give the cloud backend a per-session home-prefix resolver, read
  from `origin.store.prefix` or a fallback confirmed by session id, and use
  it for every session-scoped read, write and delete. Serve a lost assigned
  value from the log, and log a read made outside the session's own tick in
  the reader's anchor. Drop prune's rule f once the continues relation works
  on the cloud backend. Reword `origin_mismatch` and pin both codes in tests.
rationale: |
  The guard already meets the requirement, so re-opening its floor would only
  move a settled line. Failing closed under KOTO_HOME protects harnesses that
  forget to check, and the root's config is an opt-in no host-wide setup can
  export. One resolver for reads, writes and deletes closes the stale read,
  the stray copies from subdirectory ticks and a prune deleting the wrong
  copy, and it uses the prefix koto#463 began recording. The relation is the
  fact the name rule only approximated, and dropping the rule before the
  relation works on the cloud backend would release every coordinator on a
  cloud host with nothing in its place.
upstream: docs/prds/PRD-koto-trusted-removal.md
decision_provenance: inline-resolved
---

# DESIGN: Removal a lineage can trust

## Status

Planned

## Context and Problem Statement

The requirements are in the PRD this design answers
([PRD-koto-trusted-removal](docs/prds/PRD-koto-trusted-removal.md)). Part of
the feature is already on main: the cloud-only short-retention guard on bulk
prune (koto#470), the single root resolver that `KOTO_HOME` moves
(`src/koto_home.rs`, koto#471), the `session_bound_elsewhere` refusal and the
recorded `origin.store.prefix` (koto#463), and lossless pulls with
conditional pushes (koto#480). This design covers what's left. Five technical
problems remain.

**Isolation stops at the path.** `build_backend` (`src/cli/mod.rs`) calls
`config::resolve::load_config`, whose top layer applies `KOTO_SESSION_*` from
the environment, and never consults the root. A process with `KOTO_HOME` at a
scratch directory and the host's cloud variables in its environment builds a
`CloudBackend` against the real bucket. The PRD requires that under
`KOTO_HOME` only the root's own `config.toml` can select the cloud backend
(R4), that a write-free JSON report says what koto resolved (R5), that a
scratch run of init, tick, prune and cleanup leaves the real `~/.koto`
byte-identical and makes no request to an environment-only bucket (R6), and
that commands a tick runs land in the same root and backend (R7).
`koto session dir` with no name already prints the root and builds no
backend; it reports nothing about the backend.

**Prune keeps sessions by name.** `later_rule` in `src/cli/session_prune.rs`
returns rule f for any name starting with `coordinate-`. The PRD requires
that rule to go (R8), that a finished session a later session continues be
kept on both backends with the successor named (R9), and that the rule's
removal not reach a release before the continues relation works on the cloud
backend, with release notes naming `koto session pin` and
`koto session link --continues` (R10). The relation itself is another
feature's work: koto#481 adds `koto/links/continues/<pred>`, a new rule r in
`first_rule`, `Judgment::keeps()` and the `relation_keepers` report field on
the local backend, and a follow-up change adds the bucket walk that makes it
hold on the cloud backend. That feature leaves rule f in place.

**Every bucket key comes from the caller's directory.** `CloudBackend::new`
sets one `prefix` field from the hash of the working directory
(`repo_id`, `src/session/local.rs`), and about thirty session-scoped key
builders in `src/session/cloud.rs`, plus the helpers in
`src/session/sync.rs`, use it. A session's identity in the bucket is the
prefix it was created under, which koto#463 began recording as
`origin.store.prefix`. Today that record is read only by `bound_elsewhere`,
which turns a mismatch into a refusal for reads, while pushes go to the
caller's prefix unconditionally (koto#465, koto#466). The PRD requires a read
from any directory to use the session's own copy and return its current value
even over a warm host cache (R11); a session with no recorded prefix to be
located only through a fallback the bucket confirms by `session_id`, and to
refuse with `session_unlocated` otherwise (R12); writes to go to the same
copy, including a tick from a subdirectory (R13); a read from outside the
session's own workspace to refuse when the bucket can't answer, while one
from inside keeps today's warn-and-serve-local (R14); the tick anchor rule to
stay (R15); a rebind to leave the copy where it is (R16); and a read from
another directory to cost no more requests than from the session's own,
plus one listing per distinct fallback location (R21).

**A read can write into the session it reads.** `restore_assigned`
(`src/cli/context.rs`) calls `context_assign::reconcile` before a
`koto context get` or `exists`, and after a lost assigned-key store write it
stores the value into the read session (koto#436). And `read_log_target`
sends a read whose reader anchor holds the session (`koto/links/held-by/`) to
the session's own log through `append_own_read`, which pushes the state file.
The PRD requires that outside the session's own tick a read leave the state
log, key files and bucket objects byte-identical, with only a read inside
that session's own tick still logging there (R17), and that a read after a
lost write still return the assigned value, served from the log, with the
session's next tick restoring the store (R18).

**A refusal advises deletion.** Both `origin_mismatch` messages in
`check_origin` (`src/cli/init_entry.rs`) end with "remove it with
`koto session cleanup <name>`", and `tests/init_entry_flags.rs` pins that
text. `execution_anchor_mismatch` (`src/cli/mod.rs`) names the bound
directory and `koto session rebind`, never a removal. The PRD requires
neither code to name `session cleanup`, `--cleanup`, `session prune` or
`workspace prune`; the `origin_mismatch` messages to name another session
name and either the directory the session belongs to or finishing it with
the koto version that started it (R19); and the four documents that describe
the codes to agree (R20).

Two requirements constrain everything: sessions created before
`origin.store.prefix` existed keep working from their own directory exactly
as today (R22), and everything except the name rule's removal can ship
before the continues relation does (R23).

## Decision Drivers

- **Fail closed where a mistake can't be undone.** Bucket deletes are
  permanent and shared by every host; anything that could reach a real bucket
  by accident should stop rather than warn.
- **One answer to "which copy is this session".** Reads, writes and deletes
  that disagree on the copy are how a session forked and lost events before
  koto#480.
- **No guess becomes a read.** koto#463 refused instead of resolving because
  a wrong guess is a silent stale read. Any resolution has to be confirmed or
  refuse.
- **Nothing ships that releases a live lineage.** The name rule can only go
  when something else keeps what it kept.
- **Request cost stays flat.** Cross-directory reads are the normal case for a
  successor, and the cloud read path already has a request budget.
- **Old sessions keep working.** Sessions with no recorded prefix and hosts
  running an older koto beside a newer one exist today.
- **Small, separately shippable pieces.** Four of the five pieces don't need
  the relation and can go out before it.

## Considered Options

### Decision 1: The removal guard's floor and confirmation

Resolved inline.

The open question the feature started with was where the guard's floor sits,
what counts as a whole-store selection, and how a person confirms on purpose.
koto#470 answered all three and is on main: on the cloud backend, a direct
`koto session prune --apply` refuses with `prune_short_retention` (exit 2)
when `retention.keep` or `retention.idle_period` is under one hour, the
selection is `--all`, `--execution-dir` alone or `--older-than` under an
hour, and the judgment would remove at least one session; the error names
each short setting with value and source, the backend, the bucket, the store
path and the count; `--confirm-short-retention` goes ahead; and every apply
prints `koto session prune: removing from <store>` first. `koto workspace
prune --yes` refuses the same way.

Key assumptions:
- The PRD's R1 to R3 restate this behaviour, so nothing in the code changes.

#### Chosen: keep koto#470's guard and add the missing tests

The guard stays as shipped. The work is tests the shipped suite lacks: the
`--execution-dir` and `--older-than 1s` selections and the one-hour edge on
both sides, a confirmed run over a store mixing released, pinned and
hierarchy-kept sessions that checks exactly the released ones go locally and
in the bucket, the local session directories listed before and after a
refusal (the shipped tests check the bucket), a store where nothing would be
removed, and the stderr store line appearing before any removal.

#### Alternatives Considered

**Guard the local backend too.** A one-second retention against a real local
store also removes almost everything. Rejected because local removals on a
host can be recovered from the host's own archives, the guard exists for the
bucket deletes no host can undo, and every test suite that prunes a scratch
store directly on purpose would need the flag.

**A lower or configurable floor.** Rejected because the floor only gates the
direct-apply form; the preview-then-plan path is unaffected at any
retention, and a configurable floor is one more setting a test environment
could inherit.

**Guard marker-mode prune (`--migrated-from`).** Rejected for now because it
removes migration markers and old copies beside them, not finished sessions,
and the continues relation changes what marker prune may remove anyway.

### Decision 2: Isolation under `KOTO_HOME`

Resolved inline.

A scratch `KOTO_HOME` moves every path but leaves backend selection to
`load_config`, whose environment layer wins. The question is how koto keeps
a scratch run off a real bucket, how a caller who really wants the cloud
backend under a moved root says so, and how a harness checks before it runs.
The PRD fixed the property (R4 to R7) and the opt-in (the root's own
config). What's left is where the rule lives and what the report prints.

Key assumptions:
- `KOTO_HOME` hasn't shipped in a release, so nobody depends on combining it
  with an environment-only bucket.
- A tick passes `KOTO_HOME` to its commands only when the ticking koto has it
  set (`koto_home_override`), so a nested koto is under the same rule as its
  parent.

#### Chosen: decide the backend in `load_config`, report through `koto session dir --json`

After `load_config` applies every layer, when `koto_home_override()` is set
it checks the origin `session_env` recorded for `session.backend`. If the
backend resolved to `cloud` from any origin other than the user file under
the root (`Origin::User` with the root being `KOTO_HOME`), it sets the backend
back to `local`, records the origin as `Default`, and returns a warning naming
the setting it ignored (for example `KOTO_SESSION_BACKEND` from the
environment, or the project file's `session.backend`). The bucket, endpoint,
region and credentials still resolve from every layer, so a root whose
`config.toml` says `backend = "cloud"` keeps using credentials from the
environment and never needs a secret written into it. `build_backend` prints
the warning once to stderr. Because the rule sits in `load_config`, every
verb and every koto a tick starts gets the same answer from the same root.

`koto session dir --json` with no name runs `load_config` (no backend, no
network, no cache write) and prints
`{"root": ..., "backend": "local"|"cloud", "backend_source": "default"|"user-config"|"project-config"|"env", "bucket": ...}`,
with `bucket` only for the cloud backend and no credential or endpoint
userinfo. When the rule above set the backend back to local, the object also
carries `backend_ignored` naming the setting it ignored (the variable or the
file, never its value), so a harness sees the downgrade without reading
stderr. The bare `koto session dir` output is unchanged, so the scripts
and tests that parse it keep working. The rule compares the user file's path
with `<root>/config.toml`, not only the origin tag.

The isolation test extends `tests/koto_home_test.rs`: a sentinel `HOME` with
a populated `.koto` (sessions, `requests/`, `config.toml`), a scratch
`KOTO_HOME`, the in-process bucket stand-in configured through
`KOTO_SESSION_*` deliberately left in the spawned commands' environment, and
a run of init, a tick, a confirmed prune and `koto session cleanup`. It
asserts no request reached the stand-in, the sentinel hashes the same, and
one warning names `KOTO_SESSION_BACKEND`; the same run with the root's
config selecting cloud reaches the stand-in.

#### Alternatives Considered

**Report only, and let harnesses refuse.** Rejected because the incident was
a script that skipped a setup step; protection that depends on every harness
remembering a check fails the same way.

**A dedicated opt-in variable.** Rejected because a host-wide setup can
export it as easily as it exports the cloud variables, and the incident's
script inherited everything the host exported.

**Refuse mutating verbs under `KOTO_HOME` with environment-only cloud
config.** Rejected because it protects prune and cleanup but not the writes
every init and tick make to the bucket, and a refusal per verb is more
surface than one rule at config load.

**Change bare `koto session dir` to print JSON.** Rejected because external
suites and `tests/koto_home_test.rs` parse its single-line output.

### Decision 3: Which bucket copy a named session reads and writes

Resolved inline.

This is the feature's largest change and the one the "single resolver"
question names. Under the cloud backend the copy a command touches is
`<prefix>/<name>/`, and `prefix` is fixed per process from the working
directory. The session's own copy is the one under `origin.store.prefix`.
The question is how every session-scoped operation finds and uses that copy,
what happens for sessions without the record, and what a read does when the
bucket can't answer.

Key assumptions:
- Sessions created since koto#463 carry `origin.store.prefix`; most live
  lineages on the affected host are that recent.
- A host's local store holds at most one session per name, so the local
  header is a reliable source of that name's recorded prefix on that host.
- The feature that adds the continues relation already writes a pointer under
  a predecessor's prefix taken from the relation's `prefix` field, so a
  session-scoped prefix that isn't `self.prefix` already exists in that code.

#### Chosen: a per-session home-prefix resolver in `CloudBackend`, used by every session-scoped key

`CloudBackend` gains `home_prefix(id) -> Result<Home, Unlocated>`, memoized
per process in a `RefCell<HashMap>`. It reads the session's local header
(no request). With `origin.store.prefix` present and of the shape `repo_id`
produces (16 lowercase hex, checked with the existing `is_workspace_prefix`),
that prefix is the home; a value of any other shape is treated as missing.
With no local copy on this host, the only candidate is the caller's own
prefix: when the bucket holds the session there, the read goes ahead as it
does today from the session's own workspace on a second host, and otherwise
the result is `Unlocated` with `NoLocalCopy`.

`home_prefix` is resolved once per public backend method and the `Home` is
passed down to the key builders, so the builders stay infallible and a
command touching several sessions resolves each one separately. The
`SessionBackend` trait gains `home_of(id) -> Result<HomeInfo, Unlocated>`,
which the local backend answers with "own workspace" for every session, so
`refuse_if_bound_elsewhere` keeps taking the `Backend` enum and calls one
method for both backends.
Without it, the candidates are the hash of the execution anchor
(`execution_dir`, else `origin.anchor`) and the prefix the host's sync stamp
names, deduplicated, in that order. `home_prefix` itself makes one delimiter
listing per distinct candidate (at most two, which is what the PRD's request
bound allows) and keeps the candidates whose state object exists. The
`session_id` check then happens in the state pull the read makes anyway: the
pull reads the fetched header and refuses, writing nothing, when its
`session_id` differs from the local header's, so the memoized `Home` for a
fallback session is confirmed by the first pull rather than by an extra GET.
`session_id` is a random UUIDv4 minted at creation
(`generate_session_id`), so a match identifies one creation, not a name. No confirmed candidate, two confirmed
candidates, a recorded prefix whose copy is missing or carries another
`session_id`, or no local copy at all produce `Unlocated` with a reason, which
the CLI maps to `session_unlocated` (exit 2) naming the session, the reason
and both locations when there are two. `Home` carries the prefix and whether
it equals `self.prefix` (`own_workspace`).

Every session-scoped key builder takes the home prefix instead of
`self.prefix`: `state_key`, `session_prefix`, the manifest, version, template,
context-key, batch and marker keys, and the `sync.rs` helpers, which already
take the prefix as an argument. The same applies to the delete paths
(`sync_delete_session`, `delete_remote_session_checked`), so a prune run from
any directory deletes the session's own copy rather than a copy under the
caller's prefix. Before any bucket delete, and before the local copy is
removed, the delete path reads the first line of the state object at the
home prefix (one ranged GET per session, on top of today's listing) and
deletes only when its `session_id` matches the local header's; a mismatch or
a missing object leaves both copies alone and reports the session as
skipped, whichever way the home prefix was found.

Pushes keep koto#480's conditional writes, with one narrowing: koto#480
replaces a bucket copy whose header names a different `session_id` (an
earlier session of the same name). That replacement now happens only when
the home is the caller's own prefix; a push to a home found any other way
refuses with the `session conflict` error instead, so a header that names
the wrong prefix can never overwrite another workspace's live session. `self.prefix` stays for what is genuinely about the caller's
workspace: the prefix a new session records at `koto init`, a child spawn or
an import, `try_list_sessions`, and the pin scan of the caller's workspace.

With reads and writes on one copy, `bound_elsewhere` stops being a refusal
for a located session; `refuse_if_bound_elsewhere` keeps its call sites and
refuses only for `Unlocated`. A pull for a session whose `Home` isn't
`own_workspace` turns every failure (listing error, timeout, authorization
error, 5xx, a GET failing after a good listing) into an error, and
`ctx_exists` no longer folds a listing failure into `false` in that case; a
pull from the session's own workspace keeps today's warning and local copy.
The freshness skip compares the stamp's `state_key` with the home key, so a
stamp written under another prefix never skips a fetch.

A rebind rewrites the header's anchor and epoch and pushes to the home
prefix; `origin.store.prefix` doesn't change, so the copy stays put. The
rebind fence keeps working on directories (`execution_dir`,
`retired_anchors`), not on prefixes. `koto next` keeps its anchor check
before anything else and, from a subdirectory of the anchor, now pushes to
the home prefix (koto#465). Every push path uses the home prefix whatever the
caller's directory (koto#466).

#### Alternatives Considered

**Re-point the backend per command.** Build the backend with the named
session's prefix instead of the caller's. Rejected because commands touch
several sessions at once (a reader's anchor, a held-by check, hierarchy
members, the relation's walk), and a single field can only be right for one
of them.

**Reuse the `--workspace` reader for cross-directory reads.** It already
reads any prefix through a scratch store and never writes. Rejected as the
general path because it fetches the whole state file on every call, ignores
the host's copy, can't serve writes, and would give reads and writes two
different copies, the split this decision exists to remove. It stays as the
way to read a session this host has no copy of.

**Keep refusing from another directory (today's behaviour).** Rejected
because a successor can't read its predecessor from its own directory, and
every lane keeps running every koto call from one directory.

**Resolve by name across the bucket.** List every prefix for the name when
the host has no copy. Rejected because a name isn't unique across
workspaces and hosts sharing a bucket, and a scan picks by recency at best;
it would also need a bucket-wide listing on every miss.

### Decision 4: What keeps a session in place of the name rule

Resolved inline.

Rule f keeps every `coordinate-` session forever, live lineage or not. The
continues relation's feature adds rule r, `Judgment::keeps()` and the
`relation_keepers` report, first on the local backend and then, with its
bucket walk, on the cloud backend; it leaves rule f alone. The questions are
how prune reads the relation, when rule f can go, and what an operator does
about existing coordinator sessions.

Key assumptions:
- The relation's feature ships its local and cloud halves in one release, as
  its plan says.
- Coordinators on the affected host run on the cloud backend.

#### Chosen: delete rule f once the relation works on the cloud backend, and read the relation only through `keeps()`

Prune reads the relation through the relation feature's own interfaces and
adds nothing: rule r in `first_rule` and `keeps()` decide, and the entry's
`kept_by_relation`, `relation_keepers` and the text line
`kept: continued by <keeper> (<standing>)` report it. This feature's change
is to delete the `coordinate-` branch from `later_rule`, remove rule f from
the module documentation, `docs/guides/cli-usage.md`, the session retention
guide and the koto-user command reference, and turn the coordinator tests in
`tests/session_prune_test.rs` around: a finished `coordinate-x` with no keeper
goes, a pinned one and one with a running hierarchy member stay naming their
keeper, and one whose successor holds a continues relation stays naming the
successor on both backends.

The change merges only after the relation's bucket walk is on main, which
the plan records as a dependency. The retention guide and the release notes
gain a paragraph: finished `coordinate-` sessions are no longer kept by
name; `koto session prune` without `--apply` previews which a prune would
release; `koto session pin` or a `koto session link --continues` from the
successor keeps one before upgrading.

#### Alternatives Considered

**Remove rule f as soon as the relation lands on the local backend.**
Rejected because the local-only relation refuses on the cloud backend, so
every coordinator on a cloud host would be released with nothing keeping it.

**Keep rule f beside the relation as a fallback.** Rejected because it keeps
the defect the feature removes: a session kept for its name, forever, and a
removal nobody can explain from the session's facts.

**Backfill relations for existing coordinators at upgrade.** Have koto write
`continues` links between `coordinate-` sessions it guesses form a chain.
Rejected because the guess is by name and creation time, exactly the
inference the relation replaces; the preview plus pin or link leaves the
call with the operator who knows the lineage.

### Decision 5: Reads that write nothing, and refusals that remove nothing

Resolved inline.

Two smaller questions share one theme, a read or a refusal that changes
somebody else's session. First, how a read keeps koto#204's guarantee (a
read after a lost assigned-key write returns the assigned value) without
`reconcile` writing into the session, and where a read whose reader anchor
holds the session is logged. Second, what the `origin_mismatch` messages say
instead of `koto session cleanup`.

Key assumptions:
- The session's log is the source of truth for assigned values, as
  `reconcile` itself treats it.
- `tests/doc_names.rs` fails any message that names a verb that doesn't
  exist, so new remedy text can only name real verbs.

#### Chosen: serve from the log, log in the reader's anchor, and name only non-destructive remedies

`restore_assigned` stops calling `reconcile` on the read path. Instead
`context get` and `exists` compute `outstanding_assignments(events)` for the
key, and when the store's value differs from the assigned one they answer
with the assigned value without storing it. `context list` doesn't call the
restore today; it gains the same log-served answer for the keys it lists, a
new behaviour that keeps the three reads consistent. The repair needs no new
home: `koto next` already runs `reconcile` before its advancement loop
(`src/cli/mod.rs`), so deleting the call in `restore_assigned` leaves the
session's own next tick as the one place a lost value is restored. The
`--workspace` reader already skips the restore, so all reads agree. The unit
tests in `src/cli/context.rs` that pin the old write-back flip to assert
that the store is untouched and the answer is the assigned value.

`read_log_target` keeps `Session` only for a read inside the session's own
tick (`enclosing_tick()` names it). Outside it, a reader whose
`KOTO_ANCHOR` names an existing anchor other than the session gets
`Anchor(<anchor>)`, as today for anchors that don't hold the session; a
reader whose anchor is the session itself, or holds it, or names no existing
anchor, goes to the anchorless spool. A read outside the tick therefore never
appends to the session it reads. The held-by probe that decides this is
itself a read of the session and goes through the home-prefix resolver like
any other.

The `origin_mismatch` message for a session from another worktree or store
becomes: session `<name>` belongs to another worktree or session store
(started in `<recorded>`, this invocation runs in `<requested>`); use another
session name, or run the command from `<recorded anchor>` where the session
belongs. The one for a session with no origin record becomes: session
`<name>` has no origin record, so it can't be confirmed as this caller's;
finish it with the koto version that started it, or use another session
name. `execution_anchor_mismatch` keeps its text. A test in
`tests/init_entry_flags.rs` (flipping the assertion that pins the cleanup
text) and one in `tests/execution_anchor_test.rs` assert that none of
`session cleanup`, `--cleanup`, `session prune` and `workspace prune` appear,
and that the named remedies do. `docs/guides/cli-usage.md` and
`docs/guides/custom-skill-authoring.md` change to the same remedies;
`docs/reference/error-codes.md` and the koto-user references already name
none and gain the remedies.

#### Alternatives Considered

**Restore only inside the session's own tick, and let reads return what the
store holds.** Rejected because a finished session never ticks again, so a
lost write would read as missing forever, which breaks koto#204's guarantee
exactly where archive readers need it.

**Read a lost value through a scratch copy, as `--workspace` does.** Rejected
because it costs a full state fetch per read to avoid a write the log
already makes unnecessary.

**Suggest `koto session import` for `origin_mismatch`.** It continues another
workspace's session from here. Rejected as the default remedy because it
marks the source migrated, which changes the other workspace's session, and
it needs the cloud backend.

**Also reword the "already exists" refusals.** They name
`koto session cleanup` for a name in the caller's own store. Left out
because the PRD's wording requirement names the two mismatch codes, and for
the caller's own finished session cleanup is the intended remedy; the
continues relation offers a better one later.

## Decision Outcome

The feature ends up as five changes that meet in two places: the cloud
backend and the prune judgment.

The guard stays as koto#470 shipped it, and the suite gains the cases it
lacks. Isolation moves into `load_config`: with `KOTO_HOME` set, only the
root's own `config.toml` can select the cloud backend, anything else asking
for it is ignored with one warning naming the setting, and
`koto session dir --json` tells a harness the root, the backend, where the
backend came from and the bucket, without building a backend. Because the
rule lives where every koto reads its config, a koto that a tick starts is
under the same rule and the same root.

The cloud backend learns that a session's bucket copy belongs to the session,
not to the caller. `home_prefix(id)` reads the local header's
`origin.store.prefix`, or for an older session confirms its anchor's prefix
or the stamp's prefix by listing and by `session_id`, and refuses as
`session_unlocated` when it can't. Every session-scoped key, push, pull and
delete uses that prefix. A read from a foreign directory treats any bucket
failure as an error rather than serving the local copy, and a stamp written
under another prefix never short-circuits a fetch, so a warm cache can't
return a stale value. `koto next` keeps its anchor check and stops writing
stray copies from subdirectories, a rebind leaves the copy where it is, and
prune deletes the right copy wherever it runs. Reads stop writing: a lost
assigned value is answered from the log and repaired by the session's next
tick, and a read outside the session's own tick is logged in the reader's
anchor or spool. The `origin_mismatch` messages name another session name
and the directory where the session belongs, or finishing it with its own
koto version, and tests pin both codes.

The name rule goes last. Once the continues relation works on the cloud
backend, rule f is deleted and prune keeps a finished session because a
later session continues it, naming that successor through the relation's own
report fields. Release notes and the retention guide tell an operator to
preview, then pin or link anything a lineage still needs before upgrading.

### Rationale

The pieces reinforce each other. The resolver is what makes a read from the
successor's directory possible, and the read-purity change is what makes
that read safe to run against a finished predecessor. Fail-closed isolation
and the guard together mean a test that forgets its setup can neither reach
the bucket under a scratch root nor empty a real store with a test retention.
And the relation can replace the name rule only because reads and removals
already agree on which copy is the session's. The main trade-off accepted is
that the resolver changes every session-scoped bucket key; it's a larger
change than refusing, but it removes three defects (stale reads, stray
copies, wrong-copy deletes) with one rule.

## Solution Architecture

### Overview

Five changes, four of them independent of the continues relation:

1. Guard tests in `tests/session_prune_test.rs` (no source change).
2. Isolation: a `KOTO_HOME` backend rule in `src/config/resolve.rs` and a JSON
   form of the no-name `koto session dir`.
3. Home-prefix resolution in `src/session/cloud.rs` and `src/session/sync.rs`,
   with the CLI guard in `src/cli/mod.rs` narrowed to unlocated sessions.
4. Read purity in `src/cli/context.rs`.
5. Refusal wording in `src/cli/init_entry.rs`, with tests and four documents.

The last change, deleting rule f in `src/cli/session_prune.rs`, waits for
the relation's cloud support.

### Components

```
load_config ──(KOTO_HOME rule)──> KotoConfig ──> build_backend ──> CloudBackend
     │                                                               │
     └── koto session dir --json (no backend)                         ├─ home_prefix(id) ─> Home | Unlocated
                                                                      │     ├─ local header: origin.store.prefix
                                                                      │     └─ fallback: anchor hash, stamp prefix
                                                                      │           (listing + session_id confirm)
                                                                      ├─ key builders(home) ─ state, manifest,
                                                                      │     version, ctx, batch, marker, delete
                                                                      └─ self.prefix ─ init/spawn/import record,
                                                                            workspace listing, pin scan
CLI read (context get/exists/list, status)
     ├─ refuse_if_bound_elsewhere ─> only Unlocated -> session_unlocated
     ├─ outstanding_assignments(log) -> answer, no store write
     └─ read_log_target -> Session only inside own tick; else anchor or spool

koto next, before advancing -> reconcile(store, log)   (already there; the read-path call goes)

session_prune later_rule: rule f deleted (after the relation's cloud support)
```

### Key Interfaces

```rust
// src/session/cloud.rs
pub struct Home { pub prefix: String, pub own_workspace: bool, pub via: HomeSource }
pub enum HomeSource { Recorded, Anchor, Stamp }
pub struct Unlocated { pub session: String, pub reason: UnlocatedReason }
pub enum UnlocatedReason {
    NoLocalCopy,
    NoConfirmedLocation { tried: Vec<String> },
    Ambiguous { prefixes: [String; 2] },
    RecordedCopyMismatch { prefix: String },   // missing, or another session_id
}
impl CloudBackend {
    pub fn home_prefix(&self, id: &str) -> Result<Home, Unlocated>;
}
```

```text
koto session dir --json
{"root":"/abs/root","backend":"cloud","backend_source":"user-config","bucket":"name"}

error code session_unlocated (exit 2), flat error JSON like session_bound_elsewhere:
  {"error":"session_unlocated: ...","code":"session_unlocated","command":"context get",
   "session":"S","reason":"ambiguous","locations":["<p1>","<p2>"]}
```

`backend_source` maps `session_env::Origin`: `User` to `user-config`,
`Project` to `project-config`, `Env` to `env`, `Default` to `default`.

### Data Flow

A read of S from directory B: `build_backend` (config already decided by the
`KOTO_HOME` rule) builds `CloudBackend` with `self.prefix = hash(B)`;
`refuse_if_bound_elsewhere` calls `home_prefix(S)`, which reads the local
header and returns `Home { prefix: P_S, own_workspace: false }`; the state
pull lists and fetches `P_S/S/...`, comparing the stamp's key with
`P_S/S/...` before any freshness skip and failing hard on any bucket error;
the key pull does the same; `context get` computes the assigned value from
the fetched log if the store disagrees, prints it, and logs the read in the
reader's anchor or spool. Nothing is pushed for S.

A write to S from B, or a tick from a subdirectory of S's anchor: the same
`Home`, then the conditional push from koto#480 under `P_S`. A prune from
any directory: the judgment as today, then `delete_remote_session_checked`
under each member's `P_S`.

## Implementation Approach

### Phase 1: Guard tests

The cases Decision 1 lists, in `tests/session_prune_test.rs`, against the
in-process stand-in. No source change. Independent of everything else.

### Phase 2: Isolation under `KOTO_HOME`

The `load_config` rule and its warning, `koto session dir --json`, the CLI
guide's description of both, and the extended `tests/koto_home_test.rs`
with the sentinel home, a gate that runs `koto session dir --json` inside a
tick, and the cleanup step. Independent.

### Phase 3: Refusal wording

The two `origin_mismatch` messages, the flipped and new tests, and the four
documents. Independent.

### Phase 4: Reads write nothing

The read path's log-served answer, `reconcile` at the tick start,
`read_log_target`'s narrower `Session` case, and byte-identity tests for
running and finished sessions with and without `KOTO_ANCHOR`, plus the
lost-write test. Independent of Phase 5, though Phase 5's cross-directory
byte-identity test runs after both land.

### Phase 5: Home-prefix resolution from the record

`home_prefix` for sessions with `origin.store.prefix` and for the no-local-copy
case, the trait method, the key builders, the push narrowing, the delete
paths with their identity check, the narrowed guard and `session_unlocated`,
the strict foreign pull and the stamp comparison, with tests for the PRD's
reads-by-name criteria that involve recorded sessions and the request-count
comparison. This closes koto#465 and koto#466. The cloud sync guide and the
koto-user error tables document `session_unlocated` and the lifted refusal.
Largest phase; independent of the relation. It touches files the relation's
bucket walk also changes, and the walk computes another session's prefix
itself, so landing this phase first lets the walk call `home_prefix` instead
of rebuilding prefix handling that would have to be unified later.

### Phase 6: The fallback for sessions with no recorded prefix

The anchor-hash and stamp candidates, their listing, the confirmation in the
pull, and the ambiguous and mismatch refusals, with their tests. It serves
only sessions created before koto recorded the prefix, so it lands after
Phase 5 as its own change and can be dropped if those sessions are gone by
then; until it lands, such a session read from a foreign directory refuses as
`session_unlocated`.

### Phase 7: Remove the name rule

Delete rule f, update the tests, the guides, the skill reference and the
release notes paragraph. It merges only once a test on the cloud backend
shows a running successor holding a continues relation keeps its finished
predecessor through `koto session prune --apply --all`; that test is part of
this phase and fails until the relation's bucket walk is on main.

## Security Considerations

**Isolation is a safety property, and it now fails closed.** Under
`KOTO_HOME`, a cloud backend can only be selected by a file inside the root,
so inherited credentials can't send a scratch run's writes or deletes to a
real bucket. Credentials themselves keep their existing sources and checks
(`session_env::check`), and `koto session dir --json` prints the bucket name
only: never the endpoint's userinfo, the access key or the secret.

**The resolver trusts the local header, then the bucket.** A session's home
prefix comes from its own header on this host, which only koto writes. The
fallback for older sessions never acts on an unconfirmed location: it needs
both the object and a matching `session_id`, and two matches refuse. That
keeps a stray or planted copy under another prefix from being read or
written as the session. A writer with bucket access could still plant a copy
with a matching `session_id` under a candidate prefix of an older session;
that is the same trust boundary the bucket already has, since such a writer
can rewrite the session's own copy directly.

**Deletes follow the resolver, and check identity first.** Prune's bucket
deletes move from the caller's prefix to each session's home prefix. That
makes a prune from the wrong directory delete the session's real copy where it
used to delete nothing useful, which is the point, and it's why the guard's
refusal and the relation's keep both run before any delete. Because a home
prefix can come from a local header someone could edit, every delete first
reads the header at the home prefix and proceeds only when its `session_id`
matches the local one, and a recorded prefix of the wrong shape counts as
missing. A tampered header can then at worst make a read refuse; it can't
point a delete at another session's copy.

**Identity means one creation.** The delete check, the push narrowing and the
fallback confirmation all compare `session_id`, a random UUIDv4 minted when
the session is created, never derived from its name. A push to a home found
any way other than the caller's own prefix refuses rather than replacing a
copy with another `session_id`, so a header that names the wrong prefix can't
overwrite another workspace's live session either.

**Writes from other directories are now effective.** Before this change a
write from the wrong directory forked a harmless copy; now it lands on the
session's own copy. That's the requirement (a successor writes into the
lineage it continues), and the protections that apply to the session's own
directory apply here too: koto#480's conditional push, the rebind fence on
retired anchors, and `koto next`'s anchor check for ticks.

**A prune from any directory deletes the right copy.** Prune already walks the
whole local store whatever the directory; before this change its bucket
deletes landed under the caller's prefix and left the session's real copy
behind. Now they reach the real copy, behind the same guard, the same
judgment and, for a plan, the same re-judgment at apply that refuses a plan
the store no longer matches (`prune_plan_stale`), plus the identity check
above.

**The cloud opt-in under `KOTO_HOME` doesn't widen where credentials go.**
Once the root's config selects the cloud backend, the endpoint and bucket
still pass `session_env::check`, which already refuses an endpoint or bucket
from a project file when koto's credential pair supplies the credentials, so
inherited credentials can't be sent to a bucket or endpoint a checked-out
repository names. The documented opt-in is the single line
`backend = "cloud"`; credentials stay in the environment, and the CLI guide
says not to put them in the root's file.

**Reads stop writing into other sessions.** Removing the read-path
`reconcile` and the held-by own-read append removes a write a reader could
trigger into a session it doesn't own, and with it a push of that session
under the reader's credentials.

**Refusals stop advising deletion.** The new text names only another name,
the owning directory, or the original koto version. No refusal points an
agent at a removal of another workspace's session.

No new input reaches a shell, and no new file holds a secret.

## Consequences

### Positive

- A successor reads its predecessor by name from its own directory and gets
  the bucket's current value, and the lanes can drop the one-directory rule.
- Ticks from a subdirectory and writes from other directories stop leaving
  stray copies, and a prune from any directory deletes the right copy.
- A scratch `KOTO_HOME` can't reach a real bucket unless its own config asks.
- Finished sessions can be read through koto without changing them.
- No refusal tells an agent to delete another workspace's session.
- Prune keeps what a lineage needs and says which successor keeps it.

### Negative

- The resolver changes every session-scoped bucket key, which is a wide
  change in `src/session/cloud.rs` and overlaps files the relation's bucket
  walk also changes.
- A read from a foreign directory fails when the bucket is unreachable, where
  a read from the session's own directory serves the local copy.
- Copies earlier per-directory pushes left under other prefixes stay in the
  bucket, unread and unreported.
- An older session the fallback can't confirm can only be read from its own
  directory, and a host with no local copy can't read by name.
- Reads made by a session's driver outside its own tick no longer appear in
  that session's log, only in the driver's anchor or the spool.
- Under `KOTO_HOME`, a caller who wants the cloud backend must write
  `backend = "cloud"` into the root's config.
- After the name rule goes, a finished coordinator whose successor hasn't
  written the relation is removed once retention passes unless an operator
  pins or links it.

### Mitigations

- The resolver phase lands as one change with tests per key family, and the
  plan orders it relative to the relation's bucket walk so one rebases on the
  other once.
- The foreign-read failure is the requirement: a stale answer is worse than a
  refusal, and the message says to retry or read from the session's own
  directory.
- Stray copies are harmless once nothing reads or writes them; listing them
  is separate work.
- `session_unlocated` names the reason, and `--workspace` reads a session this
  host has no copy of.
- A driver's out-of-tick reads are still recorded, in its anchor or in the
  read spool, which the run journal's visit fold already reads; the plan's
  read-purity issue checks the journal's read counts against the new
  locations.
- The `KOTO_HOME` warning names the ignored setting, and the CLI guide shows
  the one-line opt-in.
- The release notes and retention guide name the preview, `koto session pin`
  and `koto session link --continues` before anyone upgrades.
