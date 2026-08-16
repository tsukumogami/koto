# Category C: AC Discriminability

## Verdict
findings

## Critical findings

- category: C
  affected_issue_ids: [2]
  description: "Issue 2 AC 'recording that delivery changes nothing observable for existing override callers' passes vacuously for an implementation that never appends a delivery record on the --full/override path at all — nothing recorded trivially satisfies 'nothing observable changed,' and no other AC in Issue 2 exercises an override call immediately followed by a plain koto next on the same occupancy, so the resulting bug (the plain call wrongly re-delivers instructions the agent already got via override) goes uncaught."
  pattern: "4 (state-without-transition), adapted — the AC checks output-shape equivalence for the override caller but never exercises the causal chain (a subsequent plain call) that would reveal whether the append actually happened."

- category: C
  affected_issue_ids: [3]
  description: "Issue 3's 'ordinary non-batch session returns immediately while a first process is mid-tick' AC does not discriminate a locking bug from a correct implementation. Confirmed against src/cli/mod.rs:3746-3770: the advisory flock is acquired only when state_is_batch_scoped(...) is true; non-batch ticks never take any lock at all. So in the non-batch scenario there is nothing for a wrongly-added handle_status lock to contend with — an implementation that unconditionally attempts a non-blocking lock inside handle_status would also 'return immediately' here, same as a correct implementation that never locks. Only the adjacent batch-scoped AC actually exercises real contention."
  pattern: "5 (integration/concurrency scope gap) — the AC's chosen scenario cannot observe the property (no locking) it claims to verify, because no lock exists in that scenario under either implementation."

- category: C
  affected_issue_ids: [3]
  description: "Issue 3 AC 'adds a conditionally-present key naming the divergence rather than failing' does not specify the key's name or value shape (the design doc is equally silent, only citing stale_template_source_dir as a precedent). A wrong implementation could add a bare boolean like {\"template_mismatch\": true} — technically present-and-conditional, and its key name arguably 'names' the divergence — without identifying what diverged (old hash vs. new hash), and the AC as written would pass."
  pattern: "7 (existence-without-correctness) — presence of a conditionally-added key without any assertion on its shape or content."

- category: C
  affected_issue_ids: [2]
  description: "Issue 2's byte-identity AC ('responses are byte-identical to the pre-change binary's for the same template and call sequence') does not say which call sequences must be covered. Contrast with the upstream PRD's own criterion, which explicitly requires the comparison 'on every path above' (conditional, unconditional, directed, self-transition, rewind, override, batch-child init). As written, an implementer could capture the baseline and diff against only the simplest sequence (e.g., init + one next) and call the AC satisfied, while a stray field or ordering difference on the directed-transition or rewind sequence for an instruction-free template goes undetected."
  pattern: "5 (integration/concurrency scope gap), adapted to verification-sequence scope — the AC's untestable ambiguity in which sequences count lets a partial check stand in for the full one the PRD (R6's AC) actually requires."

- category: C
  affected_issue_ids: [1]
  description: "Issue 1 AC 'No observable behavior changes. The full suite passes and no response differs from before this issue' has no operational check available at the point this issue lands: the byte-identity fixture/baseline this claim would need is explicitly Issue 2's prerequisite work (per the plan's own Implementation Sequence section) and does not exist yet. So the AC reduces to 'the existing suite passes,' which was never written to assert exact response bodies for the paths this feature touches. A wrong Issue 1 implementation that partially wires the new predicate into a response-construction site (contrary to the issue's own Goal) would only be caught if an existing test happens to pin the exact response for that corner case — which the design's own rationale (no current test compares whole response bodies) says is not the case."
  pattern: "5 (integration/concurrency scope gap), adapted — the claim needs a check ('no response differs') that the codebase does not yet have a mechanism to make, so the AC is unfalsifiable in a way distinct from 'trivially true because nothing is wired in' (which is fine) and 'guaranteed to be caught if violated' (which is not true)."

- category: C
  affected_issue_ids: [5]
  description: "Issue 5 AC 'Every skill under the plugin tree still has at least one eval, and any eval asserting the old delivery behavior is updated to assert the new one' is satisfiable by deleting the meaningful assertion and replacing it with a trivial one. 'At least one eval' is a bare existence count with no content requirement (a no-op eval that only checks exit code 0 counts). 'Updated to assert the new one' does not specify what the new assertion must check (e.g., that a second non-advancing tick omits the instructions, or that a rewind re-delivers them) — a superficial edit that changes wording but not the underlying check technically satisfies 'updated.'"
  pattern: "7 (existence-without-correctness) — presence of an eval, and presence of a changed eval, without any assertion about what it must verify."

- category: C
  affected_issue_ids: [4]
  description: "Mechanical pattern-pass match, lower confidence: no AC in Issue 4's body contains any of the pattern-3 trigger terms (fail, failure, error, invalid, edge case, empty, missing, not found, rejected, unauthorized, timeout, concurrent, conflict, duplicate). The closest candidate, the abandonment-notice-and-pointer-both-apply case, functions as an edge case in substance but is not phrased with any trigger keyword, so it does not satisfy the false-positive guard under the literal detection rule. Flagged per the pattern pass's 'flag immediately, no further reasoning' instruction; substantively the both-apply AC provides some negative-path-adjacent coverage, so this is the weakest finding in this set."
  pattern: "3 (happy-path-only, issue-level)"

## Reasoning

**F1 (Issue 2, override recording):** The design (Solution Architecture, "Data flow, natural-advancement path" step 4) states the delivery record is appended "if it carried the instructions," which per Decision 3/4 includes override-forced deliveries. But no AC in Issue 2 sequences an override call followed by a plain `koto next` on the same occupancy to check that the plain call now suppresses. The "changes nothing observable" AC only checks the override caller's own response shape, which is satisfied whether or not the append happens. Rewrite: add an AC that performs `koto next --full` on an occupancy that has not yet received a natural delivery, then a plain `koto next` immediately after, and asserts the second call omits the instructions (proving the override path's append actually feeds the predicate).

**F2 (Issue 3, non-batch lock AC):** Read `src/cli/mod.rs:3746-3770` directly — the flock is scoped by `state_is_batch_scoped(...)`, comment states "Non-batch workflows intentionally skip the lock." Since ordinary ticks hold no lock to block `handle_status` on, "returns immediately while mid-tick" is true regardless of whether `handle_status` itself tries to lock. The AC as written cannot fail for a `handle_status` that (wrongly) attempts its own lock, because there's no contender. Rewrite: this scenario should instead assert that `handle_status` makes no `lock_state_file`/flock syscall at all during the retrieval (e.g., via strace or a mock backend that panics/records if locking is attempted), independent of whether another process is running.

**F3 (Issue 3, mismatch key):** Neither the plan nor the design names the key or its value shape for the hash-mismatch signal. `stale_template_source_dir`'s own shape (checked in the code: a nested object, not a bare boolean) is the only precedent, but the AC doesn't say the new key must follow the same shape. Rewrite: name the key explicitly (e.g., `template_hash_diverged`) and require it carry both the recorded and current hash values, not just a presence signal.

**F4 (Issue 2, byte-identity scope):** The PRD's own equivalent AC enumerates "on every path above," explicitly covering conditional, unconditional, directed, self-transition, rewind, override, and batch-child-init sequences. The plan's Issue 2 AC drops that enumeration and speaks only of "the same template and call sequence" (singular, unspecified). Rewrite: restate the PRD's explicit path list in the plan AC so the baseline capture and comparison are pinned to the same set the PRD already specified, closing the gap between the two documents.

**F5 (Issue 1, unfalsifiable claim):** Cross-checked against the plan's own "Implementation Sequence" section, which states the byte-identity baseline "does not exist today" and must be captured before Issue 2's first behavior-changing commit — i.e., strictly after Issue 1. So Issue 1 cannot lean on that fixture. The AC's only real teeth is "does the full existing suite still pass," which was not written against this feature's response paths. Rewrite: replace with a mechanical check that is available at Issue 1 — e.g., grep/compile-time assertion that no call site outside the new module invokes `instructions_delivered_this_occupancy`, which directly verifies the Goal's "without wiring either into a response path" rather than relying on an absent regression fixture.

**F6 (Issue 5, eval content):** Neither R23 (PRD) nor this AC specifies what the updated eval must assert beyond "the new one," and "at least one eval" per skill has no minimum-content bar. Rewrite: require the updated eval assert at least the two-tick suppression sequence (first delivery, then omission on a non-advancing repeat) or the rewind re-delivery, by name, and require the plugin-tree eval count to be checked against a stored baseline count (not just ">= 1") so a skill silently losing coverage elsewhere doesn't slip through.

**F7 (Issue 4, mechanical pattern-3 match):** Flagged per the phase-3 pattern pass rule, which requires flagging on keyword absence without further reasoning. Substantively the both-apply-notices AC and the "declares no instructions -> no pointer" AC both function as edge-case coverage, so confidence on this one is lower than the others; included per the taxonomy's literal instruction rather than independent conviction.
