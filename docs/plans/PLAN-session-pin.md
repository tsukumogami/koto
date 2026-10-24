---
schema: plan/v2
status: Active
execution_mode: single-pr
split_mode_source: none
tracking_level: none
upstream: docs/designs/DESIGN-session-pin.md
milestone: "Pin a finished session so no removal rule takes it"
step_count: 6
---

# PLAN: Pin a finished session so no removal rule takes it

## Status

Active

## Scope Summary

Pin a koto session with a reason so no removal by rule, by subtree or by
explicit cleanup takes it on any host sharing the store, and release it only
through an unpin that a caller by hand, or an agent session whose start was
handed `release-pin`, may make. The work lands in one pull request in this
repository.

## Decomposition Strategy

Walking skeleton. Step 1 pins one finished session by hand on the local
backend and runs `koto session prune --apply --all` past the retention: the
pin is written, the scan reads it as a per-session fact, the pinned session is
kept and named while its unpinned sibling goes, and an unpin by hand lets the
next prune take it. Step 2 puts every other removal behind the same guard.
Later steps add the reporting surfaces and unreadable pins, release authority,
the cloud backend, and import and older-build compatibility, each on top of
the path step 1 opens.

## Steps

### Step 1: a pinned session survives koto session prune, which names its pin, until it is unpinned by hand

**Goal**: The CLI reaches the pin record, the engine's per-session fact and the prune's apply and report, run on one finished local session and one unpinned sibling dispatched by it. After this step an operator can pin a finished session with a reason, see `koto session prune --apply --all` keep it and say why while removing its unpinned sibling, and release it by hand so the next prune takes it.

**Acceptance Criteria**:

- [ ] `koto session pin A --reason demo` on a finished local session exits 0 and prints JSON with `name` `A`, `pin.reason` `demo`, `pin.set_by` `null`, a `pin.set_at` timestamp and `store` `local`; running it again with a different reason also exits 0, prints the first pin unchanged and leaves A's files as they were
- [ ] A reason that is empty or only whitespace, one of 501 Unicode scalar values, or one holding a tab, a newline or U+0007 makes `koto session pin` exit 2 and write no pin; a reason of exactly 500 scalar values, multibyte characters included, is accepted
- [ ] With A pinned, finished and past the retention, `koto session prune --apply --all` lists A as kept with `kept_by_pin: true` and a `pin` object (`reason` `demo`, `set_by` `null`, `set_at`, `source` `local`), and A's log and keys hash the same afterwards; the text form prints `pinned: demo (set by hand at <time>)`
- [ ] The same prune removes B, which A dispatched and which holds no pin: a pin keeps only its own session, and B's entry carries none of the pin fields
- [ ] Pinning changes no hierarchy: `koto session hierarchy` for A and for B lists the same members and the same dispatched-by edge before and after the pin is set, and its output for A states that the pin keeps A only
- [ ] `koto session unpin A` with no `KOTO_ANCHOR` exits 0 and prints a release record (`released_by` `null`, `authority` `null`, and the released pin's reason, setter and time); the very next `koto session prune --apply --all` removes A
- [ ] `koto context add A koto/links/pin ...` and `koto context remove A koto/links/pin` each exit 2 and leave the pin as it was
- [ ] Under `KOTO_ANCHOR=<an anchor>`, `koto session unpin A` exits 2 with an `authority_required` error naming `release-pin`, writes no release record and leaves A pinned; for a session that holds no pin the prune output is byte for byte what a build without this feature prints

**Check**: In a store isolated by a fresh HOME, run the walkthrough below with the local backend. `term.md` stands for a one-state template that reaches its terminal state on the first `koto next`; the fixture supplies it.

```bash
export HOME="$(mktemp -d)"
export KOTO_RETENTION_KEEP=1s
unset KOTO_ANCHOR

koto init A --template term.md
koto init B --template term.md
koto session link B --dispatched-by A
koto next A --no-cleanup        # A reaches its terminal state and stays
koto next B --no-cleanup        # B reaches its terminal state and stays

koto session pin A --reason demo
# expect exit 0 and JSON: name A, pin.reason demo, pin.set_by null, pin.set_at, store local

sleep 2                          # B and A are now past the 1s retention
koto session prune --apply --all
# expect JSON: A listed as kept with kept_by_pin true and pin.reason demo;
# B listed as removed; `koto session list` still shows A and no longer shows B

koto session unpin A
# expect exit 0 and a release record with released_by null, authority null
koto session prune --apply --all
# expect JSON: A removed; `koto session list` is empty
```

The walkthrough passes when each expect line holds, in order, including the exit codes.

**Dependencies**: None

**Files**: `src/session/pin.rs`, `src/session/links.rs`, `src/session/hierarchy/facts.rs`, `src/session/hierarchy/judge.rs`, `src/session/hierarchy/render.rs`, `src/cli/session.rs`, `src/cli/session_prune.rs`, `src/cli/mod.rs`, `src/session/import_links.rs`

### Step 2: every other removal keeps a pinned session and says why

**Goal**: The terminal tick, the parent sweep, the same-name replace, `koto workspace prune`, `koto session cleanup` and `koto cancel --cleanup` keep or refuse a pinned session through the one removal guard, and their output carries the pin. Step 1 left `koto session prune` reading the local pin directly; this step adds the guard (`guard_removal` and `remove_guarded`, local read only for now) and moves the prune's delete onto it too, so every removal decides through one path. After this step a session pinned by hand on the local backend survives every removal koto makes, and each one says why.

**Acceptance Criteria**:

- [ ] Each step of the PRD's removal set S (terminal tick, parent sweep, `koto session prune --apply --all`, `koto workspace prune --root X --yes`, `koto session cleanup P`, same-name replace) leaves a pinned P's log and keys hashing the same as before the step (after the replace, under the name the replace reports), and its output carries `kept_by_pin: true` and a `pin` with `reason` `record of the merged change`, `set_by: null`, P's `set_at` and `source: local`. The text form prints `pinned: record of the merged change (set by hand at <time>)`.
- [ ] A terminal tick of a pinned session prints `retention.reason` `pinned`, with and without `--no-cleanup`, and for a session pinned while running. A pinned session ticked to a failure terminal prints a `retention` carrying both the pin and `failure_terminal`. The unpinned U is still removed at the same tick, as on a build without the feature.
- [ ] `koto session cleanup P` and `koto cancel P --cleanup`, on a finished and on a running pinned session, exit 2, write nothing (no event, P hashes the same), and name the pin's reason, setter and time and say to unpin first. `koto cancel R` without `--cleanup` on a running pinned R exits 0, cancels it and leaves it pinned. `koto session cleanup nosuch` behaves as on a build without the feature.
- [ ] A same-name replace of a pinned finished predecessor moves it aside under the new name its answer reports, with its log, keys, pin and release record unchanged. The new run holds the original name with no pin, and `koto session hierarchy` for the moved name shows the same pin.
- [ ] With P finished and past the retention, `koto workspace prune --root X --yes --force`, and `koto session prune --apply` with each of `--all`, `--execution-dir <P's directory>`, `--older-than 1s` and `--apply --plan` on a plan that listed P, each keep P and name the pin.
- [ ] A pin set late is honoured. With stdin held open, `koto workspace prune --root X` lists P for removal and waits; P is pinned from another process; after the test confirms, P is kept and its entry carries the pin. The same holds when P is pinned between `koto session prune` writing a plan and `--apply --plan` running it.
- [ ] A pin keeps its session only. With A pinned, B dispatched by A, and both finished past the retention, `koto session prune --apply --all` keeps A with the pin and removes B. A pinned child keeps neither its parent nor its own children once they are released. `koto session prune` and `koto workspace prune` remove the same sessions from the same fixture.
- [ ] A child is pinned on its own and found by its own name and pin, with or without its lineage to X. With koto children C1, C2 and C3 of an unpinned X, all finished and only C2 pinned, the parent sweep at X's terminal tick and `koto workspace prune --root X --yes` past the retention keep C2 with `kept_by_pin: true` and its pin and remove C1 and C3; with X pinned too, X is also kept and named; a released same-name replace of X sweeps C1 and C3 and keeps C2; C2's files and pin hash the same across each run.
- [ ] A pinned child outlives its parent. Once X is removed, `koto status C2` and `koto context get C2 <key>` still read C2 by name, its hierarchy query names X as an absent peer, and every later removal keeps C2 until `koto session unpin C2`, after which the next prune past the retention removes it.
- [ ] No output surface touched by this step describes a session with a readable pin as alive or as unreadable on the pin's account. A pinned session that is also held by a live anchor carries the pin as well as the anchor.
- [ ] Output for sessions nobody pinned is unchanged: U's entries, and the entry for a finished session C in a pinned session's hierarchy held by a live anchor, equal byte for byte the same entries from the same fixture without the pin. They carry none of `kept_by_pin`, `pin` or `pin_unreadable`.

**Check**: On the local backend, build the PRD's setup fixture: retention 20s, idle period 10s and stale threshold 60s set through the environment, every agent session deleted and its anchor closed, calls by hand. Create P and U as running koto sessions in one store with X as P's koto parent, then pin P and record a hash of its log and keys. Run the removal set S in this order and read each output.

```sh
koto session pin P --reason 'record of the merged change'

# S1: tick P and U to their success terminal, without --no-cleanup
#   P: kept, retention.reason == "pinned", kept_by_pin true, pin.reason as above
#   U: removed (koto session list no longer shows U)
# S2: tick X to its terminal, so the parent sweep runs
#   P appears in kept_children with kept_by_pin true and pin.reason
# S3: wait 25 seconds past the hierarchy's release instant, then
koto session prune --apply --all
#   P kept and named in preview and apply entries, no alive keeper, no unreadable item
# S4: in a fresh fixture where X finished with --no-cleanup
koto workspace prune --root X --yes
#   P in the kept list with kept_by_pin true
# S5:
koto session cleanup P ; echo "exit $?"
#   exit 2, message names reason, setter and time, says unpin first; P hashes the same
# S6:
koto init P --template term.md --replace-terminal   # same name as P
#   answer reports a new name for the moved P; the new run holds P's name, unpinned
```

After each of S1 to S5 compare P's hash with the recorded one (after S6, hash the moved copy), and see `kept_by_pin: true` and `pin.reason` in every output above. Then repeat the flag cases from the criteria (`workspace prune --force`, each `session prune` filter, `--apply --plan`) with P past the retention, run `koto cancel R --cleanup` and `koto cancel F --cleanup` on a running and a finished pinned session and see exit 2 with no event written, and run `koto cancel R` and see it cancel and stay pinned. Finally diff U's and C's outputs against the same fixture built without any pin and see no difference.

**Dependencies**: Step 1

**Files**: `src/session/pin.rs`, `src/cli/mod.rs`, `src/cli/workspace.rs`, `src/cli/init_entry.rs`, `src/cli/session_prune.rs`, `src/cli/next_types.rs`, `src/session/hierarchy/render.rs`

### Step 3: koto session hierarchy and koto session list --pinned show pins, and an unreadable pin keeps only its session

**Goal**: An operator reads every pin, its release record and every unreadable pin from the hierarchy query and the pin listing, and a damaged pin keeps its own session and nothing else, on the local backend.

**Acceptance Criteria**:

- [ ] `koto session hierarchy P` (text and JSON) shows P's pin (`reason`, `set_by`, `set_at`), its latest release record after an unpin, and that the pin keeps P only; for B in P's hierarchy it lists `P` as a pinned member; P's files hash the same before and after the query.
- [ ] With three pinned sessions (one running, one with an unreadable pin) and ten unpinned, `koto session list --pinned` lists exactly the three, sorted by name, each with its pin or `pin_unreadable` and its state; with none pinned it prints an empty list and exits 0.
- [ ] For each damaged pin (invalid content, 5 KiB of valid content, a symlink, a directory, an index entry whose file is gone), every removal in the PRD's set S keeps P and reports `pin_unreadable` as `unparseable`, `too_large`, `not_a_file`, `not_a_file` and `missing_content`; a finished session that P dispatched is removed by the prune after the retention.
- [ ] A pin file present with its index entry removed keeps P through S and is reported as a readable pin.
- [ ] Pinning a session with an unreadable pin exits 2 naming it; unpinning it by hand exits 0 and the release record's pin fields are `null`.
- [ ] Pin, unpin and the release record add no event to P's log, and the hierarchy query reports the same stop instant, release instant and anchor `last_activity` before and after.
- [ ] A pinned child whose `parent_workflow` lineage to its parent was removed is kept and also listed, with `kept_by_pin: true` and its pin, in the parent sweep's kept-children entries and `koto workspace prune`'s kept list (step 2 keeps it but doesn't list it).
- [ ] A pinned session held by a live anchor is reported with both the pin and the anchor in the hierarchy query and in a prune preview, and the anchor's keeper entry is unchanged by the pin.

**Check**: Build the fixture with HOME set to a fresh temporary directory, pin P by hand, replace its pin file by each damaged form in turn, and after each run `koto session prune --apply --all --json` and `koto session list --pinned --json`; see P kept with the named `pin_unreadable` and its dispatched child removed. Then compare `koto session hierarchy P --json` before and after a pin and an unpin and see the same stop instant, release instant and `last_activity`.

**Dependencies**: Step 2

**Files**: `src/session/pin.rs`, `src/session/hierarchy/facts.rs`, `src/session/hierarchy/render.rs`, `src/cli/session.rs`

### Step 4: an agent session unpins only when its start was handed release-pin

**Goal**: The start hook records the authority handed in `KOTO_AUTHORITY` in the agent session's anchor and removes the variable from its shells, and `koto session unpin` under an anchor decides from that record alone, run with synthetic hook inputs and no niwa.

**Acceptance Criteria**:

- [ ] A start with `KOTO_AUTHORITY=release-pin` leaves the anchor recording `release-pin`, and the session's env file unsets `KOTO_AUTHORITY`; `koto session unpin P` under that anchor exits 0 and its release record has `released_by` naming the anchor and `authority: release-pin`.
- [ ] A start with no `KOTO_AUTHORITY` whose anchor records no authority leaves the anchor's files hashing the same and makes no bucket request; one after a `release-pin` start clears the record.
- [ ] Under an anchor whose start had no `KOTO_AUTHORITY`, or was handed only `bogus`, `koto session unpin P` exits 2, writes no release record, leaves P pinned, and names `release-pin`, the anchor, and that the authority is handed at the agent session's start; `bogus` appears under `unknown_authorities` in the anchor's hierarchy query.
- [ ] `KOTO_AUTHORITY=release-pin koto session unpin P` under an anchor whose start was handed nothing is refused the same way.
- [ ] A `claude -p` child launched from the session's shell, a successor anchor whose own start was handed nothing, and a later start of the same agent session handed nothing are each refused.
- [ ] A `KOTO_ANCHOR` naming no session, an ordinary session, or an unreadable anchor is refused, saying the anchor's authority couldn't be read.
- [ ] The setter's own anchor, holding no authority, is refused; P stays pinned after its setter's anchor is closed, idles past the idle period, and is removed.
- [ ] An unpin under an anchor leaves that anchor's log and the host's read spool hashing the same; a call by hand still unpins.

**Check**: Run the hook script with synthetic SessionStart inputs and `KOTO_AUTHORITY=release-pin` in its environment, read the anchor's recorded authority with `koto session hierarchy <anchor> --json` and grep the env file for `unset KOTO_AUTHORITY`; then run `koto session unpin P` with `KOTO_ANCHOR` set to that anchor (exit 0) and to an anchor started without the variable (exit 2, message names `release-pin`).

**Dependencies**: Step 2

**Files**: `plugins/koto-skills/hooks/session-start.sh`, `src/session/anchor.rs`, `src/cli/session_anchor.rs`, `src/session/pin.rs`

### Step 5: under the cloud backend a second host keeps a pinned session and reports the bucket's pin

**Goal**: Two hosts share a stand-in bucket with the cloud backend set by environment; every removal on the second host keeps a session the first host pinned, from the bucket's pin object, and pin and unpin write only their own objects.

**Acceptance Criteria**:

- [ ] Host 2, which never pulled P, gets exit 2 from `koto session cleanup P`, naming the pin with `source: bucket`, and the stand-in records no delete under P's prefix.
- [ ] With host 2 holding a copy of P taken before the pin, its `koto session prune --apply --all`, `koto workspace prune --root X --yes`, terminal tick of X, same-name replace of P, terminal tick of a session in P's hierarchy and `koto cancel P --cleanup` each keep P in the bucket and report `source: bucket`; the unpinned U is removed by the same commands.
- [ ] A host 2 context write into P that pushes an index without the pin doesn't unpin: the removals above still keep P and the pin object is still in the bucket.
- [ ] P held by an anchor that lives only in host 1's local store (opened with `KOTO_SESSION_*` dropped) is kept on host 2 from its pin alone, and the report names the pin, not the anchor.
- [ ] `koto session pin P` from host 2 holding a stale copy, and from a host holding no copy, exits 0 and the stand-in records a write of the pin object only, with no write of P's log, state, index or version objects; `koto session unpin P` writes only the release object and deletes only the pin object.
- [ ] With writes refused, `koto session pin P` exits 3 and neither the bucket nor any local copy holds a pin; running it again after writes are accepted exits 0. With deletes refused, `koto session unpin P` exits 3 and P stays pinned.
- [ ] With the stand-in failing listings, and returning an unparseable listing, cleanup and cancel `--cleanup` exit 3 and delete nothing, and both prunes, the terminal tick and the replace keep and report `bucket_unchecked` with the matching `error`.
- [ ] An unpin on one host frees P on the other: host 1's stale local pin file is treated as released because the bucket's release record names its `id` (a pin set, released and set again within one second is told apart), and the next prune after the retention removes P from the bucket; a whole pull on host 1 from an index lacking the pin still shows P pinned.
- [ ] A session pinned under the local backend, after the switch to cloud and its first push, has its pin in the bucket as its own object and no pin entry in its local index, and host 2 keeps it with `source: bucket`.
- [ ] After host 1 pins and unpins P from its local copy, its sync cache reads P as current (not ahead or diverged), its local index has no pin entry, and a later `koto context add P <key>` from host 1 leaves the bucket's pin and release objects as they were.
- [ ] For a prune over 20 finished sessions past the retention, 5 pinned in the bucket only, the stand-in counts at most 20 listings and 5 pin reads beyond what the same prune makes without this feature.

**Check**: Run the two-host stand-in test: two working copies at the same canonical path with separate local stores and the cloud backend set by environment; pin P on host 1, run each removal on host 2, and see P's objects listed the same before and after with `source: bucket` in every report, then read the stand-in's request log for the stale-copy pin and see a single PUT of the pin object.

**Dependencies**: Step 2

**Files**: `src/session/cloud.rs`, `src/session/sync.rs`, `src/session/mod.rs`, `src/session/pin.rs`

### Step 6: an imported pinned session stays pinned and a koto v0.23.0 host keeps it

**Goal**: A pin travels through `koto session import` and a koto v0.23.0 release keeps a pinned session on the same store, and the guides say which releases honour pins.

**Acceptance Criteria**:

- [ ] Importing P (pinned, with a release record from an earlier pin) leaves the moved copy with the same pin and release record, and the removal set S there keeps it; an unreadable pin is carried unchanged.
- [ ] Importing an anchor that records `release-pin` leaves the moved copy with no recorded authority.
- [ ] After the import, `koto session unpin` on the migrated source exits 2 and writes nothing, and `koto session prune --migrated-from <source workspace>` removes the source copy while the moved copy stays pinned.
- [ ] With a pin written by this build, the koto v0.23.0 release binary (checked against a SHA-256 digest committed to the repository, run with a scrubbed environment) keeps P through `koto session prune --apply --all`, `koto workspace prune --root X --yes`, a terminal tick of X and a same-name replace, and its prune entry for P lists an unreadable item naming the pin's record.
- [ ] The changelog entry and the session-retention guide name the first release that honours pins in full, name `koto session cleanup`, `koto cancel --cleanup` and `koto session import` as not honouring pins in older releases, and list pin and unpin among the known cross-session writers with a link to koto#445; each guide that runs `koto session cleanup` as a recovery step has an unpin-first line, checked by a doc test.
- [ ] The R28 local cost test with 200 pins passes on the CI runner.

**Check**: Import a pinned P with the per-command cloud configuration in the environment and run S in the new workspace, seeing P kept and pinned; then run the opt-in CI job that fetches the v0.23.0 binary, verifies its digest and runs its prune on a store holding a pinned session, seeing P kept with an unreadable-record reason; then run the doc test.

**Dependencies**: Step 5

**Files**: `src/session/import_links.rs`, `CHANGELOG.md`, `docs/guides/`, `plugins/koto-skills/skills/koto-user/references/`, `.github/workflows/`

## Implementation Sequence

The steps run in order. Steps 3, 4 and 5 each need only step 2 and may be
detailed and built in any order after it; step 6 follows step 5. The plan's
owner revisits each step after the one before it reaches its terminal, and
commits any change to a later step on its own, touching only this plan.
