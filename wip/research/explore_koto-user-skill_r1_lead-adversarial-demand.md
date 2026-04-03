# Demand Validation: koto-user Skill

**Lead**: adversarial-demand
**Topic**: Creating a `koto-user` skill — a guide for agents running koto-backed workflows

---

## Q1: Is demand real?

**Finding**: No issue, PR description, commit message, or code comment in this repository documents an agent failing to use a koto-backed workflow for lack of guidance. The closest evidence is structural: four sequential PRs (#120–#125) shipped new engine behavior (structured gate output, gate override mechanism, compiler validation, backward compatibility) without any corresponding update to skills, AGENTS.md, or koto-user-facing documentation. The commit `ffd2665` ("docs: document structured gate output in CLI and skill authoring guides") explicitly closes "doc gaps introduced by the structured gate output implementation" — confirming the gap was recognized after the fact, not before it caused a reported failure. The scope capture in `wip/explore_koto-user-skill_scope.md` identifies this pattern directly: "The gate-transition roadmap was fully implemented in PRs #120–#125 without any corresponding skill update — demonstrating real drift risk."

The evidence shows recognized documentation debt and a named drift risk, not confirmed reports of agents failing in practice.

**Confidence**: Low — the drift risk is documented by the maintainer as a design-level concern, but no external reporter, issue, or failure event has been recorded.

---

## Q2: What do people do today instead?

**Finding**: Three distinct substitutes exist and are all present in the repository:

1. **`plugins/koto-skills/AGENTS.md`** (550 lines): A plugin-level runtime reference covering every `koto next` response shape, all error codes, and two end-to-end worked examples (koto-author workflow and the work-on workflow). This is the closest functional equivalent to a koto-user skill. It documents all action values, the `blocking_conditions` field, `expects` schema, `details` and `advanced` fields, directed transitions, rewind, cancel, and error handling. As confirmed by `wip/research/explore_koto-user-skill_r1_lead-plugin-structure.md`: "AGENTS.md at the plugin root is already a nearly complete koto-user reference document."

2. **`plugins/koto-skills/.cursor/rules/koto.mdc`** (207 lines): A Cursor-specific rules file covering the same runtime loop with a full pseudocode execution example and dispatch table. Addresses Cursor and Windsurf users who don't use the AGENTS.md path.

3. **`docs/guides/cli-usage.md`**: A human-readable CLI reference with the full `koto next` response shapes and flag documentation.

4. **The `README.md` agent integration section**: A step-by-step cycle (init → next → execute → submit → repeat) with JSON output examples.

Agents today would read one or more of these files. The gap is not absence of documentation; it's absence of a structured, installable skill that wraps these resources into a guided execution loop with `${CLAUDE_SKILL_DIR}` integration and skill-standard framing.

**Confidence**: High — all four substitutes are present in the repository and verified by direct file reading.

---

## Q3: Who specifically asked?

**Finding**: No external party has filed an issue or PR requesting a koto-user skill. The only artifact naming this gap is `wip/explore_koto-user-skill_scope.md` (commit `e010080`, authored by Daniel Gazineu, dated 2026-04-03), which frames the koto-user skill as a design question following the gate-transition roadmap completion. The scope document's Research Lead 5 is labeled "lead-adversarial-demand" — the present investigation — indicating this demand validation is a first-pass check before committing to the work.

Issue #74 ("docs(koto): update AGENTS.md and hello-koto for new engine capabilities") demonstrates that the maintainer tracks agent-facing documentation proactively, but that issue was maintainer-filed, not user-reported.

**Confidence**: Absent — no external requestor can be cited. The demand is maintainer-recognized, not user-reported.

---

## Q4: What behavior change counts as success?

**Finding**: No acceptance criteria for a koto-user skill appear in any issue, PR, or design document. The scope capture in `wip/explore_koto-user-skill_scope.md` describes the koto-user persona's knowledge needs (init, next, evidence submission, gate behavior, overrides, rewind) and names the skill, but does not specify measurable success criteria. The existing prompt regression eval infrastructure (issue #37, now merged) and `eval.sh` in the plugin root provide a testing harness, but no eval cases for koto-user have been defined.

From the existing AGENTS.md content and the koto-author skill structure, a plausible behavioral success criterion would be: an agent running a koto-backed workflow correctly dispatches on the `action` field rather than reading `directive` as a freeform instruction, handles `blocking_conditions` before submitting evidence, and resumes correctly after an interruption. These are implied by the documentation but not formally specified anywhere.

**Confidence**: Low — no explicit acceptance criteria exist in any durable artifact.

---

## Q5: Is it already built?

**Finding**: No `koto-user` skill directory exists anywhere in the repository. Checked:

- `plugins/koto-skills/skills/` — contains only `koto-author/`
- `plugins/koto-skills/` root — contains `AGENTS.md`, `eval.sh`, `hooks.json`, `skills/`
- `docs/guides/` — contains `cli-usage.md`, `cloud-sync-setup.md`, `custom-skill-authoring.md`, `library-usage.md`, `template-freshness-ci.md`
- `README.md` — has an agent integration section, not a skill
- `docs/` full tree — no koto-user directory or SKILL.md

The functional equivalent (`AGENTS.md`) exists, but it is not packaged as an installable skill with `${CLAUDE_SKILL_DIR}` path resolution, skill-standard YAML frontmatter, or a structured execution loop entry point.

**Confidence**: High — the directory does not exist and the search was exhaustive.

---

## Q6: Is it already planned?

**Finding**: No open GitHub issue, roadmap document, or milestone references a koto-user skill. Searched all 74 issues (open and closed) — none match "koto-user", "user skill", or "running workflow guide." The current roadmaps (`ROADMAP-gate-transition-contract.md`, `ROADMAP-session-persistence.md`) are both marked complete and contain no forward reference to a user skill.

The only planning artifact is `wip/explore_koto-user-skill_scope.md` (commit `e010080`), which is an in-progress exploration scope document, not a committed roadmap entry or filed issue.

**Confidence**: High — systematic search of all issue records and roadmap files found no planned work item.

---

## Sources Examined

| Artifact | Type | Relevant Finding |
|----------|------|-----------------|
| `plugins/koto-skills/AGENTS.md` | File (550 lines) | Functional koto-user substitute; nearly complete runtime reference |
| `plugins/koto-skills/.cursor/rules/koto.mdc` | File (207 lines) | Second runtime substitute for Cursor/Windsurf users |
| `plugins/koto-skills/skills/` | Directory listing | No koto-user subdirectory exists |
| `wip/explore_koto-user-skill_scope.md` | Wip artifact | Maintainer-authored scope, names drift risk; cites PRs #120–#125 |
| `wip/research/explore_koto-user-skill_r1_lead-plugin-structure.md` | Research artifact | Confirms AGENTS.md overlap and mechanical feasibility |
| GitHub issues (all 74, open + closed) | Issue tracker | No external request for koto-user skill |
| `git log` (full history) | Commit history | No commit message citing agent confusion or missing user docs |
| PRs #103, #109, #120–#125 | Merged PRs | koto-author added; gate-transition features shipped without skill updates |
| `docs/guides/cli-usage.md` | Docs file | Third substitute; comprehensive CLI reference |
| `README.md` agent integration section | Docs | Fourth substitute; step-by-step loop description |
| Functional test features | Test fixtures | Show agent interaction patterns from engine perspective, no user-side confusion captured |

---

## Calibration

**Demand not validated.**

The majority of questions return absent or low confidence:

- Q1 (Is demand real?): **Low** — drift risk named by maintainer; no reported failures
- Q2 (What do people do today?): **High** — four documented substitutes exist
- Q3 (Who asked?): **Absent** — no external requestor
- Q4 (What counts as success?): **Low** — no acceptance criteria in any artifact
- Q5 (Already built?): **High** — confirmed not built (positive non-existence evidence)
- Q6 (Already planned?): **High** — confirmed not in any roadmap or issue (positive absence evidence)

This is **demand not validated**, not **demand validated as absent**. The distinction matters:

- There is no positive evidence that the koto-user skill was considered and rejected.
- There is no evidence that the existing substitutes (AGENTS.md, koto.mdc, cli-usage.md) have been explicitly evaluated as sufficient replacements for an installable skill.
- The maintainer has recognized the gap as a design question (scope document exists) but has not filed issues, defined success criteria, or committed to building it.

The gap between "recognized by maintainer" and "validated by evidence of need" is the open question this research cannot close from available artifacts alone. Demand may be real — the drift pattern across PRs #120–#125 is a concrete structural problem — but the leap from "drift risk" to "agents are failing today without this" is not supported by any artifact in this repository.
