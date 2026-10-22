---
schema: design/v1
status: Planned
problem: |
  koto removes finished sessions at six points without asking whether a
  related session is alive, knows nothing about agent sessions, and keeps no
  relation that survives an import, so the sessions above and below a
  finished worker lose its log and keys while they still need them.
decision: |
  Each session keeps koto-owned links about itself under a reserved `koto/`
  prefix: who dispatched it, and which agent session's anchor holds it. The
  koto-skills session-start hook opens one anchor per agent session through
  a new `koto session anchor` verb group. niwa's on-reap command closes an
  observed anchor in a close session of its own, and koto reads an
  unobserved one closed after an idle period. A new engine in
  `src/session/hierarchy.rs` scans the store's edges once per command,
  judges only the component asked about, and answers every removal point
  and `koto session hierarchy` with one verdict. Reads from another agent
  session go into the reader's anchor. Import settles the moved session's
  own held-by links. niwa gains a pass-through list, a reap key and a
  bounded on-reap command, and shirabe links at each dispatch path that
  uses no request.
rationale: |
  Links in the session's own keys are the only relation that survives
  import and keeps one writer per session; anchors bring agent sessions into
  koto without koto reading harness state; and a two-level engine is the
  only option that meets the one-second query bound while letting prune and
  the query print the same reason. Everything needed lives in the local
  store, so koto alone delivers the rule and niwa only closes anchors
  earlier.
decision_provenance: inline-resolved
user_visible_surface: true
upstream: docs/prds/PRD-session-retention.md
---

# DESIGN: Session retention across the dispatch hierarchy

## Status

Planned

## Context and Problem Statement

The PRD asks koto to keep a session readable for at least as long as any
session in its dispatch hierarchy is alive, with an idle, resumable agent
session counting as alive. Technically that is four problems koto doesn't
have the pieces for today.

**No relation koto can follow survives what happens to sessions.** koto's
parent-child lineage lives in the session header (`parent_workflow`) and
import refuses a child and carries none. Request legs live in
`~/.koto/requests/`, a host-local store import drops. The only relations
that are context keys, which import copies verbatim, are shirabe's
`chain/parent` and `/coordinate`'s `legs/*/*.child`, and koto parses only the
first, single-valued, in one place (`session_prune.rs`). The design needs a
koto-owned record of who dispatched whom and which agent session holds a
session, written by each session about itself (the PRD's one-writer rule,
R1), resolved by name (R5), and carried by import (R22).

**koto has no notion of an agent session.** Its only liveness signal is the
log: a session is alive until terminal or cancelled. An agent session that
idles after its koto run, or a person's session that runs no koto workflow,
is invisible. The owner's ruling gives each agent session an anchor: a koto
session opened by koto's Claude Code plugin at session start, open while the
agent session exists. That needs the plugin's hooks to load at all (today
they don't: `plugins/koto-skills/hooks.json` sits at the plugin root in a
flat shape Claude Code doesn't read), an anchor identity that survives
resume, compaction and `/clear` (R6), a way to learn whether niwa will
observe the session's deletion (R9), a close that never writes into the
anchor (R10), an idle close koto computes when it reads (R11), and an
observed anchor that closes only through that observed close (R12).

**Six removal points decide alone.** The terminal tick (`terminal_retention`
and step 6 of `finish_terminal_tick` in `src/cli/mod.rs`), the parent sweep
(`sweep_children` in `src/cli/workspace.rs`), the same-name replace
(`init --replace-terminal`, `src/cli/init_entry.rs`), `koto workspace prune
--root`, `koto session prune` (`first_rule` and `remove_one` in
`src/cli/session_prune.rs`) and the explicit `koto session cleanup` and
`koto cancel --cleanup`. Only prune looks at another session, one hop away.
The PRD wants one rule over the whole connected hierarchy (R14) at every
removal by rule or subtree, the terminal tick keeping every session with an
edge (R15), a re-run moving a held predecessor aside (R17), explicit removal
reporting what it broke (R18), and a reason wherever koto keeps something
(R19).

**Reads write.** `koto context get` and `koto context exists` append a
`context_read` event to the read session's log, from the reader's process.
The PRD needs a read from another agent session to leave the read session
byte-identical while the read is still recorded, in the reader's own anchor
(R21), because koto's run journal consumes those events.

Outside koto, niwa needs three generic pieces that name no koto (R25 to
R29): passing declared variables through to a worker's settings environment,
marking sessions it will reap, and running a declared on-reap command with a
bound. shirabe needs a link call on each dispatch path that uses no request
and to drop its own reclaimable rule (R30 to R32).

## Decision Drivers

- **One writer per session** (R1). Every new write lands in the session the
  command opens, ticks, links or imports, or in the running agent session's
  own anchor. Nothing writes into a dispatcher, a peer or another agent
  session's anchor. Under the cloud backend a second writer would race the
  session's own sync, and a migrated source refuses writes.
- **Local first, no network** (R33). Everything except import works with the
  local backend and no cloud configuration; nothing turns cloud on or edits
  `~/.koto/config.toml`.
- **Independence.** koto never requires niwa, and niwa's code never names
  koto (R29). This design adds no `CLAUDE_*` read to the koto binary (it
  already reads `CLAUDE_CODE_SESSION_ID` for /workflows and the run
  journal); the plugin's shell hooks may read harness variables.
- **Fail toward keeping** (R35). An unreadable session, link or close record
  keeps the hierarchy it touches; the only cost of a wrong link is
  over-retention while something is alive.
- **Bounded cost** (R34). The rule for one session answers in under a second,
  and a prune preview over 2,000 sessions in under ten, on a GitHub-hosted
  runner; the terminal tick pays at most one store scan.
- **No regression for lone sessions** (R15). A session with no link and no
  lineage keeps today's terminal removal.
- **Compatibility of the event log.** New event kinds and fields must pass
  koto's event-feed validation and leave existing readers (the run journal,
  the dashboard, `koto status`) working.
- **Small, reviewable increments.** Each piece lands in its own pull request
  with tests, in an order where every merged state is sound.
## Considered Options

### Decision 1: How links, anchor state and close records live in sessions

Resolved inline.

Links have to survive `koto session import`, which copies context keys
verbatim and nothing else a session holds beside its log; they have to be
written only by the session they belong to (R1); and the retention engine
has to enumerate them cheaply across the whole store (Decision 3).

Key assumptions:

- No caller writes keys under `koto/` today. shirabe's session convention
  reserves its own top-level areas (working state, research, hand-offs, the
  chain keys, session bookkeeping and the coordinator's record and legs),
  none named `koto`; koto itself writes only `workflows/publish-location`
  and `batch_final_view`.

#### Chosen: koto-owned keys under a reserved `koto/` prefix, one key per peer

`koto context add` and `koto context remove` refuse any key under `koto/`
(exit 2, naming the prefix); koto writes those keys itself with writer
`koto`, the way `batch_final_view` is written today. Links are one key per
peer, so a session holds any number and writing an existing link changes
nothing:

| Key | Held by | Names |
|---|---|---|
| `koto/links/dispatched-by/<peer>` | the dispatched session, or an anchor | a session that dispatched it |
| `koto/links/held-by/<anchor>` | any session an agent session opened or ticked | that agent session's anchor |
| `koto/links/closes/<anchor>` | an anchor's close session | the anchor it closes |

Each value is one JSON object: `{"v":1,"session":"<peer>",
"session_id":"<peer's id or null>","workspace":"<path or null>",
"at":"<rfc3339>"}`, plus `"was":"<old name>"` on a held-by link import
rewrote. The last key component is the peer's name; a peer name that fails
the context-key component grammar (only `~`, used by internal epoch
branches, can) is refused and no link is written.

The anchor's observed marker is `koto/anchor/observed` in the anchor:
`{"v":1,"key":"<niwa reap key>","at":...}`. An observed close lives in a
close session of its own, named `<anchor>-closed`, opened already finished
(koto stages it under a temporary name and renames it into place, so no
reader sees a partial one) and holding `koto/links/closes/<anchor>` and
`koto/anchor-close`:
`{"v":1,"anchor":"<name>","anchor_session_id":"<id>","kind":"observed",
"key":"<niwa reap key>","at":"<rfc3339>"}`. A close record whose
`anchor_session_id` differs from the anchor's current header id doesn't
apply, so a record left for an earlier anchor of the same name can't close
a reopened one. Import writes `koto/imported-from` into the copy it creates
(Decision 6).

#### Alternatives Considered

- **A header field.** Import rewrites the header, the header is line 1 of
  the log, and a growing set would rewrite it on every link.
- **One JSON key holding a list.** Every link write becomes a
  read-modify-write with a lost update between a tick and a link call, and
  import's rewrite has to parse and re-serialise it.
- **Reusing `chain/parent` or an open `refs/` area.** shirabe owns
  `chain/parent` and erases it in `adopt`; an unreserved area could be
  forged or deleted by any `koto context add`.
- **A shared close-record session.** Several close commands would write one
  session, and it would join unrelated hierarchies.

### Decision 2: The anchor protocol

Delegated to /decision.

Each Claude Code agent session needs one anchor that its hook opens, that
stays open while the agent session exists, and that every shell command in
the session can name. The hook runs on startup, resume, compact and
`/clear`, must finish well under a second, must always exit 0, and must be
silent without koto. Research into the harness (a reference source copy and
the live 2.1.308 build) found:

- every hook gets its own env file,
  `~/.claude/session-env/<session id>/sessionstart-hook-<index>.sh`;
- settings hooks sort before plugin hooks, so a plugin's `unset` comes after
  niwa's `export`;
- the files are pasted into every Bash command as one link of an `&&`
  chain, so every line must exit 0;
- `/clear` mints a new session id, and the payload carries no link to the
  old one;
- resume and compact keep the id, but a resume hook's env file can sit in
  the directory of the session being left;
- the hook's own process environment never holds the session's own
  exports, so an anchor variable it sees was set by whoever launched it.

Key assumptions (each with a fallback; the hand checks are listed under
Implementation Approach):

- Plugin hooks get `CLAUDE_ENV_FILE` in the per-index layout above. If not,
  the hook also emits `additionalContext` carrying the single export line
  for `KOTO_ANCHOR` and nothing else, as the /workflows hook does today, and
  sessions it can't reach fall back to the koto-level reading.
- Resume keeps the harness session id. Checked by hand; if a resumed
  session gets a new id, the fallback under Identity finds the predecessor.
- `/clear` gives a new `session_id` and no previous id. If wrong, the same
  anchor name is simply reused.
- The hook can compute a stable process token `<host>:<pid>:<epoch-start>`
  for the claude process (walking up from `$PPID` past shells; start time
  from `/proc/<pid>/stat` or `LC_ALL=C ps -o lstart=`). If not, `/clear`
  opens an unlinked anchor, a documented limitation.
- A watcher the hook detaches (`setsid` or `nohup`, all descriptors
  redirected) outlives the hook. If not, the carrier becomes a second
  SessionStart entry with `"async": true`.
- niwa's from-hook writes `export NIWA_REAP_KEY='<value>'` as a plain,
  single-quoted line in its own env file (Decision 7 pins it); the watcher
  strips the quotes.
- A `niwa dispatch` worker has the `--settings` environment in its process
  environment before plugin hooks run.

#### Chosen: session-id anchors behind one koto-owned open verb, with a koto-side `/clear` bridge

**Identity.** The anchor for harness session `S` is `anchor-<S>`. Resume
and compaction recompute the same name with no stored state. An idle-closed
anchor reads open again once the new start event lands. A removed anchor
reopens under the same name. An observed-closed anchor stays closed and koto
opens the next free successor, `anchor-<S>-s2`, `-s3` and so on (not a dotted
suffix, because koto reads a dotted name as batch-child lineage); "current"
is the highest suffix contiguous from `-s2` whose dispatched-by link names
the one before it, and the
successor holds a dispatched-by link to the closed one. Whether the anchor
is observed-closed is answered by one shared check, introduced with the
anchors and used by the engine too: `<anchor>-closed` exists and its record
passes the validation rules (kind, name, anchor session id, key and
manifest). `open` refuses a harness id ending in `-closed`. A migrated
anchor met at a start is treated like an observed-closed one: a successor
opens, holding a dispatched-by link to it. A hand-cleaned and reopened
`anchor-<S>` gets a fresh close session at its next close: the close command
replaces a close session whose recorded anchor id doesn't match.

**Resume under a new id.** If a `resume` start finds no anchor for its
session id, `open` looks for a predecessor through hints only: an anchor
whose `anchor_started` recorded the same transcript path digest, or whose
harness id equals the stem of the payload's transcript path. On a match it
opens a successor holding a dispatched-by link to it, with `predecessor`
recorded in its `anchor_started` (every successor records it, since the
close walk follows it); with no match it opens a fresh anchor. The hint join
only adds a link; it never replaces the name match. A resume successor never
copies its predecessor's observed marker, because a hint can be wrong and
koto can't tell from the stored token digest whether the predecessor's
process is gone. It takes a marker only from its own environment or its own
watcher, as a startup does, so the close walk reaches it only when niwa's
key for this session matches; a false hint gets a link and nothing more,
and one session's reap can't close another live session's anchor.

**`/clear`.** The new id gets a new `anchor-<S'>`, linked by dispatched-by
to its predecessor; two conversations never share an anchor, which keeps
R1 when someone resumes the pre-clear conversation while the post-clear one
runs. koto finds the predecessor itself: it lists `anchor-*` sessions whose
log changed since the process start in the token, reads them newest first,
and takes the first whose `anchor_started` event carries the same token. On
a match it writes the link and copies the predecessor's observed marker. The
scan runs only on `/clear`, and a test bounds it under 250 ms on a store of
2,000 sessions plus 400 anchors.

**Kind.** The anchor's one-state template is embedded in koto with
`include_str!` and compiled into the session directory the way
`koto init --from-stdin` does, so it never depends on the template cache or
the plugin's path. Its header carries `kind: "anchor"`, and koto recognizes
an anchor by that field alone; the close session's header carries
`kind: "anchor-close"`. `koto next` on either is refused, and the
/workflows materializer skips them.

**Verbs**, in a new `koto session anchor` group:

- `open --harness-session <id> --source <startup|resume|compact|clear|other>
  [--process <token>] [--dispatched-by <name>] [--observed-key <value>]
  [--workspace <dir>]`: open or continue under the session lock; it refuses
  when the name already holds a session that isn't an anchor. It ignores
  `KOTO_ANCHOR` in its own environment, so an anchor never holds a held-by
  link. Every call appends one `anchor_started` event (`source`,
  `harness_session`, `process` (a SHA-256 prefix of the token, so the
  host name and pid aren't stored in clear text, since the event may sync), `workspace`, and
  `predecessor`, `reason`,
  `dispatched_by` when present). It writes the dispatched-by link only when
  it creates an anchor (new, reopened, successor or post-clear); on a
  continue the inherited value goes into the event only. It writes the
  observed marker from `--observed-key` when the anchor has none. It prints
  `anchor=<name>`, `opened=new|continued|reopened|successor`,
  `observed=yes|no`, and after a clear `previous=<name>` and
  `inherit_until=<rfc3339>`; on failure it prints no `anchor=` line.
- `observe --anchor <name> --key=<value>` and `observe --anchor <name>
  --inherit`: write only into the named anchor, which must have
  `kind: "anchor"` (the watcher names its
  own anchor explicitly and never trusts an inherited `KOTO_ANCHOR`, which is
  the dispatcher's); idempotent. `--inherit` copies the marker from the
  predecessor its own `anchor_started` recorded.
- `close --key=<value> --harness-session=<id>`: records as closed, at the
  current time, in each one's close session, the anchor whose
  `anchor_started` recorded that harness id and the successors reached from
  it through the predecessor chain (anchors whose `anchor_started` names a
  `predecessor` after `/clear`, compaction or a resume under a new id), each
  only when its observed marker matches the key. It never writes into an
  anchor (R10). A child anchor carrying the same key, which records a
  dispatcher rather than a predecessor, is never matched, and neither is a
  successor opened because its predecessor already read closed. An anchor
  that already has a valid close session is an idempotent success, so a
  repeated or concurrent close changes nothing and exits 0. The close
  session is staged under a reserved `.koto-staging-` prefix that listing,
  prune and the manifest ignore and that a later close sweeps once an entry
  is older than 10 minutes (never a fresh one, which may be a close in
  progress; staging times later than now are clamped to now, so a
  future-dated entry is still swept in time), then
  renamed into place; a staged entry missing at rename time is a failed
  run that niwa repeats, never a corrupt state; a rename that finds a valid close session already
  there counts as success, and replacing a stale one is a single rename
  over it. There is no caller-supplied time. `observe` and `close` refuse a key or
  harness id that doesn't match `^[A-Za-z0-9_-]{1,128}$`, the same pattern the
  hook and niwa use.

**Hook.** `plugins/koto-skills/hooks/hooks.json` replaces the root file, in
the matcher-group shape; one SessionStart group runs the new
`hooks/session-start.sh` next to the existing /workflows script, and the Stop
hook is kept. `session-start.sh`:

1. `trap 'exit 0' EXIT`; `command -v koto >/dev/null 2>&1 || exit 0`; read
   `session_id` and `source` from the payload with `jq` only (no grep
   fallback for any value that reaches the env file), and give up silently
   unless `session_id` matches `^[A-Za-z0-9_-]{1,128}$`.
2. Compute the process token.
3. Keep the inherited values: the dispatcher is
   `${KOTO_DISPATCHER_ANCHOR:-${KOTO_ANCHOR:-}}` (a niwa worker gets the
   first through niwa's pass-through; a `claude -p` child inherits both from
   its parent's shell), and `key=${NIWA_REAP_KEY:-}`, taken only when no
   `KOTO_ANCHOR` was inherited: a process launched from an agent session's
   shell inherits that session's `KOTO_ANCHOR`, and a niwa worker has it
   removed, so a child never records its parent's reap key even if the
   key reached it. koto itself never reads `KOTO_DISPATCHER_ANCHOR`.
4. `out=$(env -u KOTO_ANCHOR -u KOTO_DISPATCHER_ANCHOR -u
   CLAUDE_CODE_SESSION_ID koto session anchor open ... 2>/dev/null) || true`.
   Steps 1 to 5 are the hook's synchronous part; a test bounds them well
   under one second (95th percentile under 300 ms on the CI runner).
5. When `CLAUDE_ENV_FILE` is a regular file (not a symlink) whose directory
   is named after the payload's session id, replace the block between
   `# >>> koto-skills` and `# <<< koto-skills` through a temporary file in the
   same directory and `mv`: `export KOTO_ANCHOR='<own>'` and
   `export KOTO_DISPATCHER_ANCHOR='<own>'` (single-quoted, and only after
   `<own>` matches `^anchor-[A-Za-z0-9_-]+(-s[0-9]+)?$`), or `unset
   KOTO_ANCHOR KOTO_DISPATCHER_ANCHOR` when open printed no valid name, so a
   failed hook never leaves the dispatcher's anchor in the shells;
   `unset NIWA_REAP_KEY`; and a final `:`. Otherwise the hook emits the
   single `KOTO_ANCHOR` export line as `additionalContext`.
6. On `startup`, or a `resume` that opened a successor, with no key, detach a watcher that, every 2 seconds for 180
   seconds, reads (never sources) the sibling `sessionstart-hook-*.sh` files
   for an `export NIWA_REAP_KEY=` line, strips its single quotes, accepts
   the value only when it then matches `^[A-Za-z0-9_-]{1,128}$`, and runs
   `koto session anchor observe
   --anchor <own> --key=<v>`. On `clear` with `observed=no` and `previous=`
   set, detach a watcher that loops `observe --anchor <own> --inherit` until
   `inherit_until`, never longer than 180 seconds. Watchers run with
   `KOTO_ANCHOR` and `KOTO_SESSION_*` unset, fully detached (new session,
   every descriptor redirected), and exit early when the claude process in
   the token is gone.
7. Exit 0.

The common path costs one `ps` and one koto call, about 50 ms. A `claude -p`
child inherits its parent's `KOTO_ANCHOR` and `KOTO_DISPATCHER_ANCHOR` and no
niwa variable, so its own hook records the parent as its dispatcher, replaces
both in its own shells, and is never observed. A `niwa dispatch --detach`
worker gets `KOTO_DISPATCHER_ANCHOR` through niwa's declared pass-through,
and niwa's removal list keeps the dispatcher's `KOTO_ANCHOR` out of its
launch environment, so a worker with no start hook (a Codex worker) has no
`KOTO_ANCHOR` and never acts under its dispatcher's anchor. The guarded close form a workspace declares as niwa's on-reap
command is documented in the koto-user skill and the CLI guide (Decision 7).

#### Alternatives Considered

- **Session-id anchors with a plugin pointer file at `/clear`.** Needs the
  same process token and koto-side check as the scan, but adds a second
  state location, garbage collection and a `$TMPDIR` fallback, and a lost
  pointer splits the hierarchy. A close call: the two are equally reliable,
  and the pointer's only edge, an O(1) lookup, matters little on a path that
  runs only at `/clear`.
- **One anchor per process across `/clear`.** Gives the same retention once
  the marker is copied, but a wrong match writes into another session's
  anchor and a resumed pre-clear conversation shares an anchor with the
  post-clear one; both break R1.
- **A template file shipped with the plugin, opened with existing `init`
  verbs.** Copying the marker at `/clear` takes several non-atomic calls, the
  lifecycle branches live in bash, and recognition by template file name can
  be spoofed or dangle when the cache is evicted.
- **A random anchor name carried only by the variable.** A resume in a new
  process has nothing to read the name from.

### Decision 3: The retention engine and its wiring

Delegated to /decision.

Inbound edges live in other sessions (a dispatched-by link sits in the
dispatched session, and only a child's header names its koto parent), so
every question about a session needs a pass over the store. A full log
parse of every session is too slow for R34's one-second query: on a real
store of 941 sessions and 67 MB of logs, `koto session list --state` took
0.68 s against 0.04 s for header lines alone.

Key assumptions:

- Anchors and close sessions are recognizable from the header line
  (Decision 2's `kind`).
- Import writes `koto/imported-from` into the copy (Decision 6), so migrated
  detection is a key lookup in both the query and prune.
- Link values and close records carry the peer's session id (Decision 1).
- The move-aside keeps the predecessor's links and header lineage
  (Decision 4).
- R15's "has lineage" includes being a koto parent, so a parent is kept at
  its terminal tick.
- Sessions imported before this release carry no `koto/imported-from` and
  read as ordinary sessions, which only over-retains.

#### Chosen: a two-level engine in `src/session/hierarchy.rs`

One whole-store edge scan, shared by every caller of a command; full
liveness inspection only of the component asked about; one `Judge` and one
`Verdict` behind every removal point and the query.

- **Reading.** `SessionFacts::read(base, name)` reads the header's first line
  (`parent_workflow`, `kind`, `template_name`, `session_id`), the `koto/`
  entries of `ctx/manifest.json` plus any non-empty file under
  `ctx/koto/links/` the manifest doesn't list yet, each link body, and
  `ctx/chain/parent`. It reads the local sessions directory directly, never
  through the cloud backend's `list` or `read_events`, which reach S3.
- **Graph.** `Graph::scan(base)` builds an undirected edge list (links,
  `parent_workflow` with the epoch-branch rule of Decision 4, well-formed
  `chain/parent`), the migrated set and a
  union-find, once per command. Absent and migrated names are leaf members
  that never join two sessions. A link naming a migrated source resolves to
  the local copy whose `koto/imported-from` names it. A link whose session-id
  hint names a moved-aside predecessor joins it too.
- **Liveness.** `Judge::new(&graph, settings, now)` fully inspects a
  component's members the first time it's asked, with today's
  `inspect_full` plus one new field, `stopped_at`: the timestamp of the
  event that made the session terminal, or of the trailing cancel, not the
  last event (which reads move). A koto session is alive while running or
  unreadable. A close session is never alive; its stop is its record's `at`.
  An anchor's last activity is the latest event in its own log, where only
  its agent session writes (start events, `anchor_activity` events, its own
  read records); events other processes write into members never count, so
  a bare-shell read, a child's completion or a `koto context add` keeps no
  anchor open. Its candidate close instants are a valid observed close
  (final) and, when it has no observed marker (unobserved or imported), last
  activity plus the idle period. An observed anchor has only the observed
  close: no idle or other rule closes it. Past last activity plus the stale
  threshold it is reported `stale`, a report that closes nothing. An anchor
  reads closed when any candidate has passed, and its stop is the earliest.
- **Verdict.** `Keep { keepers, retention, unreadable }` or
  `Release { release }`, with `Keeper { name, kind, observed, stale, close_command? }` and
  `retention = { release: { at, member, close_kind }, passes_at }`;
  `close_kind` is `terminal`, `cancelled`, `observed` or `idle`. An observed
  keeper anchor past its stale threshold carries `stale: true` and a
  `close_command` string, the one command an operator runs to close it by
  hand:
  `koto session anchor close --key=<marker> --harness-session=<id>`, filled
  in from the anchor's observed marker and recorded harness session id,
  each single-quoted. It is built only when both values match
  `^[A-Za-z0-9_-]{1,128}$` (the close verb's own check); otherwise the anchor is
  reported stale with a malformed marker and no command. One serializer and
  one text renderer, so prune and the query print the same reason and the
  same command, through the same check.
- **Query.** `koto session hierarchy <name> [--json]`: kept or not, keepers
  (each stale anchor with its close command) or release instant and when
  the retention passes, unreadable items, the members with kind, liveness,
  stop instant and absent or migrated flags, and the three settings with
  their origins. It writes nothing.
- **Settings.** `retention.keep` (default 7d), `retention.idle_period` (7d)
  and `retention.stale_after` (30d), overridden per command by
  `KOTO_RETENTION_KEEP`, `KOTO_RETENTION_IDLE_PERIOD` and
  `KOTO_RETENTION_STALE_AFTER`, resolved environment, then project, then
  user, then default, and reported with origins by `koto config list`.
  They stay out of the `KOTO_SESSION_*` family, which koto hands to tick
  commands and shirabe scrubs as cloud configuration. They parse late with
  the prune's `parse_age` (s, m, h, d). The stale threshold only sets when
  a stale report starts, and one not longer than the retention is refused,
  naming the setting, so a stale report never comes before a correct close
  would have released anything.
- **Fail toward keeping.** A malformed link, close record or `chain/parent`
  taints its component; an unreadable log taints its component and counts
  as alive; an unreadable header, manifest or links directory taints every
  component; an unlistable sessions directory makes `Graph::scan` fail, so
  prune removes nothing and exits non-zero and the tick's edge test answers
  "edge".
- **Clock.** One `now` per command; every instant is a koto-written
  timestamp on the same host clock. Tests shorten the durations through the
  environment.

Wiring:

- **Terminal tick.** At both `terminal_retention` call sites, before the
  answer prints: `FailureTerminal`, then `edge_of` (a new
  `RetentionReason::Hierarchy`, `"hierarchy"`, with or without
  `--no-cleanup`), then `NoCleanup`, else removal as today. `edge_of` checks
  the session's own `koto/links/`, `parent_workflow`, `chain/parent` and
  child signals, then makes one pass that stops at the first inbound hit. No
  retention is evaluated and no settings are read, so a lone session costs
  one header-level pass and is removed as today.
- **Parent sweep.** A parent with children always has an edge, so it is
  kept at its terminal tick, and so are its children, which have lineage to
  it. The sweep at that tick therefore removes nothing: it lists each
  terminal child with its reason in a new `kept_children: [{name, reason}]`
  field beside `retention`, and prune reclaims the subtree later. The sweep
  still removes, by the rule, when a released replace removes a
  predecessor's descendants.
- **Same-name replace.** Keep moves the predecessor aside (Decision 4) and
  the answer carries its new name and reason; release replaces as today.
- **`koto workspace prune --root X`.** Each subtree member except X is
  judged with only the lineage between X and its direct children dropped;
  X is judged with every edge. That is the narrowest reading that lets the
  person who named X dissolve X's subtree while a grandchild under a kept
  parent stays kept and no kept child is left naming a removed parent.
  Released members go deepest first and X last; the rest are listed with
  reasons. `--force` never overrides the rule.
- **`koto session prune`.** Rule a becomes "alive by the engine"; rules b and
  c become rule `h` (hierarchy), whose `detail` is the rendered verdict with a
  structured `hierarchy` field; d, e, f and g stay. Filters pick candidates
  only, and `--older-than` measures from the stop instant. At apply: one
  fresh scan, each component re-judged as a unit, each member removed under
  its own lock after a status and session-id check, anchors before their
  close sessions. A session the rule now keeps is skipped and listed, exit 0;
  `--apply --plan` still refuses (exit 4) when the fresh judgment would
  remove something the plan didn't name.
- **`koto session cleanup` and `koto cancel --cleanup`.** Before removing,
  the output gains `live_members: [{name, kind, observed}]`, every other live
  member when the rule would have kept the session, else empty.

#### Alternatives Considered

- **One full inventory read by every caller.** The query would parse every
  log, about 2 to 4 seconds at 2,000 real-shaped sessions on the runner,
  against the 1-second bound.
- **A persistent host-level edge index.** A second mutable record that
  import, cloud pull, relocate, manual removal, older binaries and crashes
  all bypass; a sound answer still stats every session.
- **Per-question probes with no graph.** Prune stays quadratic (millions of
  stats at 2,000 sessions); the probe survives only as the tick's `edge_of`.
- **Closing an observed anchor without niwa.** An idle close after a longer
  period would release a live agent session's records. A close once every
  directly linked session is gone can never fire, because the anchor's own
  agent session can be shown gone only by the observed close itself. Past
  the stale threshold the engine reports the anchor with its close command
  instead, and an operator closes it by hand.
- **Sub-choices:** no edge for being a koto parent (removes a parent while
  its hierarchy is alive); masking all lineage inside a `--root` subtree
  (removes a grandchild under a kept parent); a byte search for migration
  (splits the query from prune, or reads every log per query).

### Decision 4: Moving a held predecessor aside at a same-name replace

Resolved inline.

#### Chosen: move the predecessor's subtree aside, re-pointing lineage explicitly

The replace finds the predecessor's koto descendants with the same guarded
walk the sweep uses, factored into one helper in `src/cli/workspace.rs` that
both call: by `parent_workflow` in each header, whatever the names, with one
lineage rule defined once in `src/session/hierarchy.rs` and shared by the
walk, `Graph::scan`, `edge_of` and today's `derive_superseded_branches`
(which carries its own `starts_with` copy of it now): a `parent_workflow`
that splits at its last `~` into `<p>` and a run of digits, and isn't itself
a session, counts as lineage to `<p>` (a batch rewind leaves its relocated
children pointing at that never-created name). The walk keeps the sweep's
visited set and depth cap, but where the sweep skips a branch the move
refuses, so no part of the subtree is left under the old names; it re-reads
each descendant's status just before moving it and treats an unreadable
descendant like a live one.

- **A live descendant refuses the replace.** When any descendant is running
  or waiting to promote its result to a leg, the replace refuses with
  `replace_held_live`, naming them. Moving such a session would break the
  driver still calling `koto next` on it, or re-point its leg at whoever
  holds the name next; leaving it in place would attach it to the new run.
- **Otherwise the subtree moves.** The predecessor becomes `<name>_r<N>`,
  with N the lowest value leaving every target name free. A descendant whose
  name starts with `<name>.` or `<name>~` is renamed to the same suffix under
  `<name>_r<N>`, deepest first, through a checked `relocate`
  (`<name>~K.<task>` becomes `<name>_r<N>~K.<task>`). A descendant with any
  other name keeps it. Every session in the subtree then has its
  `parent_workflow` set explicitly with `rewrite_header` by one prefix-swap
  rule: when the original parent equals `<name>` or starts with `<name>.` or
  `<name>~`, that prefix becomes `<name>_r<N>`; the predecessor keeps its
  original parent. Nothing is left to `relocate`'s derivation from a dotted
  name. Lineage stays intact, and the new run
  starts with no children, as today.
- **A failure partway moves back.** If any rename or header rewrite fails,
  everything already moved goes back with its original header values
  restored, not re-derived, and the replace refuses with `replace_failed`.
  Under the cloud backend each session has its own remote prefix, and
  today's `relocate` deletes each old object as it copies, skips one whose
  copy fails and still returns success, and `rewrite_header` only warns when
  its push fails. The move therefore runs in three steps:

  1. **Check names.** A candidate `_r<N>` name is free only when a checked
     listing of its remote prefix finds no object at all, a migrated marker
     included; a failed listing refuses the replace.
  2. **Copy and re-point.** A checked relocate variant copies each renamed
     session's objects to the new prefix without deleting the old ones and
     fails on any skipped copy. The state log's object name embeds the
     session name (`koto-<id>.state.jsonl`), so the copy skips it and every
     renamed session, the predecessor included, gets a strict
     `ensure_pushed` under its new name. Each `rewrite_header` is likewise
     followed by `ensure_pushed`, and any failed push counts as a failed
     rewrite. Any
     failure in this step moves back: renamed sessions return by a
     local-only rename, never through the cloud `relocate`; every restored
     header, a kept-name descendant's included, is pushed again with
     `ensure_pushed`; and the new prefixes are deleted with the checked
     remote delete, which is safe because the old prefixes still hold
     everything.
  3. **Delete old prefixes.** Only once every copy and every header push has
     succeeded does the replace delete the old remote prefix of every
     renamed session with the checked delete, before the new run is created.
     From the first delete on, a failure rolls forward rather than back: the
     moved sessions keep their new names and the replace reports the old
     prefixes it couldn't empty, because a move-back now could delete the
     only copy of objects that existed only in the bucket. The new run is
     still created, with a warning; leftover objects under an old prefix can
     mix into the new run's prefix, as today's best-effort cleanup allows.

  Implementation notes: the name check in step 1 also checks the local
  store, and each remote listing ends in `/` so `_r1` doesn't match `_r10`;
  the move-back's local-only rename is a narrow new backend method, not a
  reach into `LocalBackend` from the CLI; and the held case in
  `init_entry.rs::create` no longer calls the descendant sweep or
  `backend.cleanup(name)`, whose best-effort remote delete would hide a
  step 3 failure.

  If the move-back itself fails,
  or the process dies mid-move, the sessions already renamed stay readable
  under their new names with their lineage re-pointed, the next replace
  picks a fresh N, and the hierarchy query shows them: the failure
  over-retains and never loses a record.
- **What else holds.** Log events, keys and links are untouched. The
  session-id-hint rule (Decision 3) applies to every moved session, so a
  link by name that now resolves to the new run also joins the moved one.
  A malformed retention setting at a replace counts as "keep" and moves the
  subtree aside with a warning. A chain child adopted before shirabe's link
  call (Phase 10) holds only a `chain/parent` name, which carries no session
  id, so it follows the name to the new run; that over-retains and nothing
  more. The replace answer gains `moved_to`, and
  `replaced_result` is unchanged.

`_` can't appear in a shirabe topic (`^[a-z0-9-]+$`), so a moved name never
collides with a skill session. The move is a store-level rename and lineage
re-point of the predecessor's own subtree, which the PRD's R17 sanctions; it
writes no context key in any session.

#### Alternatives Considered

- **Refusing the replace.** Breaks every re-run of a skill topic while a
  coordinator lives.
- **Keeping the old keys inside the new run.** Mixes two runs' state.
- **A `~` suffix.** Listings and the dashboard skip `~` names as epoch
  branches, so the predecessor would vanish from view.

### Decision 5: Recording reads from another agent session

Resolved inline.

`koto context get` and `koto context exists` append a `context_read` event
to the read session today; `koto context list`, `koto status` and
`koto session list` append nothing. koto's run journal folds a visit's
`context_read` events from the session's own log.

#### Chosen: the reader's anchor holds the record

In `get` and `exists`, with the reader's anchor taken from `KOTO_ANCHOR`:

| Reader anchor | Read session | Logged in |
|---|---|---|
| names an existing anchor, and the read session holds `koto/links/held-by/<it>` or is it | any | the read session, as today |
| names an existing anchor otherwise | any | the reader's anchor, with a new optional `session` field naming the read session |
| unset, or names no existing anchor | any | the read session, as today (a known cross-session writer) |

A failure to append to the reader's anchor warns on stderr and never fails
the read. The event-feed spec gains the optional `session` field. The
consumers of `context_read` events are named and each handles the new
field: the read session's run journal stops counting reads by other agent
sessions with no code change, because its fold reads only its own log, and
the run journal's `context_reads` records (`src/run_journal/context_reads.rs`)
attribute an anchor's read record to the session its `session` field names;
read records carry context key names and value hashes, so they stay on the
host, with one exception: the user's own koto cloud backend, which they
enabled, syncs to their own bucket, and every other shipper keeps
`context_read` events local (this build changes none); and the
idle computation treats a read record as activity of the reader's anchor,
never of the read session. The anchorless row stays a known cross-session
writer, tracked by koto#412.

#### Alternatives Considered

- **Recording nothing.** Loses events the run journal consumes.
- **Recording in the reader's current koto session.** The CLI doesn't know
  which one, and a bare reader has none.

### Decision 6: Import settles held-by links and clears the marker

Resolved inline.

#### Chosen: rewrite in `stage_import`, and mark the copy

After `stage_import` (`src/session/cloud.rs`) verifies each key against the
manifest and before it writes the staged manifest, the moved copy's
`koto/links/held-by/*` keys are settled by the PRD's table, with the
importing command's `KOTO_ANCHOR` as the importing environment's anchor:

- kept when the name resolves in the local store to an anchor that is
  open and not migrated (its `execution_dir` is a hint only);
- otherwise rewritten to `koto/links/held-by/<importer's anchor>` with
  `"was"` holding the old name;
- with no importer anchor, dropped.

`read_source_manifest` treats the bucket as untrusted for the reserved
area: it drops `koto/anchor/observed`, `koto/anchor-close` and every
`koto/links/closes/*` key alongside the host-local keys, refuses a source
whose header has `kind: "anchor-close"`, and validates every link body it
keeps (a malformed one is dropped and reported). Every import writes
`koto/imported-from` (`{from_session, from_session_id}`) into the copy,
replacing any value the source carried, and the manifest is re-serialised
for the changed keys. The source gets nothing
beyond koto's existing migrated mark. An imported anchor's last activity is
the `session_imported` event import already appends.

#### Alternatives Considered

- **Settling at the first tick after import.** A finished session never
  ticks again, so a stale held-by would pin it.

### Decision 7: niwa's three pieces

Resolved inline.

#### Chosen: a pass-through list, a reap key, and a bounded on-reap command

- **Pass-through.** `[session.dispatch_env] passthrough = ["NAME", ...]`,
  a new agent-neutral table beside `[session.env]`, since Codex workers get
  it too. It is separate because `[session.env]` is written at apply time
  and this one is read at dispatch, from niwa's environment at that moment.
  At load, niwa refuses a name starting `NIWA_` or `CLAUDE_`,
  `OTEL_RESOURCE_ATTRIBUTES`, a duplicate, or a name also declared in
  `[session.env]` `vars` or `promote`. In `runDispatch`
  (`internal/cli/dispatch.go`), a new launch-settings contributor copies each
  set name from niwa's environment into the one `env` object of the
  `--settings` document, merged with the lineage
  `OTEL_RESOURCE_ATTRIBUTES` (the env value type becomes a small ordered
  map, widening the first named exception (the lineage `env` key) to the constant-values rule in
  `dispatch_settings.go`), and into the launched process environment; an
  unset name is skipped. Codex workers get the process environment only.
  `[session.dispatch_env] unset = ["NAME", ...]` lists names niwa removes from a
  worker's launch environment, so a value the dispatching shell carries
  never reaches a worker with no hook to replace it. Because the settings
  document travels on the worker's command line, niwa also refuses names
  that look like credentials, matched case-insensitively (ending `_KEY`,
  `_APIKEY`, `_SECRET`, `_TOKEN`, `_PAT`, `_PASSWORD`, `_PASSWD` or
  `_CREDENTIALS`, or starting `AWS_`), and koto's guide says to declare only
  `KOTO_DISPATCHER_ANCHOR` for pass-through and `KOTO_ANCHOR` for removal.
- **Reap key.** `NIWA_REAP_KEY`: for `niwa dispatch`, the dispatch id, set in
  the settings and process environments before launch; for `niwa instance
  from-hook`, the session id, written as a plain, single-quoted `export
  NIWA_REAP_KEY='<value>'` line to the hook's `CLAUDE_ENV_FILE`. The
  session mapping already holds both values (`DispatchID`, `SessionID`), so
  at reap niwa recomputes the key from the mapping (the dispatch id when
  present, else the session id); the only new stored field is the
  mapping's count of failed on-reap runs, which the ten-run limit reads.
  niwa sets or passes
  the variable only when the value matches `^[A-Za-z0-9_-]{1,128}$`. A
  dispatch sets it even when no lineage value is composed, replaces any
  `NIWA_REAP_KEY` inherited from niwa's own environment in the worker's
  process environment (built from `os.Environ` in `dispatch_launcher.go`),
  and removes an inherited one when it has no valid key. The key is an
  identifier, not a secret. At reap the recomputed value is checked
  against the same pattern again, and a mapping whose value fails it is
  skipped with a warning line rather than handed to the command.
- **On-reap.** `[session.on_reap] command = "<shell>"`, `timeout = "10s"`,
  `env = ["NAME", ...]` (its own list, with the same load-time refusals as
  the pass-through lists; a name may also appear in `[session.dispatch_env]`,
  since this list reaches only the command's process environment and never a
  command line, so a variable the close command needs never has
  to ride on every worker's command line).
  niwa runs `sh -c`, from the workspace root (the removed instance may be
  gone), with only `PATH`, `HOME`, `NIWA_REAP_KEY`,
  `NIWA_SESSION_ID`, `NIWA_INSTANCE_PATH` and each name in
  `[session.on_reap] env` set in niwa's environment (so a variable the close
  command needs, such as koto's session-store location, reaches it without
  niwa naming it), a per-run timeout and a
  30-second wall budget per niwa step; a run the budget can't start is
  skipped. The command runs in its own process group, killed whole on
  timeout. A failure, timeout or skip prints one `niwa: warning: on-reap for
  session <id>: <reason>` line, with the reason truncated and stripped of
  control characters; niwa keeps that session's mapping, so the next reap
  that finds the session gone runs the command again, and drops it with a
  final warning after ten failed runs. A mapping kept because its session
  was still running, kept by `--keep-session`, or left behind by an instance
  deleted by hand is reaped the same way once its session is gone; reap
  walks the stored mappings as well as the instances on disk
  (`selectReapTargets` walks only the latter today), so a mapping whose
  instance and session are both gone is still found. The
  step's outcome is unchanged. The command has the trust level of a hook the workspace
  configuration installs. Call sites: the mapped reap in `reap.go` (with idle-grace
  reclaim and the reaps `dispatch`, `create` and `watch` run first),
  `removeOwningSessions` in destroy when it removes or finds gone a session,
  and `watch`'s discard of a staged review whose session is gone (looking up
  the mapping for the key). Not reset, `--keep-session`, the unmapped
  backstop or a dispatch rollback. In destroy the session step runs before
  the instance directory goes (`destroy_session.go`), and
  `removeOwningSessions` deletes the mapping right after removing the
  session today. That changes: destroy runs the command after the session's
  removal succeeded, from the workspace root whether or not the directory
  still exists, and deletes the mapping only once the command succeeds, as
  reap does; a failed run keeps the mapping for the next reap, and a failed
  session removal runs nothing and keeps the mapping. A reap or destroy that
  can't read `[session.on_reap]` from the workspace configuration counts
  each session's run as failed, so its mapping is kept. `niwa watch`'s
  review launch, which calls the launcher directly with `os.Environ()`
  rather than going through `runDispatch`, gets the same environment
  shaping (pass-through, removal list, reap key), because it is built into
  the launch-request path both use.
- **Workspace configuration** (dot-niwa) declares koto-skills in
  `[claude] plugins`, `[session.dispatch_env] passthrough =
  ["KOTO_DISPATCHER_ANCHOR"]` and `unset = ["KOTO_ANCHOR"]`, and
  `[session.on_reap] command = "[ -n \"$NIWA_REAP_KEY\" ] || exit 0;
  command -v koto >/dev/null 2>&1 || exit 0; koto session anchor close
  --help >/dev/null 2>&1 || exit 0; exec koto session anchor close
  --key=\"$NIWA_REAP_KEY\" --harness-session=\"$NIWA_SESSION_ID\""`, so a
  session with no key, a host with no koto, or an older koto without the
  verb exits 0 quietly, and the close reaches only the agent session niwa
  removed (`NIWA_SESSION_ID` is already in the command's environment, so
  niwa still names no koto). `NIWA_SESSION_ID` is the mapping's
  `SessionID`, the Claude Code session id the agent session runs under,
  which is the `session_id` its start hook received and recorded as
  `harness_session`; for a dispatched worker the reap key is the dispatch id
  but the session id is still that one. `close` prints `closed=<n>`, so a
  binding that matched nothing is visible in the command's output, and it
  still exits 0 then, because a session without the plugin has no anchor
  and must not count as a failed run.

#### Alternatives Considered

- **A detached on-reap run.** A crash between the reap and the run loses
  the close silently, and with no idle close for observed anchors a lost
  close would keep a hierarchy until someone closes it by hand.
- **Skipping a failed run.** The same: nothing else would close the anchor.
- **Passing `KOTO_ANCHOR` itself through.** A worker with no start hook
  would act under its dispatcher's anchor, writing into another agent
  session's anchor.
- **A denylist naming `KOTO_SESSION_*`.** niwa would name koto (R29).

### Decision 8: shirabe's link calls and the reclaimable rule

Resolved inline.

#### Chosen: link at each opener, and retire the rule

- **Chain dispatch.** `skill-session.sh adopt`, when one parent matched, runs
  `koto session link <child> --dispatched-by <parent>` after writing
  `chain/parent`; a failure is retried once, then `adopt` prints
  `warning=link-failed` and records it in a context key in the child's own
  session and goes on, as the openers do. The script's koto floor rises to
  the release that ships the verb. `skill-session.sh open` and `/work-on`'s
  PLAN-mode opener gain `--dispatched-by <name>` in the same change, with
  the same retry and record.
- **`/execute` coordinated children.** The step agent's `koto init` for the
  `/work-on` child gains `--dispatched-by execute-<slug>`. These are root
  sessions opened inside the run's own agent session, so its anchor holds
  them as well; the link keeps them when the run starts from a bare shell.
- **`/coordinate` message-path workers.** `coordinate-open.sh` gains
  `--dispatched-by <name>` too, which links after opening, retries once on
  failure, then prints `warning=link-failed` and records it in a context key
  in the worker's own session, and goes on. `render-brief.sh` puts the exact
  opener invocation, with `--dispatched-by <dispatcher_session>` filled in,
  into the brief's Goal, so no entry point's skill text passes the flag; the
  worker renders its result with a new `skills/coordinate/scripts/render-result.sh`,
  which its report step runs (the Reporting section `render-brief.sh` emits
  names it), reading the recorded warning from the worker's own session; the
  coordinator learns of it from the report and never reads the worker's key. Re-brief, hand-off and unwatchable paths re-render the brief, so
  they carry it; after a re-brief, `dispatcher_session` names the
  coordinator run that rendered the new brief. A recycled coordinator, a new
  run carrying an earlier run's record, is linked to the old run by
  `coordinate-open.sh` with a dispatched-by link in the new run's session,
  and `references/koto-session-retention.md` says so. Lands inside or after
  the teardown build's brief-renderer, opener and worker-dispatch changes
  (shirabe#981, #984 and #985), and any `dispatch-worker.sh` change goes in
  with it.
- **`/execute`'s rendered command.** Where `/execute` renders the step
  agent's `koto init` command, `--dispatched-by` goes into that rendered
  command rather than into prose.
- **Retire** `skill-session.sh reclaimable`, its tests and the convention's
  reclaimable section; the convention names `koto session hierarchy`.
- **Guard.** A shirabe test fails when a template's gate or default action,
  or a script it runs, launches `niwa dispatch` or `claude` without unsetting
  `KOTO_SESSION_*` (R32; none does today).

#### Alternatives Considered

- **A koto-side variable naming the dispatcher for message-path workers.**
  Nothing carries it across a detached dispatch without the pass-through,
  and the brief already names the dispatcher.
- **Re-pointing `reclaimable` at koto.** It has no caller, and a second
  answer is the drift the change removes.

## Decision Outcome

The pieces compose into one loop. An agent session starts; its plugin hook
opens `anchor-<S>` and exports `KOTO_ANCHOR`. Every koto session the agent
session opens or ticks records a held-by link to that anchor, and every
dispatch records a dispatched-by link in the dispatched session: through
`init --koto-leg`, a self-attach, the link verb shirabe calls, or the
dispatched agent session's own anchor reading the inherited variable. niwa,
when present, marks sessions it will reap; the anchor records that marker.

Every removal by rule or subtree asks one engine one question: is anything
in this session's connected hierarchy alive, and if not, has the retention
passed since the last member stopped? An anchor is alive until niwa's
on-reap command records its observed close; only an unobserved or imported
anchor closes after its idle period, counted from activity in its own log.
An observed anchor closes only through an observed close: niwa's next reap
repeats a lost one, and past the stale threshold the anchor is reported
stale with the command that records one by hand. Everything the engine
needs is in sessions' own keys and logs on the local store, so it works
with koto alone, with no niwa and no cloud. Import carries links verbatim
and settles only the moved session's own held-by links, so a hierarchy
follows a session by name. Reads from another agent session go into the
reader's anchor, so the read session stays byte-identical.

Three existing koto behaviours still write into a session from another
session's process and stay outside this build, named so nobody mistakes
them for part of the rule: a child's terminal tick appending its completion
to its parent's log (koto#409 tracks replacing it with the parent reading
its children at its own tick); `koto context add` naming any session
(koto#411); and a read with no reader anchor logging into the read session
(koto#412). Under the cloud
backend a migrated source refuses all three. Nothing in this design treats
a completion record as a link or a sign of life, and none of the three
counts toward an anchor's activity.

## Solution Architecture

### Overview

```
 agent session S (Claude Code)
   SessionStart hook ──> koto session anchor open ──> anchor-S  (kind: anchor)
        │  export KOTO_ANCHOR=anchor-S                ▲   koto/anchor/observed
        ▼                                             │ held-by
   koto init / koto next  ─────────────────────────> session B  koto/links/held-by/anchor-S
   dispatch (leg, link verb, inherited variable) ──> session C  koto/links/dispatched-by/B
                                                              
 niwa reap ──> on-reap: koto session anchor close --key K --harness-session S ──> anchor-S-closed
                                                                koto/links/closes/anchor-S

 removal points ──> hierarchy engine (Graph::scan, Judge) ──> Keep{keepers} | Release
```

### Components

**koto: link store** (`src/session/links.rs`, new). Constants for the
reserved prefix and link kinds; `write_link(store, session, kind, peer,
hints)` through `add_with_writer(WRITER_KOTO)` plus a `context_added` event,
the pattern `workflows_surface/discover.rs` uses; `read_links` for a
session. The refusal of `koto/` keys, case-insensitive, sits in the
context store's write path, so every caller of `add` and `remove` meets it.

**koto: link writers.**
- `koto session link <session> --dispatched-by <name>` (`src/cli/session.rs`).
- `koto init --dispatched-by <name>` (`Command::Init`, `InitArgs`; written in
  `init_entry::create` and `handle_init` after the intent block).
- `init --koto-leg` and a self `koto request attach`: the request view's
  `requested_by`, already read by `precheck_attach`
  (`src/engine/request_store/attach.rs`), becomes a
  dispatched-by link when it passes the session-name rule.
- Held-by: `koto init` and `koto next` read `KOTO_ANCHOR`; when it names an
  existing anchor other than the session being opened or ticked and no
  `koto/links/held-by/<it>` exists, they write one. When that anchor reads
  closed by an observed close and has a successor, they also write a
  held-by link to the current successor, so a resumed session whose shells
  still name the old anchor stays held. Each such `koto init` and tick also
  appends an `anchor_activity` event to that anchor, the agent session's own
  anchor, at most one per tenth of the idle period (read from the anchor's
  last event, so the throttle needs no state); this is the only activity the
  idle computation reads. A value that names no anchor writes nothing. koto
  reads only `KOTO_ANCHOR` as the caller's anchor, never
  `KOTO_DISPATCHER_ANCHOR`. `koto next` on an anchor or close session is
  refused.

**koto: anchors** (`src/session/anchor.rs` and
`src/cli/session_anchor.rs`, new). Two embedded templates: the anchor's
one-state template that never finishes (`src/session/anchor_template.md`),
and the close session's template whose initial state is terminal
(`src/session/anchor_close_template.md`), so a close session reads finished
everywhere, older binaries included. The header gains an optional `kind`
(`anchor`, `anchor-close`), parsed as a string and serde-defaulted, so older
logs read unchanged and an unknown kind reads as an ordinary session.
`anchor_started` joins the event types and the feed spec
(`docs/reference/session-feed.md`). Anchors and close sessions are left out
of `koto workflows` (so the Stop hook's "active workflow" check and agents
listing workflows never see them) and of the dashboard's workflow views;
`koto session list` shows them with their kind.

**koto: hierarchy engine** (`src/session/hierarchy.rs`, new):
`SessionFacts`, `Graph`, `edge_of`, `Judge`, `Verdict`, `Scope`,
`RetentionSettings`. So that `src/session` keeps depending on nothing in
`src/cli`, the pieces it reuses move down first: `inspect_full`, its status
types and the template cache from `src/cli/session_inventory.rs` to
`src/session/inventory.rs` (re-exported where the CLI uses them), and
`parse_age` from `src/cli/session_prune.rs` to `src/session/duration.rs`.
`inspect_full` gains `stopped_at`. The terminal tick never reads the
settings, so a malformed setting can't fail a tick; the query and both
prunes refuse it.

**koto: query** (`src/cli/session.rs`): `koto session hierarchy <name>
[--json]`.

**koto: removal points.** `terminal_retention` and `finish_terminal_tick`
(`src/cli/mod.rs`, `next_types.rs`), `sweep_if_parent` and `sweep_children`
(`src/cli/workspace.rs`), the replace path (`src/cli/init_entry.rs`),
workspace prune (`src/cli/workspace.rs`), session prune
(`src/cli/session_prune.rs`), cleanup (`src/cli/session.rs`) and cancel
(`src/cli/mod.rs`).

**koto: reads** (`src/cli/context.rs`, `log_cli_read`): the reader-anchor
table of Decision 5.

**koto: import** (`src/session/cloud.rs`, `read_source_manifest`,
`stage_import`): Decision 6.

**koto: settings** (`src/config/`): three keys, defaults, env overrides,
project precedence, origins.

**koto-skills** (`plugins/koto-skills/hooks/hooks.json`,
`hooks/session-start.sh`, kept `hooks/session-start-workflows.sh`, Stop
hook; `.github/workflows/validate-plugins.yml`; the koto-user skill's
references).

**niwa** (`internal/config`, `internal/cli/dispatch.go`,
`dispatch_settings.go`, `instance_from_hook.go`, `reap.go`,
`destroy_session.go`, `watch.go`, a new `internal/cli/on_reap.go`, and
`internal/config/session.go` for `[session.dispatch_env]` and
`[session.on_reap]`).

**shirabe** (`scripts/skill-session.sh`, `skills/execute/references/
step-agent-handoff.md`, `skills/coordinate/scripts/render-brief.sh`,
`coordinate-open.sh`, `skills/work-on/scripts/work-on-open.sh`,
`references/skill-session-convention.md`,
`references/koto-session-retention.md`, a template guard test).

**dot-niwa** (workspace configuration): the three declarations of
Decision 7.

### Validation and hardening

These rules bind the components above wherever they read untrusted bytes or
honour a record that can shorten retention.

- **The reserved area.** The refusal of `koto/` keys sits in the context
  store's write path for the agent writer, not only the CLI arm, and
  compares case-insensitively, so `KOTO/links/...` on a case-insensitive
  filesystem is refused too.
- **Reading links.** `SessionFacts::read` uses `lstat` and treats a
  symlinked session directory, `ctx/koto/` directory, or any symlink or
  non-regular file under `ctx/koto/`, as unreadable (taint, keep); caps a
  link body at 4 KiB; accepts a file the manifest doesn't list only for
  dispatched-by and held-by, never for `closes`; parses `kind` as a string,
  so an unknown kind reads as an ordinary session rather than an unreadable
  header; and reads a link body with an unknown version as an edge by its
  `session` name when that parses.
- **Honouring a close.** A close record counts only when its session's
  header has `kind: "anchor-close"`, its name is `<anchor>-closed`, its
  `anchor_session_id` equals the anchor's header id, its `key` equals the
  anchor's observed marker, and the manifest lists it; anything else taints
  and keeps.
- **Honouring a migration.** A session X counts as migrated only when a
  local copy's `koto/imported-from` names it with `from_session_id` equal to
  X's header id, the copy's log holds the matching `session_imported` event,
  and X's last event is no later than that event; a live X keeps appending,
  so a forged record can't mark it gone.
- **Clock.** Every instant read from disk is clamped to `now` for activity
  and stop computations, and the query reports the clamp. A component
  holding a timestamp more than five minutes in the future is tainted, kept
  and named, so one bad timestamp holds its own hierarchy, not the store.
- **Prune apply.** Each component is re-scanned and re-judged immediately
  before its members are removed.
- **Rendering.** Every renderer strips control characters from names,
  hints and reasons; the query lists the edge that joined each member. A
  stale anchor's close command is printed only when its marker and harness
  session id pass the close command's own checks; otherwise the anchor is
  named with no command.
- **Store permissions.** koto assumes `~/.koto` is private to its user
  (0700 directories, 0600 files) and documents it; anyone who can write the
  store can forge anything in it.

### Key Interfaces

```
koto session link <session> --dispatched-by <name>
koto init <name> ... [--dispatched-by <name>]
koto session anchor open --harness-session <id> --source <s> [--process <t>]
                         [--dispatched-by <name>] [--observed-key <v>] [--workspace <dir>]
koto session anchor observe --anchor <name> (--key=<v> | --inherit)
koto session anchor close --key=<v> --harness-session=<id>
koto session hierarchy <name> [--json]

KOTO_ANCHOR                      the running agent session's anchor
KOTO_DISPATCHER_ANCHOR           the dispatcher's anchor, read only by the start hook
KOTO_RETENTION_KEEP                default 7d   (retention.keep)
KOTO_RETENTION_IDLE_PERIOD         default 7d   (retention.idle_period)
KOTO_RETENTION_STALE_AFTER         default 30d  (retention.stale_after)
```

`koto session hierarchy --json`:

```json
{
  "session": "B",
  "kept": true,
  "keepers": [{"name": "anchor-1f3c...", "kind": "anchor", "observed": true}],
  "retention": null,
  "unreadable": [],
  "members": [{"name": "B", "kind": "koto_session", "alive": false,
               "stopped_at": "2026-10-22T18:01:02.000Z", "close_kind": "terminal"}],
  "settings": {"retention": {"value": "7d", "origin": "default"}}
}
```

When nothing is alive, `keepers` is empty and `retention` is
`{"release": {"at", "member", "close_kind"}, "passes_at"}`; `kept` is
`true` until `passes_at`. An observed keeper past the stale threshold adds
`"stale": true` and `"close_command"`, the hand-close command with the
anchor's marker and harness session id filled in; prune prints the same.

Tick answer: `"retention": {"retained": true, "reason": "hierarchy"}` and
`"kept_children": [{"name", "reason"}]`. Cleanup and cancel:
`"live_members": [{"name", "kind", "observed"}]`. Replace: `"moved_to"`.
Prune kept entries: rule `h` with a `hierarchy` object equal to the query's
JSON.

niwa configuration:

```toml
[session.dispatch_env]
passthrough = ["KOTO_DISPATCHER_ANCHOR"]
unset = ["KOTO_ANCHOR"]

[session.on_reap]
command = "[ -n \"$NIWA_REAP_KEY\" ] || exit 0; command -v koto >/dev/null 2>&1 || exit 0; koto session anchor close --help >/dev/null 2>&1 || exit 0; exec koto session anchor close --key=\"$NIWA_REAP_KEY\" --harness-session=\"$NIWA_SESSION_ID\""
timeout = "10s"
```

### Data Flow

1. **Start.** The hook computes `anchor-<S>`, runs `anchor open` (link to an
   inherited dispatcher, observed marker from `NIWA_REAP_KEY`, an
   `anchor_started` event), and writes `export KOTO_ANCHOR`,
   `export KOTO_DISPATCHER_ANCHOR` and `unset NIWA_REAP_KEY` to its env
   file.
2. **Work.** `koto init` and `koto next` add held-by links and throttled
   `anchor_activity` events in the agent's own anchor; dispatches add
   dispatched-by links in the dispatched session; nothing is written
   elsewhere.
3. **Read.** `context get/exists` log in the read session only for the
   session's own agent session; other readers' records go into their own
   anchors.
4. **Terminal.** The tick keeps any session with an edge (`hierarchy`); the
   sweep lists kept children; lone sessions go as today.
5. **Close.** niwa's reap runs the on-reap command, which writes
   `anchor-<S>-closed`, and repeats at later reaps until it succeeds; with
   no niwa, the engine reads an unobserved anchor closed after its idle
   period.
6. **Reclaim.** `koto session prune` (or workspace prune, or a re-run)
   removes a hierarchy once nothing is alive and the retention has passed
   since its release instant, anchors before their close sessions.
7. **Move.** `koto session import` copies links verbatim, settles held-by,
   marks the copy `koto/imported-from`, and clears an anchor's marker.

## Implementation Approach

Each phase is one pull request with its tests, in an order where every
merged state is sound. Phases 1 to 8 are koto, 9 niwa, 10 and 11 shirabe,
and 12 the workspace configuration.

### Phase 1: koto-skills hooks load

Move the hooks to `plugins/koto-skills/hooks/hooks.json` in the
matcher-group shape (SessionStart running today's /workflows script; the
Stop hook), delete the root file, point `validate-plugins.yml`'s Stop-hook
smoke test at the new path, and add a shape check that fails when the file
is missing or flat. Independent of everything else; today's /workflows hook
starts working, and the Stop hook still checks `koto workflows`, which no
anchor reaches yet. The changelog says that this turns on the plugin's
dormant /workflows and Stop hooks for every koto-skills user.

### Phase 2: dispatched-by links

The reserved `koto/` prefix in the context store's agent write path, the
link store, `koto session link`, `koto init --dispatched-by`, and the
leg-bound and self-attach links. Links are data only; no removal changes.

### Phase 3: anchors and held-by links

The two embedded templates, the header `kind`, `anchor_started`, the
`koto session anchor open|observe|close` verbs with successors, the `/clear`
scan (with its 250 ms test) and marker inheritance, close sessions, held-by
links and throttled `anchor_activity` events from `KOTO_ANCHOR` at init and
tick (with the successor rule), the resume fallback, refusal of `koto next`
on anchors, and their exclusion from `koto workflows`, the /workflows
materializer and the dashboard. Depends on Phase 2.

### Phase 4: the engine, settings and the query

Move `inspect_full`, its types, the template cache and `parse_age` into
`src/session`; add `src/session/hierarchy.rs` with `stopped_at`, the
validation rules, the three settings, the stale report with its filled-in
close command, and `koto session hierarchy`. The cost
test (`tests/retention_cost.rs`, a release-mode job on `ubuntu-latest`,
with anchors carrying large logs of read records in the fixture) and the
property test that the two-level verdict equals a full-read reference land
here. No removal point changes yet. Depends on Phase 3.

### Phase 5: prunes and explicit removal

Session prune's rule `h`, the engine-based rule a, the apply re-scan per
component and the narrowed `--plan` contract; workspace prune's subtree
scope; both prunes' stale report; cleanup and cancel's `live_members`.
Depends on Phase 4.

### Phase 6: the terminal tick, the sweep and the re-run

`edge_of` and the `hierarchy` reason at both terminal call sites, the sweep's
`kept_children`, the replace's subtree move-aside with `moved_to`, its
`replace_held_live` refusal and its move-back on failure, and the
changelog entry saying end-of-run removal no longer removes a session with a
link or lineage. It also carries the removal-outcome halves of the
scenarios Phase 4 checks only as verdicts (Co-parent, No lineage needed,
Release instant, Absent peer, Observed anchors never idle-close, and
Single writer over them).
Depends on Phase 5, whose prune those end-to-end scenarios check.

### Phase 7: reads and import

The reader-anchor table in `context get/exists` and the feed's `session`
field; import's held-by settlement, `koto/imported-from`, the dropped
reserved keys and the anchor marker clear, with the one-host import tests
under per-command cloud configuration in the environment; the run
journal's attribution of an anchor's read record to the session it names,
and the other named read consumers. Depends on
Phase 6, since the import scenarios run the terminal tick and the prune.

### Phase 8: the anchor hook

`hooks/session-start.sh` with its watchers, the bash suite on synthetic
payloads, a test that bounds the hook's synchronous part, and the docs:
`docs/guides/cli-usage.md` and the koto-user skill's
`references/command-reference.md` carry the verbs, settings, the guarded
on-reap form, the pass-through and removal names, and the corrected
`workspace prune` text; the koto-user `SKILL.md` gains one line at most (the
plugin's hook opens the anchor, so don't run anchor verbs, and ask
`koto session hierarchy` why a session is kept). Depends on Phases 1 and 7, because its
end-to-end scenarios (First dispatch, Idle close, Bare shell, No niwa) check
prune and terminal-tick outcomes and reads that leave the read session
untouched, and its docs describe them. Hand
checks recorded on the pull request: plugin hooks get `CLAUDE_ENV_FILE` in
the per-index layout; `/clear` mints a new id; where a resume hook's env
file points; whether resume keeps the session id; the detached watcher
survives; a dispatched worker sees the `--settings` environment at its first
hook.

### Phase 9: niwa pass-through, reap key and on-reap command

One pull request, because both pieces write the one `env` entry of the
launch settings: today that entry is a value type holding only
`OTEL_RESOURCE_ATTRIBUTES`, set only when a lineage value exists, so two
parallel changes would clobber each other. This phase turns it into a small
ordered map that holds the lineage attributes when there are any, the
declared pass-through names, and `NIWA_REAP_KEY`, which is present even when
no lineage value is composed. It also adds `[session.dispatch_env]`
`passthrough` and `unset` with their load-time refusals (read at dispatch
only, so no apply-time materializer sees them), `NIWA_REAP_KEY` for dispatch
(replacing or removing an inherited value) and from-hook (recomputed from
the mapping at reap), `[session.on_reap]`, `on_reap.go` with its per-run
timeout, process group, step budget, its own `env` list
and mapping retention, reap's walk over stored mappings, destroy's key read
before the mapping is deleted, and the call sites. Independent of koto.

### Phase 10: shirabe links for chains and `/execute`, and the rule's retirement

`adopt`'s link call, `--dispatched-by` on `skill-session.sh open` and the
`/work-on` PLAN-mode opener, the `/execute` step agent's `--dispatched-by`, retiring
`reclaimable`, the R32 guard test, and the koto floor bump. Waits for a koto
release carrying Phases 2, 4 and 5 (the convention names the hierarchy
query).

### Phase 11: shirabe `/coordinate` message-path links

The brief's rendered opener invocation, `coordinate-open.sh`'s
`--dispatched-by` with its retry and recorded warning, and the recycled
coordinator's link to its earlier run. Waits for Phase 10 and for the
teardown build's brief-renderer, opener and worker-dispatch changes
(shirabe#981, #984 and #985).

### Phase 12: workspace configuration

The dot-niwa declarations of Decision 7. Waits for releases carrying
Phases 3 to 8 (koto) and 9 (niwa); its pull request records the
end-to-end niwa scenarios run against those releases, including a detached
dispatch and the child of a reaped worker. Applying it on a host is the
operator's step.

The critical path is koto's serial chain, Phases 2 to 8, each of whose
end-to-end checks needs the one before it; the release gates come after it
and are short by comparison.

## Security Considerations

**Trust boundary.** koto trusts the filesystem permissions of the user's own
store. Links, close records and anchors are plain files, so any process of
the same user can create or alter them, and on a shared store any user can;
this design claims no isolation beyond that. What it controls is what a
forged record can do. A forged link, or an anchor kept busy, can only keep
sessions longer, and `koto session cleanup` is the operator's override. A
close or a migration is the only record that can shorten retention, so koto
honours one only when it matches the anchor or source it names on header
id, key, kind, name and manifest (see Validation and hardening), and it
taints and keeps anything it cannot read or validate. The stale report
never closes anything; it prints a close command for an operator to run,
built from values in the anchor, so it prints one only when both values
pass the close command's own checks, and a forged anchor can't hand the
operator a different command.

**Shell injection through the environment file.** The harness sources each
hook's environment file before every Bash command, so anything written
there is code. The hook allowlists the harness session id, writes only the
anchor name it validated, single-quoted, through a temporary file created
with the original file's mode and then `mv`,
refuses a symlinked or misplaced file, and falls back to `additionalContext`
otherwise. niwa validates and single-quotes the reap key it writes there. The
watcher reads, never sources, niwa's file and validates the value before
passing it as `--key=<value>`.

**Cross-anchor writes.** The dispatcher's anchor reaches a dispatched
session only as `KOTO_DISPATCHER_ANCHOR`, which koto never treats as the
caller's anchor, and niwa's removal list keeps `KOTO_ANCHOR` out of a
worker's launch environment, so a worker with no start hook never writes
into its dispatcher's anchor. Watchers name their own anchor explicitly and
run with `KOTO_ANCHOR` unset, because the inherited value is the
dispatcher's;
`observe` refuses anything that isn't an anchor. `KOTO_ANCHOR` is otherwise
an unauthenticated name: koto acts on it only when it names an existing
anchor. A same-user process that sets it to another agent session's anchor
can put read records and held-by links there, which over-retains; that is
inside the same-user trust boundary above, not a cross-session write koto
makes on its own.

**Bucket input.** Import is the one point where a session written
elsewhere enters the store, and it treats the bucket as untrusted for the
reserved area: it drops close records, `closes` links and observed markers,
refuses a close session as a source, validates link bodies and rewrites the
imported-from record. Ordinary cloud sync of the user's own sessions trusts
the bucket the way the local store is trusted, so write access to the
bucket is equivalent to write access to the store, including the power to
record an early close.

**Clock.** Instants from disk are clamped to the current time, and a
component with a future timestamp is kept and named, so a bad clock or
forged date can't release a hierarchy early and holds only its own
hierarchy until someone clears it. One unreadable
header still freezes reclamation until the named session is removed.

**Data leaving the host.** Only what koto already syncs. Anchor events
carry a digest of the process token, not the host name or pid; link hints
carry a workspace path, which session headers already carry. A read from
another agent session is recorded in the reader's anchor with the read
session, the key names and the value hashes. Read records stay on the
host with one exception: the user's own koto cloud backend, which they
enabled, syncs to their own bucket, and every other shipper keeps
`context_read` events local. niwa's pass-through puts declared variables on the
worker's command line, so it refuses names that look like credentials,
and the guide says to declare only `KOTO_DISPATCHER_ANCHOR`. The name heuristic can't
catch every secret (a `DATABASE_URL`, say); what keeps credentials out is
that the workspace owner declares exact names. No shirabe path launches a
worker from inside a tick (R32).

**Commands run on niwa's behalf.** The on-reap command has the trust level
of a hook the workspace configuration installs. It runs with a scrubbed
environment, in its own process group, killed whole at its timeout. The
close it runs is bound to the removed agent session's id as well as the
reap key, so a key that leaked to a child, or a repeated close after a
failure, can't close another agent session's anchor; and a child's hook
ignores an inherited reap key whenever it also inherited `KOTO_ANCHOR`.

**Plugin and version trust.** Once the plugin's hooks load, a plugin release
runs code at every session start; workspaces should pin the plugin version.
A koto older than this release ignores links and can remove sessions an
anchor holds. The hook needs the `koto session anchor` verbs, so an older
koto opens no anchor and the hook unsets `KOTO_ANCHOR`; the changelog says a
store shared by mixed koto versions loses the guarantee.

## Consequences

### Positive

- One rule, one engine and one reason string answer "may this go, and why"
  at every removal point and in a query anyone, shirabe included, can call.
- A finished worker's record outlives its dispatcher's need for it, with no
  caller remembering `--no-cleanup`.
- Everything works locally with koto alone; niwa adds an earlier, observed
  close, and its pieces are generic and inert without koto.
- The koto-skills hooks start loading, which also turns on today's
  /workflows rendering.
- Hierarchies follow a session through import by name.

### Negative

- Sessions with any edge, every koto parent and batch child included, now
  wait for a prune instead of leaving at their terminal tick, so stores grow
  until someone prunes.
- An unobserved agent session idle past the idle period, or an imported
  anchor, loses its hold (the owner's idle close covers only these two
  cases).
- An observed anchor whose close never lands on a host where niwa was
  removed keeps its hierarchy until someone closes or removes it; it is
  reported stale, never closed on idleness or by any other rule. This is
  the only anchor that can linger, since niwa repeats every other lost
  close, and it over-retains, the safe direction.
- One unreadable header freezes all reclamation until it is removed.
- A long-lived anchor can make one large component, and a query's cost
  grows with its component.
- The anchor protocol leans on harness behaviour that is observed rather
  than documented (env file layout, `/clear` ids, the process token).
- A session resumed after niwa observed its deletion has shells that still
  name the closed anchor; koto adds a held-by link to the successor on each
  init or tick, but finished work the agent doesn't touch again is held only
  by the closed anchor's hierarchy. Rare, since it needs a resume after a
  deletion.
- Every `koto init` and tick under an anchor may append one event to that
  anchor, throttled to one per tenth of the idle period.
- Three existing cross-session writers remain.
- A store shared by an older koto loses the guarantee: the older binary
  ignores links.

### Mitigations

- The changelog and the CLI guide say plainly that end-of-run removal no
  longer reclaims hierarchy members; `koto session hierarchy` shows why each
  is kept, and prune's reasons name it.
- The 7-day idle default keeps a weekend away inside the hold; all three
  durations are configurable.
- Every taint names the item, so the unreadable header is one
  `koto session cleanup` away; the first performance levers (a head-and-tail
  log read in `Judge`, then a lean scanner behind an equivalence test) are
  identified before any index.
- Each harness assumption has a fallback that degrades to the koto-level
  reading, never to removal, and the hand checks gate Phase 8's merge.
- koto#409, koto#411 and koto#412 track the three cross-session writers.
- Past the stale threshold (`retention.stale_after`, 30 days by default),
  prune and the hierarchy query name each stale observed anchor with the
  one command that closes it,
  `koto session anchor close --key=<marker> --harness-session=<id>`, filled
  in from the anchor, so an operator sees it and closes it by hand.
