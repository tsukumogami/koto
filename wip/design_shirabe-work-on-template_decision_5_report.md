<!-- decision:start id="error-recovery-self-loops-blocked-states" status="assumed" -->
### Decision: Error Recovery Model for Self-Loops and Blocked States

**Context**

The shirabe /work-on koto template has several self-looping states (analysis on scope_changed, implementation on partial_tests_failing, pr_creation on creation_failed) and a terminal done_blocked state reachable from multiple blocking conditions. The current design has two structural problems: (1) self-loops cycle indefinitely — the agent has no loop count and koto has no built-in iteration tracking, so an agent stuck on partial_tests_failing will keep submitting the same evidence and receiving the same directive forever; (2) done_blocked is terminal — once reached, the workflow cannot be resumed without re-initialization, even for recoverable failures like a flaky CI test that a human can fix in minutes.

koto's engine does not track loop counts internally. Evidence is epoch-scoped (cleared on every state transition). The JSONL event log is append-only and readable, so agents can count prior Transitioned events for a given state by reading the state file directly — but this requires the directive to instruct the agent to do so. Terminal states in koto cannot be resumed; `koto rewind` (a real mechanism confirmed in types.rs) can return a workflow to a prior state from outside the running workflow, but requires the human to know to use it.

Four options were evaluated: (a) loop_count evidence field + escalation transition, (b) retry vs. escalate enum values per self-looping state, (c) replace done_blocked with a resumable non-terminal blocked state, (d) keep current model with explicit loop exit documentation in directives. Full bakeoff with per-option validators and cross-examination found strong convergence.

**Assumptions**

- koto rewind is functional as a recovery path for done_blocked. The Rewound event payload exists in types.rs and is recognized by derive_state_from_log, but the CLI implementation was not verified. If koto rewind is not yet callable from the CLI, the done_blocked mitigation strategy in this decision requires a CLI implementation ticket.
- koto's when-conditions use exact JSON equality, not range comparison. This means a loop_count >= N condition cannot be expressed directly in a transition — escalation must use a distinct enum value (the escalate variant), not a numeric threshold on loop_count.
- The self-loop states (analysis, implementation, pr_creation) are the primary loop risk. ci_monitor's self-loop is indirect (via evidence fallback) and less common.

**Chosen: Option B — Retry vs. Escalate Evidence Values, with koto rewind documentation for done_blocked**

Each self-looping state's evidence enum gains _retry and _escalate variants for the retry-eligible evidence value. The existing self-loop transition condition changes from the current value (e.g., `implementation_status: partial_tests_failing`) to `partial_tests_failing_retry`. A new escalation transition is added: `when: {implementation_status: partial_tests_failing_escalate}` routes to done_blocked. Directive text in each self-looping state gains one paragraph specifying the escalation threshold (default: 3 failed retry submissions) and instructing the agent to switch from _retry to _escalate after that threshold.

For done_blocked: the state remains terminal. Its directive gains a paragraph explaining how to resume: "If the blocker has been resolved externally, run `koto rewind <originating-state>` (e.g., `koto rewind implementation`) to return the workflow to that state without reinitializing." The specific originating states for each reachable-to-done_blocked path should be noted in the directive.

Applied to each self-looping state:
- analysis: `plan_outcome: scope_changed` → split into `scope_changed_retry` (self-loop) and `scope_changed_escalate` → done_blocked
- implementation: `implementation_status: partial_tests_failing` → split into `partial_tests_failing_retry` (self-loop) and `partial_tests_failing_escalate` → done_blocked
- pr_creation: `pr_status: creation_failed` → split into `creation_failed_retry` (self-loop) and `creation_failed_escalate` → done_blocked

**Rationale**

Option B addresses the indefinite self-loop problem with the minimal template change that creates a structural escalation path: adding _retry/_escalate enum variants and a corresponding transition. The routing conditions are clean and the escalation point is an explicit agent decision — not an automatic trigger based on a count, but a conscious choice the agent makes at iteration N. This fits the design's evidence-as-decision-record pattern: the escalate value records "I judged this situation as requiring human intervention."

The cross-examination found strong convergence against alternatives. Option A (loop_count field) reduces to option B plus an optional audit field — the routing logic is identical because koto's when-conditions use exact equality, requiring an escalate enum value in either case. If loop audit quality is important for a specific deployment, loop_count can be added as an optional supplementary field without changing option B's routing model. Option C (resumable blocked state) is architecturally correct but over-engineered: routing `continue` back to the appropriate origin state requires either multiple blocked states or origin encoding, adding significant complexity for a problem that koto rewind solves without template changes. Option D (docs only) is viable but unnecessary given option B's low implementation cost — structural escalation paths are preferable to pure directive text because they are auditable and template-visible.

The koto rewind approach for done_blocked is a deliberate documentation-level solution rather than a template-level one. The resumed workflow state after a human uses koto rewind is clean: the Rewound event is a state-changing event recognized by the engine, evidence is fresh, and the next `koto next` call re-evaluates gates from the rewound state. This recovers naturally without requiring the template to model a resumable blocked state.

**Alternatives Considered**

- **Option A (loop_count evidence field)**: Routing logic is identical to option B (still requires an escalate enum value) because koto uses exact equality for when-conditions. loop_count becomes a supplementary audit field rather than the routing mechanism. Rejected as the primary approach since it adds directive complexity (agent must read and count state file events) without changing the routing model. Can be added as an optional enhancement to option B if audit trail quality is prioritized.

- **Option C (resumable blocked state)**: Architecturally correct — blocked workflows are paused, not finished, and a non-terminal state models this accurately. Rejected because routing `continue` back to the appropriate recovery state requires either three separate blocked states or origin encoding in evidence, adding significant template complexity. koto rewind achieves the same recovery goal at zero template cost. Reconsider in a future template revision if koto's template model gains dynamic routing or if koto rewind proves insufficiently discoverable in practice.

- **Option D (documentation only)**: The enforcement limitation is real (agent adherence required in all options), but option B's structural escalation path is visible in the template and carries audit value that option D's directive text does not. Rejected as the primary approach given option B's low cost. Option D's koto rewind documentation strategy is adopted as a complement.

**Consequences**

What changes: three self-looping states gain two enum variants each (retry/escalate) and a new escalation transition to done_blocked. done_blocked directive gains koto rewind instructions. The template grows by three transitions and six enum values — a small, contained change.

What becomes easier: loop progress is auditable in the event log (escalate transitions are visible); agents have a clear path out of indefinite loops; done_blocked workflows are recoverable without reinitializing the entire workflow.

What becomes harder: directive text for self-looping states must specify the escalation threshold consistently across states (risk of drift if states are updated independently). The retry/escalate naming convention must be established and documented for template authors.
<!-- decision:end -->
