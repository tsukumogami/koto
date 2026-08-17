# Verdict: PASS

Nothing in this diff is wrong, nothing in it is speculative, and it implements the
option the design chose rather than one of the three it rejected. The findings
below are all comment-quality and naming; none of them blocks.

## Findings

### 1. MODERATE — the same argument is written twice, forty lines apart

On `epoch_slice`:

> It is deliberately *not* [`delivery_window`]. The two answer different
> questions -- "since the machine last entered this state" against "since the
> agent last arrived here from somewhere else" -- so they now disagree across a
> self-entry, on purpose. Sharing one scan is what keeps them from disagreeing
> about anything else.

On `latest_epoch_gate_failed`, `src/engine/persistence.rs:1105-1110`:

> This boundary is *not* the delivery window. A self-transition closes the
> epoch, so a gate verdict from before a loop's last lap drops out of scope and
> the blocked badge clears; it does not close the delivery window, so an agent
> looping in one phase is not re-sent instructions it holds. The two are
> separate answers to separate questions and a change to one must not move the
> other.

Two statements of one claim, each with a different half of the supporting
material — the first has the residual invariant (one scan, so no other
disagreement), the second has the concrete consequence (the badge clears). A
maintainer editing one will not know to edit the other, and "the two are separate
answers to separate questions" is already near-verbatim in both.

**Fix**: put the whole argument — both questions, both consequences, the residual
invariant — on `epoch_slice` once, and reduce the `latest_epoch_gate_failed`
paragraph to the gate-specific consequence plus a link.

### 2. MODERATE — the replacement never says what breaks if a maintainer unifies them

This is the heart of the remit. The overturned comment argued the two predicates
must not disagree; the replacement says they now must. It explains *that* they
differ and *what* each answers, but it justifies keeping `AnyEntry` on the gate
path by provenance rather than by reason:

> This is the boundary the gate-blocked classification reads, and it
> is unchanged from the one koto has always used.

"Unchanged from what we had" is not an argument a maintainer will accept against
"then why not widen both and delete a function." The design has the actual
answer — Option C is rejected because widening in place "is the change that moves
the dashboard's blocked badge across a self-loop, with no test in the repo to
catch it" — and that sentence appears nowhere in the code. The nearest thing is
the trip-wire note buried in a test at line 2711 ("the test that fails loudly if
someone widens the shared scan in place"), which a maintainer reads only after
they have already made the change.

Note this cuts the other way too: the file's *pre-existing* regression net for
`latest_epoch_gate_failed` is empty. Repo-wide, the only assertions on it are the
one added by this commit at line 2727, plus two indirect cases in
`src/cli/dashboard_data.rs:1279,1307` that never exercise a self-transition. The
"behaviorally identical" claim rests on reading the `AnyEntry` arm and on one new
assertion. That makes the missing sentence more load-bearing, not less.

**Fix**: one sentence on `epoch_slice`, e.g. — widening this to the delivery
window would keep a gate verdict from a previous lap in scope and leave the
blocked badge lit across a loop; no dashboard test catches that, which is why the
unit case at `the_epoch_and_the_delivery_window_disagree_across_a_self_transition`
exists.

### 3. MODERATE — `ArrivalFromElsewhere` is false about a case the suite pins

```rust
/// Only an entry from a different phase, plus every rewind.
ArrivalFromElsewhere,
```

The variant's own doc has to append "plus every rewind" to correct its name, and
the correction is not hypothetical: `instructions_delivered_resets_on_a_same_phase_rewind`
builds `rewound(4, "review", "review")` and asserts the window opens. That is an
arrival from *here*. A name whose doc comment's second clause contradicts it is a
name that will mislead the next reader of the match arm, who sees
`EventPayload::Rewound { to, .. } => to == current_state` with no `from` test and
has to reconcile it against a variant called "from elsewhere."

**Fix**: `ArrivalOrRewind`, or name the actual predicate — the arm's rule is "not
a self-transition," and `NotASelfTransition` is true of all three arms without a
caveat. Caveat: the design's Decision Outcome pins the current spelling, so this
is a deviation request rather than a defect against the plan.

### 4. MINOR — the rewind argument is sound but its framing overstates

> **The rewind arm deliberately does not bind `from`.** A rewind opens both
> windows whatever phases it records, and not reading the field is what keeps
> the delivery answer from coming to depend on how `koto rewind` chooses its
> destination.

The argument is sound as far as it goes: `entry_slice`'s output on a `Rewound` is
invariant under every value of `from`, so the delivery answer genuinely cannot
vary with rewind's destination logic. But it is a discipline, not a structural
guarantee — `Rewound { to, .. }` keeps the `..`, so binding `from` is a one-word
addition, not a compile error. The design states this better than the code does
("one deleted line away from coupling"): what the choice buys is that coupling
requires someone to *add* a read, not to *delete* a carve-out.

Two things would break it. First, that addition. Second — the one the comment
does not cover — the guarantee is scoped to the `Rewound` *payload*, not to the
rewind *feature*. Both emitters use it today (`src/cli/mod.rs:2044` and
`src/cli/retry.rs:551`, verified), but if a koto#199 fix made a rewind append a
`Transitioned` or `DirectedTransition` instead, the delivery answer would start
reading rewind's destination through the other two arms and nothing here would
notice.

**Fix**: swap "not reading the field is what keeps" for the design's framing, and
add a clause: this holds as long as a rewind is recorded as `Rewound`.

### 5. MINOR — the retired word survives in the file whose rename retires it

Three uses of "occupancy" remain in `src/engine/persistence.rs`, all in outline
1's own file: the test name at line 2613
(`instructions_delivered_true_within_the_current_occupancy`), and comments at
2631 and 2747. Line 2747 is the one that is now imprecise rather than merely
stale:

> // *inside* the current occupancy and the name check is the only thing

The slice under discussion is the delivery window, which can span several
occupancies — that is the entire change. The PRD is actively retiring this word
(design Decision 3), and this commit renamed a `pub fn` to shed it.

**Fix**: rename the test to `..._within_the_current_window` and rewrite the two
comments to say window.

### 6. MINOR — `instructions_delivered_this_window` is vague where it is read

The old name was no better, but "this window" names nothing a reader of
`src/cli/mod.rs` can resolve without opening `persistence.rs`; "occupancy" at
least suggested a phase visit. `instructions_delivered_since_arrival` answers the
question at the call site. The plan's acceptance criterion pins the shipped name,
so this is a deviation request, not a defect.

### 7. MINOR — one referrer the compiler will not catch

The commit message says "The crate does not build until the two call sites follow
the rename." There are three referrers, and the third is the one a build will not
surface: `src/cli/next_types.rs:378` still carries the intra-doc link
`[crate::engine::persistence::instructions_delivered_this_occupancy]`. CI runs
`cargo clippy -- -D warnings` (`.github/workflows/validate.yml:92`) and no
`cargo doc` step, so a broken intra-doc link fails nothing. Outline 2 owns the
fix; the risk is that it is the one item in that outline with no mechanical
backstop.

### 8. MINOR — two of four derives are unused

`#[derive(Clone, Copy, PartialEq, Eq)]`: only `PartialEq` is load-bearing.
`boundary == Boundary::AnyEntry` resolves to `PartialEq::eq(&boundary, ..)`, so
the closure captures by reference and nothing moves — `Copy` is not required, and
nothing keys a map on `Boundary` or asserts equality on one, so `Eq` is unused.
Free and idiomatic on a fieldless enum; recorded for completeness, not a change
request.

## What earns its place

- **The rename cost is fully paid and smaller than the design implies.**
  `instructions_delivered_this_occupancy` is not on the frozen surface:
  `docs/STABILITY.md` pins `CURRENT_SCHEMA_VERSION`, the four `SessionBackend`
  methods, `StateFileHeader` and `EventPayload`; `src/lib.rs:16-34` names the
  eight re-exported types and the `derive_state_from_log` alias and nothing else;
  `koto-stability-tests/src/lib.rs` references neither name. No downstream
  consumer breaks. A revision must not reintroduce a deprecated alias for a
  function nobody outside the crate imports.
- **No drift toward a rejected option.** `Boundary::` appears at exactly four
  sites, all inside `entry_slice` and the two wrappers — no call site names a
  boundary value, which is precisely what separates the shipped Option D from the
  rejected Option B. One scan, not two, which is what separates it from A. The
  gate arm reads `boundary == Boundary::AnyEntry ||`, unconditionally true on the
  gate path, which is what separates it from C.
- **Both single-caller wrappers.** They look inlinable and are not: inlining them
  *is* Option B, and the design rejects it on a real hazard — a wrong argument at
  `latest_epoch_gate_failed`'s call is a silent behavior change with no type
  error. They also carry the doc anchors the rest of the module links to.
- **`epoch_slice`'s identity with `occupancy_slice`.** Same three variants, same
  `to == current_state`, same `&events[idx + 1..]` and whole-log fallback. Given
  finding 2's empty pre-existing test net, the whole-log `None` arm and the
  `AnyEntry` arm must survive any revision untouched.
- **The record-not-shape paragraph on `instructions_delivered_this_window`**, and
  the crash-case test that pins it. It is the one place the comment set explains a
  design choice a reader would otherwise reverse.

## Summary

Every line here is required by outline 1 and the shipped code is the design's
Option D, not a drift toward the sibling scan, the call-site parameter, or the
widened helper; the rename breaks no pinned surface, since neither `docs/STABILITY.md`
nor `koto-stability-tests/` names the function. The comment set is the weak half:
it states twice, in two places that will drift, that the two boundaries differ on
purpose, and never states what breaks if a maintainer unifies them — the design's
own reason, that widening moves the dashboard's blocked badge with no test to
catch it, is missing from the code that most needs it. `ArrivalFromElsewhere` is
contradicted by its own doc's second clause and by a rewind case the suite pins,
and three uses of the retired word "occupancy" survive in the file whose rename
exists to retire it.
