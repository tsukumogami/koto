# Exploration Findings: hierarchical-workflows

## Core Question

How should koto model hierarchical workflows where a parent workflow spawns children via `koto init --parent`, children run independently through their own templates, and the parent can query child state/evidence to inform its own transitions?

## Round 1

### Key Insights
- Gate-based fan-out (`children-complete` gate type) is the best fit -- zero advance loop changes, reuses blocking_conditions, gets gates.* routing + overrides for free (fan-out-primitive, advance-loop)
- Header-only lineage (`parent_workflow: Option<String>` in StateFileHeader) is the minimum viable approach -- backward-compatible, no schema_version bump, satisfies query patterns via existing list() (state-file-lineage)
- All major workflow engines converge on: external child templates, no shared state, explicit result propagation, fail-propagation by default (prior-art)
- `koto context get` is already a cross-workflow query primitive requiring zero code changes (query-interface)
- Dot-separated naming convention (`parent.child`) is valid per existing validate_workflow_name() and provides natural isolation (isolation-model)
- Temporal's Parent Close Policy is the only prior art for child lifecycle on parent completion (prior-art)

### Tensions
- Fan-out visibility: gate approach keeps fan-out invisible to koto vs state-level declaration making it explicit. Gate approach can be layered later.
- Temporal vs corrective gates: agents can't distinguish "retry later" from "fix something" in current BlockingCondition shape
- Convention-based vs metadata-based isolation: not mutually exclusive, recommend both

### Gaps
- No read-only query command (koto query documented in CLAUDE.md but doesn't exist)
- CloudBackend S3 listing doesn't read headers, so parent/tree filtering would need header downloads
- Parent Close Policy equivalent not designed
- No polling hint mechanism for temporal gates

### Decisions
- Gate-based fan-out over action-based or state-level declaration
- Header-only lineage over dual-event or directory nesting
- Flat storage with metadata filtering over directory-based isolation
- Naming convention + metadata as complementary isolation mechanisms
- Abandon as default parent close policy
- External child templates, no implicit state sharing

### User Focus
Auto mode -- no user input this round. Decisions derived from research convergence.

## Accumulated Understanding

koto's hierarchical workflow model should be built on three primitives:

1. **Lineage via header metadata.** `koto init <name> --parent <parent-name>` writes `parent_workflow: Some("<parent-name>")` to the child's StateFileHeader. No parent-side event needed initially. Discovery uses existing `list()` which already reads all headers. `koto workflows` gains `--roots` and `--children <parent>` flags.

2. **Convergence via a `children-complete` gate type.** A new gate type that queries child workflow states through the session backend. Returns structured output (`pending`, `completed`, `failed` arrays) that feeds into `gates.*` when-clauses for outcome-dependent routing. Blocked children surface through existing `blocking_conditions` response shape. Override mechanism lets agents bypass stuck children.

3. **Cross-workflow queries via existing primitives.** Parent agents use `koto context get <child> <key>` to read child artifacts and `koto workflows --children <parent>` to enumerate children. A read-only `koto query` command would improve observability but isn't strictly required for MVP.

The model preserves koto's role as a contract layer: it tracks relationships and exposes state, but doesn't launch agents or manage child process lifecycle. The parent agent spawns children externally, hands them workflow names, and uses koto to check their status and read their results.

Open design questions for the design doc: parent existence validation on init, cleanup cascading policy, polling/retry hints for temporal gates, and CloudBackend implications.

## Decision: Crystallize
