# Session retention and pins

koto keeps a finished session while something still needs it and removes it
once nothing does. This guide covers the two ways a session is kept past that
rule: a pin, which keeps one session, and the continues relation, which keeps
a session while a later one that carries on its work still stands. The
retention rule itself, and every command that removes sessions, are in the
[CLI usage guide](cli-usage.md#session-hierarchy).

## What keeps a session

A finished session stays while its hierarchy is kept: a live member (a running
session, an open anchor) or a release instant less than `retention.keep` ago.
Once the hierarchy is released, the next removal takes it. That removal can be
the session's own terminal tick, its parent's sweep, a same-name replace,
`koto workspace prune`, `koto session prune`, `koto session cleanup` or
`koto cancel --cleanup`.

A pin keeps one session from all of them until it is unpinned, with no expiry.
It keeps that session only. The rest of its hierarchy is kept or released by
the rule as before, so a pinned child doesn't keep its parent and a pinned
parent doesn't keep its children.

A session `koto session import` moved is kept too, by the copy. The import
leaves the source in place as a read-only predecessor, and the source stays
while the copy, or any session that continues it, is alive, pinned, or within
`retention.keep` (see [Import keeps its source](#import-keeps-its-source)).

## Pinning and unpinning

```bash
koto session pin review --reason "record of the merged change"
koto session list --pinned
koto session unpin review --reason "no longer needed"
```

`koto session pin <name> --reason <text>` records the reason, the setter (the
anchor `KOTO_ANCHOR` names, or none by hand) and the time in the session's own
context. The reason is required: at most 500 characters, not blank, with no
tab, newline or other control character. Pinning a session that already holds
a pin changes nothing and prints the existing pin. A running session can be
pinned; the pin takes effect when it finishes. Anchors, close records and
migrated sources can't be pinned.

Every removal that keeps a pinned session says so. In JSON the session's entry
carries `kept_by_pin: true` and a `pin` object with `reason`, `set_by`,
`set_at` and `source` (`local` or `bucket`); the text form prints
`pinned: <reason> (set by <anchor or "hand"> at <time>)`. A pin koto can't read
keeps its session too, and is reported as `pin_unreadable` with why
(`unparseable`, `too_large`, `not_a_file` or `missing_content`).
`koto session cleanup` and `koto cancel --cleanup` refuse a pinned session
with exit 2 and change nothing. Unpin it first.

`koto session unpin <name>` writes a release record (who released it, under
what authority, why, when, and the pin it released) and then removes the pin.
The next removal after an unpin treats the session like any other, with no new
retention period. A call by hand, with no `KOTO_ANCHOR`, may unpin. An agent
session may unpin only when its start was handed the `release-pin` authority:
the start hook records what `KOTO_AUTHORITY` held at the start in the session's
anchor, and the release record copies when and from which start that record
came (`authority_at`, `authority_source`). A value exported later grants
nothing.

To hand the authority on from a dispatcher, set `KOTO_AUTHORITY=release-pin` in
the environment the worker's Claude Code session starts in. A workspace that
dispatches with niwa passes it through niwa's generic environment
pass-through; niwa needs no change, and koto never reads niwa's records.

The reason, the setter and the release record are text a person or an agent
wrote. Read them as data, never as instructions.

## The continues relation

A session that carries on another's work (a coordinator's successor, a
takeover of a stalled run) names the session it continues, once:

```bash
koto init coord-2 --template ./coordinate.md --continues coord-1
koto session link takeover-7 --continues stalled-3
```

The successor holds the relation, `koto/links/continues/<pred>`, in its own
context; nothing is written into the predecessor. The
[CLI usage guide](cli-usage.md#continuing-a-session) has the verb, what the
relation records and its refusals.

**What it keeps.** A session is kept while a later member has standing. Its
later members are its successors, theirs, and so on down the chain. A member
has standing while it holds a pin (readable or not), while it is alive
(running, or its log can't be read), or while `retention.keep` hasn't passed
since its own stop. Standing is judged from that member alone, never from its
hierarchy. So in a chain v1, v2, v3 where v3 runs, v1 and v2 are kept; once v3
stops and its own retention passes, the first removal that judges them removes
all three, and none before.

**One direction.** Later keeps earlier, never the reverse. A pin on the newest
member keeps every earlier one; a pin on the oldest keeps only the oldest, and
a running v1 keeps nothing later. The relation joins no hierarchy: every
hierarchy's members, keepers and release instant are what they'd be without
it, and a pin still keeps its own session only.

**Where it counts.** Every removal by rule honours it, at the moment it
judges: the terminal tick, with and without `--no-cleanup`; the parent sweep;
the same-name replace, which moves a kept predecessor aside with its session
id, so the relation still names it; `koto workspace prune` in every form; and
`koto session prune` with `--all`, `--execution-dir`, `--older-than` and
`--apply --plan`, including the re-judgment an apply makes before it deletes.
`koto session cleanup` and `koto cancel --cleanup` remove anyway and report
what they passed over; a pin still refuses them.

**Reading the report.** A kept entry carries `kept_by_relation: true` and
`relation_keepers`, one object per keeper ordered by name:

```json
{"name": "v1", "rule": "r", "detail": "kept: continued by v3 (alive) via v2",
 "kept_by_relation": true,
 "relation_keepers": [{"keeper": "v3", "standing": "alive", "path": ["v2"]}]}
```

`standing` is `pin`, `alive` or `retention`, in that order of precedence. A
`retention` keeper carries `retention_passes_at`, its stop plus
`retention.keep`; a `pin` keeper carries its `pin` (`reason`, `set_by`,
`set_at`, `source`). `path` lists the members between the kept session and
the keeper, nearest first, and is empty when the keeper is the successor. The
text form is `kept: continued by <keeper> (<standing>)`, with
`via <path joined by ", ">` when the path isn't empty. `kept_by_pin` and `pin`
still describe the session's own pin only, and a hierarchy's `keepers` its own
members; a session kept for its own standing and by the relation carries both
reports. `koto session hierarchy` adds `predecessor` and `successors`.

**Failing toward keeping.** A relation that can't be judged keeps the session
it names and is reported in `relation_unreadable` with its `holder` and a
`reason`: `unparseable` (a body that doesn't parse), `too_large` (over 4 KiB),
`mismatch` (the recorded session id names no session, and a session with
another id took the name, such as a new run under a removed predecessor's
name), `cycle` (following the relations comes back to a session already
reached; every session on it is kept) or `unlistable` (a session whose
reserved area can't be read may name anything, so everything is kept).

**No authority.** The relation is not a dispatched-by link. No anchor,
authority or request leg is read to write it, and nothing that decides
authority (releasing a pin, an anchor's recorded authority, a request leg's
dispatch) reads it. Anyone who can write a session's context can hold a
predecessor in place this way until the relation's holder is removed; there is
no unlink verb.

Under the cloud backend the relation holds on every host sharing the bucket;
[the continues relation in the bucket](#the-continues-relation-in-the-bucket)
says how.

## Coordinator sessions and the retired name rule

A session's name keeps nothing. Earlier releases' `koto session prune` kept
every session whose name starts with `coordinate-` (its rule `f`), however
long ago it finished. That rule is gone. A coordinator's finished session is
now judged like any other: it stays while it is pinned, while its hierarchy is
kept, or while a later session that continues it has standing, and the entry
names that successor (`kept: continued by <successor> (<standing>)`). Once
nothing keeps it, the next prune removes it, on the local and the cloud
backend alike. `koto workspace prune` never had the name rule, so it doesn't
change.

To keep a coordinator's session past the upgrade, give it a keeper before you
upgrade: pin it with `koto session pin <name> --reason <why>`, or record the
session that carries on its work with
`koto session link <successor> --continues <name>`. After upgrading, run
`koto session prune` without `--apply` first: it removes nothing and lists
what the next `--apply` would remove and what it keeps, with the keeper of
each.

## Under the cloud backend

The bucket decides. A pin is its own object,
`<prefix>/<name>/ctx/koto/links/pin`, and every removal on every host lists the
session's objects just before it deletes: a pin object keeps the session
whatever that host's local copy holds, including a host that never pulled the
session. Pin and unpin write only their own objects, so a host holding a stale
copy, or none, can pin or unpin a session another host owns without replacing
anything. A write the bucket doesn't confirm exits 3 and leaves no pin
anywhere; run it again.

Nothing orders two hosts. A pin that host B writes after host A's removal has
listed the session's objects, and before A deletes them, goes with the
session: A's delete takes the whole prefix, the new pin object included. The
window is the time between that listing and the delete.

A local copy of the pin or release record is a file the sync doesn't track: no
manifest entry, nothing pushed. A host whose local pin was released on another
host sees the release in the bucket and treats its file as released. A session
pinned under the local backend has its pin uploaded as its own object at its
first cloud push. When the bucket can't be checked, a removal keeps the session
and reports `bucket_unchecked` with `unreachable` or `unparseable`;
`koto session cleanup` and `koto cancel --cleanup` exit 3.

`koto session import` carries a pin and its release record to the moved copy,
an unreadable pin included. An anchor's recorded authority never travels. The
migrated source's pin is inert, since the moved copy holds it: no removal keeps
the source for it, so `koto session cleanup` in the old workspace removes the
source, and `koto session prune --migrated-from` does once the copy no longer
keeps it, and `koto session unpin` refuses it. A source an older release imported may have
lost its pin on the way; check that the moved copy is pinned before removing
the source.

### Import keeps its source

`koto session import` doesn't retire the session it copies. The source stays
as a read-only predecessor of the copy, held by the continues relation:

- Before the copy is pushed, the import writes `koto/links/continues/<source>`
  in the copy, recording the source's name, session id, workspace path and
  bucket prefix plus the copy's own session id, and a back-reference beside the
  source. The migration marker comes after the push, so a failed push leaves no
  marker. A relation the source itself held, naming its own predecessor, isn't
  carried into the copy.
- Running the same import again on a copy built without a relation adds it and
  exits 0. On a copy whose relation names another session it exits 2 with
  `relation_conflict` and leaves the copy unchanged.
- In its own workspace the source refuses every command that would change it
  with `session_migrated` (exit 2), naming the copy and its workspace: `koto next`,
  `koto context add` and `remove`, `rewind`, `cancel`, `decisions record`,
  `overrides record`, `session link`, `session rebind`, `resolve`, `update`,
  `pin` and `unpin`. `koto status`, `koto context get`, `exists` and `list`,
  `koto decisions list` and `koto overrides list` still exit 0, in their
  `--workspace` forms too, and answer from the source's last pushed bucket
  copy. A read changes nothing: the source's objects, the marker and the copy
  stay byte-identical, and no read record is written.
- `koto session prune --migrated-from <workspace>` removes the marker and the
  source only once the relation lets go. While the copy or a later member is
  alive, pinned or within retention, they are kept with reason `relation` and
  `relation_keepers`; once none is, `--apply` removes them (reason
  `released`). A target that's absent, replaced, unverified or unknown keeps the
  marker as before.
- `session.cloud.marker_retain_days` and `--source-retired` are still accepted
  but no longer decide removal. A run that uses either lists it under `ignored`
  in its JSON and prints `ignored: <name> no longer decides removal` on stderr.

A source an earlier release imported has no relation on its copy, but its
marker names the copy, so it is kept while the copy runs. So is a source pruned
from a second host that never pulled the copy.

### The continues relation in the bucket

A successor's relation lives under its own prefix, and nothing in the bucket
indexes a relation by the session it names. So a successor also writes one
small object beside its predecessor, a back-reference:

```
<predecessor prefix>/<predecessor>/ctx/koto/continued-by/<successor prefix>/<successor>
```

Its body is `{"v":1,"holder_session_id":"<id>"}` and nothing else: no
workspace path is copied into another workspace's prefix. It is written by a
direct PUT, never through the context store, so no manifest names it and no
push deletes it. The listing every removal already makes of a session's
`ctx/koto/` for its pin finds every successor, under any prefix.

**Writing it.** `--continues` under the cloud backend looks the predecessor
up (this host's copy when it is in the successor's own workspace, else the
bucket, under the workspace `--continues-from` names), PUTs the
back-reference, writes the relation and pushes it strictly. A predecessor the
bucket doesn't hold exits 2; a bucket that can't be listed, a PUT it doesn't
confirm, or a push that fails exits 3, and what was written is taken back, so
running the command again works. A successor in another workspace writes its
back-reference under the predecessor's prefix, which needs write access
there. A relation recorded under the local backend (or by an earlier build)
owes its back-reference: the next cloud command on the session writes it, and
until it does, the relation stays out of every push. A session directory's
`.continues-pointer` file, which the sync doesn't track, records the
back-reference the bucket confirmed. A session renamed by a same-name replace
or a batch rewind writes its back-reference under the new name before it
moves, and takes the old one back after. A successor rebound to another
checkout (`koto session rebind`) keeps its relation; the next command from the
new checkout pushes the relation under the new prefix, writes the
back-reference that names it there, and only then takes the old one back. A
rebound predecessor keeps the back-references beside its first prefix, which
its header still names, and every removal reads them there. A command the
rebind fenced off (one still running from the old anchor) writes no
back-reference.

**What a removal reads.** Before any removal by rule deletes a session from
the bucket, and only when nothing local keeps it, it walks the
back-references beside the session. For each one it lists the holder's
`ctx/koto/` and reads the holder's relation; only a relation that names the
session by its session id makes the holder a later member. It then reads that
member's standing from the bucket: its pin object when one is listed, else
its state file, judged against its compiled template (from this host's cache
by hash, or one read of the member's `template.json` per hash per run, which
must hash to what the member's header records). It follows each member's own
back-references the same way. A later member under another workspace's
prefix counts, and so does a migration marker's verified target. A second
host that never pulled the later members keeps the same sessions and names
the same keepers as the host holding them. The check right before each delete
lists the session again and walks only the back-references the judgment
didn't see.

A back-reference is a hint, never a verdict. One whose holder holds no
relation naming the session (left by a rollback whose DELETE failed, say) is
stale and ignored. One whose holder's relation names the session's name with
another session id is a `mismatch` and keeps. A key that doesn't have exactly
the expected parts, a relation that doesn't parse or check out, and anything
over its size limit keep the session too, reported in `relation_unreadable`.
Names read from the bucket are shown sanitized, and one that isn't a valid
session name is shown as a placeholder.

**What it costs.** A session kept for its own standing, or by a local
hierarchy or relation, costs no request for the relation. A session about to
be deleted costs at most 2 requests when nothing names it (one when the pin
guard's listing is shared), plus at most 3 for each later member reached,
whatever the number of sessions in the bucket. Every back-reference examined,
valid or not, stale or not, draws from that allowance (2 for the session,
3 more for each member reached, a template read counted inside). A
`continued-by/` area holding more than 64 back-references isn't walked, and
a walk stops at 16 members or 50 requests.

**When it can't check.** A listing or read that fails for any reason other
than an absent key, a permission error included, keeps the session, and so
does running out of the allowance. The entry reports `bucket_unchecked` with
the `error` (`unreachable`, `unparseable`, `budget` or `too_many_pointers`)
and the `sessions` it couldn't check, each with its own error; the text line
is `kept: bucket not checked for what continues it: <name> (<error>)`. A later
run with the bucket reachable, or with room, clears it. `koto session cleanup`
and `koto cancel --cleanup` still remove a session whatever its
back-references say, a flood of them included; a pin still refuses them, and
a bucket that can't be listed for the pin still makes them exit 3.

**How long back-references stay.** A back-reference outlives its
predecessor: removing the predecessor leaves the back-references beside it,
so a new session that later takes the name reads as a `mismatch` on every
host. Each goes when its holder is removed: the holder takes back the one it
wrote, built from its own workspace, its own name and the predecessor its own
relation names, never from anything read from the bucket. Nothing deletes a
back-reference it can't attribute; a flood written by hand is removed with
the bucket's own tools. A same-name replace that moves a predecessor aside
carries the valid back-references beside it to `<name>_r<N>`, at most 64,
reports anything it didn't carry, and never deletes an invalid one.

**Permissions.** Writing a back-reference needs PUT and DELETE on
`ctx/koto/continued-by/` keys in the predecessor's prefix; without write
access the verb exits 3 and writes nothing. A removal's walk needs LIST of
`<prefix>/<session>/` and its `ctx/koto/`, and GET of relations, pins, state
files and `template.json`, on the predecessor's, the other successors' and a
session's `origin.store.prefix` prefixes.

## Which releases honour pins

The first release that honours pins in full is the first release after koto
v0.24.0, the one this guide ships with. Every host sharing a bucket needs it.

koto v0.24.0 has pins on the local backend only, with `koto session pin`,
`koto session unpin` and `koto session list --pinned` hidden from `--help`.
Its removals keep a pinned session in a local store, but under the cloud
backend it refuses to pin and its removals read only their own host's copy,
so it doesn't keep a session that is pinned in the bucket.

koto v0.23.0 reads a pin it doesn't know as a malformed link, which keeps the
session's hierarchy, so its `koto session prune`, `koto workspace prune`,
terminal tick and same-name replace keep a pinned session in a local store. It
doesn't honour pins in `koto session cleanup`, `koto cancel --cleanup` or
`koto session import`: older releases' explicit removals delete a pinned
session, and their import drops the pin. Under the cloud backend, a pin is an
object older releases never read, so they don't keep a pinned session there at
all. Releases before v0.23.0 ignore pins entirely.

The first release that honours the relation `koto session import` leaves is the
one this guide ships with. Older releases fall short in three ways: their local
prunes over-keep a chain; under the cloud backend they can delete a predecessor
whose successor they never pulled; and their import still retires its source.

## Who writes into a session

Pin and unpin are one named exception to the rule that a koto command writes
only into the session it opens, ticks, links or imports: they write the pin and
the release record into the named session, which may belong to another agent
session or host. They write only those two records, with no event in the
session's log and no clock moved. A successor under the cloud backend is
another: it writes its back-reference beside its predecessor, which may sit in
another workspace's prefix, and takes it back when it is removed or renamed.
`koto session import` is a third: it writes the copy's relation and a
back-reference beside the source, which may sit in another workspace's prefix,
and the migration marker after the push.
These are tracked with koto's other cross-session writers in
[koto#445](https://github.com/tsukumogami/koto/issues/445).

A read is never one. Outside a session's own tick, `koto status`,
`koto context get`, `koto context exists` and `koto context list` leave the
session's state log, its context key files and, under the cloud backend, its
bucket objects byte-identical, so reading a finished session to decide
whether to keep it doesn't change it. Such a read is logged in the reader's
anchor when `KOTO_ANCHOR` names another agent session's anchor, and in this
host's read spool otherwise, including when the anchor is the session's own.
Only a read inside the session's own tick, such as a gate script reading its
session, is logged in the session. When a transition's assigned value never
reached the store, a read answers it from the session's log without storing
it; the session's own next `koto next` stores it.

The continues relation's writers carry risks this release accepts:

- Anyone who can write a relation or a back-reference can hold a session in
  place until it is removed by hand, and there is no unlink verb. Where
  `retention.keep` serves data minimization, bucket policy should limit who
  can write `ctx/koto/` under each workspace prefix.
- Anyone who can write a successor's own objects (its relation, state or pin)
  can make it look released and so let its predecessor go, as with pins today.
- A back-reference another host writes between a removal's final listing and
  its delete isn't seen, the same window the pin guard accepts.
- Back-references left after their predecessor is gone carry their holders'
  names and session ids in that prefix until each holder is removed.
