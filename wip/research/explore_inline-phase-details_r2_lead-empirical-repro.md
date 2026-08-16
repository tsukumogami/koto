# Lead: Which of the five suspected defects reproduce on current main?

**Tested binary:** built with `cargo build --release` (finished in 1m52s, exit 0) at
commit `1e3a515` ("docs(explore): capture round 1 findings for inline-phase-details").
While I was measuring, HEAD advanced to `1b35372` ("docs(explore): add round 2 leads
for inline-phase-details"); `git diff --stat 1e3a515 1b35372 -- src/ Cargo.toml
Cargo.lock` is empty, so the binary is byte-identical to what current main would
produce. Binary at
`/home/dgazineu/dev/niwaw/tsuku/tsuku+koto_90-f3dfa61e/public/koto/.claude/worktrees/docs+inline-phase-details/target/release/koto`.

**All five claims reproduce.** Every one of them. I also found three things nobody
asked about, one of which is a straightforward correctness bug in `koto rewind` that
has nothing to do with details.

## Environment note that matters for anyone repeating this

My first run produced garbage — `koto next` returned `{"state":null}` for everything.
The cause is not koto's logic: the user's real `~/.koto` holds several hundred legacy
flat-layout sessions, and **every single koto invocation** re-runs a migration scan
that emits one `koto: migration skipped <name>: session already exists` line per
session to stderr. That is ~20KB of stderr per command. It did not corrupt the JSON,
but it made the transcripts unreadable and it is a real performance and usability
wart worth filing separately.

Every measurement below therefore ran with `HOME` pointed at a scratch directory
(`/home/dgazineu/.claude/jobs/95e1f4bf/tmp/r2/home`), giving a clean `~/.koto`. I
confirmed at the end that no `r2repro-*` session leaked into the user's real
`~/.koto` (`find /home/dgazineu/.koto -maxdepth 3 -name "*r2repro*"` returns nothing).

Session files live at `$HOME/.koto/sessions/<name>/koto-<name>.state.jsonl` — a flat
layout, **not** the `~/.koto/sessions/<repo-id>/<name>/` the brief described.

## On the template grammar

The issue author's `states:` YAML block **is** koto's real grammar — that part of the
sketch is fine and compiles. Two other things bit me hard, and both are worth knowing
because they are traps for anyone writing a repro:

1. **An `accepts:` block does not stop advancement.** A transition without a `when:`
   clause is unconditional and fires immediately, regardless of `accepts`. My first
   linear chain (`s1 → s2 → s3 → done`, every state with `accepts`, every transition
   unconditional) auto-advanced through *all four states in a single `koto next`* and
   terminated:

   ```
   ### next (s1, first visit)
   {"action":"done","state":"done","advanced":true,"details_len":0}
   ```

   The session then auto-cleaned itself, so the follow-up `koto rewind` returned
   `{"command":"rewind","error":"workflow 'r2repro-c2' not found"}`. To make a state
   actually wait for evidence you must give its transition a `when:` clause that
   references an accepts field (`when: {status: completed}`).

2. **A template with a cycle breaks `koto next` outright.** My first attempt at a
   cyclic template (for the repeated-`--to` test) had `s2 → s1` and `s3 → s2`
   back-edges. Plain `koto next` on the *initial* state fails immediately:

   ```
   $ koto next r2repro-dbg
   {"error":{"code":"template_error","details":[],"message":"cycle detected: advancement loop would revisit state 's2'"}}
   exit=3
   ```

   Note this fires on the very first tick at `s1`, before anything could possibly
   revisit `s2`. Interestingly `koto next --to <state>` and `koto rewind` still work
   fine on that same template — only the auto-advance path refuses. So the cyclic
   template was still usable for Claim 3.

Templates used: `gateloop.md` (two command-gated states), `linear2.md` (three
evidence states, conditional transitions), `passthru2.md` (auto-advance through an
intermediate), `chain.md` (cyclic, for repeated `--to`). All under
`/home/dgazineu/.claude/jobs/95e1f4bf/tmp/r2/`.

## Findings

Measurement query throughout:
`koto next <name> 2>/dev/null | jq -c '{action, state, advanced, details_len: (.details // "" | length)}'`

The response field is `action` (values `gate_blocked`, `evidence_required`, `done`),
not `status`.

---

### Claim 1 — blocked, non-advancing ticks re-send details: **DEFECT**

Template `gateloop.md`: `alpha` (gate `test -f .../a.txt`) → `beta` (gate
`test -f .../b.txt`) → `done`. Files toggled between ticks.

```
### init
{"name":"r2repro-c1","state":"alpha"}
### tick 1: alpha gate FAILS (a.txt absent) -- first ever tick
{"action":"gate_blocked","state":"alpha","advanced":false,"details_len":183}
### tick 2: alpha still blocked, no transition
{"action":"gate_blocked","state":"alpha","advanced":false,"details_len":183}
### tick 3: alpha still blocked, no transition
{"action":"gate_blocked","state":"alpha","advanced":false,"details_len":183}

### touch a.txt; tick 4: alpha advances -> beta, beta gate fails
{"action":"gate_blocked","state":"beta","advanced":true,"details_len":151}
### tick 5: beta still blocked, no transition
{"action":"gate_blocked","state":"beta","advanced":false,"details_len":151}
### tick 6: beta still blocked, no transition
{"action":"gate_blocked","state":"beta","advanced":false,"details_len":151}
```

Reproduces exactly as the author measured on 0.11.4. Ticks 1–3 resend alpha's 183
bytes; ticks 4–6 resend beta's 151 bytes. Nothing decays.

The mechanism is worse than "the check is off by one". The event log shows why:

```
{"from":null,"to":"alpha","condition_type":"auto"}      <- appended by init
{"state":"alpha","gate":"a_exists","outcome":"failed"}  <- tick 1
{"state":"alpha","gate":"a_exists","outcome":"failed"}  <- tick 2
{"state":"alpha","gate":"a_exists","outcome":"failed"}  <- tick 3
{"state":"alpha","gate":"a_exists","outcome":"passed"}  <- tick 4
{"from":"alpha","to":"beta","condition_type":"auto"}    <- tick 4
{"state":"beta","gate":"b_exists","outcome":"failed"}   <- tick 4
{"state":"beta","gate":"b_exists","outcome":"failed"}   <- tick 5
{"state":"beta","gate":"b_exists","outcome":"failed"}   <- tick 6
```

A blocked tick appends only a `gate_evaluated` event. `derive_visit_counts` counts
only `Transitioned`/`DirectedTransition`/`Rewound`, so the count is *frozen at 1* for
as long as you sit in the state. `full || count <= 1` is therefore permanently true.

The consequence is stronger than the issue states: **for a gate-blocked state the
suppression never fires at all.** The count can only exceed 1 by *re-entering* the
state. A state you enter once and block on forever resends its details on every tick,
indefinitely. This is the exact scenario the details mechanism was built for — an
agent stuck retrying a gate — and it is precisely where it does nothing.

Note also that `init` appends `Transitioned{from:null, to:"alpha"}`, so the initial
state starts at count 1, not 0. That happens to be harmless (1 ≤ 1 is the intended
first-visit behavior) but it means the initial state gets exactly one "free" visit
worth of budget like any other.

---

### Claim 2 — `koto rewind` suppresses details on the rewind target: **DEFECT**

Template `linear2.md`, three evidence states with conditional transitions.

```
### init
{"name":"r2repro-c2","state":"s1"}
### next (s1, first visit)
{"action":"evidence_required","state":"s1","advanced":false,"details_len":77}
### next (s1 AGAIN, no advance)
{"action":"evidence_required","state":"s1","advanced":false,"details_len":77}
### with-data -> s2 (first visit)
{"action":"evidence_required","state":"s2","advanced":true,"details_len":122}
### next (s2 again, no advance)
{"action":"evidence_required","state":"s2","advanced":false,"details_len":122}
### with-data -> s3 (first visit)
{"action":"evidence_required","state":"s3","advanced":true,"details_len":43}
### rewind (expect land on s2)
{"children":[],"children_relocated":0,"name":"r2repro-c2","state":"s2","superseded_branch":null}
### next after rewind (s2) <-- CLAIM 2
{"action":"evidence_required","state":"s2","advanced":false,"details_len":0}
### next --full after rewind (s2)
{"action":"evidence_required","state":"s2","advanced":false,"details_len":122}
```

Confirmed, and the `--full` line is the clincher: the details are still there (122
bytes), the visit-count check is simply refusing to ship them. `s2` was entered once
by `Transitioned` and once by `Rewound`, count 2, suppressed.

This is the worst-flavoured of the five. Rewinding is the operation you reach for
*because* something went wrong and you need to redo a step — the moment you most want
the extended guidance back. The mechanism removes it at exactly that moment.

**Bonus bug found here, unrelated to details.** Consecutive rewinds do not walk
backward; they oscillate. The second `koto rewind` from `s2` went **forward to `s3`**:

```
### rewind again (expect s1)
{"children":[],"children_relocated":0,"name":"r2repro-c2","state":"s3","superseded_branch":null}
### next after 2nd rewind (s1)
{"action":"evidence_required","state":"s3","advanced":false,"details_len":0}
```

`handle_rewind` (src/cli/mod.rs:1985) picks `state_changing[len-2]` — the
second-to-last state-changing event — but the `Rewound` event it just appended is
itself state-changing, so after one rewind the second-to-last entry is
`Transitioned{to: s3}`. Rewind therefore ping-pongs `s2 ↔ s3` forever and can never
reach `s1`. Worth its own issue.

---

### Claim 3 — `--to` never suppresses details: **DEFECT**

Template `chain.md` (cyclic, so `s2` and `s3` are mutual transition targets).

```
### init
{"name":"r2repro-c3","state":"s1"}
-- next --to s2 (FIRST directed entry into s2)
{"action":"evidence_required","state":"s2","advanced":true,"details_len":166}
-- next --to s3
{"action":"evidence_required","state":"s3","advanced":true,"details_len":43}
-- next --to s2 (SECOND directed entry into s2)
{"action":"evidence_required","state":"s2","advanced":true,"details_len":166}
-- next --to s3 (SECOND directed entry into s3)
{"action":"evidence_required","state":"s3","advanced":true,"details_len":43}
-- next --to s2 (THIRD directed entry into s2)
{"action":"evidence_required","state":"s2","advanced":true,"details_len":166}
```

Confirmed. Three directed entries into `s2`, 166 bytes every time, even though each
one appends a `DirectedTransition` that `derive_visit_counts` does count. The count
reaches 3 and nothing changes.

The reason is structural, not a threshold problem: the `--to` branch (src/cli/mod.rs
~3336–3355) appends the `DirectedTransition` and then calls `dispatch_next` directly.
`dispatch_next` (src/cli/next.rs:50-54) builds `details` from
`template_state.details` with no visit check whatsoever — the check lives only in
`handle_next`'s advance-loop result path (src/cli/mod.rs:4008-4010), which the
directed path never reaches.

So the suppression rule is not one rule with a bad threshold. It is a rule that
exists on one of two code paths, and the two paths disagree.

---

### Claim 4 — auto-advanced intermediate states never surface details: **DEFECT**

Template `passthru2.md`: `entry` (evidence) → `middle` (unconditional, auto-advances)
→ `last` (evidence).

```
### init
{"name":"r2repro-c4","state":"entry"}
-- next (entry, first visit)
{"action":"evidence_required","state":"entry","advanced":false,"details_len":40,
 "directive":"ENTRY DIRECTIVE: submit status completed."}
-- with-data -> auto-advance entry -> middle -> last
{"action":"evidence_required","state":"last","advanced":true,"details_len":38,
 "directive":"LAST DIRECTIVE: blocked here until evidence arrives."}
-- next --full on last (does middle ever surface?)
{"action":"evidence_required","state":"last","advanced":false,"details_len":38,
 "directive":"LAST DIRECTIVE: blocked here until evidence arrives."}
-- did MIDDLE DETAILS ever appear in any response?
0
-- event log confirms middle WAS visited:
{"to":"entry","from":null}
{"to":"middle","from":"entry"}
{"to":"last","from":"middle"}
```

Confirmed, and note it is not just details — `middle`'s **directive** never surfaces
either. The event log proves `middle` was genuinely entered exactly once
(`Transitioned{from:"entry", to:"middle"}`, visit count 1, squarely inside the
first-visit window), yet neither its directive nor its details reach the caller in
any response, including `--full`. `grep -c "MIDDLE DETAILS"` over the `--full` output
returns 0.

The advance loop keeps only `final_state` and looks up `compiled.states[final_state]`
for both directive and details. Everything crossed on the way is discarded. So this
is arguably not a details bug at all — it is a pre-existing property of auto-advance
that the details feature inherited. If a template author puts real instructions in an
auto-advancing state, they are unreachable by construction.

---

### Claim 5 — `koto next --full` is not a safe read: **DEFECT (confirmed)**

Two separate measurements.

**(a) Does a non-advancing tick mutate the log?** Yes, when the state has gates.

```
-- lines after init: 4
-- next (alpha blocked)
{"action":"gate_blocked","state":"alpha","advanced":false}
-- lines after non-advancing next: 5
-- events appended by that non-advancing tick:
{"seq":4,"timestamp":"2026-08-16T18:57:30.879Z","type":"gate_evaluated",
 "payload":{"state":"alpha","gate":"a_exists","output":{"error":"","exit_code":1},
            "outcome":"failed","timestamp":"2026-08-16T18:57:30.879Z"}}
```

Exactly one `gate_evaluated` event per gate, per tick. Nothing else. On a **gateless**
evidence state a non-advancing tick appends nothing at all (verified separately:
7 lines before, 7 after). So the append is purely the gate-evaluation record.

Beyond the append, the tick **executes every gate command** — arbitrary shell, in my
template `test -f ...`, in real templates whatever the author wrote. That alone
disqualifies `koto next` as a read.

**(b) Can `--full` advance the workflow?** Yes, and it did.

```
-- NOW: gate passes (touch a.txt). Run 'koto next --full' as a supposedly safe read:
{"action":"gate_blocked","state":"beta","advanced":true,"details_len":151}
-- lines before: 5  after: 8
-- events appended by the --full call:
{"state":"alpha","gate":"a_exists","output":{"error":"","exit_code":0},"outcome":"passed",...}
{"from":"alpha","to":"beta","condition_type":"auto"}
{"state":"beta","gate":"b_exists","output":{"error":"","exit_code":1},"outcome":"failed",...}
```

`advanced: true`, `state` moved `alpha → beta`, three events appended including a real
`Transitioned`. `--full` is a plain `koto next` with one boolean flipped in the
details branch; it shares the entire advance path.

**This settles the context-recovery question: `koto next --full` cannot be used to
re-read the current phase.** An agent that has lost context and calls it to
re-orient will, if the gates happen to pass, silently advance the workflow past the
state it was trying to read — and in a template with a terminal state within reach,
could run the workflow to completion and trigger auto-cleanup. Any design that
proposes `--full` as the recovery affordance is proposing a footgun.

---

### Bonus: `koto status` and `koto phase-info`

`koto status` is genuinely read-only and genuinely useless for this purpose:

```
$ koto status r2repro-c4
{
  "current_state": "last",
  "is_terminal": false,
  "name": "r2repro-c4",
  "template_hash": "cfca61a3df9d8394c7100cfc037c1510c7515e7da7be29fe70124e6be1390203",
  "template_path": "/home/.../.cache/koto/cfca61a3....json"
}
```

Grepping the output for `directive|details|DIRECTIVE|DETAILS` returns nothing. It
gives you the state *name* and a path to the compiled template JSON, and that is all.
Its help text describes it as "Show the current status of a workflow (read-only, no
state changes)" — so the read-only seam exists, it just carries no instructional
content.

`koto phase-info` does not exist:

```
$ koto phase-info foo
error: unrecognized subcommand 'phase-info'

Usage: koto <COMMAND>

For more information, try '--help'.
exit=2
```

Exit code 2, clap's standard unrecognized-subcommand error. The full command list is
`version, init, next, cancel, rewind, workflows, template, session, context, status,
decisions, overrides, config, workspace, request, dashboard, help`.

## Implications

**Five claims, five reproductions — but they are not five independent defects.** They
decompose into three distinct root causes plus one inherited limitation:

1. **The visit counter measures the wrong thing** (Claims 1 and 2). It counts
   *entries into* a state, not *responses sent about* a state. Since a blocked tick
   entering nothing increments nothing, sitting still resends forever; since a rewind
   is an entry, redoing a step suppresses. The counter is inverted relative to intent
   on both ends. Any fix that keeps `derive_visit_counts` as the input will keep both
   symptoms.

2. **The rule is implemented on one of two code paths** (Claim 3). `dispatch_next`
   has no check; `handle_next`'s advance path has one. This is not a tuning problem,
   it is a missing call site. Note also that whatever contract round 1 inferred from
   reading `dispatch_next` alone would have been wrong about the advance path, and
   vice versa — the two paths genuinely disagree, and a reader who found one may not
   have found the other.

3. **`--full` is not a read** (Claim 5). This kills the "just use `--full` for
   context recovery" answer outright. `koto status` is the only true read seam and it
   carries no directive or details. If the exploration wants a recovery affordance,
   something new has to exist — a `--dry-run` on `next`, an extension to `status`, or
   the `phase-info` command the issue contemplates. Confirming `phase-info` does not
   exist means this is greenfield, not a fix.

4. **Auto-advance discards crossed states** (Claim 4). This one is different in kind
   from the other four: it is not a details bug, it predates details, and it swallows
   the directive too. Whether it belongs in this issue's scope is a judgment call, but
   it should be *named* as pre-existing rather than counted as a details regression —
   otherwise a fix will get scoped as "make details work on crossed states" when the
   honest framing is "auto-advance has never surfaced intermediate instructions at
   all."

For anyone weighing designs: the mechanism's stated goal is to spare an agent from
re-reading long guidance on repeat visits. Empirically it does the opposite on the
repeat-visit case that actually matters (Claim 1, blocked retries — never suppresses)
and fires on the case where you want the guidance most (Claim 2, rewind — always
suppresses). The behavior is close to exactly backwards from the intent.

## Surprises

- **The rewind ping-pong bug** (documented under Claim 2). Two consecutive rewinds
  move you *forward*. `s1` is unreachable by rewinding once you have gone past it.
  This is a plain correctness bug in `handle_rewind`, independent of issue #90, and I
  did not go looking for it — it fell out of setting up the details test.

- **`accepts:` does not gate advancement.** I assumed an accepts block made a state
  wait. It does not; the transition's `when:` clause does. My first linear chain ran
  four states to terminal in one tick. This is a genuine authoring trap and it means
  some existing templates may be auto-advancing through states their authors think
  are interactive.

- **Cyclic templates break `koto next` but not `--to`/`rewind`.** The cycle detector
  fires on the first tick at the initial state, before any revisit could occur, with
  `template_error` exit 3. Yet directed transitions around the same cycle work fine.
  That inconsistency may be intentional, but it means "can this template express a
  loop?" has different answers depending on which verb you use.

- **`middle`'s directive is dropped too, not just its details.** I expected Claim 4 to
  be about details. It is broader.

- **The migration-scan stderr flood.** Several hundred lines of
  `migration skipped ...` on every single koto invocation against the user's real
  `~/.koto`. Cosmetic, but it broke my first two runs and it will break anyone else's.

- **`--full` after a rewind returns the details fine** (122 bytes). The suppression is
  purely the counter's decision; nothing is lost or unavailable. That is mildly
  encouraging for fix feasibility — the data is always there, only the predicate is
  wrong.

## Open Questions

- Should the counter be replaced by a "have I already *sent* these details on this
  visit?" record — i.e. an event appended when details ship, scoped to the current
  epoch? `latest_epoch_gate_failed` (persistence.rs) already establishes an
  epoch-slicing helper that could be reused. I did not test whether such an event
  type exists.

- Is the `--to` path's lack of a check deliberate (directed transition = explicit
  operator intent = always show full context) or an oversight? The code carries no
  comment either way. If deliberate, Claim 3 is not a defect and the count is four,
  not five — this needs an author/design-doc answer, not more measurement.

- Does the rewind ping-pong bug affect the `materialize_children` epoch-branch
  relocation logic that `handle_rewind` also drives? I saw `children_relocated: 0`
  throughout because my templates had no children. Untested.

- Should auto-advance concatenate crossed states' directives into the response rather
  than discarding them? That is a design question this measurement can inform but not
  answer.

- I did not test details behavior under `koto next --with-data` combined with gate
  failure (the accepts-fallthrough path at src/cli/next.rs:56-69, where a gated state
  with an `accepts` block falls through to `EvidenceRequired` instead of
  `GateBlocked`). That path also carries `details.clone()` and may have its own
  visit-count interaction.

## Summary

All five claimed defects reproduce on the current source (`1e3a515`, source-identical
to HEAD `1b35372`): blocked ticks resend details forever because the visit counter
freezes when you do not transition, rewind suppresses them because it counts as a
re-entry, `--to` never suppresses because the directed path calls `dispatch_next`
which has no check at all, auto-advanced intermediate states surface neither details
nor even their directive, and `koto next --full` demonstrably advanced my workflow
from `alpha` to `beta` while appending three events — so it cannot serve as a
context-recovery read, and `koto status` (the only true read seam) returns no
directive or details, while `koto phase-info` does not exist (exit 2). These collapse
into three real root causes plus one pre-existing auto-advance limitation, and the
net effect is that the mechanism withholds guidance precisely when an agent most
needs it (after a rewind) while never withholding it in the retry loop it was built
for. The biggest open question is whether the `--to` path's missing check is
deliberate design intent rather than an oversight, since that alone decides whether
this issue is scoped as four defects or five.
