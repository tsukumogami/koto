# Lead: Which decisions in shirabe's workflows are classifier-shaped today?

## Findings

### Method and scope

I read the three koto templates shirabe ships (`skills/work-on/koto-templates/work-on.md`,
`skills/scope/koto-templates/scope.md`, `skills/execute/koto-templates/execute.md`), the
phase references that carry the judgment each template state delegates to, the prose-only
skills that make routing or scoring calls (explore, plan, design, prd, review-plan, charter,
release), the shared protocol files under `references/` (`decision-protocol.md`,
`split-triggers.md`, `worktree-discipline.md`), and the existing decision manifest at
`docs/specs/decision-points.md`. The Rust crates (`crates/shirabe`, `crates/shirabe-validate`)
are deterministic validators and PR-body tooling; they encode no workflow judgment, so they
contribute nothing to the inventory beyond being a place deterministic logic can live.

Rating rubric, applied against the Jev constraints in the scope file:

- **Closed set**: can the answer be a fixed enum, with an escape-hatch key (`unclear`) that
  routes back to the agent?
- **Pruned state**: can the inputs be assembled mechanically (issue body, a doc section, a
  file list, a diff stat) into well under 32k tokens, without the agent's working memory?
- **No arithmetic/dates**: counts, thresholds, retry caps and ages either absent or
  precomputable by koto or a script.
- **~90% tolerance**: is a wrong answer cheap (reversible, re-checked later, only resizes
  something) or expensive (deletes a doc, ships unmet criteria)?
- **Injection**: where the state text comes from. External = GitHub issue bodies, CI logs,
  upstream commits written by others. Author = text the human operator typed. Agent = text
  the agent itself wrote earlier. Repo = committed docs/code, which third-party PRs can seed.

Fit classes: **Good now** (drop-in as a declared question), **Reshape** (fit once the named
change lands), **Poor** (needs generative reasoning or a human), **Deterministic** (not a
classifier job at all; the agent currently does something a script or a koto gate should
compute).

Frequency is per workflow run of the owning skill; "per child" means per work-on child of an
`/execute` run, which multiplies by the number of plan issues (typically 3-10).

### Inventory table

| # | Decision | Where | Options today | Jev shape | Fit | Reshaping needed | Freq / run | Injection |
|---|----------|-------|---------------|-----------|-----|------------------|-----------|-----------|
| 1 | Free-form task ready for direct implementation? | work-on template `task_validation` | proceed / exit (+ rationale) | choice {proceed, exit, unclear} | Good now | Make `rationale` optional on auto-decided ticks (koto records the distribution instead) | 1 (free-form runs only) | Author |
| 2 | Research gathered enough context | work-on `research` | sufficient / insufficient | n/a | Deterministic (dead) | Field routes nowhere: the only transition is unconditional. Drop it or give it a route | 1 | Agent |
| 3 | Still appropriate after research? | work-on `post_research_validation` | ready / needs_design / exit (+ revised_scope) | choice {ready, needs_design, exit, unclear} | Reshape | `context_summary` must be structured facts (files touched, existing solution found y/n, prerequisites) rather than free prose; `revised_scope` stays agent-written, so only offload when verdict is not a narrowed-ready | 1 | Agent (summary quotes repo) |
| 4 | Plan outline item clear enough to code against | work-on `plan_validation` | proceed / exit | choice {proceed, exit, unclear}; or noul "ACs exist and are specific enough to code against" | Good now | None beyond state = the outline item from `plan-to-tasks.sh` output | 1 per child (3-10 per /execute) | Repo |
| 5 | Issue stale since filing? | work-on `staleness_check` (gate runs `check-staleness.sh`, agent decides when gate fails) | fresh / stale_requires_introspection / override / blocked | noul "do these upstream changes plausibly invalidate the issue's assumptions?" | Reshape | Precompute commits since issue creation touching files the issue references (the date/age reasoning is exactly what Jev can't do). `override`/`blocked` stay agent/human | 1 per issue-backed run | External (issue body + others' commit messages) |
| 6 | Introspection outcome | work-on `introspection` / `references/phases/phase-2-introspection.md` | approach_unchanged / approach_updated / issue_superseded | choice | Poor (partial) | The state is `introspection.md`, which the agent must write first. Jev could classify an already-written artifact, but the expensive part is generative | 0-1 | Agent |
| 7 | Plan outcome incl. already-complete detection | work-on `analysis` | plan_ready / already_complete / blocked_missing_context / scope_changed_retry / scope_changed_escalate | - | Poor | Planning is generative. The "retry up to 3 times, then escalate" rule is a counter koto should enforce, not a judgment | 1-3 | Agent |
| 8 | Issue type (routes whether panels run) | work-on `analysis` + resubmitted at `implementation` | code / docs / task | choice {code, docs, task, unclear} | Good now | State = issue title/body + precomputed changed-file list. Resubmission at implementation should become a koto context carry (omitting it today silently stalls the workflow) | 1-2 per child | External + Repo |
| 9 | Implementation status | work-on `implementation` | complete / partial_tests_failing_retry / _escalate / scope_expanded_retry / blocked | - | Poor / Deterministic | Tests-failing is an exit code; retry cap is a counter; scope expansion is generative | 1-4 | - |
| 10 | Panel verdict aggregation | work-on `scrutiny` / `review` / `qa_validation`; `phase-4a-scrutiny.md:31-32` | passed / blocking_retry / blocking_escalate | - | Deterministic | Already defined as "any `blocking_count > 0`": a jq gate over the results JSON, not a judgment. Retry cap (2) is a counter | 3 per round | - |
| 11 | Per-finding severity (blocking vs advisory) | reviewer outputs feeding #10 | blocking / advisory | choice {blocking, advisory, unclear} per finding | Reshape | Reviewers themselves stay generative; offload only the severity label per finding, with a per-reviewer rubric as criteria | ~5-20 per child | Repo (diff) |
| 12 | Verification pass/fail and map matching | work-on `verification`; SKILL.md "Definition of Done" | passed / failed / cannot_verify | - | Deterministic | Glob-matching changed files to the verification map and reading exit codes is script work | 1-2 per child | - |
| 13 | Every acceptance criterion met? | work-on `finalization` | ready_for_pr / deferral_requested / issues_found | noul per AC "is this AC satisfied by the diff and test evidence?" | Reshape (shadow first) | Split into per-AC nouls; precompute diff summary + test names; koto ANDs them. Wrong "met" ships unmet work, so auto-execute only on a high threshold, else agent | 1 x #ACs (3-8) per child | Repo |
| 14 | Deferral approval | work-on `deferral_approval` | approved / rejected | - | Poor (human gate by design) | Must never be offloaded: the template says a deferral is legitimate only once a human approves | 0-1 | - |
| 15 | Design diagram update needed? | work-on `pre_pr_evidence` | updated / not_applicable | noul "does this change alter anything the design's diagram depicts?" | Reshape | State = diagram block + diff stat. Cleanup_done is already gate-checked | 1 per child | Repo |
| 16 | CI outcome, red-check acceptance | work-on `ci_monitor`, `phase-6-pr.md` | passing / failing_fixed / failing_unresolvable | - | Poor (safety) | Manifest marks CI failure handling HALT in both modes; `session_role` is already scripted | 1 | External (CI logs) |
| 17 | needs-triage: proceed or reclassify | work-on `SKILL.md:36-44` | proceed directly / reclassify | choice {proceed, reclassify, unclear} | Good now | State = issue body + `## Label Vocabulary`. Not in the template yet, so it needs a state before koto can own it | 0-1 | External |
| 18 | Upstream drift impact | execute `worktree_discipline_check`; `skills/work-on/references/phases/phase-2.5-worktree-discipline.md` | none / informational / intent-changing | choice {informational, intent_changing, unclear} | Reshape | Compute `none` deterministically (no overlap between `git diff --name-only` on main and PLAN-referenced paths); send only the overlapping diff + PLAN references to Jev. intent-changing stops the chain, so threshold high | 1 per /execute | Repo + External (others' commits) |
| 19 | Batch outcome | execute `spawn_and_await` tick 2 | all_success / needs_attention | - | Deterministic | Directive literally says "if no child reached done_blocked". koto knows child terminals | 1 | - |
| 20 | Pause before finalize | execute `pr_finalization` | pause / finalize | - | Deterministic | Agent echoes `{{PAUSE_BEFORE_FINALIZE}}`; a `when` on the variable removes the "decision" | 1 | - |
| 21 | Genuine blocker vs take-the-default | execute `spawn_and_await` autonomy paragraph; `decision-protocol.md` | stop / continue with recorded default | choice {blocker, take_default, unclear} | Reshape | Needs the emergent decision framed as a question with candidate default before it can be classified; this is the decision-protocol reshaping (#40) | 0-5 | Mixed |
| 22 | R6 shape predicates P1/P2/P3 | scope `references/phases/phase-1-discovery.md:185-300` | fires / does-not-fire (x3) | 3 nouls | Good now (P1), Reshape (P2, P3) | P2 needs a precomputed repo component listing to compare against; P3 is mostly a frontmatter grep plus one prose noul. P1 is described as a "count" but only tests existence | 3 pre-PRD + 3 post-PRD per /scope | Repo |
| 23 | Cold-start projection | scope phase-1 | keyword match on slug | - | Deterministic | Already keyword-driven | 1 | - |
| 24 | Chain proposal answer | scope `chain_proposal` | proceed / adjust / bail | choice over the author's reply | Poor (low value) | Human is the latency; parsing their reply saves nothing | 1 | Author |
| 25 | Fold: keep or absorb upstream doc | scope `fold` + `phase-2-chain-orchestration.md` Consolidation Judgment | keep / absorb (+ finding) | noul "does the upstream hold anything beyond its declared contribution that one section would lose?" | Reshape | State = the two docs pruned to body sections (may approach the 32k budget for PRD+DESIGN). Absorb deletes a document, so offload `keep` freely and require high P for `absorb`; the finding stays agent-written | 0-3 per /scope | Repo |
| 26 | Hop outcomes, exits, bail acks | scope `hop_*`, `finalize`, `exit_*`, `bail` | landed/skipped/rejected/bail, retry/abandon, cancel/force | - | Deterministic or human | Read from the child's result or asked of the author | 4-6 | - |
| 27 | Entry assessment of a needs-triage issue | explore `references/phases/phase-0-setup.md` §0.4 | needs investigation / needs breakdown / ready (3-agent jury, majority vote) | choice {investigation, breakdown, ready} | Good now | None. A calibrated distribution replaces the jury's majority + "dissent" field directly; saves three subagent spawns | 0-1 per /explore | External |
| 28 | Crystallize stage 1 and stage 2 | explore `references/quality/crystallize-framework.md`, `phase-4-crystallize.md` | 5 categories, then 4 entry points, scored as signals minus anti-signals with demotion and tiebreakers | ~35-40 nouls (one per signal/anti-signal row) in one request | Reshape | Split each table row into a noul; move the scoring, demotion, "within 1 point" tiebreak and insufficient-signal fallback into koto (arithmetic). Candidacy preconditions (qualifying PLAN exists, visibility) are file checks. Near-ties fall back to the agent | 1 per /explore | Agent (findings) + Repo |
| 29 | Explore further vs ready to decide | explore `SKILL.md:291-299`, `phase-3-converge.md` | explore further / ready | noul "significant gaps, open questions, or contradictions remain" | Reshape | State = findings file's open-questions/gaps section; `--max-rounds` cap is a counter for koto | 1-3 (per round) | Agent |
| 30 | Narrowing questions, decision capture, decision review | explore `phase-3-converge.md` §3.3-3.5 | open | - | Poor | Generative extraction of decisions from prose | per round | - |
| 31 | needs_label per roadmap feature | plan `references/phases/phase-1-analysis.md:133-147` | needs-prd / needs-design / needs-spike / needs-decision | choice (+ unclear) | Good now | State = feature description; priority ordering rule is a deterministic tiebreak | 1 per feature (3-8) | Repo |
| 32 | Decomposition strategy | plan `phase-3-decomposition.md:84-116` | walking skeleton / horizontal | nouls per criterion, or choice | Reshape | The "3+ issues" criterion and tiebreak are counts to precompute; the "matches on more counts" rule is arithmetic koto does over nouls | 1 | Repo |
| 33 | Value-confirmation guard per unit | plan `phase-3-decomposition.md` §3.5a | pass / ambiguous / fail | choice {pass, ambiguous, fail} | Good now | `ambiguous` is already the escape hatch; both non-pass outcomes route to the same recorded review item, so 90% is tolerable | 1 per unit (2-8) | Repo |
| 34 | Execution mode + split branch | plan §3.6, `references/split-triggers.md` | single-pr / multi-pr + Hard Constraint / Incremental Value / Stated Preference | 1-2 nouls | Reshape | Most of the stack is deterministic (roadmap input -> multi-pr; preference flag > CLAUDE.md > default). Residual judgments: "a named hard constraint exists" and the value-guard aggregate from #33 | 1 | Repo |
| 35 | AC discriminability per AC | review-plan `phase-3-ac-discriminability.md`, `references/templates/ac-discriminability-taxonomy.md` | none or one of 7 named patterns | choice {ok, fixture_anchored, mock_swallowed, happy_path_only, state_without_transition, integration_gap, name_drift, existence_only, unclear} | Reshape (shadow first) | Compress each 12KB taxonomy pattern to a one-line criterion; state = one AC + its issue context. False negatives defeat the review's purpose, so use it as a pre-screen that clears confident "ok"s and sends the rest to the agent | 1 per AC (20-60 per plan) | Repo |
| 36 | Scope gate, design fidelity, sequencing findings | review-plan phases 1, 2, 4 | findings lists | - | Poor | Cross-document contradiction finding is generative; verdict synthesis ("any critical -> loop-back", `phase-5-verdict.md:16-22`) is deterministic | 4-12 agents per run | - |
| 37 | Complexity assessment | design `SKILL.md:221-238` | Simple / Complex | nouls for API-surface change, new test infra, cross-package | Reshape (low value) | File count precomputed. Both branches recommend "Plan (Recommended)", so the classification changes nothing downstream today | 1 | Repo |
| 38 | Cross-validation conflicts | design `references/phases/phase-3-cross-validation.md` §3.2 | conflict / no conflict per assumption | noul per (assumption, peer choice) pair | Good now | State per pair is two short strings; many pairs in one parallel request. Restart-once rule stays in koto | A x D pairs (~10-40) | Agent (decision reports) |
| 39 | Security outcome and per-dimension applicability | design `phase-5-security.md` | considerations / N/A with justification / (3 outcomes) | noul per dimension | Poor for verdict, Reshape for triage | Under-reporting is the costly error; at most use nouls to order which dimensions the researcher looks at | 1 | Repo |
| 40 | Tier classification and confirmed/assumed status | shared `references/decision-protocol.md` | Tier 1-4; status confirmed / assumed / escalated | score (4 anchored levels) + noul "evidence clearly favors one option" | Good now (tier), Reshape (status) | Anchors already exist (reversibility, heuristic confidence, phase primacy). Known points are pre-tiered in `docs/specs/decision-points.md` | ~3-10 emergent per run, all skills | Agent |
| 41 | Validation jury per doc | prd/brief/strategy/roadmap/vision `phase-4-validate.md` (e.g. prd:36-191) | 3 reviewers PASS/FAIL each; synthesis all-pass / minor / significant | nouls per checklist question (e.g. "are ACs binary pass/fail?") | Reshape (as pre-screen) | Checklist items become nouls. Spawn the full jury only when some noul fails or is low-confidence. The issue text and fixes stay generative. This is the rubric-check use case the Jev doc cites at ~91.5% agreement | 3 agents x rounds per doc; ~12 per /scope run | Repo |
| 42 | PRD loop-back (investigate more leads) | prd `phase-2-discover.md` §2.5 | proceed / loop | noul, same as #29 | Reshape | Same reshaping as #29 | 1-2 | Agent |
| 43 | Thesis-shift signal | charter `references/phases/phase-1-discovery.md` §1.4 | 3 positive categories or no-signal default | choice {thesis_invalidated, scope_extension, vision_rejection, no_signal} | Good now | `no_signal` is the escape hatch and default | 1 per /charter | Author |
| 44 | Release bump | release `SKILL.md:112-118` | major / minor / patch | - | Deterministic | Commit-prefix mapping is a regex; the residual "does this feat warrant major" is a human call | 1 | - |
| 45 | Approach selection, decider agents, open-question resolution | design D3/D6, prd P5, decision skill | open | - | Poor | Comparative reasoning with text output | several | - |

### Frequency and where offload saves the most

Grouped by what an offload actually removes from the main thread:

1. **Juries and subagent spawns replaced or gated** (biggest latency win, minutes per run):
   the explore entry assessment (#27, three agents -> one request), the doc validation
   juries used as pre-screens (#41, about a dozen agents in a full `/scope` run), and the
   AC discriminability pre-screen (#35, 20-60 per plan).
2. **Reference files the agent no longer loads** (biggest context win): crystallize (#28)
   needs the 19KB `crystallize-framework.md` in context today; AC discriminability needs the
   12KB taxonomy; the decision protocol and split triggers are loaded across skills. If koto
   owns the question, the criteria live in the template and never reach the agent.
3. **High-multiplicity per-child checks in `/execute`**: plan_validation (#4), issue_type (#8),
   per-finding severity (#11), per-AC finalization (#13), design-diagram (#15). Individually
   cheap, but they fire in every child, so a 6-issue plan yields roughly 80-120 atomic
   questions. These run in child sessions, not the coordinator, so the saving is in child
   context and wall-clock rather than the coordinator's thread.
4. **Single low-stakes routing calls**: task_validation, needs-triage, needs_label, value
   guard, thesis-shift, R6 predicates, tier classification. Low per-call saving, but they are
   drop-in and make good first targets because a wrong answer is cheap or re-checked later
   (R6 is explicitly re-evaluated after the PRD lands and only resizes a roster).

## Implications

Only three skills (work-on, scope, execute) run on koto templates, and most of the best
candidates live in skills that don't: explore's entry assessment and crystallize, plan's
labels/value guard/decomposition, review-plan's AC taxonomy, design's cross-validation, and
the validation juries. Offloading those needs the prose phase to become a declared koto state
first, which is lead 4's problem. Inside the existing templates the good-now candidates are
fewer: `task_validation`, `plan_validation`, `issue_type`, and (after precomputation)
`staleness_check` and `worktree_discipline_check`.

A consistent reshaping pattern falls out of the inventory: split compound judgments into
atomic nouls, let koto do the arithmetic (sum signals, apply demotion, count retries, compare
to thresholds), precompute anything involving dates, counts or file overlap in a gate or
`default_action`, and keep an `unclear` key or a confidence floor that hands the state back to
the agent. Crystallize (#28) is the clearest demonstration: its scoring procedure is already
"count signals minus anti-signals", which is exactly the part Jev must not do and koto can.

Almost every enum evidence field is paired with a free-text `rationale`/`detail`/`finding`
field. Jev can't write those. Templates would need to let an auto-decided tick omit the prose
field and have koto record the question, the distribution, the model version and a state hash
as the audit trail, with the agent writing prose only on the fallback path or on outcomes that
need it (exit messages, failure reasons).

A second, cheaper change should land regardless of Jev: several "decisions" the agent submits
today are deterministic (#2, #10, #12, #19, #20, #23, #26, #44 and the retry caps in #7/#9).
They belong in koto gates, `when` clauses on variables, or scripts. Moving them first shrinks
the agent's surface and avoids routing deterministic facts through a probabilistic classifier.

Human gates (#14 deferral approval, #24 chain proposal) and safety halts (#16 CI failures)
must be marked non-offloadable in whatever schema lead 2 designs, not just left undeclared.

Injection exposure concentrates in the issue-driven decisions (#5, #8, #17, #27): the state is
a GitHub issue body anyone can write. Those are also the ones where a wrong answer is mostly
recoverable (the exploration or implementation continues and later states re-check), which
limits the blast radius, but a classifier auto-executing "ready" on an injected issue body is
the main abuse path.

## Surprises

- `research.context_gathered` in `work-on.md` is decorative: its only transition is
  unconditional, so `insufficient` changes nothing.
- The design skill's complexity assessment (`skills/design/SKILL.md:221-238`) recommends
  "Plan" in both branches, so the Simple/Complex classification has no effect on routing.
- `issue_type` must be resubmitted at `implementation`; the directive warns that omitting it
  stalls the workflow with no error. That's a context-carry bug disguised as a decision.
- `staleness_check`'s gate calls a bare `check-staleness.sh` that shirabe doesn't ship (no
  file by that name in the repo), so outside an environment that provides it the gate fails
  and the agent makes the staleness call by hand every time.
- A decision manifest already exists (`docs/specs/decision-points.md`, 39 points across
  explore/design/prd/plan/work-on with interactive and `--auto` behavior per point), but it
  is partly stale: it cites `explore/references/phases/phase-5-produce-no-artifact.md` and
  `design/references/phases/phase-2-present-approaches.md`, neither of which exists, and it
  doesn't cover scope, execute, charter, review-plan or the template states. It's still the
  natural seed for a registry of classifier-eligible decisions.
- The explore entry assessment's own text says it "is not a verdict" and only feeds
  crystallize as evidence, which makes it an unusually safe first target: an error there is
  explicitly overridable by later findings.
- The scope R6 predicate P1 is described as a count ("architectural-alternatives count") but
  its firing rule is existence of at least one, so the Jev no-counting limit doesn't bite.

## Open Questions

- Should per-finding and per-AC checks (#11, #13, #35) run inside koto (one question per
  finding means koto must iterate over structured reviewer output), or should the reviewer
  subagents call Jev themselves? The former keeps the audit trail in koto; the latter needs
  no engine iteration support.
- Does koto's template model support a variable-length question set (one noul per AC or per
  unit), or only a fixed set of questions per state? Several of the best candidates are
  per-item.
- What confidence floor makes the costly-error decisions (#13 ship readiness, #18 drift stop,
  #25 absorb) acceptable, and should those start in shadow mode only?
- For the jury pre-screens (#41), what is the false-negative rate of a noul checklist against
  the current three-agent jury? Session logs of past jury outputs could answer this.
- Is the 32k state budget enough for `fold` (#25) when the pair is a PRD and a DESIGN, or does
  it need section-level pruning?

## Summary

Of about 45 decision points across shirabe, roughly 12 are good fits now (task/plan validation, issue type, needs-triage, explore's entry-assessment jury, needs_label, the value guard, thesis-shift, R6 predicates, design cross-validation, tier classification), about 15 fit after reshaping (split into atomic nouls, let koto do the arithmetic, precompute dates, counts and file overlap: crystallize, loop decisions, staleness, drift, fold, per-AC and per-finding checks, jury pre-screens), and the rest need generative work, a human gate, or are really deterministic facts the agent is submitting by hand. The biggest savings come from replacing subagent juries (explore entry assessment, doc validation juries, the AC-discriminability pre-screen) and from never loading large criteria files like the 19KB crystallize framework, while per-child checks in `/execute` fire most often (roughly 80-120 atomic questions for a 6-issue plan). Most of the best candidates live in prose-only skills rather than the three koto templates, and several template "decisions" (batch_outcome, pause_decision, panel aggregation, verification) should become koto gates first, whether or not Jev is adopted.
