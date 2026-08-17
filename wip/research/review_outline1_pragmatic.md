# Verdict: PASS

## Findings

1. **Minor — `epoch_slice` and `delivery_window` are one-line aliases with one caller each.**
   Diagnosis: four items now stand where one did, and two of them exist mainly to host doc prose; `epoch_slice` carries 22 lines of comment for `entry_slice(events, s, Boundary::AnyEntry)` called exactly once.
   Fix: optional — inline both into their sole callers and pass the variant directly, moving each doc block into the public predicate above it. Keeping them is defensible (the names are the vocabulary the rest of the doc uses); do not add a third.

2. **Nit — the `Boundary` enum is the right call; a bool would not have been.**
   Diagnosis: `entry_slice(events, s, true)` at the call site says nothing, and the two variants are asymmetric in a way (`Rewound` opens both) a bool named `skip_self_entry` would misdescribe.
   Fix: none. Likewise, two independent scans would have duplicated three match arms and the whole-log fallback — the shared scan is the simpler of the two options actually available.

3. **Minor — the "these two are separate on purpose" argument is made four times.**
   Diagnosis: it appears on `Boundary`, on `entry_slice`, on `epoch_slice` ("Unifying them breaks the dashboard, silently", 9 lines), and again on `latest_epoch_gate_failed` (6 more lines) — the last two are the same paragraph twice, one hop apart.
   Fix: keep the `epoch_slice` version (it names the failure mode and the guarding test); cut the closing paragraph of `latest_epoch_gate_failed` to one sentence pointing at it.

4. **Nit — `epoch_slice`'s doc names a test by identifier.**
   Diagnosis: `the_epoch_and_the_delivery_window_disagree_across_a_self_transition` is a hand-maintained link that rots silently on rename, and rustdoc will not check it.
   Fix: say "the one test asserting both boundaries against a single log" and drop the identifier, or leave it — cheap either way.

5. **No finding — all four new tests earn their place.**
   `survives_a_self_transition` is the core rule; `false_on_a_self_transition_with_no_record` is the only thing proving the predicate is keyed on the record rather than on the entry event's shape; `resets_on_a_same_phase_rewind` is the only cover of the unbound `from` in the rewind arm; `the_epoch_and_the_delivery_window_disagree` is the only cover of `epoch_slice` at all and the only assertion that the two boundaries diverge. Its delivery half overlaps test 1, but its gate half is unique — folding it would lose the regression.

6. **No finding — the rename is worth its cost.**
   "Occupancy" now denotes the epoch boundary specifically; leaving `..._this_occupancy` on the predicate that no longer uses it would actively mislead. Cost is five references in `src/cli`, already assigned to outline 2.

7. **Informational — the branch does not compile at `a7fc426`.**
   `src/cli/mod.rs:2914,3417,4298` and `src/cli/next_types.rs:378` still name `instructions_delivered_this_occupancy`. The plan states this outright (PLAN lines 139-142: "the crate does not build until this outline lands"), so it is sequencing, not a defect — noted only so it is not mistaken for one.

8. **Informational — `src/cli/mod.rs:3377-3402` now asserts a false invariant.**
   That comment claims the directed-transition call "provably evaluates to `false` on every call" because the synthetic entry event always opens a fresh occupancy. Under `ArrivalFromElsewhere` a `--to <current phase>` no longer opens one, so the call can now return true — which is exactly what `instructions_delivered_survives_a_directed_self_transition` asserts. Outline 3 owns comment rewrites; this one is a flipped fact, not a stale name, so it should not be handled as a mechanical find-and-replace.

## Summary

No over-engineering worth blocking on: the enum plus one shared scan is the smaller of the two designs that were actually available, and each of the four new tests covers something the others do not. The excess is prose, not structure — the same "these boundaries are deliberately different" argument is made four times across adjacent doc blocks, and the two wrappers are single-use one-liners that read more as doc anchors than as code.
The one thing to carry forward is finding 8: a call-site comment in `src/cli/mod.rs` now states an invariant this change inverts, and it needs rewriting rather than renaming.
