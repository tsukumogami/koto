# Verdict: PASS

Re-review of the revised `docs/designs/DESIGN-self-loop-suppresses-details.md`,
read fresh from disk. The blocking finding is fixed. Every other finding is
resolved or resolved-with-residue. What is left is two sentences that survived
the revision and now contradict the passages that replaced them, plus three
imprecisions in the new Security Considerations text. None of them change what an
implementer builds.

## Re-review

| # | Finding | Severity (first pass) | Status | Where |
|---|---|---|---|---|
| 1 | Decision 3 committed to editing a fixture `label` the PRD forbids | BLOCKING | **Partially resolved** — decision text fixed, one stale summary line left | `:182-191`, `:412-413` fixed; `:512-514` stale |
| 2 | "agree by coincidence" / "behavior-preserving refactor" both false | MODERATE | **Partially resolved** — both load-bearing passages fixed, driver bullet left | `:68-74`, `:325-331` fixed; `:116-118` stale |
| 3 | Eval suite and delivery-test comments missing from the surfaces list | MODERATE | **Resolved** | `:361-372`, `:415-423` |
| 4 | Option A rejected on a margin that does not discriminate | MODERATE | **Resolved** | `:128-141` |
| 5 | "exactly one situation" understated Option C's exposure | MINOR | **Resolved** | `:76-82` |
| 6 | Phases 2 and 3 restated the PRD's criteria | MINOR | **Resolved** | `:403-413` |
| 7 | `entry_slice`'s body unsketched where the R15 argument rests on it | MINOR | **Resolved** | `:264-276` |
| 8 | "four passages" undercounted response-shapes.md | MINOR | **Resolved** | `:362-364` |
| 9 | Truncated-log fallback can suppress where the old rule delivered | MINOR (advisory) | **Still open** | not addressed |
| 10 | Three of four new line citations off by 2-3 lines | MINOR (new) | **Open** | `:72` |
| 11 | koto#200 paragraph argues from "gap" about a duplicate-seq failure | MINOR (new) | **Open** | `:463-473` |
| 12 | Pointer guarantee drops "non-terminal" | MINOR (new) | **Open** | `:446-447` |

### 1 — partially resolved

Decision 3 now reads "One of those strings states the boundary this work moves"
(`:182-184`), and the new paragraph at `:186-191` rules the label out with all
three reasons: it is a harness slug rather than a delivery claim, it is a
three-site edit because of the hardcoded required-labels list, and the PRD's
criterion permits only `notes` and `description`. Phase 3 says "except for the
one prose string" (`:412-413`). That is the fix, correctly stated.

What survived is the Consequences bullet at `:512-514`:

> The byte-identity fixture takes a diff, in the one file whose purpose is not to
> have one. Bounded by an acceptance criterion that permits exactly **the two
> prose strings** and forbids any change inside a recorded response body.

Two problems, in a document whose thesis is that stale text gets believed. It
contradicts Decision 3 and Phase 3, and it is the sentence an implementer
skimming Consequences for the fixture constraint would land on — which is
exactly the path back to the failure the finding named. It is a one-word edit:
"the one prose string". Not blocking, because the decision text is explicit and
argues the case, but it should not merge as is.

### 2 — partially resolved

The Context bullet is now right and better than what I asked for. `:68-74`:

> each open-code a similar backwards walk rather than calling the helper — and
> each already differs from it, returning empty where the helper returns the
> whole log when no entry event names the phase ... They also take no phase
> argument, deriving the current state themselves. They are neighbours of the
> helper, not callers of it.

Verified against source. `derive_evidence` returns `None => return Vec::new()`
at `src/engine/persistence.rs:743-746`; `derive_overrides` the same at `:817-820`;
`derive_last_gate_evaluated` short-circuits on `?` at `:860`; `occupancy_slice`
returns `None => events` at `:1044`. All three call `derive_state_from_log`
themselves rather than taking a `state` parameter. The Solution Architecture
paragraph at `:325-331` carries the same correction and draws the right
conclusion — "unifying them would change behavior on exactly the case a unit
test pins", which is `instructions_delivered_reads_the_whole_log_when_no_entry_event_names_the_state`
at `:2652`.

The Decision Drivers bullet at `:116-118` was not updated:

> A behavior reversal and a **behavior-preserving refactor** of three open-coded
> scans in the same commit would be neither.

The design now says twice that folding those three in would not be
behavior-preserving. Same one-line fix: the driver is reviewability, and the
honest phrasing is "a behavior change to three neighbouring walks", which makes
the driver stronger rather than weaker.

### 3 — resolved

Nine places, at `:361-372`, naming "the koto-user eval suite's explanations of
the two evals PRD R20 protects" and "the comments inside
`tests/instructions_delivery_test.rs` that narrate the rule alongside each
assertion". Phase 4 (`:415-423`) rewrites both, and picks up the changelog's
now-dangling function reference. The hedge from "All of them share a stock
phrase" to "Most share" is right — the eval prose and the test comments use
"occupancy" without the "leave and re-enter" formula.

One item from my list is still unnamed: `tests/status_phase_retrieval_test.rs:497`
("First occupancy of `implement`"). It sits inside the very test case Phase 2
already reworks under the PRD's prescription, so it moves with that work. Not
worth another sentence.

### 4 — resolved

`:128-134` now rejects Option A on one margin, states the margin as slim,
concedes the chosen option still leaves four walks with three open-coded, and
explicitly withdraws the comment argument: "the stale doc comment is not an
argument against A, because every option here rewrites it." That is the honest
version. Option B at `:136-141` now opens "Option D minus the two wrappers: same
enum, same inner scan, discriminator exposed at the call sites instead of hidden
behind names", which makes D's actual contribution legible.

### 7 — resolved

`:264-276` sketches the index selection and both arms of the fallback, and says
what depends on them: "`epoch_slice`'s identity with `occupancy_slice` depends
on keeping both arms exactly as they are." The sketched block matches
`src/engine/persistence.rs:1042-1045` character for character.

### 9 — still open

Unchanged from the first pass, and still only advisory. `:274-276` now discusses
the `None` arm, but frames it as identity-preservation rather than naming the one
case where the widened window changes the answer through it: a log whose only
entries naming P are self-entries falls through to the whole-log fallback and can
find a delivery record from an earlier visit, suppressing where `occupancy_slice`
would have delivered. Reachable only through a truncated log. It is the single
case where this change's failure direction is unsafe, and the "failure direction
must stay safe where it can" driver (`:107-110`) does not acknowledge it.

## New findings in the revised text

### 10. MINOR — three of the four new line citations are off

`:72` cites `(:743-746, :815-818, :857, against :1044-1047)`. Checked:

- `:743-746` — exact, `derive_evidence`'s `None => return Vec::new()`
- `:815-818` — the block is at `:817-820`; `:815` is the closing `});` of the find_map
- `:857` — the `?` that produces the behavior is at `:860`; `:857` is an `else {`
- `:1044-1047` — the match is `:1042-1045`; `:1044` is the right line, the range is shifted past the end of the function

All four land a reader inside the right function, so nothing is misleading. But
the surrounding citations in this document (`:1028`, `:1058`, `:722`, `:796`,
`:844`, `:1017-1019`) are exact, and a reader who spot-checks one of the loose
ones will trust the rest less.

### 11. MINOR — the koto#200 paragraph argues from a gap about a duplicate

`:464-466` names the failure mode as two writers claiming the same sequence
number, then `:467` argues from "a log with a sequence gap is rejected wholesale
by the reader". Two writers claiming the same seq produces a duplicate, not a
gap.

The conclusion survives, and I confirmed why: the reader's check at
`src/engine/persistence.rs:663-668` is `event.seq != expected_seq` against a
counter incremented by one per line, so a duplicate fails it exactly as a gap
does — the second line carrying seq N arrives when `expected_seq` is N+1. The
error text happens to say "sequence gap" for both, which is presumably where the
wording came from. Say "a log whose sequence is not monotonic by one" and the
paragraph is airtight.

The rest of that paragraph checks out. The torn-final-line recovery is real and
pinned by the test at `:1374-1385`. And "a suppressing lap appends one event
where today it appends two" is right: a suppressed response skips the
`InstructionsDelivered` append because it is gated on `resp.carries_details()`
(`src/cli/mod.rs:3457`).

### 12. MINOR — the pointer guarantee drops "non-terminal"

`:446-447`: "every non-error response for an instruction-carrying phase carries a
pointer naming that command."

`with_directive_prefix` passes `Terminal` through unprefixed alongside `Error`
(`src/cli/next_types.rs:367-368`), so a terminal phase that declared details
would take no pointer either. PRD R11 says "Every response for a **non-terminal**
phase that declares instructions", and R12 makes the terminal carve-out
deliberate. The narrowing to non-error is a genuine improvement over the previous
draft's unqualified claim; it just needs the other qualifier back.

The rest of the rewritten Security section verifies clean. The `0600` argument is
grounded — state files are opened with `opts.mode(0o600)` at
`src/engine/persistence.rs:96`, `:165`, `:377` and `:409`, and `:1693` asserts
the mode — and is the right permission to reason from, unlike the directory mode
the previous draft cited. The Error-variant exception is correctly described as
pre-existing and untouched by this change. The at-most-once bound on the
unbounded scan is right: `.iter().any(..)` at `:1100` scans forward and
short-circuits, and a scan that finds nothing delivers and records, so the next
tick short-circuits.

## Case trace

Unchanged from the first pass — the mechanism in Decision Outcome is the same
one, now with the fallback sketched. All eighteen rows re-checked against the
revised sketch; every one still matches. Log notation: `T{a→b}` transition,
`D{a→b}` directed, `R{a→b}` rewind, `ID{p}` delivery record.

| # | Arrival case | Log | `delivery_window` opener | Window contents | Sketch answer | PRD requires | Match |
|---|---|---|---|---|---|---|---|
| 1 | Fresh init (R2) | `T{None→P}` | that event (`None != Some(P)`) | empty | deliver | deliver | yes |
| 2 | Conditional/unconditional/`skip_if` arrival (R3) | `…T{G→P}` | `T{G→P}` | empty | deliver | deliver | yes |
| 3 | Directed into a different phase (R3) | `…D{G→P}` | `D{G→P}` | empty | deliver | deliver | yes |
| 4 | Loop-back to a phase visited earlier (R3) | `T{G→P},ID{P},T{P→Q},T{Q→P}` | `T{Q→P}` | empty | deliver | deliver | yes |
| 5 | Same-tick round trip (R3) | same, within one tick | `T{Q→P}` | empty | deliver | deliver | yes |
| 6 | Self-transition, window has a record (R4) | `T{G→P},ID{P},T{P→P}` | `T{G→P}` | `ID{P},T{P→P}` | suppress | suppress | yes |
| 7 | Two consecutive self-transitions (R4) | `T{G→P},ID{P},T{P→P},T{P→P}` | `T{G→P}` | contains `ID{P}` | suppress | suppress on both | yes |
| 8 | Directed self-transition (R5) | `T{G→P},ID{P},D{P→P}` | `T{G→P}` | contains `ID{P}` | suppress | suppress | yes |
| 9 | Rewind, different source (R6) | `T{G→P},ID{P},T{P→Q},ID{Q},R{Q→P}` | `R{Q→P}` | empty | deliver | deliver | yes |
| 10 | Rewind, same source and target (R6) | `T{G→P},ID{P},T{P→P},R{P→P}` | `R{P→P}` (arm ignores `from`) | empty | deliver | deliver | yes |
| 11 | Non-advancing tick, gate-blocked (R7) | `T{G→P},ID{P},GE{failed}` | `T{G→P}` | contains `ID{P}` | suppress | suppress | yes |
| 12 | Non-state-entry event after a suppressing tick (R13) | `T{G→P},ID{P},T{P→P},DecisionRecorded` | `T{G→P}` | contains `ID{P}` | suppress | suppress | yes |
| 13 | Self-entry with **no** record anywhere (R1, crash case) | `T{G→P},T{P→P}` | `T{G→P}` | `T{P→P}` only | deliver | deliver | yes |
| 14 | `--full` on a suppressing self-loop tick (R8) | as #6, `full=true` | `T{G→P}` | contains `ID{P}` | `full` wins → deliver, and `carries_details()` records | deliver, next plain tick omits | yes |
| 15 | Multi-hop leaving an intermediate record inside the window | `T{G→P},ID{P},T{P→Q},ID{Q},T{Q→P}` | `T{Q→P}` | empty | deliver | deliver | yes |
| 16 | Intermediate record inside a *self-loop* window | `T{G→P},ID{P},T{P→P},ID{Q}` | `T{G→P}` | `ID{P},T{P→P},ID{Q}` | `ID{P}` matches on name → suppress | suppress | yes |
| 17 | Terminal phase (R12) | any | n/a | n/a | `Terminal` passes through both combinators untouched (`src/cli/next_types.rs:367`) | no instructions, no pointer | yes |
| 18 | No entry event names the phase (truncated log) | `ID{P}` alone | none → whole-log fallback | `ID{P}` | suppress | not specified | see finding 9 |

## Mechanism re-verification

The Decision Outcome sketch is unchanged except for the added fallback, so the
first pass's verification carries. Re-confirmed against the revised text:

**`epoch_slice` ≡ `occupancy_slice`.** Under `AnyEntry` the guard short-circuits
before `from` is read, so `opens` reduces to `to == state` on all three entry
arms and `_ => false` matches today's `None` arm at `:1035`. The now-sketched
tail (`Some(idx) => &events[idx + 1..], None => events`) is identical to
`:1042-1045`. Both halves of the identity are now on the page.

**Types and lifetimes.** `from: &Option<String>` autoderefs through
`Option::as_deref` to `Option<&str>`. `from: &String != state: &str` resolves via
`impl PartialEq<str> for String` plus the blanket `impl PartialEq<&B> for &A`;
same for every `to == state`. `entry_slice<'a>(&'a [Event], &str, Boundary) -> &'a [Event]`
ties one input lifetime to the output with no elision ambiguity. `Boundary` is
`Copy`. No default-set clippy lint fires on the derive list.

**Initial entry works for the stated reason.** Production writes
`Transitioned { from: None, to: initial_state }` at `src/cli/init_child.rs:502`
and `:671`, and `handle_init` errors if a newly initialized log has no
`Transitioned` event (`src/cli/mod.rs:1854-1860`). The `from.as_deref() != Some(state)`
argument is doing the work, not the fallback.

**Exactly two unit tests invert.** All eight cases at
`src/engine/persistence.rs:2546-2657` re-traced against `delivery_window`:
`instructions_delivered_resets_on_a_self_transition` (`:2603`) and the directed
half of `instructions_delivered_resets_on_arrival_by_directed_transition`
(`:2648`) flip; the other six hold.

**Rename has no external exposure.** `occupancy_slice` is private.
`instructions_delivered_this_occupancy` is `pub` but not re-exported —
`src/lib.rs:31` has the only persistence re-export commented out, `:52` exports
`EngineError` alone. `docs/STABILITY.md` names neither; `koto-stability-tests/`
references nothing in `persistence`.

## Rename impact list

Unchanged from the first pass. Reproduced for the implementer.

`occupancy_slice` → `epoch_slice` (private):

- `src/engine/persistence.rs:1000-1027` — doc comment, including the "Shared rather than copied" sentence at `:1017-1019`
- `src/engine/persistence.rs:1028` — definition
- `src/engine/persistence.rs:1059` — call from `latest_epoch_gate_failed`
- `src/engine/persistence.rs:1082` — intra-doc link in the delivery check's comment
- `src/engine/persistence.rs:1100` — call from the delivery check (becomes `delivery_window`)
- `src/engine/persistence.rs:2653` — unit-test comment naming the shared fallback
- `src/cli/mod.rs:3383` — prose reference inside the "provably false" proof

`instructions_delivered_this_occupancy` → `instructions_delivered_this_window`:

- `src/engine/persistence.rs:1076-1098` — doc comment (rewrite, not rename)
- `src/engine/persistence.rs:1099` — definition
- `src/cli/mod.rs:2914` — the `use` import
- `src/cli/mod.rs:3377` — comment reference
- `src/cli/mod.rs:3417` — directed-path call
- `src/cli/mod.rs:4298` — natural-path call
- `src/cli/next_types.rs:378` — intra-doc link; a stale one is a broken rustdoc reference, not caught by `clippy -D warnings`
- `src/engine/persistence.rs:2500` — test-module section header
- `src/engine/persistence.rs:2549, 2566, 2579, 2591, 2603, 2608, 2623, 2631, 2644, 2648, 2656, 2657` — twelve unit-test call sites
- `docs/designs/current/DESIGN-inline-phase-details.md:264, 280, 357`
- `CHANGELOG.md:26` — history, exempt under R18, now named by Phase 4

Prose surfaces, all now covered by the design's nine:

- `plugins/koto-skills/skills/koto-user/references/response-shapes.md:38, 39, 44, 45, 74, 107, 168, 171, 550`
- `plugins/koto-skills/skills/koto-user/references/command-reference.md:96`
- `plugins/koto-skills/skills/koto-author/SKILL.md:67`
- `plugins/koto-skills/skills/koto-author/references/template-format.md:118, 120, 122`
- `plugins/koto-skills/.cursor/rules/koto.mdc:171, 173`
- `docs/guides/cli-usage.md:82, 117`
- `docs/reference/session-feed.md:683`
- `plugins/koto-skills/skills/koto-user/evals/evals.json:134, 136, 140, 149, 153, 155`
- `tests/instructions_delivery_test.rs:373, 374, 400, 450, 480, 507, 550, 551, 567, 584`
- `tests/status_phase_retrieval_test.rs:497` — moves with the Phase 2 rework
- `docs/prds/PRD-inline-phase-details.md:140, 142, 161, 166, 167, 262, 275`
- `docs/designs/current/DESIGN-inline-phase-details.md:221, 247, 253, 358, 404`
- `src/cli/next_types.rs:374`
- `src/cli/mod.rs:3366, 3386, 3390, 3392, 4283`
- `src/engine/types.rs:783-785` — "Until that wiring exists, a session log will never contain one", now false
- `tests/next_response_baseline.rs:362` and `tests/fixtures/next-response-baseline/instruction-free.json:84` — the one permitted fixture edit

Correctly untouched: `docs/prds/PRD-koto-next-output-contract.md:130` already
states the new rule, and `DESIGN-koto-next-output-contract.md`'s Decision 3
asserts neither boundary, so a cross-reference is the right size.

## Mechanical checks

`shirabe validate docs/designs/DESIGN-self-loop-suppresses-details.md --check R7
--visibility public` — empty stdout, empty stderr, exit 0. Verbatim output is
nothing at all.

All nine required sections present and in order: Status (`:37`), Context and
Problem Statement (`:47`), Decision Drivers (`:100`), Considered Options
(`:120`), Decision Outcome (`:213`), Solution Architecture (`:311`),
Implementation Approach (`:387`), Security Considerations (`:430`), Consequences
(`:478`).

No banned words — no tier/tiered, robust, leverage, comprehensive, holistic,
facilitate. 35 em dashes over 4572 words, one per 131, under threshold and
slightly denser than the previous draft. Contractions present, sentence length
varies hard, no hollow gerunds, no forced rule of three, no demonstratives
without antecedents. The mixed `—` and ASCII ` -- ` for the same job persists;
still cosmetic.

## Summary

The blocking finding is fixed, and fixed properly: Decision 3 now edits one
prose string and rules the harness label out of scope with the three reasons
that make it out of scope. The rewritten claim about the three neighbouring
walks is not just corrected but improved — it now cites the four line ranges
where they diverge and names the unit test that pins the difference, which is a
stronger argument than the one it replaces. Surfaces, altitude, the unsketched
fallback and the two undercounts are all resolved, and the new Security section
checks out against the source, including the `0600` claim and the at-most-once
bound.

Two sentences did not get the memo. The Consequences bullet at `:512-514` still
says the criterion "permits exactly the two prose strings", and the Decision
Drivers bullet at `:116-118` still calls folding the three walks together a
"behavior-preserving refactor". Both contradict the passages that replaced them,
and the first is the one an implementer skimming for the fixture constraint would
read. Two one-line edits.

Three new imprecisions, all minor and all in text added this round: three of the
four new line citations are off by two or three lines, the koto#200 paragraph
argues from "gap" about a duplicate-seq failure (the reader rejects both, so the
conclusion holds), and the pointer guarantee dropped "non-terminal" while
correctly gaining "non-error". PASS.
