<!-- decision:start id="shirabe-work-on-template-mode-routing" status="assumed" -->
### Decision: Workflow Mode Determination and Routing

**Context**

The shirabe /work-on template needs to support two modes: issue-backed (GitHub issue number provided) and free-form (task description only). These modes share all states from analysis onward but diverge through their context-gathering phases. koto's evidence model is epoch-scoped — evidence submitted at any state is cleared on every transition — so routing fields cannot carry forward automatically between states. The decision affects whether an entry state is needed, how the convergence point between modes is handled, and how the overall template topology reads to future maintainers.

Four options were evaluated: (a) single entry state captures mode, single setup state requires mode re-submission to route at the convergence point; (b) entry state routes to mode-specific paths, two separate setup states eliminate re-submission; (c) init-time --var flag encodes mode at workflow initialization, eliminating the entry state; (d) two separate template files with duplicated shared states. Source code investigation confirmed that epoch-scoping is implemented exactly as documented (advance.rs clears evidence on every transition), the --var CLI flag is not implemented (variables: HashMap::new() hardcoded), and gate command variable substitution is not implemented (gate.rs passes commands directly to sh -c without substitution).

**Assumptions**

- The --var CLI flag and {{VAR_NAME}} gate substitution will be implemented in a future koto release. When both ship, the template should be migrated from option (b) to option (c).
- The two setup states in option (b) will have distinct (not duplicated word-for-word) directive content covering mode-specific preparation work. If their content becomes identical, the template should be restructured.
- Running in --auto mode without user confirmation. This decision is marked "assumed" accordingly.

**Chosen: Split Topology — Two Separate Setup States (Option b)**

The template uses an entry state that accepts mode evidence (enum: issue_backed, free_form) and routes to the appropriate diverged path (context_injection for issue-backed, task_validation for free-form). The two paths each terminate in a mode-specific setup state: setup_issue_backed transitions unconditionally to staleness_check, setup_free_form transitions unconditionally to analysis. No mode re-submission is required at the convergence point. Both paths merge at analysis and share all subsequent states.

**Rationale**

Option (b) is the best immediately shippable choice because it eliminates the primary maintainability problem (setup re-submission requiring epoch-scoping knowledge) without blocking on unimplemented engine features.

The critical distinction is between the entry state's one-time round-trip (meaningful initialization: establishes mode, creates an evidence record, routes diverged paths) and setup re-submission in option (a) (no new information: agent re-states a known value to satisfy an evidence gate). Option (b) keeps the meaningful initialization and eliminates the meaningless repetition.

Option (a) is rejected because the epoch-scoping confusion it creates is an unbounded, recurring maintenance cost: any contributor who reads the template and sees mode submitted at both entry and setup will be confused, and the explanation requires understanding a non-obvious engine constraint. The single setup state's elegance does not outweigh this cost.

Option (c) is the correct long-term architecture — mode determination belongs at initialization, not execution — but is blocked by two unimplemented features: --var CLI support and {{VAR_NAME}} substitution in gate commands. Adopting (c) today would require engine work before the template functions, creating an ordering dependency that blocks the shirabe skill release.

Option (d) is rejected because it duplicates approximately 12 shared states (analysis through pr_creation and terminal states), violates the explicit duplication constraint, and recreates the exact drift problem (work-on vs. just-do-it divergence) that motivated this design effort.

All four validators reached consensus on option (b) after cross-examination. Option (d) was unanimously rejected. Options (a) and (c) were conceded by their respective validators in favor of option (b).

**Alternatives Considered**

- **Entry State + Single Setup Re-Submission (a)**: Mode submitted at entry routes to diverged paths; mode re-submitted at single setup routes to staleness_check vs. analysis. Rejected because re-submission requires contributors to understand epoch-scoping to understand why the same field appears twice. The cognitive load is unbounded — every future convergence point requires the same explanation. Validator conceded in Phase 4.

- **Init-Time --var (c)**: Mode encoded at koto init via --var flag, entry state eliminated, initial_state or variable-dependent gate determines first state. Rejected for current implementation because neither --var CLI support nor {{VAR_NAME}} gate substitution is implemented (confirmed in source). This is the target architecture for a future template version. Validator conceded the blocker in Phase 5 while advocating for documenting (c) as the successor.

- **Two Separate Templates (d)**: Two template files (work-on-issue.md, work-on-freeform.md) each containing the full state list for their mode. Rejected unanimously because 12 shared states would be duplicated, drift risk is confirmed by design history, and this recreates the exact separation-that-caused-divergence problem the design is solving.

**Consequences**

The template will have ~16 states: one entry state, two diverged context-gathering paths (context_injection for issue-backed, task_validation for free-form), two setup states (setup_issue_backed routing to staleness_check, setup_free_form routing to analysis), and all shared states from analysis onward. The entry state's accepts block declares mode as an enum field with mutual exclusivity enforced by the koto validator. Both setup states have unconditional transitions.

What becomes easier: template maintainers can read the workflow topology without koto engine expertise; routing bugs in setup are impossible (unconditional transitions); the template compiles and runs against current koto with no engine changes.

What becomes harder: changes to setup-phase behavior must be applied to two states instead of one; the two setup directives must be kept semantically distinct to justify the split.

Future path: when koto --var and gate substitution ship, the entry state and dual setup states can be replaced with a single initial_state selection at init time (or a variable-dependent router), reducing the template from ~16 to ~13 states.
<!-- decision:end -->
