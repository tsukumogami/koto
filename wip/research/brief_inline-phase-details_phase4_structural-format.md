# Reviewer: structural format

## Verdict
PASS

The document satisfies every mechanical check in the format contract: frontmatter fields, status validity, FC03 status match, required sections in canonical order, legal `upstream` absence, public-visibility cleanliness, durable paths, and the writing-style word list, and `shirabe validate` returns a clean envelope with exit code 0.

## Check results

- **FC01 (required fields)** — PASS. Frontmatter carries `status: Draft`, `problem: |...`, `outcome: |...`. Optional field `motivating_context` is present (lines 16-22) and its shape is legal — a plain literal-block scalar explaining why the brief exists now (koto#90, PR #109, the two audits), distinct from `problem` and `outcome` per the format reference. `upstream` is absent, which is legal (optional field).
- **FC02 (valid status)** — PASS. `status: Draft`, one of the three legal values.
- **FC03 (status match)** — PASS. Body `## Status` heading's first non-blank line (line 29) is exactly `Draft` — the bare word alone, nothing else on that line. Prose ("Authored under `/scope`'s chain...") follows after a blank line (line 31), which is the legal shape. Frontmatter `status: Draft` matches case-insensitively.
- **FC04 (required sections present)** — PASS. All five present: Status, Problem Statement, User Outcome, User Journeys, Scope Boundary. An optional References section also appears after Scope Boundary, which is allowed.
- **FC15 (canonical order)** — PASS. Actual heading order found: Status (27) → Problem Statement (36) → User Outcome (82) → User Journeys (105) → Scope Boundary (150) → References (201). Matches the canonical order exactly, with the optional References section correctly trailing all required sections.
- **`upstream:` legality** — PASS (N/A). Field is absent. Absence is explicitly legal per the format reference ("Optional, because a brief may be authored from a freeform topic...").
- **Public-visibility cleanliness** — PASS. No `private/` paths, no private repo names, no internal codenames. Issue references found: `koto#90`, `PR #109`, `issue #193` — all koto's own (public repo) issue/PR numbers, which the format reference explicitly permits ("public GitHub issue numbers from the same repo are routinely cited and not in scope of this restriction").
- **Durable paths** — PASS. References section cites three paths (`docs/prds/PRD-koto-next-output-contract.md`, `docs/designs/current/DESIGN-koto-next-output-contract.md`, `docs/guides/cli-usage.md`), all durable. A full-document grep for `wip/` returned zero matches — no dangling references anywhere in prose, frontmatter, or References.
- **Writing style** — PASS. Grep for "tier/tiered", "robust", "leverage", "comprehensive/holistic", "facilitate", and preamble phrase "it's worth noting" returned zero matches. No emojis found (checked via Unicode emoji-range grep).

## Validator output

Command run:
```
shirabe validate --format json --visibility=Public docs/briefs/BRIEF-inline-phase-details.md
```

Exit code: `0`

Full envelope:
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

## Required changes

None.
