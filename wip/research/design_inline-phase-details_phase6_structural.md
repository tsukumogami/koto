# Reviewer: structural and security

## Verdict

PASS

The document satisfies all frontmatter, section-order, alternatives-depth, security-fidelity, consequences, public-cleanliness, and durable-path checks, and the validator returns clean with exit code 0.

## Check results

1. **Frontmatter** -- `schema`, `status`, `upstream`, `problem`, `decision`, `rationale` all present, all four required fields as literal block scalars (`|`). Frontmatter `status: Proposed` matches body `## Status` section's first non-blank line, "Proposed" (line 45), exactly -- no trailing prose on that line, prose follows after a blank line as required.

2. **Required sections, present and in order.** Actual heading order found: Status (43) -> Context and Problem Statement (52) -> Decision Drivers (85) -> Considered Options (105) -> Decision Outcome (212) -> Solution Architecture (257) -> Implementation Approach (335) -> Security Considerations (366) -> Consequences (421). Matches the required order exactly; no Context-Aware sections apply (Tactical + Public gets none of Market Context / Required Tactical Designs / Upstream Design Reference).

3. **Considered Options depth.** All four decisions carry genuine alternatives with concrete rejection rationale: D1 has A/B/C/D (D disproved with a specific byte-identical-log argument, not merely disfavored; C rejected on a compile-time constraint; B rejected on total-code and honesty grounds against A); D2 has A/B/C (C rejected on a hard constraint, not preference); D3 has A/B/C/D (A/B rejected for forcing a signature change with no behavioral gain, D rejected as over-scoped); D4 has A/B/C/D (D kept as a required complement, not eliminated as a straw option). No strawmen found.

4. **Security Considerations, mandatory and substantive.** Compared against the Phase 5 review's recommended section: the design's Security Considerations (366-419) carries every substantive point forward -- no new trust boundary for the second read path, substitution confined to the non-shell-safe pipeline, the new event variant's forward/backward-compat via the `Unknown` catch-all, occupancy-bounded (not tick-bounded) write growth, existence-check semantics bounding the concurrent-append race to a harmless duplicate, the directive-splice ordering argument, and "unlocked by construction" for `handle_status`. The one finding requiring an explicit ruling -- the `koto status` template-hash verification gap -- is ruled on rather than deferred: "`handle_status` verifies the hash, and reports a mismatch rather than failing on it" (407-408), with reasoning (don't deny an agent its only remaining instructions) and a concrete mechanism (a conditionally-present key following the existing `stale_template_source_dir` convention). This is a stronger response than either option the review posed (add the check, or document it as best-effort) -- it adds the check AND documents the caller-visible signal. Nothing the review raised was dropped.

5. **Consequences.** Positive (5 items), Negative (4 items), Mitigations (3 items) all present. Negatives read as genuine costs, not disguised positives: new log writes, a new public enum variant, larger `status` output, and a crash-window redundant re-delivery -- each is stated plainly with its actual severity, not spun. Mitigations map cleanly onto the negatives that need them (the `is_empty()` guard onto the write-growth negative, print-then-append ordering onto the crash negative, length-capping onto no listed negative but addressing pointer-budget drift called out earlier in the doc).

6. **Public-visibility cleanliness.** No private-repo paths, no private repo names, no internal codenames. All cited paths are koto-internal (`src/cli/mod.rs`, `src/engine/persistence.rs`, etc.) or public-repo PRD requirement IDs (PRD R3, R4, R6, R9, R10, R11, R12, R13, R14, R16, R17, R18, R20, R21). No issue numbers of any kind appear. Clean.

7. **Durable paths.** Zero occurrences of `wip/` anywhere in the document (confirmed via `grep -n "wip"`, no matches). The Considered Options intro (107-109) now reads "Four decisions were decomposed and evaluated independently, each against its own alternatives. Their substance is recorded below; the working reports do not survive the branch, so nothing here defers to them." -- this describes the working reports' non-durability without pointing at a `wip/...` path, so there is no dangling-pointer risk.

8. **Validator.** Ran `shirabe validate --format json --visibility=Public docs/designs/DESIGN-inline-phase-details.md` from the worktree's `koto` root. Exit code 0. Full envelope:

```json
{
  "schema_version": "shirabe-validate/v1",
  "summary": {
    "outcome": "clean",
    "errors": 0,
    "notices": 0
  },
  "findings": [],
  "advisory": {
    "summary": "Draft posture: no draft-tolerable findings to flag.",
    "notes": []
  }
}
```

9. **Writing style.** Searched for "tier/tiered", "robust", "leverage", "comprehensive/holistic", "facilitate", and common preamble phrases ("it's worth noting", "it is worth noting") -- zero matches. Searched the emoji Unicode ranges -- zero matches. Prose is direct, uses contractions ("it's", "don't"), and varies sentence length; no flagged patterns found.

## Required changes

No required changes.
