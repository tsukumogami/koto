# Clarity Review

## Verdict: PASS
The PRD is specific about behavior and contracts, and its open items (bound, unit style, cloud fetch policy) are deliberately deferred with a testable floor; the ambiguities below are fixable without restructuring.

## Ambiguities Found
1. R1 / AC2: "shown as absent" / "explicit absent marker" -> the marker text and form are unspecified; in machine-readable output it could be null, an omitted key, or a string -> state that the one-shot form emits the field with a documented absent value (not omitted) and that the TUI uses a fixed marker string.
2. R2: "a stable, documented order" -> the order is not named (manifest order, alphabetical, created-at); two developers would choose differently -> name the order (e.g. alphabetical by key name) or state that the design must pick one and AC must test it. No AC currently verifies ordering.
3. R2: "its recorded writer (when present)" and "created-at time" -> behavior for a missing created-at, and the time format/timezone, are unspecified -> state the absent behavior and require a documented timestamp format.
4. R3: "a fixed bound" / "a human-readable unit style fixed by the design" -> deferred values are acknowledged, but nothing requires the design to set one bound for both TUI and one-shot, or says whether the bound is bytes, characters or display columns -> specify the bound's unit (bytes) and that it is one constant shared by all surfaces.
5. R3: "content hash" -> algorithm unspecified (sha256? the manifest's existing hash?) -> name the algorithm or say it reuses the manifest's recorded hash.
6. R3: "invalid UTF-8 (sampled prefix)" (Decisions) vs "invalid UTF-8 or containing NUL" (R3) -> the sample size is unspecified, so a value that is invalid only after the sample is classified as text; the requirement and the decision section disagree on whether the whole value is checked -> define the sample size or check the full value.
7. R3 / AC10: "Control characters never reach the terminal unescaped" -> unclear whether newlines and tabs in an excerpt count as control characters (multi-line excerpts are likely wanted), and what the escaped form looks like -> list the allowed whitespace set and the escape format.
8. R4: "a key requested but not present is reported as absent" -> "requested" has no referent; the view does not otherwise take key requests (the one-shot mode takes a session name only) -> clarify whether a key-selection argument exists, or drop the clause.
9. R4: "the reason" for unreadable -> no vocabulary or minimum; "never a stack of errors" (Goals) is subjective -> require a one-line reason, no multi-line error text.
10. R5: "in place of content" -> ambiguous whether R1 facts (name, state, etc.) still render for a migrated session, or only the migration notice -> state exactly which sections remain.
11. R6: "without blocking the dashboard's refresh loop" -> no measurable criterion and no AC covers it; "scroll or equivalent" is open-ended -> give a testable form (e.g. refresh ticks continue while a 100-key session loads; every key reachable by keyboard).
12. R6: "carries R1-R5" -> R1-R5 are content requirements, but the dashboard "detail surface" is never defined as a screen/key binding; "focus one session" (user story) lacks the interaction -> name how the detail is entered and left.
13. R7: "a new flag" / "machine-readable form" / "bounded" -> format (JSON? TSV?) and flag name are left open; AC3 says "documented" but nothing says which format is acceptable -> require a specific format (e.g. JSON) or require the design to choose and the AC to name it.
14. R7 / N1: "bounded by the number of keys times the bound plus fixed overhead" -> "fixed overhead" is unquantified and escaping can expand an excerpt (a control-char-heavy value escapes to several times its length) so keys x bound may not hold -> bound the output after escaping, or define the multiplier.
15. R8: "compact summary" -> "compact" is subjective; no limit on the number of key names listed for a session with many keys, which conflicts with N1's intent -> cap the listed names or say the list is complete and state why that is acceptable.
16. R8: "additive change to its contract" -> the field names and placement are not given; AC7 refers to a "shape-guard fixture" that the PRD never defines -> link the fixture or describe what it asserts.
17. R9: "merged local and remote manifests" -> merge semantics when the two disagree (size or writer differs) are unspecified -> state which side wins on conflict.
18. R9 / N2: "a constant number of remote requests" -> no number or ceiling; a constant of 1,000 satisfies it -> state a ceiling or "at most N" so AC5 is verifiable, or define the verification method (counting fake-remote calls at two key counts).
19. R10: "Incidental read-path effects that already exist ... are not widened" and "no event spam proportional to key count" -> "incidental", "widened" and "spam" are not testable; no AC covers R10 at all -> add an AC: after viewing, the session's event log and context store are byte-identical (local case), and event count increases by at most a fixed number.
20. AC1: "several keys" -> count unspecified; "visible truncation marker" -> not defined; fine for a fixture but not binary on its own -> specify the fixture (e.g. 5 keys, one at bound+1 bytes, one containing NUL).
21. AC3: "stable across two consecutive runs" -> any timestamp or elapsed-time field would break this, and the PRD does not say the output excludes the view-time clock -> require that output contains no render-time values.
22. AC4: "the previous release documents" -> assumes the feed columns are documented somewhere; if not, the AC is unverifiable -> cite the document or add a golden-output fixture.
23. AC6: "exit without error" -> exit code 0 is implied but not stated; "no empty key list masquerading" is subjective -> state exit code 0 and that the key section is absent or replaced by the migration notice.
24. AC8: "unreadable in a test fixture" -> the method (permissions, missing blob, corrupt) is unspecified, and different causes give different reasons -> list at least the fixture methods that must be covered.
25. User stories: "at a glance" (story 2), "closer look", "apparent failure" -> subjective; stories lack the input and observable outcome needed to derive test cases (e.g. what the operator sees for a session with 0 keys) -> add the zero-key case and the empty-session rendering to R2/AC.
26. Problem statement / Goals: "One look answers..." and "Content is always legible" -> "legible" is subjective and absolute ("always"), and the problem statement gives no scale (how many keys, how large) to judge solutions against -> add typical and worst-case session sizes.
27. N3: "no breaking change" -> overlaps R7/R8 and does not define who counts as an "existing consumer" for the workflow file -> reference the shape-guard fixture as the definition.

## Suggested Improvements
1. Add an AC for R2 ordering and the zero-key session: ordering is a stated requirement with no test, and the empty case is the most likely edge to diverge between implementations.
2. Add an AC for R10 (read-only) with a concrete before/after comparison: the requirement is currently untestable as written.
3. Fix the machine-readable format in the PRD (or in a named design hand-off) and define the absent-field representation: scripted consumers are a stated user story, and this is the main contract they will depend on.
4. Reconcile R3's "invalid UTF-8" with the Decisions section's "sampled prefix", and name the shared bound's unit.
5. Replace "a constant number of remote requests" with a stated ceiling or a two-size comparison test.
6. Define the dashboard detail's entry and exit interaction and a measurable non-blocking criterion for R6.

## Summary
The PRD is largely precise: requirements are numbered, scope is fenced, and the deferred values (excerpt bound, unit style, cloud policy) are explicitly handed to the design with a testable floor. The remaining ambiguity clusters around under-specified markers and formats (absent, unreadable, machine-readable output), a few unquantified terms ("constant", "compact", "bounded"), and requirements without acceptance criteria (R2 ordering, R6 non-blocking, R10 read-only). I found 27 ambiguities; none is structural, so the verdict is PASS with fixes recommended.
