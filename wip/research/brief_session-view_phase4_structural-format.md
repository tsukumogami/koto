# Structural Format Review

**Verdict:** PASS

The BRIEF has valid frontmatter, all five required sections in order, a bare "Draft" status line matching the frontmatter, no private references, and no banned-word or em-dash violations.

## Violations Found
None.

Checks performed:
1. Frontmatter: schema, status (Draft), problem (4 lines) and outcome (4 lines) are present. No `upstream`, so there is no visibility conflict.
2. Sections: Status, Problem Statement, User Outcome, User Journeys and Scope Boundary appear in order. Open Questions follows them, which is allowed because status is Draft.
3. FC03: the first non-blank line under `## Status` is `Draft`, equal to the frontmatter status.
4. No placeholders. Every section has real content. The Scope Boundary has explicit In scope and Out of scope lists, and the exclusions are real ones a PRD author might otherwise assume are in.
5. Frontmatter and body agree. `problem` and `outcome` paraphrase the Problem Statement and User Outcome without contradicting them.
6. Style: none of the banned terms from rules.yaml appear in the prose. "Journeys" appears only in the required heading and the section's `###` journey headings. Em dashes number about 5 in roughly 900 words, which is under the 10 per thousand threshold (and under the 300-word minimum logic). There are no emojis and no AI attribution.

## Public-Visibility Flags
none. Issue references koto#308, koto#162 and koto#234 are public same-repo issues. No `private/` paths, private repos or private filenames appear.

## Suggested Improvements
1. Out of scope, first bullet: "the maintainers ruled the view rides existing surfaces (2026-10-10)" is attribution with a date but no citation. Cite a public issue or PR, or drop the attribution and state the constraint directly.
2. Problem Statement, last sentence: "reads refuse, and the surfaces that bypass the backend show holes" is slightly abstract. Name the surface, for example "the dashboard shows blank fields".
3. Scope Boundary, In scope: "each to the depth that fits it" is vague. The Out of scope bullet on key content in the workflow view already carries the specifics, so this phrase could be cut.

## Summary
The BRIEF passes structural format review with zero violations. It is public-clean and meets the writing-style rules. The suggested improvements are optional polish.
