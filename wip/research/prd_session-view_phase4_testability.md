# Testability Review

## Verdict: PASS
A test plan can be drafted from the ACs for most requirements; the gaps below are real but fixable without restructuring.

## Untestable Criteria
1. AC3 "documented machine-readable form": the form is not named, so a parser test cannot be written -> name the format (e.g. JSON with a listed field set) or defer to the design with a stated schema-check test.
2. AC3 "bounded": no bound figure; N1 gives the formula (keys x excerpt bound + fixed overhead) but the constants are left to the design -> AC should assert output length <= keys*EXCERPT_BOUND + OVERHEAD using the named constants.
3. AC5 "constant number of remote requests": no mechanism or count is named -> require a fake/counting remote and assert the request count is identical for N=3 and N=50 keys.
4. AC4 "same column count and positions as the previous release documents": depends on the prior release's docs -> pin a golden-output fixture and compare byte-for-byte (R7 says byte-compatible).
5. AC1 / R3 "human-readable unit style" and R6 "usable / scroll or equivalent / without blocking the refresh loop": subjective -> specify the unit style in the design, and test that refresh ticks continue during a slow content load and that the 50th key is reachable.
6. AC7 "shape-guard fixture is updated": process step, not behaviour -> assert the shape guard passes and that a pre-change consumer fixture still parses.

## Missing Test Coverage
1. R10 read-only: no AC. Add: snapshot the store (files, manifest, event log) before and after viewing; assert no change and no per-key event growth.
2. N2 on non-cloud / N1 for the dashboard: only partly covered. Add an AC that content rendering never exceeds the bound for a very large (e.g. 100 MB) value.
3. R3 edge cases: UTF-8 truncation at a multibyte boundary, a value exactly at the bound (no marker) and bound+1 (marker), NUL byte in otherwise valid text, invalid UTF-8 in a sampled prefix, and an empty value.
4. R4 "key requested but not present is reported as absent": no AC (AC8 covers only unreadable). Add an absent-key case.
5. R2: stable ordering and completeness (no dropped keys, large key counts) have no AC.
6. R1: the AC covers only absent intent/origin; name, id, template and the old-session no-anchor case are not explicit.
7. R5: AC6 does not say what the exit code is for the dashboard detail, and does not cover the migration marker being malformed or missing the workspace field.
8. R9: merged local+remote manifest metadata (a key present in both, with conflicting size) and the output stating why content was not shown are not asserted.
9. R8: no AC for a session with zero keys, or for the total size and count accuracy.
10. One-shot mode errors: unknown session name, no session store, missing flag argument. No AC.
11. AC10 should name ESC, CSI, OSC and C1 controls, in key names and writer fields as well as values.
12. N3: the workflow file's existing fields "keep names, types and meaning" is only covered by a fixture update; add a schema compatibility test.
13. N4/AC9 is testable (help diff), fine.

## Summary
Most requirements map to an observable check, and the ACs reference fixtures that can be built. The weak spots are unspecified formats and constants (machine-readable form, bound, unit style), no AC for read-only behaviour (R10) or absent keys (R4), and thin edge-case and error coverage, since the ACs lean on the happy path plus one unreadable and one migrated case. Tightening those would make it fully plannable from the ACs alone.
