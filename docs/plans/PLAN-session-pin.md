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

**Goal**: The terminal tick, the parent sweep, the same-name replace, `koto workspace prune`, `koto session cleanup` and `koto cancel --cleanup` keep or refuse a pinned session through the one removal guard, and their output carries the pin. After this step a session pinned by hand on the local backend survives every removal koto makes, and each one says why.

**Acceptance Criteria**:

- [ ] Each step of the PRD's removal set S (terminal tick, parent sweep, `koto session prune --apply --all`, `koto workspace prune --root X --yes`, `koto session cleanup P`, same-name replace) leaves a pinned P's log and keys hashing the same as before the step (after the replace, under the name the replace reports), and its output carries `kept_by_pin: true` and a `pin` with `reason` `record of the merged change`, `set_by: null`, P's `set_at` and `source: local`. The text form prints `pinned: record of the merged change (set by hand at <time>)`.
- [ ] A terminal tick of a pinned session prints `retention.reason` `pinned`, with and without `--no-cleanup`, and for a session pinned while running. A pinned session ticked to a failure terminal prints a `retention` carrying both the pin and `failure_terminal`. The unpinned U is still removed at the same tick, as on a build without the feature.
- [ ] `koto session cleanup P` and `koto cancel P --cleanup`, on a finished and on a running pinned session, exit 2, write nothing (no event, P hashes the same), and name the pin's reason, setter and time and say to unpin first. `koto cancel R` without `--cleanup` on a running pinned R exits 0, cancels it and leaves it pinned. `koto session cleanup nosuch` behaves as on a build without the feature.
- [ ] A same-name replace of a pinned finished predecessor moves it aside under the new name its answer reports, with its log, keys, pin and release record unchanged. The new run holds the original name with no pin, and `koto session hierarchy` for the moved name shows the same pin.
- [ ] With P finished and past the retention, `koto workspace prune --root X --yes --force`, and `koto session prune --apply` with each of `--all`, `--execution-dir <P's directory>`, `--older-than 1s` and `--apply --plan` on a plan that listed P, each keep P and name the pin.
- [ ] A pin set late is honoured. With stdin held open, `koto workspace prune --root X` lists P for removal and waits; P is pinned from another process; after the test confirms, P is kept and its entry carries the pin. The same holds when P is pinned between `koto session prune` writing a plan and `--apply --plan` running it.
- [ ] A pin keeps its session only. With A pinned, B dispatched by A, and both finished past the retention, `koto session prune --apply --all` keeps A with the pin and removes B. A pinned child keeps neither its parent nor its own children once they are released. `koto session prune` and `koto workspace prune` remove the same sessions from the same fixture.
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

The hierarchy query and `koto session list --pinned` show pins, release records and unreadable pins by name, and an unreadable pin keeps its own session and nothing else.

### Step 4: an agent session unpins only when its start was handed release-pin

An agent session whose start was handed `KOTO_AUTHORITY=release-pin` unpins and the release record names it; one without it, a child, a successor, or one that exports the variable later is refused naming `release-pin`.

### Step 5: under the cloud backend a second host keeps a pinned session and reports the bucket's pin

On two hosts sharing a bucket, every removal on the second host keeps a pinned session from the bucket's pin, a failed bucket check keeps and says so, and pin and unpin fail loudly when the bucket can't take them.

### Step 6: an imported pinned session stays pinned and a koto v0.23.0 host keeps it

Import carries a pin and its release record, koto v0.23.0's prunes, terminal tick and replace keep a pinned session as unreadable, and the changelog and guides say which releases honour pins.

## Implementation Sequence

The steps run in order. Steps 3, 4 and 5 each need only step 2 and may be
detailed and built in any order after it; step 6 follows step 5. The plan's
owner details step 3 after step 2's work reaches its terminal, and each later
step the same way, in a commit that touches only this plan.
