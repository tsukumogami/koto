# Architect Review: DESIGN-koto-engine.md -- Problem Statement and Options Analysis

## Reviewer: architect-reviewer
## Date: 2026-02-21
## Scope: Context/Problem Statement, Decision Drivers, Considered Options (Decisions 1-6)

---

## 1. Problem Statement Specificity

### Assessment: Strong, with one structural weakness

The problem statement at lines 9-18 identifies four concrete failure modes (step skipping, resume failure, evidence loss, state divergence) and grounds each in observable behavior. This is specific enough to evaluate solutions against -- you can test whether a given design prevents each failure mode.

The "Implementation Context" section (lines 59-86) is a genuine strength. It explicitly lists predecessor anti-patterns to fix (non-atomic writes, no transition history, no template hash, dual evidence validation, hardcoded variable allowlist, tangled business logic). Each of these maps to a concrete design decision later in the document. This creates a traceable chain: predecessor weakness -> design decision -> specific API choice.

**Structural weakness: the problem statement conflates two problems.** Lines 9-18 describe the agent execution control problem (step skipping, resume failure). But the design also solves a second problem that's only implicit: the predecessor tool's internal quality issues (non-atomic writes, no history, hardcoded transitions). These are different concerns. An agent could skip steps even with a perfect state file implementation, and the state file could be poorly implemented even if the agent follows the workflow perfectly. The design handles both, but the problem statement frames everything as "agent execution control," which means the anti-pattern fixes (atomic writes, transition history, template hash) read as implementation details rather than solutions to a stated problem.

**Impact**: Low. The design decisions are still well-motivated even without a second explicit problem statement. But a reader might wonder why atomic writes get so much attention in a document about agent behavior.

### Recommendation

Consider adding a brief paragraph to the problem statement that names the second problem: "the predecessor implementation has reliability issues independent of agent behavior." This would make the connection to Decisions 2 (concurrency) and 6 (integrity) more natural.

---

## 2. Missing Alternatives

### Decision 1: State Machine Implementation

**Missing alternative: Use the predecessor's pattern directly (copy + clean up).**

The options are "custom implementation" vs two third-party libraries. A third option is explicitly not building a new `Machine` abstraction at all, and instead keeping the predecessor's `map[string][]string` transition tables and `evidenceGate` struct pattern -- just moving them from hardcoded Go maps to template-parsed data. The predecessor's `CheckTransition()` function at `transitions.go:117-128` is 12 lines. The `CheckEvidence()` function at `transitions.go:147-176` is 30 lines. The proposed ~250-line custom implementation is a superset of this existing code with the `Machine` and `Gate` abstractions added.

This isn't necessarily a better option -- the `Gate` interface and `Machine` struct are genuine improvements that enable library consumers to define custom gates. But the alternative of "clean up what exists without adding an abstraction layer" should be explicitly rejected with rationale, because it's the path-of-least-effort option that someone will ask about.

**Missing alternative for Decision 2: SQLite-based state.**

The document considers file-based options only (atomic JSON, file locking, no versioning). SQLite with WAL mode provides atomic writes, concurrent read/write support, and single-file portability. It's cross-platform, requires no daemon, and Go has mature drivers (modernc.org/sqlite is pure Go, no CGo). The trade-offs are real (binary state file instead of human-readable JSON, additional dependency, overkill for Phase 1), but the option should be named and rejected rather than absent, especially since "multiple concurrent workflows" is a stated decision driver (line 48).

**Missing alternative for Decision 5: Rewind with evidence snapshots.**

The options are "clear rolled-back evidence" vs "preserve all" vs "clear all." A fourth option: snapshot the full evidence map at each transition (store it in the history entry), and on rewind, restore the snapshot from the target transition. This gives exact state reconstruction without the complexity of tracking which keys were added when. The trade-off is storage (full evidence copy per history entry vs just key names). This may be the right option if evidence maps grow large enough that key-level tracking introduces subtle bugs (e.g., a key added in state 2 and overwritten in state 4 -- does rewind to state 3 restore the state-2 value or delete the key?).

### Decision 3: Package Layout

No missing alternatives. The three options (four packages, single package, shared types package) cover the reasonable design space.

### Decision 4: Evidence Accumulation

No missing alternatives. Replace/fail/append covers the semantic options.

### Decision 6: State File Integrity

No missing alternatives. The three options (template hash + version, full hash chain, no checking) are the right spectrum.

---

## 3. Rejection Rationale Fairness

### Decision 1: State machine libraries

**qmuntal/stateless rejection (line 104)**: Fair. The specific criticism -- "external storage API adds indirection that doesn't match koto's file-based persistence pattern" -- is grounded in a real architectural mismatch. koto loads the full state into memory, mutates it, and writes it back. A callback-based storage API assumes field-level read/write, which would require adapting koto's persistence model to the library's expectations rather than the other way around. The note that "the library can be adopted then if needed" for hierarchical states is honest about the trade-off.

**looplab/fsm rejection (lines 106-107)**: Fair but thin. "Less than what we'd need" and "guard pattern doesn't map cleanly to evidence gates" are the right reasons, but the guard-pattern mismatch could use one sentence of explanation. looplab/fsm guards work by returning an error from a callback registered on a specific transition. koto's gates are per-target-state, not per-transition -- meaning the same gate applies regardless of which source state you're coming from. This is a real semantic difference, not just an API preference.

### Decision 2: Concurrency

**gofrs/flock rejection**: Fair. The "orphaned lock risks" and "single-agent workflows" arguments are specific. The escalation path ("file locking remains the path forward for a later release") is honest about the limitation.

**No-versioning rejection**: Fair. "Silent corruption is worse than noisy failure" is a clean, testable principle.

### Decision 4: Evidence accumulation

**Fail-on-existing rejection**: Fair. "Makes retries painful" is specific and directly addresses the primary use case.

**Append-with-history rejection**: Fair. "Which value does field_equals check against?" identifies the real semantic problem with arrays.

### Decision 5: Rewind semantics

**Preserve-all rejection**: Strong. The stale-commits example is concrete and testable: if the agent rewinds past implementation and the commits evidence remains, the gate would pass on re-entry to pr_created even though the commits may no longer exist. This is a real bug, not a theoretical concern.

**Clear-all rejection**: Fair. "Throws away valid work" with the specific example of rewinding from state 5 to state 3 keeping evidence from states 1-2 is clear.

### Decision 6: State file integrity

**Full hash chain rejection**: Fair for Phase 1 scope. "Implementation complexity for a threat that isn't a practical concern" is honest about the threat model.

**No-integrity rejection**: Fair. "Even accidental modifications would silently corrupt" is the right framing.

### Overall assessment

No option reads as a strawman. Each rejected alternative has specific, testable reasons for rejection, and most include a path for future adoption if circumstances change. The weakest rejection is looplab/fsm, which could use one more sentence on the guard-mapping mismatch.

---

## 4. Unstated Assumptions

### Assumption 1: Evidence values are strings

The entire evidence model uses `map[string]string` (lines 267, 335, 345). This assumption is never stated or justified, but it has deep consequences:

- Evidence values that are naturally structured (a list of commit SHAs, a JSON blob of reviewer results) must be serialized to a single string
- The predecessor uses `json.RawMessage` for structured evidence (`ReviewerResults`, `Summary`) and `[]string` for lists (`Commits`). koto flattens all of this to strings.
- The `field_equals` gate does exact string comparison, which means evidence like `commits="sha1,sha2"` requires the agent to format the string exactly as the gate expects

This is probably the right simplification for Phase 1, but it should be explicit. The document should state: "Evidence values are strings. Structured evidence (lists, JSON) is serialized by the caller. This trades type safety for simplicity and keeps the evidence map, gate evaluation, and JSON serialization uniform."

### Assumption 2: Gates are entry requirements on target states, not exit requirements on source states

Lines 295-296 in the Machine definition show gates as part of `MachineState`. Line 571 confirms: "Gates are entry requirements -- they must pass to enter the state." This means if states A and B both transition to C, the same gates apply regardless of the source. The predecessor has a different model: gates are keyed by `from:to` pairs (`transitionEvidence` at `transitions.go:80-87`), meaning different evidence is required for the same target depending on where you're coming from.

The design's choice is simpler and probably correct for koto's use cases (template-defined workflows where the gate semantics should be the same regardless of entry path). But it's a deliberate departure from the predecessor that isn't called out in the "Anti-patterns to fix" section.

**Impact**: Medium. A template author might expect to require different evidence for `research -> validation_jury` vs `extended_research -> validation_jury`. The current design doesn't support this without making the validation_jury gate check multiple evidence keys. This should be stated as a known limitation or a deliberate simplification.

### Assumption 3: The template file must not change during a workflow

Template hash verification is a blocking check with no override (lines 200-201). This assumes that template modifications during a workflow are always errors. In practice, a template author might fix a typo in a directive, add a clarifying sentence, or correct a variable name -- changes that don't affect the state machine semantics but do change the hash. The only recourse is `koto cancel` and restart.

The design acknowledges this ("If the template needs to change, the user must `koto cancel` and start a new workflow") and frames it as a feature. That's a defensible position, but the assumption should be explicit: "We assume that any template change during a workflow could affect the state machine's behavior, and we have no mechanism to distinguish safe changes (directive text) from unsafe changes (state machine structure)."

### Assumption 4: A single state file maps to a single workflow instance

The design assumes one state file per workflow execution. The `discover` package (lines 483-497) finds active state files by filename pattern. This means running two instances of the same workflow template concurrently requires the user to manage state file naming. The design handles this by requiring `--state <path>` when multiple state files exist, but the naming convention (`koto-<workflow-name>.state.json`) means two `quick-task` instances in the same directory would collide unless the user specifies different names.

This is probably fine for Phase 1, but the assumption should be stated: "Each state file represents one workflow instance. Running multiple instances of the same template requires unique state file paths."

### Assumption 5: The Engine and Controller have no circular dependency in practice

The Engine receives a `Machine` at construction time and doesn't know about templates. The Controller ties Engine to Template. But the Controller's `New()` function (line 436) takes an `*engine.Engine` and a `*template.Template`, meaning the Controller is responsible for hash verification. Meanwhile, the transition validation sequence (line 566) says "Template hash check" is step 1 of `koto transition`.

This means the CLI (or the Controller) must perform the hash check before calling `Engine.Transition()`, since the Engine doesn't have access to the template. The Engine API doesn't include hash verification -- it's the Controller's responsibility. But the transition validation sequence at line 566 lists it as part of the transition, which could confuse an implementer. The document should clarify: "Template hash verification is the Controller's responsibility, not the Engine's. The Engine validates the state machine; the Controller validates the template."

---

## 5. Strawman Analysis

**No option is designed to fail.** Each alternative in each decision has at least one genuine strength:

- qmuntal/stateless has external storage callbacks and hierarchical states -- real features that could be useful
- looplab/fsm has 3.3K stars and community adoption
- gofrs/flock provides real concurrent access safety
- No-versioning is simpler to implement
- Fail-on-existing evidence is safer against accidental overwrites
- Append-with-history provides full audit trail
- Preserve-all evidence on rewind is simpler
- Full hash chain provides complete integrity
- No integrity checking is simpler to implement

The rejections are all specific to koto's constraints rather than being generic dismissals. This is well-constructed options analysis.

---

## 6. Architectural Consistency with Predecessor

The design intentionally departs from the predecessor in several ways, all of which are improvements:

| Predecessor Pattern | koto Design | Assessment |
|---|---|---|
| `os.WriteFile` directly | Atomic write-to-temp-then-rename | Fixes a real bug |
| No transition history | Full history with evidence tracking | Enables debugging |
| Hardcoded `AllowedVariables` map | Template-defined variables | Enables extensibility |
| `go:embed` template | Filesystem templates | Enables user authoring |
| Dual evidence validation (stored-field check + flag-based check) | Single evidence model (evidence map + Gate interface) | Eliminates confusion |
| Per-transition gates (`from:to` keyed) | Per-target-state gates | Simpler but less flexible (see Assumption 2) |
| Business logic in state package (auto-increment `ci_fix_attempts`, find-current-issue heuristic) | Clean separation: Engine validates, Controller generates directives | Proper separation |

The most significant departure is the per-target-state gate model vs the predecessor's per-transition model. This simplifies the template format (gates are declared on the state, not on each transition) but loses the ability to require different evidence depending on the source state. The design should acknowledge this trade-off explicitly.

---

## 7. Cross-Document Consistency

The engine design aligns with the strategic document (DESIGN-workflow-tool-oss.md) on:
- Package layout under `pkg/` (public API)
- Template format (YAML header + `## STATE:` sections)
- State file schema (workflow metadata, evidence, history)
- CLI surface (init, next, transition, query, status, rewind, cancel, validate)

One discrepancy: the strategic document's state file example (DESIGN-workflow-tool-oss.md line 546) uses `"metadata"` as the key for workflow variables, while the engine design uses `"variables"` (line 269) and `"workflow"` for metadata (line 270). The engine design's naming is clearer; the strategic document should be updated to match.

Another: the strategic document lists `"template_version"` as a top-level state file field (line 549), which doesn't appear in the engine design's State struct. If template versions matter for compatibility, they should be in the state file. If they don't, the strategic document should remove the field.

---

## 8. Specific Technical Concerns

### Evidence merge rollback (lines 571-575)

The document states that if a gate fails, the evidence merge is rolled back. Since the evidence map is `map[string]string` in memory, this requires either:
1. Making a copy of the evidence map before merging
2. Tracking which keys were added/changed and reverting them

The design doesn't specify which approach. Both work, but option 1 is simpler and less error-prone. This is an implementation detail, but it affects whether the "atomic evidence + gate" guarantee is easy to get right.

### Rewind with overwritten evidence keys

If evidence key "foo" is set in state 2, then overwritten in state 4, and you rewind to state 3:
- The history says "foo" was added in the transition to state 4 (via `evidence_added`)
- Rewind clears "foo" from the evidence map
- But "foo" also had a value from state 2 that should still be valid

The design says "Evidence collected before or during entry to S is preserved" (line 175), but the implementation (clear keys from `evidence_added` in post-target transitions) would delete the key entirely, including its state-2 value. The `evidence_added` array records key names, not whether they were new additions or overwrites.

This is a real edge case that needs resolution. Options:
1. Accept the data loss (document that overwritten keys are fully cleared on rewind)
2. Track "evidence_replaced" separately from "evidence_added" in history entries (store the old value)
3. Use evidence snapshots per transition (see Missing Alternative #3 above)

### Command gate timeout (line 735)

"No timeout is enforced by the engine -- the calling process's timeout applies." This means a command gate with `command: "sleep 3600"` would block `koto transition` for an hour. Since koto runs inside an agent's tool execution, the agent's timeout (if any) is the only safeguard. Some agents have per-command timeouts; some don't. A default engine-level timeout (e.g., 30 seconds, configurable per gate) would be a reasonable safety measure.

---

## Summary of Findings

### Strong points
- Problem statement identifies four specific, testable failure modes
- No strawman alternatives -- all rejected options have genuine merits
- Rejection rationale is grounded in koto's specific constraints, not abstract principles
- Predecessor anti-patterns are explicitly listed and each maps to a design decision
- The `Gate` interface enables library extensibility without complicating the built-in types
- Evidence-replace semantics and selective-clear rewind are well-motivated by the retry use case

### Items requiring attention

| Item | Severity | Description |
|---|---|---|
| Per-target-state vs per-transition gates | Advisory | Departure from predecessor's per-transition model should be explicitly acknowledged as a trade-off, with the known limitation stated |
| Evidence values are strings | Advisory | The `map[string]string` assumption has consequences for structured evidence; should be stated and justified |
| Rewind with overwritten evidence keys | Blocking (design gap) | Clearing a key that was overwritten will lose the earlier value; the document's stated semantics and the implementation algorithm disagree on the outcome |
| Template immutability assumption | Advisory | No mechanism to distinguish safe template changes (typo fixes) from unsafe ones (state machine changes); should be stated as a deliberate choice |
| Command gate timeout | Advisory | No engine-level timeout for command gates; agent timeout is the only safeguard |
| looplab/fsm rejection | Advisory | Could use one more sentence explaining the guard-mapping mismatch |
| Cross-document state file schema drift | Advisory | Strategic doc uses `metadata`/`template_version`; engine doc uses `variables`/`workflow` -- should be reconciled |
| Missing alternative: "clean up predecessor without Machine abstraction" | Advisory | The path-of-least-effort option should be explicitly rejected |
| Missing alternative: SQLite state | Advisory | Worth naming and rejecting for completeness |
