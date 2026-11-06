---
schema: design/v1
status: Proposed
problem: |
  On the cloud backend, a command naming a session with no copy on this host
  and no state under this directory's prefix asks every other workspace
  prefix whether it holds the session, so a not-found answer costs about one
  listing per prefix the bucket has ever had, multiplied by retries on a
  degraded bucket. koto#597 saw about 2m16s for a missing name against about
  4s for an existing one, which outruns callers that probe names under a
  short timeout. The sweep can't simply go away: it is what lets koto refuse
  with session_unlocated (no_local_copy) instead of reading another
  directory's session as absent, and older koto releases keep writing to the
  same buckets.
decision: |
  Newer koto writes an empty name-first index entry,
  koto-names/<name>/<prefix>, before the first state object of every session
  copy (create, import, takeover, rename, conflict resolution, first push of
  an older copy), flagged in the sync stamp so it costs one PUT per copy, and
  never removes it from a session command. held_elsewhere reads the index
  first and confirms every hit with the listing it already makes. A miss is
  trusted only when the bucket carries koto-names/_complete, a declaration
  that only an operator command writes after an audited backfill and a 7-day
  probation, and that a re-run audit revokes on seeing an older writer's
  session. On an undeclared bucket a miss runs today's sweep unchanged. The
  remaining listings keep today's retry policy.
rationale: |
  Older releases write only under <prefix>/<name>/, which no listing can
  query across prefixes, so no answer is both exact and bounded while they
  are active; the only bounded answer is a name-first key whose silence
  someone chooses to trust. Making that an explicit, audited, reversible
  owner decision keeps every reply on buckets nobody declares and bounds
  not-found at 3 requests where someone does. Capping retries, caching prefix
  listings per host, and listing the whole bucket were rejected because each
  either changes a reply or still grows with the prefix count; trusting the
  index after a one-time backfill was rejected because it silently turns
  no_local_copy into not-found for sessions older releases create later.
decision_provenance: inline-resolved
---

# DESIGN: Cloud missing-session lookup

## Status

Proposed

## Context and Problem Statement

On the cloud backend, `koto status <name>` for a session that doesn't exist
takes far longer than for one that does. koto#597 reports about 2m16s for a
missing name against about 4s for an existing one (koto 0.34.0, a bucket
holding many sessions, a degraded bucket at the time). A caller that probes
several names under a short timeout can't finish.

The time goes into one place. Before it reads a session, every command that
names one (`koto status`, `koto context get`, `koto context exists` and the
rest) passes through `CloudBackend::check_located` in
`src/session/cloud.rs`, by way of `locate_or_serve` in `src/cli/mod.rs`. For
a name with no copy on this host and no state object under this directory's
prefix, `check_located` calls `held_elsewhere`, which asks whether any other
workspace prefix holds the session:

1. one listing of the bucket's top level, split at `/`, which names every
   workspace prefix;
2. one listing of `<prefix>/<name>/` for every other prefix,
   `HELD_ELSEWHERE_FANOUT` (64) at a time, each asked up to `LIST_ATTEMPTS`
   (3) times while it fails with a transient error, on top of rust-s3's own
   retry.

Every workspace directory is its own prefix (`repo_id` hashes the canonical
path), so every clone, instance and worktree that ever pushed a session adds
one. The sweep's cost is about `ceil(N / 64)` round trips for `N` prefixes on
a healthy bucket, and on a degraded one each of those round trips can stretch
to several timeouts and backoffs. The prefix count only grows, so the
not-found answer gets slower for every user of a long-lived bucket, whatever
the state of the bucket.

The sweep isn't there by accident. It is what lets a command named from the
wrong directory refuse with `session_unlocated` and reason `no_local_copy`
(koto#518), instead of reading a session another directory holds as absent
and letting a caller create a second session of the same name. koto#526 made
the sweep parallel; koto#528 and koto#531 made a listing that fails read as
"can't tell" rather than as absent. Any fix has to keep all of that.

### The replies that must not change

For a name with no copy on this host, a command's reply today is one of:

- the session is read from this directory's prefix (state is there);
- `session_unlocated`, reason `no_local_copy`, when another directory's prefix
  holds the state object
  (`plugins/koto-skills/skills/koto-user/references/error-handling.md`, the
  error table and the `session_unlocated` reason table);
- the command's own not-found answer (`koto status` exits 2), when no prefix
  holds it;
- a "koto can't tell" error, nothing read, when the bucket answers but a
  listing still fails after its retries;
- not-found, when the bucket can't be reached at all (a host with no network
  reads a fresh name as absent, as before koto#518).

`tests/cloud_bound_dir_test.rs` pins these: among others
`a_host_with_no_local_copy_refuses_from_another_prefix`,
`a_session_never_used_anywhere_reads_as_absent`,
`a_missing_name_s_probe_asks_the_other_prefixes_at_once`,
`a_failed_listing_of_the_holding_prefix_is_not_read_as_absent`,
`a_throttled_listing_that_clears_within_the_retries_still_answers` and
`an_unreachable_bucket_still_reads_a_never_used_name_as_absent`. Callers
outside koto parse the `session_unlocated` body by `code` and `reason`, so a
change of reply in any of these cases is a breaking change.

### Scope

In scope: how `check_located` learns, for a name with no local copy and no
state under this prefix, whether another prefix holds it, and what that costs
in requests as the bucket grows. Out of scope: the dashboard's whole-bucket
reads, the continues-link and back-reference sweeps (they have their own
issues), and any change to a reply.

## Decision Drivers

- **Same reply in every case.** Each bucket state above keeps its reply,
  `session_unlocated` / `no_local_copy` included. An approach that is faster
  only by answering some of these cases differently is out.
- **Not-found stops growing with the prefix count.** On a healthy bucket the
  answer for a missing name costs a bounded number of requests, not one per
  workspace prefix.
- **A degraded bucket doesn't multiply the cost by the prefix count.** Retries
  are paid per request asked, so fewer requests is the lever, not shorter
  retries.
- **Mixed versions keep working.** Hosts on older koto releases keep writing
  sessions to the same bucket; nothing they write may be read wrongly by a
  newer host, and nothing a newer host writes may confuse an older one.
- **Cheap on the write path.** Session creation and every push already sit in
  front of the command's work; anything added there should be one request at
  most, and only once per session copy.
- **Bucket text is untrusted.** Keys and names read from the bucket are
  validated before koto builds a key from them, as koto#599 did for the
  dashboard.

## Considered Options

### Decision 1: Where check_located learns whether another prefix holds a name

Delegated to /decision.

Today the answer comes from asking every other prefix. For `N` other
prefixes and `d = ceil((N + 1) / 1000)` pages of the top-level listing, a
not-found costs `1 + d + N` requests in about `1 + d + ceil(N / 64)` round
trips on a healthy bucket: 302 requests and about 7 round trips at
`N = 300`. On a degraded bucket each listing is up to three asks of up to two
HTTP requests, so the worst case is about `6(1 + d + N)` requests, around
1,800 at `N = 300`. Every listing is also another chance of a "can't tell"
reply: with a per-listing failure rate `p`, the chance is
`1 - (1 - p)^N`.

One fact decides the shape of any answer. The question "which prefixes hold
`<p>/<name>/koto-<name>.state.jsonl`?" has its unknown in the middle of the
key, and an S3 listing narrows only by a leading prefix. Every key an older
release writes lives under `<p>/<name>/`, nothing it writes is name-first,
and nothing records its version. So while older releases still write to a
bucket, no lookup is both exact and bounded. A bounded answer has to rest on
a name-first key that newer releases maintain, and the real question is when
koto may trust that key's silence.

Two facts make a new key family safe. The only code that lists the bucket's
top level is `held_elsewhere` and `remote_feed::workspaces`
(`src/session/remote_feed.rs`), and both drop every common prefix that isn't
16 lowercase hex (`is_workspace_prefix`); releases older than `held_elsewhere`
never list the top level at all. And session names start with an ASCII
letter and contain no `/` (`validate_session_id`), so an object such as
`koto-names/_complete` can't collide with a name's entries.

Key assumptions:

- Older releases (0.34.x and earlier) stay in use against shared buckets for
  months, including CI images that reuse a checkout path and so an existing
  prefix. If buckets turn out to be single-version, the declaration below can
  follow the first backfill at once and the probation adds nothing.
- Every S3-compatible endpoint koto supports accepts a PUT under a second
  top-level key family with the credentials it already uses. If not, hosts
  whose credentials only allow `<prefix>/*` warn on undeclared buckets and
  can't create sessions on declared ones.
- `LastModified` on a state object is reliable enough for the audit to tell
  "created after the audit started" from "created before".
- rust-s3's default single retry and 60-second request timeout stay as they
  are on the sync path.

#### Chosen: A per-name index whose misses are trusted only on a declared bucket

A new top-level key family, `koto-names/<name>/<prefix>`, holds one empty
object per prefix that holds a session of that name. A newer host writes the
entry before the first state object of a session copy goes up (Decision 2
has the exact rule). Lookups change only inside `held_elsewhere`:

1. In parallel: list `koto-names/<name>/`, read the declaration
   `koto-names/_complete`, and (unless this host last saw the bucket
   declared) make today's top-level listing.
2. Each entry read back is stripped of `koto-names/<name>/`, must pass
   `is_workspace_prefix`, and isn't the caller's own prefix, all before any
   key is built from it. Each remaining prefix is confirmed with the listing
   `held_elsewhere` already makes (`<p>/<name>/`, compared with the state
   key), 64 at a time. A confirmed hit is `no_local_copy`. A confirming
   listing that still fails is "can't tell": the same listing, failing the
   same way, that the sweep would have made.
3. On a declared bucket, a miss is not-found, and an index listing that
   fails after its retries is "can't tell".
4. On an undeclared bucket, a miss runs today's sweep unchanged, skipping
   the prefixes already confirmed absent. Index and declaration failures are
   ignored on this path, so the index never adds a "can't tell" to an
   undeclared bucket, and a failed top-level listing is "can't tell" as it is
   today (Decision 4 settles what that means for a hit already confirmed).

The declaration is a small JSON object that only an explicit operator
command writes, never a lookup or a push. The command has two steps. A
re-runnable backfill-and-audit lists every prefix's sessions (plus one extra
listing for a session named `ctx`, which a `/ctx/` delimiter hides), writes
missing entries, and records when it started. Declare refuses unless an
audit finished at least a probation window ago (7 days by default, with an
override for a bucket its owner knows is single-version), every listing was
answered, and no state object without an entry has a `LastModified` after
that audit started. A newer writer always writes the entry first, so such an
object proves an older release created a session during probation, and the
refusal names it. Re-running the audit later is the check: when it finds an
entry-less state object newer than the declaration, it deletes the
declaration, which sends every newer host back to the exact sweep. A revoke
command does the same by hand.

Request counts for a not-found on a name with no local copy, counting the
own-prefix probe that `located_probe` makes before `held_elsewhere` runs:

| Case | Requests | Round trips | At N = 300 |
|---|---|---|---|
| Today, healthy | 1 + d + N | 1 + d + ceil(N/64) | 302, about 7 |
| Today, degraded worst | about 6(1 + d + N) | ceil(N/64) waves of the slowest | about 1,812 |
| Undeclared, healthy | 3 + d + N | same as today | 304, about 7 |
| Undeclared, degraded worst | about 6(3 + d + N) | same as today | about 1,824 |
| Declared, healthy | 3 | 2 | 3 |
| Declared, degraded worst | about 18 | 2, each stretched by retries | about 18 |
| Hit through the index, either mode | 4, plus the top-level listing on an undeclared bucket | 3 | 4 or 5 |

A name with `K` leftover entries adds `K` confirming listings on a declared
bucket, in `ceil(K / 64)` round trips, up to a cap past which the lookup
runs the exact sweep instead. The unreachable-bucket case is
unchanged: the own-prefix probe fails without an answer and nothing else is
asked.

This keeps the hard constraint by default. Undeclared, it's today's
behavior with faster positives for sessions newer releases wrote, and every
failure to read the new keys falls back to the exact answer. Declared, a
not-found costs 3 requests in 2 round trips whatever `N` is, and the chance
of a "can't tell" falls from `1 - (1 - p)^N` to roughly `1 - (1 - p)^2`.
Trusting the index's silence is the trade the mixed-version constraint
forces; this option makes the bucket's owner make it on purpose, checks it
against the bucket first, and reverses it with one delete. The accepted cost
is that koto#597 isn't fixed on a bucket nobody declares: every option that
fixes it by default does so by silently changing `no_local_copy` for
sessions older releases create, which is the koto#518 hazard delivered to
users who never opted in.

#### Alternatives Considered

**Per-name index, trusted after a one-time backfill**: the same key family
and write path, with misses trusted as soon as a backfill has run. A
not-found costs 2 requests healthy and about 12 degraded at any `N`.
Rejected because a session an older release creates after the backfill,
under a prefix that already exists, has no entry and reads not-found instead
of `no_local_copy`, and nothing in the bucket ever shows the gap. An
automatic backfill by the first newer host would also make the pinned test
`a_missing_name_s_probe_asks_the_other_prefixes_at_once` fail, since it
plants exactly that state. "After the backfill" needs a completion marker
anyway, which is the chosen option without its audit or its way back.

**Index for positives only, sweep for every miss**: never trust the index's
silence. Exact in every bucket state, and `no_local_copy` in about 4
requests. Rejected as the whole answer because a not-found keeps costing
`3 + d + N` requests and keeps growing with the prefix count. It survives as
the chosen option's undeclared mode.

**Cap or deadline the sweep's retries**: no new keys and exact against older
writers. But a healthy not-found still costs `2 + N` requests and grows with
`N`, and on a degraded bucket it changes replies. With one ask per listing,
a listing that would clear on its second or third ask becomes "can't tell",
which breaks `a_throttled_listing_that_clears_within_the_retries_still_answers`.
With two asks, the can't-tell rate at `N = 300` with one request in five
throttled rises from about 1.9% to about 38%. A deadline under about 121
seconds fires on the koto#597 shape (one listing stalled by two 60-second
request timeouts). Rejected because it changes replies and isn't bounded.

**Per-host TTL cache of prefix listings**: cache each prefix's session-name
listing on this host for a TTL, so repeat probes within the TTL cost one
request. Rejected because the first probe still pays today's `2 + N`, which
is the case koto#597 reports, and because no cached "not here" is exact:
nothing an older release writes at prefix level changes when a session is
added, so within the TTL a session created elsewhere reads not-found instead
of `no_local_copy`, and a deleted one reads the other way round. The
existing cross-process check window (koto#439) is a different thing: it
never covers the missing-name path, and with it turned on there, two pinned
tests change reply.

**Flat whole-bucket listing with no delimiter**: exact against every writer
with no new keys. Rejected because it costs `ceil(objects / 1000)` strictly
sequential pages, 61 for a bucket of 300 prefixes holding 20 sessions of 10
objects each, against today's 302 requests in about 7 round trips; up to six
requests a page on a degraded bucket; and `list_retrying` restarts from the
first page after a failure. Its strongest variant, a `start-after` scan over
parallel chunks of the prefix list, is exact and never costs more than today,
but still grows with `N`, can't be sent by rust-s3's `Bucket::list`, isn't
modeled by the test fake, and is hidden from a session named `ctx` by the
delimiter. It's a possible follow-up for undeclared buckets, not part of
this design.

### Decision 2: How the index stays exact against sessions written without it

Resolved inline.

Decision 1 settles most of this with evidence: the key shape, the
bucket-wide coverage rule, the backfill, and the import path. Four parts
were left loose and were checked against `src/session/cloud.rs`. First,
which writes put a state object under a new key: besides create and import,
`relocate` copies a renamed session's objects and calls
`sync_push_state(to)`, `relocate_checked` leaves the new name's state to the
caller's push, `force_push_session` (conflict resolution, and
`adopt_local_copy` through it) and `reconcile_child`'s accepted-local branch
reach `put_state_over` through `push_local_state_bytes`, and the first push
of a copy an older release created goes through `put_state`'s `If-Match`
branch rather than `If-None-Match: *`. Second, deletes: with no conditional
delete, removing an entry races a re-create of the same name under the same
prefix. Third, concurrent creation and readers racing a create. Fourth, how
the writing host remembers that a copy is indexed.

Key assumptions:

- The bucket gives read-after-write consistency for PUT, DELETE and LIST, as
  today's sweep already assumes.
- `Stamp` stays deserialized without `deny_unknown_fields`, as it is today,
  so older releases ignore a new field.
- Names are rarely reused across many prefixes and deleted each time; if one
  is, its lookups pay one confirming listing per leftover entry until the
  owner prunes.

#### Chosen: An entry before every state PUT a copy hasn't indexed, and no entry deletes outside the audit

One check covers every write path. Before a state object goes up, if the
stamp for that exact state key doesn't carry `indexed: true`, koto PUTs
`koto-names/<name>/<home prefix>` first. The check sits in `put_state` and
`put_state_over`, which between them carry create, the 501 fallback, the
first newer push of an older copy, rename, conflict resolution and
adoption, and at the head of `push_staged`, which runs before any stamp
exists and so always writes the entry. `Stamp` is already filtered on
`state_key`, which embeds the home prefix and the name, so a renamed or
re-homed copy starts unmarked and writes its new entry. A failed entry PUT
stops the state PUT and leaves the push owed, as a failed state PUT does
today. The body is empty, so the PUT is idempotent and needs no
precondition.

`Stamp` gains `indexed: bool` (serde default `false`), carried from a
previous stamp with the same `state_key` the way `manifest_etag` is carried
now. A stamp rebuilt from a pull or a probe for another key drops it, and so
does an older binary that rewrites the stamp; either costs one idempotent
PUT on the next push, never a missing entry. So the write costs at most one
request per session copy per host.

Deletes, renames, imports and failed-import rollbacks remove nothing under
`koto-names/`: `sync_delete_session`, `delete_remote_session_checked`,
`delete_migrated_source`, `relocate` and `push_staged`'s `take_back` stay as
they are. A leftover entry is a hit whose confirming listing turns it into
"not held", so it costs one listing on lookups of that one name and never
changes a reply. Leftovers are bounded by how many prefixes ever held that
name, not by the bucket's prefix count, and they help in one case: an older
release that later re-creates the name under that prefix is found through
the stale entry.

Pruning belongs to the audit, and only while the bucket is undeclared. On a
declared bucket the audit reports stale entries and deletes them only when
asked, revoking the declaration first. The prune's own race with a create
(the prune lists no state, a creator PUTs the entry, the prune deletes it,
the creator PUTs the state) can then only leave an entry-less state object
newer than the audit's start, which the declare step already refuses on.
The race ends in a refused declaration and a re-run, never a wrong reply.

Concurrent creation needs nothing new. Two hosts creating one name under one
prefix PUT the same empty entry, and the `If-None-Match: *` state PUT picks
the winner as it does now. A reader that lists the index between a
creator's entry PUT and its state PUT gets a hit whose confirmation finds no
state, and answers not-found; one that lists before the entry PUT answers
not-found too. Both answers are true at an instant inside the call, which is
what today's sweep gives a reader racing a create. Because the entry always
precedes the state and no session command removes it, every state object a
newer release wrote has its entry from the moment the object exists.

#### Alternatives Considered

**A best-effort entry delete after the state delete**: what Decision 1
first proposed; keeps the index small. Rejected because a delete racing a
re-create of the same name under the same prefix can land its entry DELETE
after the creator's state PUT, leaving a state object with no entry that a
declared bucket reads as not-found where today's sweep answers
`no_local_copy`.

**An entry PUT both before and after the state PUT**: closes most of the
delete race while keeping deletes. Rejected because it costs two requests
per create against a budget of one, and a delete that lands after the
second PUT still needs a re-list on the delete side to be safe.

**The entry after the state PUT**: one request, and nothing written for a
create that fails. Rejected because a host that crashes or loses the bucket
between the two leaves an entry-less state object that a declared bucket
then misses.

**Per-prefix coverage markers**: mark each prefix as fully indexed once it's
audited, and trust misses per prefix, giving a partial speed-up on a mixed
bucket without anyone declaring anything. Rejected because an older release
can create a session under an audited prefix (a CI image reusing a checkout
path), falsifying the marker with nothing in the bucket to show it, and
because the reader would still list every marker, which grows with the
prefix count.

**An entry body naming the session id**: lets a confirmation tell a stale
entry from a live one without a listing. Rejected because the confirming
listing is needed anyway to see the state object, and a body adds a GET.

### Decision 3: Retry and timeout policy for the listings still swept

Delegated to /decision.

Every listing `held_elsewhere` makes goes through `list_retrying`: up to
`LIST_ATTEMPTS` (3) asks while the failure is transient (429, 500, 502, 503,
504, or a transport error, which includes a timeout), with 250 ms and then
500 ms between asks. Under each ask sits rust-s3's default behavior, which
`create_bucket` doesn't change: a 60-second request timeout and one retry a
second later. So one listing is up to six requests, about 3.75 seconds on a
bucket that throttles every time and about six minutes on one that accepts
connections and never answers. The policy only ever runs against a bucket
that has already answered the own-prefix probe, so its one choice is where
the line falls between "answered" and "can't tell".

Key assumptions:

- The fast path, its own reads and their failure handling are Decision 1's
  and Decision 2's; this one covers the listings still swept, and the index
  and confirming listings use the same `list_retrying`.
- rust-s3's defaults (one retry, 60-second timeout) stay as they are in
  0.37.1. koto sets neither, so a crate upgrade that changed them would move
  the line.
- Cross-validation corrected one assumption. The decider assumed the swept
  set would shrink as hosts upgrade. Under Decision 1's bucket-wide coverage
  it doesn't: a miss on an undeclared bucket sweeps every other prefix, and
  a miss on a declared one sweeps nothing. The choice below doesn't depend
  on the swept set's size, so it stands.

#### Chosen: Keep today's per-listing policy unchanged

Every listing that remains goes through `list_retrying` exactly as now:
`LIST_ATTEMPTS` 3, `LIST_BACKOFF` 250 ms doubling, the same `is_transient`
classification, rust-s3's defaults under each ask, `HELD_ELSEWHERE_FANOUT`
64 in flight, and the existing early stop on the first hit or failure. The
top-level listing keeps its policy too. It's the only option that keeps
every reply in every bucket state; the fast path changes how many listings
are asked, not how any one of them is asked, which is the lever the drivers
name. The trade accepted is that an undeclared bucket answering slowly can
still take minutes to say not-found, as it does today.

#### Alternatives Considered

**A whole-sweep deadline**: a wall-clock budget, with listings still
unanswered at the deadline read as "can't tell". Rejected because a bucket
that answers every listing, only slowly, would go from answered to "can't
tell": the same bucket state with a different reply.

**A shared retry budget across the sweep**: each listing gets one ask free
and repeat asks come from a pool. Rejected because a broad throttle that
every listing would clear on its own retries drains the pool and reads
"can't tell", and one listing's outcome would depend on how many others
were throttled, which no test can pin.

**Fewer attempts or a shorter request timeout**: `LIST_ATTEMPTS` 1 or 2, or
a 10-second timeout with no ask after a timeout, as the dashboard reader
uses. Rejected because a listing that clears on its third ask, or answers
after more than 10 seconds, would become "can't tell", and one attempt fails
`a_throttled_listing_that_clears_within_the_retries_still_answers`. The
dashboard can afford it because it can show a prefix as unreachable;
`held_elsewhere` has only "answered" or "can't tell".

**Adaptive fan-out on throttling**: halve the concurrency on a 429 or a 503
`SlowDown` and grow it back on success. Rejected because it changes replies
in the other direction (a bucket that throttles above some concurrency,
"can't tell" today, would read answered) and makes the degraded case slower.

### Decision 4: A confirmed hit when the top-level listing fails on an undeclared bucket

Resolved inline.

On an undeclared bucket the index listing, the declaration read and today's
top-level listing go out together. If the index names a prefix whose
confirming listing finds the state object, but the top-level listing failed
after its retries, koto knows the true answer (`no_local_copy`) while today's
code, which needs the top-level listing before it asks anything else, would
say "can't tell". Decision 1 left that case open.

#### Chosen: Report "can't tell", as today

On an undeclared bucket, a failed top-level listing is "can't tell" whatever
the index found. The top-level listing returns in the first round trip and
the confirmation in the second, so waiting for it costs nothing. This keeps
the claim simple and checkable: on a bucket nobody has declared, the reply in
every bucket state, with every pattern of listing failures, is the one
today's code gives. `a_failed_listing_of_the_holding_prefix_is_not_read_as_absent`
pins the case without an entry; a new test pins it with one.

#### Alternatives Considered

**Report the confirmed hit as `no_local_copy`**: the answer is true, and
today's sweep already lets a found prefix win over a failed listing of
another prefix. Rejected because it makes the undeclared bucket's replies
differ from today's in one bucket state, which weakens the one guarantee
that lets this design ship without a migration; the gain is a correct
answer in a case that only arises while the bucket is failing listings
anyway.

### Decision 5: Whether a host remembers the declaration between commands

Resolved inline.

Every koto command is its own process, so without a memo each lookup would
read the declaration and, not knowing the answer in advance, would also
start the top-level listing in case the bucket turns out undeclared. That's
4 requests on a declared bucket instead of 3. Worse, on a degraded declared
bucket a declaration GET that fails would send the lookup back to the full
sweep, the very case koto#597 reports.

#### Chosen: A host-wide memo that shapes which requests start, trusted for at most the check window

A small file beside the local session store records, per endpoint and
bucket, the last declaration answer and when it was read. "Declared" at any
age means the lookup skips the top-level listing; "declared" younger than
`check_window_secs` (default 5) also skips the GET. A GET that fails without
an answer keeps the last value for the lookup, so a degraded declared bucket
stays on the bounded path, but only for 10 minutes after the last read that
did answer; past that, lookups fall back to the exact sweep until a read
answers again, so a host that can't read the declaration can't keep acting
on it long after a revoke. A 404 or an invalid body clears it at once, and a
lookup that finds the declaration gone starts the top-level listing then, at
one round trip more. Write paths never act on a carried value. The window
reuses the setting the guide already documents for sharing checks between
processes, so a host can turn the memo's trust off with `0`.

#### Alternatives Considered

**No memo; read the declaration on every lookup**: simpler, and never acts
on an old answer. Rejected because a declared bucket pays a fourth request
on every lookup, and a degraded one falls back to the sweep whenever the
GET fails, giving back most of the gain exactly where koto#597 hurts.

**Cache the declaration for a long TTL (minutes)**: fewer GETs. Rejected
because a revoke would take that long to reach hosts, and the revoke is the
safety valve for a declaration an older writer has made false.

## Decision Outcome

The fix gives the bucket a name-first index and keeps the sweep as the
answer of record until the bucket's owner says the index can stand in for
it. Every newer koto writes an empty object, `koto-names/<name>/<prefix>`,
before the first state object of a session copy goes up, on every path that
puts one under a new key: create, import, takeover, rename, conflict
resolution, and the first newer push of a copy an older release made. A
flag in the sync stamp keeps that to one PUT per copy per host. Nothing a
session command does removes an entry, so a state object a newer release
wrote always has one.

`held_elsewhere` reads the index first. Hits are never trusted on their own:
each is confirmed by the same `<prefix>/<name>/` listing the sweep makes
today, so a stale or forged entry costs a listing and changes nothing. What
the index's silence means depends on `koto-names/_complete`. Absent, a miss
runs today's sweep, so on an undeclared bucket every reply comes from
today's code, and a failed top-level listing is still "can't tell". Present,
a miss is not-found after 3 requests in 2 round trips whatever the prefix
count, and on a degraded bucket after about 18 requests at worst instead of
about six per prefix. The declaration comes only from an operator command
that backfills entries, waits out a probation window, and refuses while it
can see a state object an older release wrote; the same audit, re-run,
revokes it when it finds one, and the stray reply that an older writer can
cause on a declared bucket lasts only until then. The listings that remain
keep today's retry policy, so no reply moves because a listing was asked
fewer times.

### How each reply is kept

For a name with no copy on this host, and in every bucket state:

- **Read from this prefix.** The own-prefix probe in `located_probe` runs
  first and finds the state object, so `held_elsewhere` isn't called.
  Unchanged.
- **`session_unlocated`, reason `no_local_copy`.** Another prefix holds the
  state. If a newer release wrote or pushed it, its entry exists and the
  confirming listing finds it. If only older releases have touched it, there
  is no entry: an undeclared bucket finds it with today's sweep; a declared
  bucket misses it, which is the one reply change the design accepts, made
  only by an owner's declaration and undone by the next audit.
- **Not-found.** No prefix holds the state. An undeclared bucket proves it
  with today's sweep; a declared bucket with an index listing that holds
  no live entry.
- **"Can't tell".** A listing the answer depends on fails after its retries:
  the own-prefix probe (unchanged), a confirming listing (the same listing
  the sweep makes), the top-level listing or a sweep listing on an
  undeclared bucket (unchanged), or the index listing on a declared bucket.
- **Not-found when the bucket is unreachable.** The own-prefix probe fails
  without an answer and the command reads the name as absent, before any of
  the new code runs. Unchanged.

### Why the combination works

Decision 1 makes not-found bounded wherever the owner opts in and leaves
everywhere else exactly as it is, which is the only way to honor both the
reply constraint and older writers. Decision 2 is what makes the opt-in
safe to grant: with the entry first and never removed by a session command,
"a newer release wrote this state object" implies "its entry exists", so the
declaration only has to vouch for older writers, which is exactly what the
audit's probation measures. Decision 3 keeps the retry line where koto#528
and koto#531 drew it; the request count, not the retry count, is what the
index cuts. Decisions 4 and 5 are the two seams: the first keeps undeclared
buckets byte-for-byte on today's replies even when the index knows better,
and the second keeps a degraded declared bucket on the bounded path when the
declaration itself is slow to read.

## Solution Architecture

### Overview

The change adds one module and touches four places in `CloudBackend`: the
state-write helpers, `held_elsewhere`, the stamp, and a new operator
command. Replies are produced by the same code as today (`check_located`'s
error text and the `Unlocated` body), so no caller sees a new shape.

### Components

**`src/session/name_index.rs` (new).** Key building and parsing for the
index, and the declaration type:

- `ENTRY_ROOT = "koto-names/"`, `DECLARATION_KEY = "koto-names/_complete"`.
- `entry_key(name, prefix)`, which takes a name that already passed
  `validate_session_id` and a prefix that passed `is_workspace_prefix`, and
  refuses anything else.
- `listing_prefix(name)` for `koto-names/<name>/`, always with the trailing
  `/`, so a listing for `foo` never returns entries for `foobar`.
- `prefixes_from(pages, name, own)`, which strips `koto-names/<name>/` from
  each listed key, keeps only results that pass `is_workspace_prefix` with
  nothing after them, drops `own`, and de-duplicates. Anything else in the
  listing is ignored.
- `Declaration`, read with a 4 KiB cap on the bytes read (a `Content-Length`
  over the cap, or a body that runs past it, stops the read) and parsed
  strictly: a body that is larger, isn't JSON, or lacks a required field
  reads as undeclared, and `version` is parsed as an integer and the
  timestamps as timestamps rather than kept as text. Fields: `version` (1),
  `declared_at`, `machine` (the machine name koto already records on
  session events), `first_release` (informational), and `audit`
  (`started_at`, `finished_at`, `prefixes`, `sessions`, `entries_written`,
  `entries_pruned`).
- `MAX_ENTRIES_PER_LOOKUP` (256). A lookup stops reading the index listing
  at that many entries for one name (it stops paginating there, before any
  confirmation starts), and
  treats the name as polluted: on a declared bucket it runs the exact sweep
  instead of confirming entries one by one. One writer creating many entries
  for a name can then make that name's lookups no slower than today's, and
  can't make them slower still.

**The write path in `src/session/cloud.rs`.** A helper,
`ensure_indexed(id) -> anyhow::Result<bool>`, PUTs `entry_key(id, home)`
when `self.stamp(id)` doesn't say `indexed`, and is called at the top of
`put_state` and `put_state_over` before the state PUT. It returns whether
the entry is now known to be in the bucket (written now, or already marked),
and the caller passes that to `record_synced`. `push_staged` writes
the entry for its target as its first upload, before the state object, and
leaves it in place if it rolls back with `take_back`. A transient failure is
returned like a failed state PUT, so `sync_push_state` leaves the push owed
and `strict_push_state` fails. A refusal is recognized the way the 501
fallback is, as `S3Error::HttpFailWithBody(403, _)`, and reads the
declaration once: absent, the
host warns that it can't write the index and goes on with the push, leaving
the entry owed; present or unreadable, the push fails with an error that
names the missing permission, because a declared bucket would otherwise miss
this session.

**`src/session/sync_stamp.rs`.** `Stamp` gains `indexed: bool` with
`#[serde(default)]`. `Stamp::for_bytes` carries it from a previous stamp
with the same `state_key`, and `record_synced` takes the `bool` that
`ensure_indexed` returned and sets `indexed` from it. A push that went on
after a refused entry write (an undeclared bucket) stamps `indexed: false`,
so the next push tries again.

**The read path: `held_elsewhere`.** Rewritten around three steps, keeping
its signature (`anyhow::Result<bool>`) and its callers:

1. Start, in parallel through `list_retrying`: the index listing; the
   declaration GET unless the host's memo says "declared" and is younger
   than `check_window_secs`; and the top-level listing unless the memo says
   "declared" at any age.
2. Confirm every prefix from `prefixes_from` with the existing `holds`
   closure, `HELD_ELSEWHERE_FANOUT` at a time, stopping at the first hit.
3. Decide. Declared: hit is `Ok(true)`, no hit is `Ok(false)`, and an index
   listing or confirming listing that failed is `Err`. Undeclared, or a
   declaration that couldn't be read with no memo to fall back on: a failed
   top-level listing is `Err`; otherwise a confirmed hit is `Ok(true)`, and
   a miss runs today's sweep over the top-level listing's prefixes minus
   those already confirmed absent. Whenever the lookup ends up needing the
   top-level listing it didn't start in step 1 (the memo said "declared" but
   the GET now reads absent, a carried "declared" has passed its 10-minute
   limit while the GET keeps failing, or the name hit
   `MAX_ENTRIES_PER_LOOKUP`), it starts it then, at one more round trip on
   that one lookup.

**The declaration memo.** A host-wide file beside the local session store,
`cloud-index.json`, keyed by endpoint and bucket, written with the same
atomic replace the stamps use. It records the last declaration answer and
when it was read. A 404 or an invalid body clears "declared" at once. A read
that fails without an answer keeps the last value for lookups, so a degraded
bucket doesn't throw a declared host back onto the sweep, for at most 10
minutes after the last answered read (Decision 5); write paths never act on
that carried value. "Not declared" can be remembered freely, since it only
leads to the exact fallback. A memo file that can't be parsed, or whose key
doesn't match this endpoint and bucket, is ignored. The file holds no
secrets and gets the same user-only mode as the stamps.

**The operator command: `koto session index`.** Subcommands:

- `audit [--prune]`: lists the top level, then each workspace prefix with a
  `/` delimiter to name its sessions, plus `<p>/ctx/` when that common
  prefix appears; lists each session prefix's state object with its
  `LastModified`; lists `koto-names/` to compare. Writes missing entries.
  With `--prune` on an undeclared bucket, deletes entries whose prefix holds
  no state object for that name; on a declared bucket `--prune` refuses
  unless `--revoke` is also given. On a declared bucket, an entry-less state
  object newer than the declaration's audit start revokes the declaration
  and is named in the output. Prints the counts and writes them to
  `koto-names/_audit` (JSON, same 4 KiB cap and strict parse as the
  declaration), so `declare` can run on any machine. Every run replaces that
  record, so re-running the audit restarts the probation clock.
- `declare [--no-probation]`: walks the bucket again as the audit does, so
  the audit record says when the audit ran and is never the proof that the
  bucket is covered. Refuses unless the audit finished at least the
  probation window ago (7 days), answered every listing, and no entry-less
  state object is newer than its start, and then PUTs the declaration. Both
  times are the bucket's: the probation is measured from the
  `LastModified` of `koto-names/_audit` and compared with the state objects'
  `LastModified`, so a skewed local clock can't shorten it.
- `revoke`: deletes the declaration.

Every session name and prefix the audit and declare walks find in the bucket
passes `validate_session_id` and `is_workspace_prefix` before a key is built
from it, the `ctx` case included; anything that fails is skipped and counted
as skipped, never written, pruned or used to abort the run. Bucket text the
commands print (a skipped key, a declaration field) is escaped and cut to a
fixed length, so a key holding control characters can't rewrite what the
operator sees before confirming. The commands run only against the
configured bucket with the user's own credentials, print what they will
write, and need `--yes` to write without a prompt, like `koto workspace
prune`; the prompts for `--no-probation` and `--prune --revoke` say in one
line what they give up.

### Key interfaces

```rust
// src/session/name_index.rs
pub const ENTRY_ROOT: &str = "koto-names/";
pub const DECLARATION_KEY: &str = "koto-names/_complete";
pub fn entry_key(name: &str, prefix: &str) -> anyhow::Result<String>;
pub fn listing_prefix(name: &str) -> anyhow::Result<String>;
pub fn prefixes_from(pages: &[ListBucketResult], name: &str, own: &str) -> Vec<String>;
pub struct Declaration { /* version, declared_at, machine, first_release, audit */ }
pub fn parse_declaration(bytes: &[u8]) -> Option<Declaration>; // None = undeclared

// src/session/sync_stamp.rs
pub struct Stamp { /* existing fields */ #[serde(default)] pub indexed: bool }
```

Bucket layout added beside the workspace prefixes:

```
koto-names/
  _complete                       declaration (JSON, written only by `declare`)
  _audit                          last audit's record (JSON, written only by `audit`)
  <name>/
    <16-hex prefix>               empty; one per prefix that held <name>
```

### Data flow

A create on a newer host: `koto init` creates the local session, the first
push calls `put_state`, `ensure_indexed` PUTs `koto-names/<name>/<p>`, the
`If-None-Match: *` state PUT goes up, and `record_synced` stamps the copy
`indexed`. Later pushes skip the entry.

A lookup of a missing name on a declared bucket: `check_located` finds no
local copy, `located_probe` lists `<own>/<name>/` and finds no state,
`held_elsewhere` lists `koto-names/<name>/` and reads the declaration in one
round trip, finds no live entry, and answers `Ok(false)`; the command gives
its not-found reply.

The same lookup on an undeclared bucket: the index listing, the declaration
GET and the top-level listing go out together; the declaration reads 404;
the sweep runs over every other prefix as today.

## Implementation Approach

### Phase 1: Index keys and the write path

Add `name_index.rs`, `Stamp::indexed`, and `ensure_indexed` in `put_state`,
`put_state_over` and `push_staged`, with the full 403 handling (read the
declaration once; absent, warn and push with the entry owed; present or
unreadable, fail the push). No command writes a declaration yet, so a host
whose credentials can't write under `koto-names/` only warns. Nothing reads
the index yet, so every lookup is unchanged; this phase can ship alone and
starts populating buckets as hosts upgrade.

Deliverables:
- `src/session/name_index.rs` and its unit tests (key building refuses
  invalid names and prefixes; `prefixes_from` drops malformed, foreign and
  own entries).
- Changes to `src/session/cloud.rs` and `src/session/sync_stamp.rs`.
- The permissions hosts now need under `koto-names/`, in
  `docs/guides/cloud-sync-setup.md`.
- Tests in `tests/cloud_bound_dir_test.rs` or a new
  `tests/cloud_name_index_test.rs`:
  `a_create_writes_the_name_entry_before_the_state_object`,
  `a_second_push_writes_no_name_entry`,
  `a_rename_writes_the_new_name_s_entry_and_keeps_the_old`,
  `an_import_writes_the_target_s_entry_first`,
  `a_first_push_of_an_older_copy_writes_its_entry`,
  `a_failed_entry_put_leaves_the_push_owed`,
  `a_refused_entry_put_warns_and_pushes_on_an_undeclared_bucket`,
  `a_delete_leaves_the_name_entry`.
- Existing tests that list every PUT of a push or an import (for example in
  `tests/cloud_summary_retry_test.rs` and `tests/import_keeps_source_test.rs`)
  are checked and, where they compare the full list, taught to skip keys
  under `koto-names/`.

### Phase 2: Index-first lookup on undeclared buckets

Rewrite `held_elsewhere` with the three steps and the declaration memo,
treating every bucket as undeclared (the declaration read is wired but no
command writes one yet). Hits get faster; misses are today's sweep.

Deliverables:
- `held_elsewhere`, the memo file, the stderr hint after a slow undeclared
  sweep, and an explicit 60-second request timeout in `create_bucket` (the
  value rust-s3 uses today, so nothing changes), with their tests:
  `an_indexed_session_refuses_from_another_prefix_without_a_sweep`,
  `an_older_writer_s_session_is_found_by_the_sweep_on_an_undeclared_bucket`,
  `a_stale_entry_is_confirmed_and_read_as_absent`,
  `a_forged_entry_never_builds_a_key`,
  `a_failed_index_listing_changes_nothing_on_an_undeclared_bucket`,
  `a_failed_top_level_listing_is_cant_tell_even_with_an_index_hit`,
  `an_index_listing_for_a_name_never_returns_a_longer_name_s_entries`.

### Phase 3: The declaration and the operator command

Add `koto session index audit|declare|revoke` and the declared branch of
`held_elsewhere`; the write path's 403 handling, in place since Phase 1,
starts failing pushes once a declaration exists. Document the
command and the declared bucket's trade in `docs/guides/cloud-sync-setup.md`
and the error-handling reference.

Deliverables:
- The command and the declared lookup, with tests:
  `a_missing_name_on_a_declared_bucket_costs_three_requests_at_any_prefix_count`
  (planting 1, 64, 300 and 2,000 prefixes and asserting the same request
  count and two round trips),
  `a_missing_name_on_a_degraded_declared_bucket_asks_a_bounded_number_of_listings`,
  `a_declared_bucket_still_refuses_an_indexed_session_from_another_prefix`,
  `an_older_writer_s_session_under_a_declared_bucket_revokes_on_the_next_audit`,
  `a_failed_index_listing_on_a_declared_bucket_is_cant_tell`,
  `an_unreachable_declared_bucket_still_reads_a_never_used_name_as_absent`,
  `a_revoked_declaration_returns_lookups_to_the_sweep`,
  `declare_refuses_inside_the_probation_window`,
  `declare_refuses_and_names_an_entry_less_state_object_newer_than_the_audit`,
  `an_oversized_or_malformed_declaration_reads_as_undeclared`,
  `a_prune_racing_a_create_ends_in_a_refused_declaration`
  (using the fake's `put_after` to land the create inside the prune),
  `a_refused_entry_put_blocks_the_push_on_a_declared_bucket`,
  `rerunning_the_audit_restarts_the_probation`,
  `a_name_with_more_entries_than_the_cap_falls_back_to_the_sweep`,
  `a_carried_declaration_expires_while_reads_keep_failing`,
  `audit_skips_and_reports_keys_that_fail_validation`,
  `audit_output_escapes_control_characters_in_bucket_keys`.
- `docs/guides/cloud-sync-setup.md` and
  `plugins/koto-skills/skills/koto-user/references/error-handling.md`.

Through all three phases the existing tests in `tests/cloud_bound_dir_test.rs`
pass unchanged, among them
`a_host_with_no_local_copy_refuses_from_another_prefix`,
`a_session_never_used_anywhere_reads_as_absent`,
`a_missing_name_s_probe_asks_the_other_prefixes_at_once`,
`a_failed_listing_of_the_holding_prefix_is_not_read_as_absent`,
`a_throttled_listing_that_clears_within_the_retries_still_answers`,
`a_failed_own_prefix_probe_is_not_read_as_absent`,
`a_throttled_own_prefix_probe_that_clears_still_refuses`,
`an_unreachable_bucket_still_reads_a_never_used_name_as_absent` and
`a_read_from_another_directory_costs_no_more_requests`. They plant state
with a bare PUT and no entry, which is an older writer's footprint on an
undeclared bucket, so they exercise the fallback exactly as written.

## Security Considerations

**Trust model.** The cloud backend already assumes that everyone who can
write to the bucket belongs to one owner or team and isn't hostile, though
their tools may be old, misconfigured or compromised. This design keeps that
model. It adds no dependency, no endpoint and no credential type: the new
code uses the existing S3 client and `serde_json`, and fetches nothing it
would execute. The only new local state is one small file beside the local
session store, the declaration memo. It holds no secrets and is written with
the same atomic replace and user-only mode as the sync stamps.

**Input from the bucket.** Everything the new code reads from the bucket is
untrusted. A lookup takes the part of each index key after
`koto-names/<name>/` and accepts it only if it's exactly 16 lowercase hex
digits, and only then builds another key from it. The name comes from the
command line and has already passed session-name validation, which requires
a leading letter and allows no `/`, so index keys can't reach outside their
own prefix or collide with the declaration. The audit and declare commands
apply the same two checks to every name and prefix they find while walking
the bucket, and skip and report anything that fails rather than build a key
from it. The declaration and the audit record are each read with a 4 KiB
limit on the bytes read,
parsed strictly, and treated as absent if anything is wrong with it, which
sends lookups back to the exact sweep. Bucket text the operator commands
print is escaped and truncated, so a hostile key or declaration field can't
send control sequences to the terminal of someone about to confirm a write.
A memo file that can't be parsed is ignored.

**What a forged entry can do.** An index entry is a hint, not an answer.
Every hit is confirmed by listing the named prefix and looking for the
session's state object, so a forged or stale entry costs one listing and
can't make a command say a session lives where it doesn't. A writer can
create many entries for one name to slow that name's lookups; past 256
entries the lookup stops confirming them and runs the exact sweep, so the
worst it can do is today's cost for that one name.

**What the declaration can do.** The declaration is the one object that
changes replies: on a bucket that carries it, a name with no index entry
reads as not-found. The bucket can't tell the operator from any other
writer, so anyone who can write under `koto-names/` can create or delete the
declaration, and anyone who can delete an entry can make that one name read
as not-found from other directories. In both cases a command may report no
such session where it would otherwise refuse with `session_unlocated`, and a
caller could then create a second session of the same name, the hazard
koto#518 closed. Nothing is read from or written into another workspace's
session, a writer with that access could already delete or overwrite the
state objects themselves, and the effect ends when the declaration is
deleted or the next audit revokes it. Deleting the declaration is always
safe. A forged `koto-names/_audit` can at most let `declare` run before the
probation has really passed, which gives its writer nothing a forged
declaration wouldn't. A forged declaration also turns a refused entry write from a warning
into a failed push, so it can stop hosts with prefix-scoped credentials from
creating session copies until it's removed. The guide will tell owners who
want more than the convention to restrict writes of `koto-names/_complete`
to the operator's credentials with a bucket policy, to turn on object
versioning so an overwrite can be inspected and reverted, and to keep
`koto-names/` out of lifecycle rules that expire old objects, since entries
are written once and never refreshed.

**Credentials.** Hosts need PUT and LIST under `koto-names/` and GET of
`koto-names/_complete`, on top of what they use today; the guide lists the
exact permissions. Credentials limited to one workspace prefix warn on an
undeclared bucket and can't create session copies on a declared one, and
`declare` refuses while it can see the entry-less copies they leave. Read
access gains nothing from the index: confirming an entry still needs list
access to the prefix it names.

**What the new keys reveal.** Index keys expose every session name, and the
workspace prefix holding it, to anyone who can list `koto-names/`. Anyone
who can list the whole bucket could already collect the same facts by
walking every prefix, but a policy granting list access on `koto-names/`
alone now exposes all names, so session names should be treated as readable
by everyone with access to the bucket. Session commands never remove an
entry, so deleting a session leaves its name in the bucket until an operator
prunes the index; the guide and the delete documentation will say so for
people who delete sessions for privacy reasons. The declaration records the
machine name of the host that wrote it and the audit's counts. Entries have
empty bodies, and no session content, path or credential goes into the new
keys.

**Stale local answers.** A host trusts a remembered "declared" answer for
the check window, and keeps using it while later reads fail without a reply,
but for no more than 10 minutes after its last answered read. A revoke
reaches a host on its next answered read, and a host whose declaration
reads keep failing falls back to the exact sweep once that limit passes.

## Consequences

### Positive

- On a declared bucket, a not-found costs 3 requests in 2 round trips at
  any prefix count, and about 18 requests at worst on a degraded bucket,
  against about six per prefix today. A caller probing several names under
  a 30-second timeout gets through unless single requests hang.
- Fewer listings means fewer chances of "can't tell" on a degraded declared
  bucket.
- On every bucket, `no_local_copy` for a session a newer release wrote no
  longer waits on the sweep.
- Undeclared buckets keep every reply and today's not-found cost, plus two
  requests that run in parallel and add no round trip.

### Negative

- koto#597 isn't fixed on a bucket nobody declares, and declaring needs a
  7-day probation unless the owner overrides it.
- A declaration can be wrong: an idle older release that wakes after
  probation and creates a session makes that session read as not-found
  from other prefixes until the next audit revokes the declaration.
- A new key family and an operator command to build, document and test;
  tests that count every PUT of a push gain one key.
- The index only grows between audits, and a name reused under many
  prefixes pays one confirming listing per leftover entry. A deleted
  session's name stays readable under `koto-names/` until an operator
  prunes.
- Any bucket writer can create or delete the declaration, so on a declared
  bucket the reply guarantee rests on the owner's bucket policy as well as
  on koto.
- Hosts whose credentials only allow `<prefix>/*` warn on undeclared
  buckets and can't push new copies on declared ones.
- On a bucket whose single requests hang, one listing can still wait
  rust-s3's 60-second timeout, so even a declared bucket can't promise an
  answer inside 30 seconds.

### Mitigations

- After a slow sweep on an undeclared bucket, koto prints a stderr hint
  naming `koto session index audit`; stderr isn't part of the parsed reply.
- The audit is cheap enough to run on a schedule, and the guide recommends
  that for any declared bucket that older releases might still reach;
  `revoke` is one delete.
- `declare` refuses while it can see entry-less state objects, including
  those that prefix-scoped credentials leave behind, and names them.
- `--prune` is available whenever the owner wants to shrink the index or
  clear deleted sessions' names.
- The guide gives owners the bucket policy, versioning and lifecycle
  settings that keep the declaration and the entries in the operator's
  hands (see Security Considerations).
- An explicit request timeout on the probe path is a separate lever, left
  to its own issue, and setting rust-s3's 60-second timeout explicitly in
  `create_bucket` keeps a crate upgrade from moving the retry line.
