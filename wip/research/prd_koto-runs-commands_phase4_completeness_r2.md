# Completeness Review (Round 2)

## Verdict: PASS

## Disposition of the Six Prior Issues

1. **Batch/child sessions under anchoring** — RESOLVED. R16 ("A session created by another session... is anchored under the same rules. The documentation states which directory such a session is anchored to and how a developer rebinds one") plus two dedicated ACs under Execution Anchoring ("A session created by another session has a recorded anchor... (R16)" and "The rebind command of R13 works on a session created by another session (R16, R13)").

2. **Second (lock-based) nested-`koto` deadlock undocumented** — RESOLVED as a deliberate non-requirement, honestly recorded. It is named in Out of Scope ("The session-lock contention a nested `koto` invocation hits... not one of the two the brief scoped") and in Known Limitations, which states template authors "should treat invoking `koto` from inside a koto action as unsupported until that second path is addressed on its own terms." The one AC that touches nested-`koto` invocation ("...completes without a pipe-buffer deadlock or a false timeout (R18, R20)") is scoped to the pipe-buffer failure specifically and makes no claim about the lock-contention path — no AC overpromises.

3. **Cross-state output-name collision** — RESOLVED. Folded into R5 ("what happens when two states declare the same name") with a matching AC ("A template in which two states declare the same output name produces the documented behavior — rejection at compile time or a defined precedence... (R5)").

4. **Reference to a name nothing produced (typo case)** — RESOLVED. R4 now explicitly covers "a name no state in the template ever delivers — the typo case... and a name whose producing state exists but was not entered on this run," with matching ACs for both branches.

5. **"Exits non-zero" not covering spawn failure/timeout** — RESOLVED. A defined term, "failing command," is introduced in the Requirements preamble ("exits non-zero, fails to start at all, or is killed for exceeding its timeout"), R6 restates all three explicitly, R8 requires the response to say which of the three happened, and D8 records the reasoning.

6. **R20's (now R22's) six-point doc list under-covered by ACs** — RESOLVED. AC now includes a dedicated check ("The `default_action` documentation covers each of R22's six points, verifiable by reading for all six (R22)") plus a specific gate-interaction AC ("The documented behavior for whether that state's gates still evaluate after its action fails matches what the engine does (R7, R22)"), closing the exact gap called out (gate-interaction had no dedicated AC before).

## Renumbering Consistency

R1 through R26 each appear exactly once as a definition (verified by extraction — no duplicates, no gaps). Every acceptance-criterion citation (grouped by section: output routing, failure path, anchoring, shared-path defects, authoring guidance) resolves to an existing requirement id, and every multi-id citation pairs requirements that are actually related (e.g. "(R6, R7)" on the no-gates non-zero-exit AC; "(R7, R22)" on the gate-interaction documentation AC; "(R16, R13)" on the child-session rebind AC). All 26 ids are cited by at least one AC.

Checked every cross-reference in Decisions and Trade-offs, Known Limitations, and Out of Scope against the new numbering — all resolve correctly:
- D4's "R1 through R5 and R11 through R16" correctly spans the output-routing and anchoring groups under the new numbers.
- D6/Known Limitations' "R6 through R10" correctly spans the failure-path group.
- Known Limitations' "R12 guarantees every tick," "R17 exists to make sure no koto document ever implies [containment]," "R13's rebind repairs it," "R5 requires the behavior to be documented" (rewind), and "R18 fixes the pipe-buffer deadlock... R19 and R25 bound..." all point at the requirement each statement actually means.
- Out of Scope's "Distinct from R18's pipe-buffer defect" is correct (R18 is the pipe-buffer requirement).

No stale reference was found.

## New Gaps or Scope Creep

None found. R16 (child-session anchoring) is a natural fill of the anchoring scope the BRIEF already grants ("Execution anchoring — binding a session to the tree it was created in"), not new scope. The Out of Scope list still matches the BRIEF's boundary verbatim, plus the one explicit addition (session-lock contention) which is correctly excluded, not smuggled in as a requirement. R25/R26 are refinements of the already-in-scope failure path and output-visibility bounding, not new territory.

## Summary

All six issues from the round-1 review are resolved on their merits — five as new or extended requirements with matching ACs, one (the second nested-`koto` deadlock) as a deliberate, honestly-recorded non-requirement with no AC that overpromises. The R1→R26 renumbering is internally consistent throughout the requirements, acceptance criteria, decisions, limitations, and out-of-scope sections, with no dangling or misdirected cross-reference. No new gap or scope violation was introduced by the revision.
