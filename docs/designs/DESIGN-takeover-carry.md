---
schema: design/v1
status: Proposed
problem: |
  On the cloud backend, `koto init --continues <old> --supersede --carry`
  reads the predecessor's carried keys one GET at a time and keeps them in
  memory until the end, so a carry of a thousand keys takes about a quarter
  of an hour. One failed read ends the command with nothing kept, a re-run
  starts again from the first key while the predecessor is already fenced,
  and the failure is refused as exit 3 `persistence_error`, the same exit as
  `fence_unavailable`, so it reads as a lock problem rather than a network
  one.
decision: |
  The carry reads the predecessor's manifest once, skips every key the
  successor already holds with the same hash, and fetches the rest eight at
  a time, each read retried on a transient failure and checked against its
  manifest hash. Each chunk is written into the successor as it lands; a
  finished carry makes one push with the marker last, and a stopped one
  pushes what it has and then refuses `carry_incomplete` with the failure's
  class, the counts, and exit 1 when a retry can help, else 3.
rationale: |
  Parallel reads at the flush's existing width of eight cut the round trips
  by about eight, which serial reads and a server-side copy can't, and a
  wider fan-out gains little against values with no size cap; koto's own
  retry inside the read covers the slow-body failure rust-s3's retry misses.
  Writing each key as it arrives and skipping on the manifest hash makes
  progress survive a failure with no resume cursor and no extra push on the
  happy path, which chunked pushes would add to every carry. A new refusal
  code with a class-chosen exit lets a caller that keeps only the exit tell a
  retry-worthy network failure from a lock or bad-data failure, which
  reusing `persistence_error` or exit 3 cannot do.
---

# DESIGN: Takeover carry on the cloud backend

## Status

Proposed

## Context and Problem Statement

`koto init <succ> --continues <pred> --supersede --carry <prefix>...` hands a
predecessor's work to a successor: it fences the predecessor, creates the
successor, and copies every context key under each `--carry` prefix from the
predecessor into the successor. On the cloud backend that copy is the slow and
fragile step (koto#623).

Today the carry reads the predecessor's context manifest from its workspace
prefix, then GETs every matching key one after another, in one thread, and
keeps the values in memory (`CloudBackend::carry_source` in
`src/session/cloud_supersede.rs`). Only when every GET has succeeded does it
write the keys into the successor, deferred into one batch, then write the
`koto/superseded-carry-complete` marker and push the batch once
(`carry` in `src/cli/init_supersede.rs`). Each GET runs under the backend's
60-second request timeout with no retry of koto's own.

Three things follow:

- **Time.** The carry costs one bucket round trip per key, in sequence. The
  issue measured 1024 keys in about 18 minutes; a session with about 1,950
  keys needs half an hour or more of unbroken bucket access.
- **Lost progress.** A single failed GET, such as a connect timeout, ends the
  command before anything is written to the successor. The re-run starts again
  from the first key, and even a key the successor already holds is fetched
  again before koto compares it, so nothing an earlier run read is kept.
- **A misleading failure.** The failure is refused as `persistence_error`
  with exit 3, the same exit as `fence_unavailable`. A caller that keys on the
  exit code reads a network failure as a lock failure, and nothing in the
  output says how much was carried or how much remains.

The predecessor is already fenced by the time the carry runs, so on an
intermittent network a long session can be left with a fenced predecessor and
a successor that never finishes its carry.

## Decision Drivers

From the issue's Expected section:

- A `--supersede --carry` of a long session finishes in reasonable time.
- If it's interrupted, a re-run doesn't lose the keys an earlier run already
  carried.
- A failed carry says it failed on the network, not on a lock.

From the code as it stands:

- The completion marker, `koto/superseded-carry-complete`, is written after
  every carried key, and a re-run that finds it carries nothing (`carry` and
  `CARRY_COMPLETE_KEY` in `src/cli/init_supersede.rs`). A successor without
  the marker is how koto knows a carry is unfinished, so the marker stays
  last.
- The fence is written before the successor is created, and every refusal
  before the fence writes nothing (the step order in
  `src/cli/init_supersede.rs`). This design changes only the carry step.
- Bucket work that fans out is bounded: the batch flush puts objects at most
  `FLUSH_PARALLELISM` (8) at a time through `in_parallel`, and listings retry
  a transient failure through `asked_retrying` with `is_transient`
  (`src/session/cloud.rs`). New fan-out and retries should reuse those limits
  rather than invent new ones.
- The successor's carried keys reach the bucket as one deferred push, and a
  push an earlier run left open is pushed first on a re-run
  (`defer_writes_to`, `flush_interrupted_batch` in
  `src/session/cloud_supersede.rs`). The test
  `a_cross_prefix_supersede_fences_creates_and_carries` in
  `tests/session_supersede_cloud_test.rs` asserts the single push.

Argued here, not given:

- A re-run should also avoid fetching keys an earlier run already carried,
  not only avoid losing them. Otherwise a long carry on a bad network still
  needs one unbroken pass over every key it hasn't proved it holds.
- One dropped request shouldn't end the carry by itself. The issue's failures
  were single read timeouts in runs that had otherwise been working for
  minutes.
- The local backend's carry keeps its behavior. The issue is about the cloud
  backend, and the local carry reads a store on the same disk.

## Considered Options

### Decision 1: How the carry reads the predecessor's keys

The carry's cost is waiting: one round trip per key, in sequence, with
nothing overlapping. rust-s3 retries a failed request once, but only around
the send, so a timeout while a value's body streams in fails the GET outright.

**Chosen: bounded-parallel GETs at the flush's width, each under koto's
retry.** `carry_source` takes the sorted, filtered key list in chunks of
`FLUSH_PARALLELISM` (8). Each chunk's GETs run on scoped threads through a
helper shaped like `in_parallel` that returns each item's result in order.
Each GET goes through `asked_retrying` with `is_transient` (three asks,
waiting 250 ms and then 500 ms), applied to the raw `S3Error` inside
`get_with_etag`, before a 404 becomes `None`. A 404 or a 403 isn't asked
again. Worker threads only GET; the calling thread writes each chunk's keys
in key order once the chunk has joined, so the context store and the event
log keep a single writer. At 8 wide, 1024 keys cost about 128 round trips
instead of 1024.

Alternatives considered:

- **Serial GETs with koto's retry.** Rejected: it fixes the fragility but not
  the time. A 1024-key carry still takes about 18 minutes.
- **Wide fan-out at the lookup width (64).** Rejected: that width is kept for
  a lookup's small listings, and a carried value has no size cap. The gain
  over 8 is a couple of minutes at the sizes the issue names.
- **Server-side copy (CopyObject).** Rejected: it's still one request per key,
  so it saves no round trips. koto would never hold the bytes, so it couldn't
  write the key into the successor's local store or hash it for the
  `context_added` event.
- **Raise rust-s3's own retry count.** Rejected: the count is process-wide,
  retries a 404 or 403 too, and ends before the body is read, so it misses the
  slow-read failure.

### Decision 2: How carry progress survives an interruption

A deferred write already records its key in the successor's local journal
before it writes the content, and a re-run already pushes a journal an
earlier run left open (`flush_interrupted_batch`). The predecessor is fenced
before the carry, so its bucket manifest no longer changes, and that manifest
names each key's `hash` (sha256 of the content), the same form the
successor's store uses.

**Chosen: write each key as it arrives, skip on a manifest hash match, one
push.** The carry takes the defer guard before its first read and keeps the
predecessor's filtered manifest with its hashes. A key whose hash matches the
successor's `meta` hash is skipped with no GET and counted as already held.
Every other key is read and written into the successor (local store, journal,
`context_added` event) as soon as its chunk arrives. On success the marker is
written last and the batch goes up in one push, as today. When a read fails
for good while the process is alive, the carry writes what has arrived, drops
the guard, pushes the partial batch without the marker, and then refuses. If
that push fails too, the journal stays open and the same host's re-run pushes
it first. A killed process leaves its journal on disk for the next command on
that host.

Alternatives considered:

- **Push in chunks of N keys.** Rejected: each chunk is a full push (batch
  open, manifest, state file, version, marker delete), about six requests per
  chunk beyond the key PUTs, on every carry including ones that never fail. It
  only adds cover for a run killed on one host and re-run on another. It can be
  added on top of the chosen option later.
- **A resume cursor key.** Rejected: it goes up in the same batch as the keys,
  so it adds no durability, and with out-of-order reads it can only be a
  low-water mark. It can't see a successor value that differs from the
  predecessor's, and it adds a reserved key to write, order and clear.

### Decision 3: How a failed carry reports itself

The carry failure is the only refusal of this command that comes after the
fence and the create, yet it shares `persistence_error` with refusals that
write nothing, and exit 3 with `fence_unavailable`. koto's exit classes
(`docs/reference/error-codes.md`) already say exit 1 means "retry after a
backoff" and exit 3 means retrying won't help, and `backend_error` already
picks its exit from a retryable test.

**Chosen: a new refusal code, `carry_incomplete`, with its exit chosen by
class.** The body adds `superseded` and `name`; `failure` (`unreachable`,
`timeout`, `throttled` and `denied`, worked out as `remote_feed`'s
`failure_class_after` does, plus `invalid_source` and `write_failed`);
`retryable`; `keys` (the predecessor's carried keys, from its manifest);
`held` (how many the successor holds with the predecessor's value, across
runs); `remaining`; and `carried` (this run's writes, as in the success
output). The exit is 1 when `retryable` is true and 3 otherwise. `held` counts
only keys already written, so it can come out low but never high. To keep the
class all the way to the CLI, `get_with_etag` returns a typed error holding the
`S3Error` instead of a formatted string, and bad source data gets its own
error.

Alternatives considered:

- **The new code on exit 3.** Rejected: the exit still matches
  `fence_unavailable`, so a caller that keeps only the exit, like the one in
  koto#623, reads it as before, and a retry-worthy failure stays in the
  "retrying won't help" class.
- **`persistence_error` with added fields.** Rejected: the same code covers
  refusals that wrote nothing, so a caller would have to infer the state from
  which fields are present, and the exit still matches `fence_unavailable`.
- **A dedicated exit number.** Rejected: one number can't separate a network
  failure from a 403 or bad source data, and it would add a value outside the
  four exit classes koto documents.

### Decision 4: Smaller choices made while writing the architecture

**Where the retry lives.** Chosen: inside `get_with_etag`, so the fence pull
gets it too. The alternative was a carry-only wrapper. A retried read can't
write anything, so the wider reach costs at most a second on a pull that fails
for good, and it saves a second code path.

**How fetches are scheduled.** Chosen: chunks of eight that each join before
their writes. The alternative was a rolling queue that starts the next GET as
soon as one finishes. Chunks keep the writes in key order and reuse the
`in_parallel` shape. Their cost is that one stalled GET holds back seven others
for up to the request timeout, and a rolling queue can be swapped in behind
`fetch_carried` without touching `carry`.

**The partial push on failure.** Chosen: one best-effort `flush_window` before
the refusal. The alternative was retrying it. The push runs on the same failing
network, and an open journal is already pushed by the next command on this
host, so a retry would only delay the refusal.

**Checking fetched values.** Chosen: `fetch_carried` hashes each value it
reads and compares it with the manifest hash before anything is written; a
mismatch gets one more GET, in case the read caught an object mid-replace, and
then fails as invalid source. The alternative was to trust the GET, as the
carry does today. The manifest hash already decides the skip, so leaving the
fetch path unchecked would let a replaced or corrupt value into the successor,
out in the partial push, and behind the marker where no later run looks. The
check costs one sha256 per value, which is small next to the GET.

## Decision Outcome

The three choices meet at one loop. The carry reads the predecessor's
manifest once, drops keys the successor already holds by hash, fetches the
rest eight at a time with each fetch retried on a transient failure and each
value checked against its manifest hash, and writes each chunk into the
successor as it lands. A carry that finishes makes
the same single push as today, with the marker last. A carry that stops
pushes what it has, then refuses `carry_incomplete` with the failure's class,
the counts, and an exit that tells a caller whether to retry. The re-run
skips everything already held without fetching it.

Measured against the issue's Expected section: a 1,950-key carry drops from
over half an hour to about four minutes at the issue's one second a key; an
interrupted carry keeps every key it wrote, on any host when the process
lives long enough to push and on the same host when it doesn't; and a network
failure says so, by code, by class and by exit, never as `fence_unavailable`.

Cross-validation found no conflict between the decisions' assumptions. Two
couplings hold the design together. Decision 3 needs `get_with_etag` to keep
the `S3Error`, and decision 1 retries inside that same function, so the
retry wraps the raw error and the typed error carries the last one. Decision
3's `held` is decision 2's skip test run over the manifest, so the two counts
are the same number.

The behavior changes callers will see: a retry-worthy carry failure exits 1
where it exited 3, and the failing run's earlier keys now reach the bucket
before the refusal, so a re-run's `carried` covers only what was left.

## Solution Architecture

### Overview

Only step 4 of `koto init --supersede` changes, and only on the cloud
backend. The predecessor's side becomes a plan (its filtered manifest with
hashes) plus a fetcher that reads chunks of keys in parallel with retries. The
successor's side stays a single writer that writes each chunk as it lands
under the existing defer guard. The refusal path gains a typed error, a class
and counts.

### Components

- **`CloudBackend::carry_plan`** (replaces the read half of `carry_source`,
  `src/session/cloud_supersede.rs`). GETs the predecessor's manifest under its
  workspace prefix, drops host-local keys and pin records as today, filters by
  the `--carry` prefixes, drops reserved keys, validates each key name, and
  returns the sorted entries with their manifest hashes. A manifest that
  doesn't parse or names a bad key is a `CarrySourceError` (invalid source).
- **`CloudBackend::fetch_carried`** (`src/session/cloud_supersede.rs`). Takes
  one chunk of at most `FLUSH_PARALLELISM` plan entries and GETs them on
  scoped threads, returning each key's result in order. It takes the
  `CarryEntry` values `carry_plan` returned, so `carry_plan` stays the only
  place a key name from the manifest is validated. It hashes each value it
  reads and compares the result with the entry's hash; a mismatch gets one
  more GET and then is a `CarrySourceError`, so a value that doesn't match the
  manifest is never returned. A key the manifest lists but the bucket doesn't
  hold is an invalid-source error, not a retry.
- **Retrying GET** (`get_with_etag`, `src/session/cloud_supersede.rs`). Wraps
  `bucket.get_object` in `asked_retrying` with `is_transient`, and on final
  failure returns a typed `BucketReadFailed { key, source: S3Error }` in the
  `ListingFailed` style instead of a formatted string. Its other caller, the
  fence pull, gets the same retry.
- **Ordered-results helper** (`src/session/cloud.rs`, beside `in_parallel`). The
  same chunked scoped-thread shape, returning `Vec<anyhow::Result<T>>` in item
  order. `in_parallel` can be rebuilt on it.
- **`carry`** (`src/cli/init_supersede.rs`). Holds the defer guard across the
  whole carry. For each plan entry it compares the manifest hash with
  `meta(succ, key)` and counts a match as held. The rest go to
  `fetch_carried` chunk by chunk, and each chunk's values are written in key
  order through `write_koto_key` on the calling thread. On success it writes
  the marker and calls `flush_window` once. On failure it writes the chunk's
  values that arrived, drops the guard, tries `flush_window` once, and returns
  a `CarryIncomplete` error with its counts and cause. The local backend's path
  keeps today's list-and-get loop.
- **Refusal** (`run`, `src/cli/init_supersede.rs`). Downcasts
  `CarryIncomplete` and refuses `carry_incomplete` with the fields below.
  `failure` comes from the cause: a `BucketReadFailed` classed the way
  `remote_feed`'s `failure_class_after` classes one (`unreachable`, `timeout`,
  `throttled`, `denied`), a `CarrySourceError` as `invalid_source`, and anything
  from the successor's write or push as `write_failed`. Exit 1 when the class
  is retryable, else 3. Any other error keeps today's `persistence_error`.
  The `error` text is built from the class, the validated key name and fixed
  wording. It never includes the `S3Error`'s own text (endpoint, request ids,
  response body) or text read from the manifest, the parser's message and a
  rejected key name included: an invalid-source failure names its sub-cause in
  fixed wording only. `cause` is used to pick the class and is never rendered.

### Key Interfaces

```rust
// src/session/cloud_supersede.rs
// Fields private; only carry_plan builds one, from a validated key.
pub(crate) struct CarryEntry { key: String, hash: String }
pub(crate) fn carry_plan(&self, prefix: &str, pred: &str, prefixes: &[String])
    -> anyhow::Result<Vec<CarryEntry>>;
pub(crate) fn fetch_carried(&self, prefix: &str, pred: &str, entries: &[CarryEntry])
    -> Vec<anyhow::Result<Vec<u8>>>; // each Ok value matches its entry's hash

// src/cli/init_supersede.rs
pub(crate) struct CarryIncomplete {
    pub keys: Option<usize>,  // None when the plan itself failed
    pub held: usize,          // successor holds the predecessor's value
    pub carried: usize,       // written by this run
    pub cause: anyhow::Error, // BucketReadFailed, CarrySourceError or other
}
```

The refusal body:

```json
{"error":"--supersede: 'p' is fenced and 's' exists, but the carry stopped: a bucket read of 'record/x' timed out; 812 of 1950 keys carried, 1138 remain. Run the same command again to finish it",
 "command":"init","code":"carry_incomplete","superseded":"p","name":"s",
 "failure":"timeout","retryable":true,"keys":1950,"held":812,"remaining":1138,"carried":640}
```

The success output keeps its shape. `carried` counts keys this run wrote.

### Data Flow

1. `flush_interrupted_batch(succ)` pushes a batch an earlier run left open.
2. If the successor holds the marker, return 0.
3. The defer guard is taken. `carry_plan` makes one GET (the predecessor's
   manifest). Keys whose hash matches `meta(succ, key)` are counted as held,
   which costs no GET; on a host without a local copy, the successor's manifest
   is read once and cached.
4. The rest go in chunks of 8 through
   `fetch_carried`, each GET with up to three asks and each value checked
   against its manifest hash. The calling thread writes each chunk's values in
   key order.
5. On success the marker is written, the guard dropped, and one `flush_window`
   pushes everything with the marker last.
6. On failure the values that arrived are written, the guard dropped, one
   `flush_window` is tried, and the command refuses `carry_incomplete`.

## Implementation Approach

### Phase 1: Retrying, typed bucket reads

`get_with_etag` retries through `asked_retrying`/`is_transient` and returns
`BucketReadFailed` on final failure. Add the ordered-results helper. Unit
tests: a transient failure followed by success returns the value; a 403 or 404
is asked once.

### Phase 2: Plan, parallel fetch, incremental writes

Split `carry_source` into `carry_plan` and `fetch_carried`, and restructure
`carry` around the guard, the hash skip and per-chunk writes. Keep the local
path. Tests in `tests/session_supersede_cloud_test.rs`, with the fake bucket's
latency and fault injection:

- `a_cross_prefix_supersede_fences_creates_and_carries` keeps its single-push
  assertion.
- `a_re_run_after_the_fence_finishes_the_carry_once` changes. The failed run's
  earlier keys are in the bucket without the marker, the two runs' `carried`
  add up to the total, and the re-run makes no GET under the predecessor's
  prefix for a key the first run carried.
- New: a carry under latency finishes in about N/8 round trips; a key failing
  one ask (two requests, given rust-s3's own retry) still gets carried.

### Phase 3: The `carry_incomplete` refusal and docs

Add `CarryIncomplete`, the classification and the exit rule in `run`. Tests: a
transient GET failure (exit 1, `retryable: true`, counts), a manifest naming a
bad key (exit 3, `invalid_source`), a value whose hash differs from the
manifest's on both reads (exit 3, `invalid_source`, nothing written for that
key), a 403 (exit 3, `denied`), an `error` text that carries none of the
fake bucket's error message, and a manifest key holding control characters
and a long string, neither of which reaches the `error` text. Docs: a
`carry_incomplete` row in the refusal table of `docs/guides/cli-usage.md`, the
supersede section's cloud paragraph updated (parallel reads, the skip, the
partial push), and the koto-user skill's command reference beside
`fence_unavailable`.

## Security Considerations

The carry reads a manifest and object bodies from the bucket, which other
hosts and sessions can write, and copies them into the successor. The manifest
keys, manifest hashes and object bytes are therefore treated as untrusted
input.

**Key names.** Each manifest key is checked once, in `carry_plan`, with
`validate_context_key` after the host-local, pin-record, prefix and
reserved-key filters. `carry_plan` is the only place a manifest key is
validated; `fetch_carried` takes the `CarryEntry` values it returns and builds
the object path `{prefix}/{pred}/ctx/{key}` from them. `CarryEntry` has
private fields and no other constructor, so nothing else can produce one from
an unchecked string. The workspace prefix and predecessor id are checked with
`is_workspace_prefix` and `validate_session_id` before any path is built. A
manifest naming a bad key fails the whole carry as invalid source, and no GET
is made for it.

**Value integrity.** `fetch_carried` hashes each value (sha256, lowercase hex,
the form the context store and manifest use) and compares it with the entry's
manifest hash before the value is returned. A mismatch gets one more GET, in
case the read caught an object mid-replace, and then fails as invalid source
with nothing written for that key. A manifest hash that is malformed or in
another case also fails closed. Because only verified values are ever
written, the partial push after a failure contains only verified values, and
the later skip (successor hash equals manifest hash) can rely on what an
earlier run wrote. This check catches torn, corrupt and stale reads. It does
not defend against someone who can write the bucket, since the manifest and
the objects share a bucket and credentials and the fence is a koto convention
rather than a bucket-side lock. That trust in the bucket is the same as for
the rest of the cloud backend.

**Refusal text.** The `carry_incomplete` `error` string is built from the
failure class, a key name that already passed validation, counts and fixed
wording. It never includes the `S3Error` text (endpoint, request ids,
response body) or text parsed from the manifest, including the parser's own
message and a rejected key name; an invalid-source failure names the sub-cause
in fixed wording only. `CarryIncomplete.cause` is used for classification and
is never rendered into output, logs or events. A test puts control characters
and a long string in a manifest key and asserts neither reaches the `error`
text.

**Permissions and concurrency.** The carry uses the existing bucket
credentials, reads only under the predecessor's prefix and writes only the
successor. Fan-out is the existing 8-wide limit on scoped threads; worker
threads only GET, and the calling thread is the single writer to the store and
event log. A 403 isn't retried and is reported as `denied`. The retry added to
`get_with_etag` also applies to the fence pull and is read-only.

**Residual risks.** These are accepted for this change:

1. A bucket writer can alter a manifest and its objects together; the hash
   check doesn't detect that.
2. A value has no size cap, so a corrupt or hostile bucket can make a carry
   use memory (up to 8 values at once) and time; the 60-second request
   timeout bounds each read.
3. An absent predecessor manifest is read as empty, so the carry completes
   with zero keys and writes the marker.
4. A key the successor holds with a different value is overwritten by the
   verified predecessor value, as today.

## Consequences

### Positive

- A long carry takes minutes. At the issue's one second a key, 1,950 keys take
  about four minutes instead of over half an hour.
- One dropped request no longer ends a carry, and a carry that does stop keeps
  every key it wrote.
- A re-run fetches only what's missing, at the cost of one manifest GET (two on
  a host without a local copy of the successor).
- A caller can tell a network failure from a lock failure by code and by exit,
  and can see a retry making progress from `held` and `remaining`.
- Peak memory is bounded by a chunk rather than the whole carry.

### Negative

- A retry-worthy carry failure moves from exit 3 to exit 1. A caller that
  treated every exit 3 from this command as "run it again" sees a new exit.
- A chunk waits for its slowest GET, so one stalled request holds back seven
  others for up to the 60-second request timeout.
- The defer guard and the successor's batch-open marker are held for the whole
  read rather than only the write loop. A carry killed mid-read leaves the
  successor's batch open in the bucket longer, and an import of the successor
  from another workspace refuses until this host runs any command on it.
- A run killed mid-carry and re-run on another host still starts over for the
  keys that never left the first host.
- The fence pull gains the same retries, so a pull that fails for good takes
  under a second longer to report.

### Mitigations

- Exit 1 is koto's documented "retry after a backoff" class and exit 3 its
  "retrying the same call won't help" class (`docs/reference/error-codes.md`,
  "Exit classes"). A retry-worthy carry failure now lands in the class that
  says what to do with it. The carry failure was never documented on exit 3,
  and the refusal table and the koto-user reference document the new code and
  its exits. This is a visible behavior change and the pull request says so.
- A stalled chunk costs at most one request timeout per chunk, and the work
  done before it is written and pushed if the stall ends in failure.
- The open batch is pushed by the next command on the host that ran the carry,
  which is the remedy the import refusal already names.
- The cross-host kill case can be closed later by pushing every N chunks on top
  of this design, without undoing it.
