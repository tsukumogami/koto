# Reviewer: completeness

## Verdict
FAIL

Two required-change items land on unambiguous gaps (an untested non-functional requirement and an untested merge gate this change will trip); the rest are real but minor, so a fast follow-up pass should clear this.

## Requirement-to-criterion mapping

| Req | Criteria covering it | Status |
|---|---|---|
| R1 | "gated phase whose gate fails... second `koto next`... no instructions field" | Covered |
| R2 | same as R1 | Covered |
| R3 | "loops back to that phase" (conditional re-entry); "advanced past a phase and then rewound" (rewind); "directed transition into a phase never occupied" (directed); "`koto init` followed by first `koto next`" (init); "batch-spawned child's first `koto next`" (batch init) | Covered, with a gap — see Findings §2: no criterion is written against an *unconditional* transition arrival as distinct from the conditional one R3 also names |
| R4 | "Two consecutive directed transitions into the same phase" | Covered |
| R5 | "existing override flag returns the instructions on a response where the rule would otherwise have omitted them" | Covered |
| R6 | "template whose phases declare no instructions produces responses with no instructions field, on every path above" | Partially covered — see Findings §2: R6 promises byte-identical *responses*, the criterion only checks the one field |
| R7 | "retrieval, invoked with only the workflow name, returns..." | Covered |
| R8 | same criterion + "directive or instructions block containing a runtime variable comes back with the variable substituted" | Covered |
| R9 | "retrieval returns the phase's expected-evidence schema when the phase declares one" | Covered |
| R10 | "retrieval returns the instructions on a phase where the delivery rule is currently suppressing them"; "Retrieving does not change what the next `koto next` returns..." | Covered |
| R11 | "session state file is byte-identical before and after"; gate-command-not-executed; default-action-not-executed; terminal-no-cleanup | Covered |
| R12 | "retrieval succeeds while a second process holds the session, without blocking" | Covered |
| R13 | "unknown workflow name returns a structured error..."; "phase that declares no instructions succeeds and reports their absence" | Partially covered — see Findings §2: R13's own text enumerates "an unknown workflow, an unreadable or corrupt session, and a phase that declares no instructions" but not the terminal-state case, and no criterion asserts a terminal-state query is a normal (non-error) response, only that it skips cleanup |
| R14 | "Every non-terminal `koto next` response carries a pointer naming the retrieval" | Covered |
| R15 | "phase's own directive text is present and unaltered in a response that also carries the pointer" | Covered |
| R16 | "No file is added under the session directory... state-file schema version is unchanged" | Covered |
| R17 | "`koto-stability-tests` passes unmodified" | Covered |
| R18 | — | **GAP — no criterion of any kind checks per-call cost or the "no new file reads" claim** |
| R19 | "Tests exercise the response construction and the delivery rule directly, covering at minimum: the non-advancing repeat, the rewind arrival, and both directed-transition cases" | Covered |
| R20 | "`koto-user`'s response-shapes and command-reference documents describe the shipped rule and the retrieval" | Covered |
| R21 | "`koto-author`'s SKILL.md and template-format reference describe what an author can rely on" | Covered |
| R22 | "`docs/guides/cli-usage.md` and the Cursor rules file match the shipped behavior" | Covered |
| R23 | "Every skill under `plugins/*/skills/*/` still has at least one eval, and any eval asserting the old delivery behavior is updated" | Covered |
| R24 | "`CHANGELOG.md` records the change" | Covered, but thin — see Findings §4 |

Criteria mapping to no requirement (orphans): "`cargo fmt --check`, `cargo clippy -D warnings`, and the full test suite pass" and "`wip/` is empty and no committed file references a `wip/` path." Both are standing workspace-wide boilerplate (CI hygiene, wip-hygiene rule) that every PRD in this chain carries regardless of feature content — not a sign of drift, not something I'd ask the author to number a requirement for.

## Findings

### 1. Every established defect and constraint has a requirement (rubric item 1)

Walked all three phase-2 research docs and the explore findings against R1-R24. Every defect the path-matrix lead confirmed — the gate-blocked re-tick (R1/R2), the rewind inflation (R3), the `--to` missing check (R4), the accepts-fallthrough non-issue (addressed in Decisions, correctly needs no dedicated criterion), batch retry/spawn being non-issues (correctly excluded) — has a requirement or an explicit, reasoned Decisions-section disposition. The recovery-contract lead's payload analysis (directive/details/state/expects required, `advanced`/`unassigned_children` excluded, substitution required, the ten-item non-effects enumeration, the no-lock concurrency answer) maps cleanly onto R7–R13, and R11's exhaustive list matches the research's "checkable claims" list item for item except lock acquisition, which is correctly split out into R12 instead. The downstream-obligations lead's two mandatory skills, `cli-usage.md`, and the previously-uncovered `.cursor/rules/koto.mdc` are all named in R20–R22 — the PRD correctly picked up the `koto.mdc` surprise the research flagged as easy to miss. No established finding is missing a requirement.

### 2. Requirement-to-criterion gaps (rubric item 2)

**R18 has zero acceptance criteria.** It's a real, specific non-functional constraint ("one additional pass over an already-read event list, with no new file reads on the `koto next` path") and nothing in the Acceptance Criteria section checks it — not the delivery-rule group, not the constraints group. Every other non-functional requirement (R16, R17, R19) has a matching criterion; R18 alone does not. This is a genuine hole a developer could walk straight through: ship a version that re-reads the template file per tick, and no acceptance criterion catches it.

**R3's "every way of arriving" list names five paths; the criteria test four distinctly.** R3 explicitly enumerates "a conditional transition, an unconditional transition, a directed transition, a rewind, and workflow initialization." The delivery-rule criteria cover rewind, directed transition (twice), and initialization (`koto init` and batch spawn) explicitly, and cover a conditional transition via the "loops back to that phase" scenario — but no criterion is written against an *unconditional* transition arrival as a distinct case. The path-matrix research does note that conditional and unconditional transitions both resolve through the same `Transitioned`-event code path, so the mechanism is shared and a conditional-transition test likely exercises the same logic — but R3 chose to name both explicitly as separate members of "every way," and the criteria don't mirror that. Minor, but worth a one-line addition or an explicit note that the two are mechanically identical and one test stands for both.

**R6's "byte-identical" claim is checked more narrowly than it's written.** R6 says a details-free phase "produces responses byte-identical to those koto produces today" — the whole response, not one field. The matching criterion only asserts "responses with no instructions field." If the fix incidentally reorders another field or changes an unrelated key while still satisfying "no instructions field," the criterion passes and R6 fails silently. Either narrow R6's wording to match what's actually tested, or broaden the criterion to a real byte-comparison.

**R13 doesn't name the terminal-state case, and no criterion says querying a terminal workflow is a normal (non-error) response.** The recovery-contract research is explicit that this needs deciding: `koto status` treats a terminal workflow as a normal successful response, `koto next` treats terminal *evidence submission* as an error, and a read-only recovery call has no evidence to submit so neither precedent applies automatically — "it should follow `status`'s precedent." R13 as written enumerates only "an unknown workflow, an unreadable or corrupt session, and a phase that declares no instructions" — terminal state isn't in that list. The matching criterion ("does not clean up the session; the session directory still exists afterwards") tests only that cleanup is skipped, not that the response itself is a success rather than an error envelope. This is the research's own explicitly-flagged decision point, and the PRD is silent on it.

### 3. The BRIEF's user journeys (rubric item 3)

The BRIEF's `## User Journeys` section has four subsections, not five: a gate-blocked loop, a rewind, a combined context-loss journey (compaction and respawn told together in one narrative), and a template author. All four are represented in the PRD's User Stories, and the PRD's split of the combined context-loss journey into two separate stories ("As an agent whose context was compacted" / "As a freshly respawned agent resuming an existing session") is a faithful, reasonable disaggregation of what the BRIEF's own Problem Statement already treats as two distinct triggers with one shared failure mode — not an invented scenario. The PRD adds a sixth story, "As a koto maintainer... covered by tests," which has no BRIEF journey behind it but is grounded directly in the downstream-obligations research's finding that the suppression decision has zero direct test coverage today; that's a legitimate addition, not scope creep. Coverage is complete against what the BRIEF actually contains.

### 4. Downstream obligations against the research's MERGE GATES / CONVENTIONS split (rubric item 4)

Checked every row of the downstream-obligations lead's MERGE GATE table against the PRD's requirements and criteria:

- Skill eval existence, wip/-hygiene, `cargo fmt`, `cargo clippy -D warnings`, unit/integration tests, stability-tests — all covered (R17/R19/R23 plus the boilerplate criteria noted above).
- **Template compilation is a merge gate this change will trip, and nothing in the PRD names it.** The research is explicit: `validate-plugins.yml`'s `template-compilation` job runs on any PR touching `plugins/**`, and this PRD's own R20/R21/R23 mandate touching `plugins/koto-skills/skills/**` — so the gate fires on this PR by construction. It's unlikely to fail (the fix doesn't touch the marker parser), but "unlikely to fail" is exactly the kind of claim an acceptance criterion exists to pin down rather than assume. No requirement or criterion mentions it.
- PR-body mechanics and doc-lifecycle discipline are also nominal merge gates that will fire, but I don't think a PRD's Acceptance Criteria section is the right place for "PR title follows Conventional Commits" — that's process mechanics enforced identically on every PR in the workspace regardless of this feature's content, the same category as the two orphan criteria already in the doc. I'm not counting these as gaps.
- The CONVENTIONS side (eval-results-in-PR-description table, running evals manually, the skill-drift assessment) are all correctly treated as author/reviewer discipline rather than requirements — the PRD doesn't try to make CI-invisible conventions into testable criteria, which is the right call.

One softer note on R24/CHANGELOG: the downstream research flags that this repo's CHANGELOG convention distinguishes an ordinary "Added" entry from an explicitly flagged "(load-bearing)" behavior-change subsection, and that this PRD's fix does change the *default* shape of `koto next` responses for existing callers (R1–R4 alter what ships in every response, not just what a new opt-in surface returns) — which is exactly the condition the research says warrants the load-bearing callout. R24/its criterion just say "records the change," with no requirement that the load-bearing convention be followed. This is thin rather than missing — I'd call it a nit, not a blocking gap, since "records the change" is defensible as leaving format choice to the author. Noting it rather than requiring it.

### 5. Out of Scope completeness and consistency (rubric item 5)

All six of the BRIEF's Out-of-Scope exclusions (auto-advance discarding crossed phases, two-rewinds-move-forward, `accepts:` not gating advancement, migration-scan flood, retrofitting existing templates, changing the shared visit-count derivation's semantics) appear verbatim in the PRD's Out of Scope section. The PRD adds one more exclusion not in the BRIEF's list — "Requiring the directed-transition path to evaluate gates" — but this isn't a silent readmission or contradiction: the BRIEF explicitly left "which reading wins — defect or deliberate carve-out" for the `--to` path to the DESIGN to settle, and the PRD's Decisions section resolves it provisionally toward "defect," names the alternative reading explicitly, and states the DESIGN can overturn it with a stated reason. That's the PRD doing exactly the job the BRIEF assigned it, not overriding the BRIEF. No exclusion was silently dropped or silently readmitted in either direction.

### 6. Open Questions (rubric item 6)

The PRD has no Open Questions section — correct for finalization. I checked the Decisions and Trade-offs section for anything phrased as an unresolved question rather than a recorded decision. Two entries explicitly defer a choice to the DESIGN ("Whether the retrieval reports gate state is left to the DESIGN," "Where the retrieval lives and what it is called are DESIGN questions") — but both are stated as deliberate PRD/DESIGN altitude boundaries with reasoning given, not open questions about what the requirement means or how it'd be tested. R9 and R7 are each fully specified and independently testable regardless of how the DESIGN resolves either deferral. I don't read either as a hidden Open Question.

### Redundancy and unsupported claims

No requirement pair is redundant enough that one could be dropped without losing testable coverage — R1/R2/R3/R4 look similar at a glance but each pins a distinct, separately-violable case (the general rule, the non-advancing-repeat instance, the first-response instance, and cross-path parity). I didn't find any claim in the PRD — the measured 14-iteration/7,140-character example, the R9-of-`PRD-koto-next-output-contract` provenance, the `.cursor/rules/koto.mdc` surface — that isn't directly grounded in the research or the BRIEF.

## Required changes

1. Add at least one acceptance criterion for R18 (per-call cost / no new file reads on the `koto next` path). Even a narrow one — e.g., "the fix's event-derivation pass runs in the same single pass over `read_events`'s output that today's `derive_visit_counts` call already makes; no new call to a file-reading function is added to `handle_next`'s success path" — closes the hole.

2. Name the template-compilation merge gate (`validate-plugins.yml`'s `template-compilation` job, triggered because this PRD's R20/R21/R23 require touching `plugins/koto-skills/**`) in a requirement or criterion — most naturally folded into the existing downstream-obligations group alongside R22/R23, or as its own line in the "Constraints and downstream" acceptance-criteria group.

3. Extend R13 to name the terminal-state case explicitly (the retrieval on a terminal-state workflow is not an error) and add a matching criterion asserting the response is a normal success envelope reporting the terminal state — not just that cleanup is skipped. This is a decision the recovery-contract research flagged as needing an explicit PRD answer, and right now it's implied rather than stated.

4. Either add a criterion distinguishing an unconditional-transition arrival from a conditional one (matching R3's own five-item list), or add a one-line note (Decisions section is the natural place) that the two share one code path and one test stands for both — so a reader doesn't have to reconstruct that from the path-matrix research themselves.

5. (Non-blocking, recommended) Narrow R6's "byte-identical to those koto produces today" wording to match what the criterion actually checks (field presence), or broaden the criterion to a genuine byte-for-byte response comparison.
