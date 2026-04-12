# Crystallize Decision: batch-child-spawning

## Chosen Type

Design Doc

## Rationale

Issue #129 was labeled `needs-design` on arrival, which already signals the
repo's triage read. Exploration confirmed that read: the *what* is clear
(decoratively spawn a DAG of children from a parent) and the *how* was
genuinely open across multiple architectural dimensions. The round produced
concrete choices on:

- declaration shape (four candidates evaluated, state-level hook picked)
- storage strategy (three candidates evaluated, disk-derivation picked)
- insertion point in the codebase (four candidates evaluated, CLI-level
  scheduler tick picked)
- child naming / idempotency key (three candidates evaluated,
  `<parent>.<task>` picked)
- failure-routing policy (four policies compared, skip-dependents picked
  with per-batch overrides)
- dynamic-additions model (Reading A vs Reading B tension resolved: A is
  primary, B is complementary)

These are exactly the kinds of decisions a design doc exists to record, and
every one of them is load-bearing for the implementation. Without the doc,
the decisions vanish with `wip/` when the branch merges. With the doc, the
next contributor understands why the scheduler lives at the CLI layer and
not inside the advance loop, why nothing is persisted at the parent beyond
evidence events, and why child names couple to parent names. Those are the
"why future contributors need to know" questions the crystallize framework
asks about.

The gaps that remain (atomic child-spawn window, forward-compat
diagnosability, child-template path resolution, retry mechanics,
observability) are all design-time details, not research gaps. They belong
in the design doc's Decision Drivers or Considered Options sections, not
another round of discovery.

## Signal Evidence

### Signals Present (Design Doc)

- **What to build is clear, but how to build it is not.** Issue #129 gives
  a concrete brief with acceptance criteria; exploration did not need to
  refine the problem statement. What exploration refined was the
  architecture. Source: `findings.md#core-question`, the
  `lead-evidence-shape` introduction cites the issue body verbatim.

- **Technical decisions need to be made between approaches.** Every lead
  evaluated 3-4 candidates and picked one. `lead-koto-integration`
  evaluated four insertion points; `lead-evidence-shape` evaluated four
  template declaration shapes; `lead-failure-routing` evaluated four
  failure policies; `lead-dynamic-additions` evaluated two dynamic-addition
  models. This is exactly the "compare approaches" work design docs
  capture.

- **Architecture, integration, or system design questions remain.** The
  design sketch names specific files and line numbers
  (`src/engine/advance.rs:166`, `src/cli/mod.rs:1835`, etc.) and proposes
  a new module (`src/engine/batch.rs`). The integration points, compiler
  validation rules, and backward-compat story are all integration-level
  questions.

- **Exploration surfaced multiple viable implementation paths.** The
  biggest tension — Reading A vs Reading B — was a genuine fork with
  different persistence models, nesting semantics, and CLI surfaces.
  Both are viable for different shapes of work; the design doc locks in
  which one is the answer to #129.

- **Architectural or technical decisions were made during exploration
  that should be on record.** See the decisions list in
  `decisions.md#convergence-time-decisions`. Every entry needs to be
  findable by a future contributor.

- **The core question is "how should we build this?"** Yes. The user's
  scoping framing was entirely about mechanism — evidence shape,
  dependency semantics, failure routing, resume. The "what" was already
  settled by issue #129.

### Anti-Signals Checked (Design Doc)

- **What to build is still unclear** — not present. Issue #129 has
  concrete acceptance criteria; the exploration didn't refine the
  problem statement.
- **No meaningful technical risk or trade-offs** — not present. Every
  lead surfaced real trade-offs.
- **Problem is operational, not architectural** — not present. This is
  squarely a state-machine and persistence question.

## Alternatives Considered

- **PRD (demoted)**. Signals present: single coherent feature, multiple
  stakeholders (the shirabe consumer exists as a known downstream).
  **Anti-signal present**: requirements were given as input — issue #129
  is already a requirements contract. The PRD work has effectively been
  done by the issue and the exploration's scope file. Writing a PRD
  would duplicate #129 without adding new information. Per the
  `requirements given as input` anti-signal, PRD is demoted.

- **Plan (demoted)**. Signals partially present: the work is understood
  well enough to decompose. **Anti-signal present**: open architectural
  decisions remain — the atomic-spawn window, forward-compat rule,
  template path resolution, and retry mechanics are not yet decided.
  The Design Doc vs Plan tiebreaker also applies: no upstream
  (PRD or design) exists for this topic yet, so Plan has nothing to
  decompose. Design doc goes first.

- **No Artifact (demoted)**. **Anti-signal present**: architectural
  decisions were made during exploration. The storage strategy, insertion
  point, child naming rule, and failure policy are all non-obvious
  choices that a future contributor needs the reasoning for. These
  cannot live only in commit messages or code comments.

- **Decision Record (demoted)**. **Anti-signal present**: multiple
  interrelated decisions were made. A decision record handles *one*
  decision cleanly; this exploration made six. Packaging them in a
  single design doc — where they can reference each other and share
  context — is more coherent than six separate decision records.

- **Rejection Record, VISION, Roadmap, Spike Report, Competitive
  Analysis** — none had matching signals. VISION is for nonexistent
  projects; koto exists. Spike is for feasibility questions; feasibility
  was established by the integration lead mapping the changes onto
  existing code paths. Competitive Analysis is private-repo only.
  Rejection Record requires positive rejection evidence; the adversarial
  lead didn't fire and there's no reason to reject. Roadmap is for
  multi-feature sequencing; this is one feature.

## Deferred Types (if applicable)

None. No deferred type (Prototype) applies here — the exploration already
mapped the implementation onto existing code paths, so "just try it"
would skip decisions that need recording.
