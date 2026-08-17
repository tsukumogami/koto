# Verdict: PASS

Reviewed: `docs/briefs/BRIEF-self-loop-suppresses-details.md` (9,046 bytes, 173
lines) against
`/home/dgazineu/.claude/plugins/cache/shirabe/shirabe/0.18.1-dev/skills/brief/references/brief-format.md`,
plugin version 0.18.1-dev. Structural and format compliance only.

## Findings

### 1. Required sections: present, complete, canonical order — PASS

Rule: brief-format.md "Required Sections" and FC04/FC15. Five required
sections in order: Status, Problem Statement, User Outcome, User Journeys,
Scope Boundary.

Observed (heading line numbers): `## Status` (25), `## Problem Statement`
(34), `## User Outcome` (64), `## User Journeys` (82), `## Scope Boundary`
(123). All five present, in canonical order, at `##` depth, each preceded by
the `# BRIEF:` title at line 23.

Optional sections sit in permitted positions after the required block:
`## Open Questions` (158), `## References` (167). This matches the Section
Matrix ordering (Open Questions, Downstream Artifacts, References) — Downstream
Artifacts is absent, which is correct for a brief with no downstream PRD yet.
`## Open Questions` is permitted because status is `Draft`; it must be emptied
or removed before the Draft -> Accepted transition, which is a finalization
gate, not a Phase 4 blocker.

No finding.

### 2. Frontmatter: schema, required fields, block scalars — PASS

Rule: brief-format.md "Frontmatter"; FC01 (required fields), FC02 (valid
status).

- `schema: brief/v1` — present at line 2, exact match on the map key the
  validator routes on.
- `status: Draft` — line 3, in the valid set {Draft, Accepted, Done} (FC02).
- `problem: |` — lines 4-8, YAML literal block scalar, body is 4 lines. Within
  the documented 2-4 line range.
- `outcome: |` — lines 9-13, literal block scalar, body is 4 lines. Within
  range.
- `motivating_context: |` — lines 14-20, literal block scalar, 6 lines. Valid
  optional field; the format reference sets no line bound on it, and the
  sibling `BRIEF-inline-phase-details.md` carries an 8-line one.

Parsing is well-formed: `shirabe validate` read the frontmatter without a YAML
error and reported zero findings.

No finding.

### 3. `upstream:` absent — correct, and the exact trap avoided — PASS

Rule: brief-format.md "Why a brief does not name its roadmap" / R11 — never a
ROADMAP, never a PRD or anything below the brief.

The document carries no `upstream:` key at all. I verified this is the right
call rather than an omission: `ls docs/` in this repo shows `briefs`, `designs`,
`guides`, `prds`, `reference`, `testing` — there is no `docs/strategies/` and no
`docs/roadmaps/`, so there is no durable strategic ancestor in-repo to name.
The field is documented as optional precisely for the freeform-topic case.

The trap this brief could have fallen into is naming
`docs/briefs/BRIEF-inline-phase-details.md` as `upstream:` — it is genuinely
the parent framing, but it is a BRIEF, i.e. *at* the brief altitude, not above
it, and would be an R11-class violation. The document instead cites it under
`## References` (line 169), which is the correct surface for an in-repo
precedent. Same for `docs/prds/PRD-koto-next-output-contract.md` (line 171) — a
PRD is strictly below a BRIEF and would be an outright forbidden `upstream:`
value; it is correctly a reference.

No finding.

### 4. FC03 — `## Status` first non-blank line — PASS

Rule: FC03. The entire first non-blank line under `## Status` must equal the
frontmatter `status`, compared case-insensitively as a whole line.

Lines 25-27:

```
## Status

Draft
```

Line 26 is blank, line 27 is `Draft` — the bare status word alone, no trailing
period, no prose on the line. Line 28 is blank, and the explanatory paragraph
begins at line 29 (`Authored under `/scope`'s chain for koto#90.`). This is the
exact shape the format reference documents as passing, and it avoids the
documented most-common failure (`Draft. The brief stops before...`).

Frontmatter `status: Draft` matches the body word `Draft`. FC03 satisfied, and
the validator confirms it.

No finding.

### 5. Public-visibility cleanliness — PASS

Rule: brief-format.md "During /brief (drafting)" — no `private/` paths, private
repo names, private filenames, internal codenames, or private-repo issue
numbers.

I grepped the document for `private/`, `vision`, `coding-tools`, `dot-niwa`,
`tsukumogami/(vision|tools|coding-tools|dot-niwa-overlay)`, and `wip/`
case-insensitively. Zero matches (grep exit 1).

Issue references, extracted exhaustively: `koto#90` at lines 15, 29, and 46 —
three occurrences, all the same public issue in this public repo, which the
format reference explicitly permits ("public GitHub issue numbers from the same
repo are routinely cited and not in scope of this restriction"). No bare `#NN`
that could resolve ambiguously to a private repo.

Path-like strings, extracted exhaustively: `/scope` (line 29, a public skill
name, not a filesystem path), `docs/briefs/BRIEF-inline-phase-details.md`
(169), `docs/prds/PRD-koto-next-output-contract.md` (171). No private
filenames, no internal codenames.

One phrase worth naming so it is not mistaken for a leak on a later read: the
`motivating_context` says "The criterion is now ruled to govern" (line 19) and
the Open Questions say "or the ruling" (line 161). "The ruling" is an unnamed
referent, not a private artifact path — it names no repo, no file, and no issue.
It is therefore clean for visibility purposes. Whether an unnamed referent is
adequate framing is a content judgment and belongs to the other reviewer.

No finding.

### 6. Reference paths durable and existent — PASS

Rule: brief-format.md "Downstream Artifacts" / "References" — durable
repo-relative paths, never `wip/...`.

There is no `## Downstream Artifacts` section. `## References` carries two
entries, both durable repo-relative paths with a one-sentence purpose:

- `docs/briefs/BRIEF-inline-phase-details.md` — exists, 12,089 bytes, confirmed
  by `ls docs/briefs/`. I opened it and confirmed it is the framing for the
  inline-phase-details mechanism, so the description ("the framing for the
  mechanism this brief moves a boundary inside") is accurate rather than
  decorative.
- `docs/prds/PRD-koto-next-output-contract.md` — exists, 24,238 bytes, confirmed
  by `ls docs/prds/`.

Neither is a `wip/...` path. The grep in finding 5 confirms the string `wip/`
does not appear anywhere in the document, including in prose — this matters
because the brief's Scope Boundary and Open Questions discuss upstream artifacts
at length and could easily have named a staging path.

No finding.

### 7. Scope Boundary sub-heading text diverges from the sibling — ADVISORY, non-blocking

Rule: brief-format.md "Scope Boundary" — "Two explicit lists: what's IN and
what's OUT." The reference mandates two lists; it does not mandate the heading
text.

The document uses `### In` (125) and `### Out` (138). The sibling
`BRIEF-inline-phase-details.md` uses `### In scope` (155) and `### Out of scope`
(176).

I checked the whole brief corpus before calling this a divergence, and the
corpus has no single convention:

| Brief | Scope Boundary sub-shape |
|---|---|
| BRIEF-self-loop-suppresses-details | `### In` / `### Out` |
| BRIEF-inline-phase-details | `### In scope` / `### Out of scope` |
| BRIEF-session-legibility | `**IN:**` / `**OUT:**` (bold, not headings) |
| BRIEF-native-workflows-render | no `###` sub-headings |
| BRIEF-native-workflows-phase-detail | no `###` sub-headings |
| BRIEF-request-store-converge | no `###` sub-headings |

So three distinct shapes already exist across six briefs, and the format
reference is deliberately silent on which. The new document's shape is the
clearest of the three (real headings, shortest labels) and satisfies the
"two explicit lists" rule unambiguously.

Fix if the author wants sibling-exact symmetry: rename to `### In scope` and
`### Out of scope`. I do not recommend it as a blocker — there is no rule to
violate and no reader would be misled.

### 8. `outcome` frontmatter is narrower than the `## User Outcome` prose — ADVISORY, non-blocking

Rule: brief-format.md Quality Guidance, User Outcome — "Matches the `outcome`
frontmatter value. Divergence between the prose outcome and the YAML field
signals one is stale."

The `outcome` field (lines 9-13) covers two things: the loop pays for the
procedure once, and nothing becomes unreachable (arrival from elsewhere,
deliberate send-back, read-only retrieval).

The `## User Outcome` section covers those two in its first two paragraphs, then
adds a third (lines 77-80): a template author reading the agent-facing docs
finds a rule matching the engine, and a maintainer reading the durable design
record finds the same rule with the reversal recorded. That third outcome is not
summarized in the frontmatter at all, and it is load-bearing elsewhere in the
document — the Scope Boundary "In" list explicitly pulls in the agent-facing
surfaces (line 133-134) and the upstream BRIEF/PRD/DESIGN plus changelog (135-
136) on the strength of it.

This is a summary that is narrower than what it summarizes, not a contradiction,
so it is not a staleness signal in the dangerous direction and nothing
downstream would read a wrong value. But a reader who consumes only the
frontmatter would not learn that documentation correctness is an intended
outcome, and would then be surprised by a third of the "In" list.

Fix: extend the `outcome` block scalar with a clause on the documented rule
matching the engine. The field is currently 4 lines, which is the top of the
documented 2-4 range, so this needs a tightening edit rather than an append —
for example compressing the "Arriving somewhere new, returning from somewhere
else, or being sent back deliberately" enumeration to make room. Worth doing
before the Draft -> Accepted transition; not worth blocking Phase 4 over.

### 9. Spelling: `behaviour` vs `behavior` — ADVISORY, cosmetic

The document uses British `behaviour`/`behaviours` five times (lines 60, 61,
116, 121, 132). The sibling brief and the rest of `docs/briefs/`, `docs/prds/`,
and `docs/guides/` use American `behavior` — 22 occurrences of `behavior`
against 5 of `behaviour`, and all five `behaviour` are in this one document.
`docs/designs/current/` does carry `behaviour`/`behavioural` in four places, so
the repo is genuinely mixed and there is no rule being broken.

Flagging only because `### A maintainer auditing why the behaviour is what it
is` (line 116) is a heading, which is the most visible possible placement of the
minority spelling. Fix, if the author cares: normalize the five to `behavior`.
Purely cosmetic; blocks nothing.

### 10. Line width — checked, no finding

I checked line lengths since the corpus wraps prose narrowly. Six lines exceed
80 columns, all at exactly 81 (lines 12, 95, 112, 148, 161, 163). The sibling
`BRIEF-inline-phase-details.md` has eight lines over 80. Same posture, no
divergence, no finding. Recording the check so it is not re-run.

### 11. Other structural comparisons against the sibling — no divergence

- Title line: `# BRIEF: a lap around a loop is not a new arrival` (23) mirrors
  the sibling's `# BRIEF: phase instructions an agent can rely on` (25) —
  `# BRIEF: ` prefix plus a sentence-case phrase, consistent.
- User Journeys: five journeys, each with a `###` name heading (84, 93, 101,
  109, 116). Sibling has four, same shape. The format reference requires "at
  least one journey with a name heading" at finalization; five clears it.
  Whether the five are genuinely distinct entry points is the content
  reviewer's call.
- Status prose: both documents open the post-blank-line paragraph with
  "Authored under `/scope`'s chain for koto#90." and then divide ownership
  between the downstream PRD and DESIGN. Deliberately parallel.
- `motivating_context` present in both. Consistent.
- Sibling has `## References` but no `## Open Questions` (it is `Done`); this
  document has both (it is `Draft`). That difference is required by the
  lifecycle, not a divergence.

## Validator output

Command run from the worktree root
`/home/dgazineu/dev/niwaw/tsuku/tsuku+koto_90_self_loop-73478d7e/public/koto/.claude/worktrees/koto-90-self-loop`,
binary `/home/dgazineu/.tsuku/tools/current/shirabe`:

```
$ shirabe validate docs/briefs/BRIEF-self-loop-suppresses-details.md --format json --visibility=Public
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
EXIT=0
```

Zero errors, zero notices, exit 0. FC01, FC02, FC03, FC04 and the ordering
check all pass.

## Summary

PASS. The document is structurally clean: all five required sections in
canonical order, well-formed `brief/v1` frontmatter with both required block
scalars in range, an FC03-correct bare `Draft` on the first line under
`## Status`, no private paths or private-repo issue numbers, and two References
that are durable repo-relative paths I confirmed exist on disk. `shirabe
validate --visibility=Public` returns clean with zero errors and zero notices.

The most important finding is #8: the `outcome` frontmatter summarizes only two
of the three outcomes the `## User Outcome` prose carries, omitting the
documentation-correctness outcome that a third of the Scope Boundary "In" list
depends on. Findings #7 (Scope Boundary sub-headings read `In`/`Out` where the
sibling reads `In scope`/`Out of scope`) and #9 (British `behaviour` against a
mostly-American corpus) are cosmetic.

None of the three is blocking. #8 is worth fixing before the Draft -> Accepted
transition, alongside the separately-required emptying of `## Open Questions`;
#7 and #9 are the author's discretion.
