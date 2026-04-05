---
status: Current
upstream: docs/prds/PRD-hierarchical-workflows.md
problem: |
  koto's state machine is per-workflow with no awareness of other workflows.
  When a workflow needs to fan out over a collection of items -- each going
  through its own multi-state lifecycle -- the only option is an external
  orchestrator that manages per-item workflows and tracks the queue outside
  koto. This creates two sources of truth, forces reconciliation logic on
  every consumer, and means koto can't enforce ordering or dependency
  constraints across child workflows. The engine needs parent-child lineage,
  a convergence mechanism for waiting on children, and cross-hierarchy
  queries -- without taking on agent process management.
decision: |
  Add parent-child lineage via a header field on child state files, a
  children-complete gate type that queries child status through the session
  backend, a read-only koto status command for cross-workflow inspection,
  and advisory-only lifecycle coupling where koto reports affected children
  but never cascades operations automatically. The gate reuses existing
  blocking_conditions, gates.* routing, and override infrastructure with
  zero advance loop changes.
rationale: |
  The gate-based approach plugs into the existing gate evaluation system
  as a single new match arm, avoiding cross-cutting changes to the advance
  loop, StopReason enum, or response types. Header-only lineage is backward-
  compatible without a schema version bump. Advisory-only lifecycle keeps the
  agent in control, consistent with koto's role as a contract layer that
  doesn't launch or manage agent processes. Every major workflow engine's
  prior art validates the core model: external child templates, explicit
  result propagation, no implicit state sharing.
---

# DESIGN: Hierarchical multi-level workflows

## Status

Proposed

## Context and Problem Statement

koto is a workflow orchestration engine for AI coding agents that enforces
execution order through a state machine. Today each workflow is fully isolated:
its own state file, its own event log, its own evidence and decisions. When a
parent workflow needs to spawn and coordinate child workflows -- for example,
running a multi-phase implementation workflow for each issue in a plan -- the
parent agent must build an external orchestrator that duplicates what koto
already tracks.

The need spans multiple levels of nesting:

- A design workflow produces decisions, hands off to a planning workflow that
  decomposes into issues, then each issue runs through an implementation
  workflow. Today these are completely disconnected.
- A release workflow coordinates across multiple repos, each with its own plan,
  each plan containing multiple issues. Three levels.
- An exploration workflow fans out research agents, converges findings, then
  hands off to a design workflow. The handoff is a file on disk and a manual
  skill invocation.

The gate-transition contract (v0.6.0) established structured gate output that
feeds into transition routing. This foundation enables child workflow status to
be represented as gate data, reusing the existing routing and override
mechanisms rather than inventing new ones.

Issue: #127. Related: #105 (bounded iteration), #87 (workflow-scoped variables).

## Decision Drivers

- **koto is a contract layer, not an execution engine.** koto doesn't launch
  agents. The parent agent spawns children externally (Claude Agent tool,
  subprocesses, etc.) and hands them workflow names. koto tracks relationships
  and exposes state.
- **Minimal advance loop changes.** The advance loop has a clean seven-step
  pipeline. New primitives should plug into existing extension points (gates,
  evidence) rather than adding new steps or stop reasons.
- **Backward compatibility.** Existing workflows with no parent-child
  relationships must continue to work without changes. New fields should be
  optional and default-safe.
- **Both backends must work.** LocalBackend (filesystem) and CloudBackend (S3)
  share the SessionBackend trait. Changes to session storage must be viable for
  both without deep API rework.
- **Public repo, external contributors.** Design decisions need to be
  documented clearly enough that someone without organizational context can
  implement from them.

## Decisions Already Made

These choices were settled during exploration and should be treated as
constraints, not reopened.

1. **Gate-based fan-out over action-based or state-level declaration.**
   A `children-complete` gate type requires zero advance loop changes, reuses
   existing infrastructure (blocking_conditions, gates.* routing, overrides),
   and can be layered with declarative syntax later if needed. Action-based
   requires two new primitives (spawn + wait). State-level declaration is the
   most invasive change and introduces "stateful state" concepts.

2. **Header-only lineage over dual-event or directory nesting.**
   Adding `parent_workflow: Option<String>` to `StateFileHeader` requires
   minimal code changes, is backward-compatible without bumping schema_version,
   and satisfies primary query patterns since `list()` already reads all headers.
   Parent-side `ChildWorkflowSpawned` events deferred until crash-recovery
   requirements are concrete.

3. **Flat storage with metadata filtering over directory-based isolation.**
   Preserves the flat session model both backends depend on. Directory nesting
   would require reworking the entire SessionBackend trait (`create`, `exists`,
   `cleanup`, `list`, `session_dir`). Metadata-based filtering (header fields +
   CLI flags) achieves the same logical relationships.

4. **Naming convention (parent.child) as ergonomic default alongside metadata.**
   Dot-separated names are already valid per `validate_workflow_name()`.
   Convention provides zero-code-change isolation; metadata (`parent` header
   field) provides correctness guarantees. Both complement each other.

5. **Abandon as default parent close policy.**
   The parent agent manages child lifecycle. koto shouldn't force child
   termination when a parent completes. Aligned with Temporal's Parent Close
   Policy model (the only prior art that formalizes this).

6. **External child templates, no implicit state sharing.**
   Validated by all major workflow engine prior art (Temporal, Airflow, Argo,
   Prefect, Conductor, Step Functions). Children use their own template files.
   Parent-to-child data flows through explicit init-time parameters. No mid-
   execution state synchronization.

## Considered Options

### Decision 1: children-complete gate contract

The gate-transition contract (v0.6.0) established structured gate output with
typed fields. Each gate type produces a `StructuredGateResult` with an outcome
and typed output JSON. The exploration decided that a `children-complete` gate
type is the right primitive for fan-out convergence. Five sub-questions needed
resolution: how the gate identifies children, what completion condition it
checks, what the output schema looks like, how overrides work, and how agents
distinguish temporal blocking from corrective blocking.

These sub-questions are tightly coupled. The child identification strategy
constrains the output schema, and the completion condition determines what
"override to pretend done" means.

#### Chosen: Hybrid Discovery with Configurable Completion

The gate uses parent-pointer discovery as the primary child identification
mechanism. The evaluator queries `backend.list()`, reads `StateFileHeaders`,
and filters to workflows where `parent_workflow` matches the current workflow.
An optional `name_filter` field further filters by name prefix, enabling
multi-fanout scoping (e.g., only research children, not all children).

Template declaration:

```yaml
gates:
  children-done:
    type: children-complete
    completion: "terminal"        # optional, default "terminal"
    name_filter: "research."      # optional, prefix filter
```

The `completion` field controls when a child counts as complete: `"terminal"`
(default, child reached a terminal state), with `"state:<name>"` and
`"context:<key>"` reserved for future releases. The compiler rejects unknown
prefixes.

Output schema (fixed shape):

```json
{
  "total": 3, "completed": 2, "pending": 1, "all_complete": false,
  "children": [
    {"name": "explore.r1", "state": "done", "complete": true},
    {"name": "explore.r2", "state": "done", "complete": true},
    {"name": "explore.r3", "state": "research", "complete": false}
  ],
  "error": ""
}
```

Top-level aggregates enable quick `when`-clause checks
(`gates.children-done.all_complete: true`). The `children` array provides
per-child detail for routing decisions.

Override default: `{"total":0, "completed":0, "pending":0, "all_complete":true, "children":[], "error":""}` --
pretend all children are done with nothing pending.

Temporal signaling: a new `category` field on `BlockingCondition` with values
`"temporal"` (retry later, used by `children-complete`) and `"corrective"`
(fix something, default for existing gates). This is backward-compatible and
applies generically across all gate types.

#### Alternatives Considered

**Terminal-only completion with no name_filter.** Minimal implementation with
zero new `Gate` fields. Rejected because terminal-only completion is
insufficient for workflows where children have multiple terminal states with
different meanings, and would require a contract-breaking extension within a
release or two.

**Explicit child list with state-name completion.** Template-declared child
names with per-child status map output. Rejected because static child lists
don't support dynamic fan-out (the primary use case) and the variable-shaped
output map breaks `gate_type_schema()`.

**Naming-convention discovery with evidence-based completion.** Zero new fields
by reusing `pattern` and `key` for name filtering and context-key completion.
Rejected because naming convention is a weaker contract than parent-pointer
metadata, and field reuse creates semantic confusion across gate types.

### Decision 2: Lineage registration and discovery

`koto init --parent` needs concrete behavior: how it validates the parent
reference, where metadata lives, how `koto workflows` exposes hierarchy, and
whether naming is enforced. The exploration settled the high-level model
(header-only, flat storage, naming convention) but not these details.

#### Chosen: Strict Validation, Header-Only, Rich Discovery

`koto init --parent <name>` validates that the named parent workflow exists via
`backend.exists(parent)`. If missing, init fails with an error including the
parent name. This catches the most common failure (typos) at the earliest
point.

`parent_workflow: Option<String>` goes in `StateFileHeader` only (not
duplicated in `WorkflowInitialized` event payload). The header is immutable and
already read during discovery.

`koto workflows` output adds a `parent_workflow` field (null when absent) plus
two filter flags: `--roots` (only parentless workflows) and `--children <name>`
(only children of the named parent). No `--tree` flag -- agents derive trees
from the flat list.

The `parent.child` naming convention is documented but not enforced. The
`parent_workflow` header field is the authoritative link.

#### Alternatives Considered

**Warn-only validation with header+event duplication.** Warn on missing parent,
duplicate parent ref in event payload, no filter flags. Rejected because
warn-only misses typos, event duplication adds code without value, and no
filter flags pushes work onto every agent.

**Strict validation with enforced naming.** Child names must start with
`<parent>.`. Rejected because enforced naming couples the name string to
metadata, reduces flexibility, and provides marginal benefit over convention.

### Decision 3: Cross-hierarchy query interface

A parent agent needs to check child status without side effects. `koto next`
evaluates gates, runs actions, and advances state -- unsuitable for
observation. The `derive_machine_state()` function exists in `persistence.rs`
but has no CLI exposure.

#### Chosen: `koto status <name>` + existing `koto context get`

Add `koto status <name>` returning read-only state metadata:

```json
{
  "name": "design.research-agent",
  "current_state": "synthesize",
  "template_path": ".koto/research.template.json",
  "template_hash": "a1b2c3...",
  "is_terminal": false
}
```

The implementation calls `derive_machine_state()` and checks the compiled
template for terminal status. No gates evaluated, no actions run, no state
changes.

The full cross-hierarchy query surface: `koto workflows` (discover children),
`koto status <child>` (check state), `koto context get <child> <key>` (read
results).

#### Alternatives Considered

**Context-only (no new commands).** Relies on children writing status keys
voluntarily. If a child crashes before writing its key, the parent can't
determine child state. Correct but fragile.

**`koto query <name>` (full dump).** Exposes ephemeral epoch-scoped data
(evidence, decisions) not useful cross-workflow. Wastes agent context window.

**`koto workflows --status`.** Forces O(N) listing to check one child. Mixes
discovery and inspection concerns.

### Decision 4: Parent-child lifecycle coupling

Three destructive parent operations (cancel, cleanup, rewind) create edge
cases. koto can't force-terminate agent processes, and `ChildWorkflowSpawned`
events are deferred from MVP, so state-aware cascade isn't feasible.

#### Chosen: Advisory-only (inform, don't act)

koto never cascades lifecycle operations to children. Instead, every lifecycle
command that affects a parent includes child information in its JSON output:

- **Cancel:** cancels parent only; response includes `children` array with
  names and states of active children
- **Cleanup:** deletes parent session only; children become orphans with
  dangling `parent_workflow` references
- **Rewind:** walks back parent state; response includes advisory `children`
  field
- **Normal completion:** no automatic action; agent manages child lifecycle
- **Orphan discovery:** `koto workflows --orphaned` returns workflows whose
  parent no longer exists

#### Alternatives Considered

**Cancel cascades, cleanup blocks.** Automatically cancel children on parent
cancel, block parent cleanup while children exist. Rejected because it
contradicts the Abandon default, requires deferred `ChildWorkflowSpawned`
events for rewind, and takes control from the agent.

**Advisory cancel + cascading cleanup.** Cancel is advisory but cleanup
cascades recursively. Rejected because cascading cleanup is destructive and
irreversible -- deletes child evidence that may still be needed.

**Per-child metadata with opt-in cascade flags.** Each child gets
`on_parent_cancel` and `on_parent_cleanup` header fields. Rejected for MVP
because it adds significant surface area for usage patterns that don't exist
yet.

## Decision Outcome

**Chosen: Hybrid Gate + Strict Lineage + koto status + Advisory Lifecycle**

### Summary

Hierarchical workflows are built on four primitives that reinforce each other.
`koto init --parent <name>` registers the parent-child relationship by writing
`parent_workflow` to the child's state file header after validating the parent
exists. The `parent.child` naming convention is recommended but not enforced --
the header field is the authoritative link.

A new `children-complete` gate type checks child workflow status during the
parent's advance loop. The evaluator scans session headers for workflows whose
`parent_workflow` matches the current workflow, optionally filtered by
`name_filter`. If children haven't reached their completion condition (terminal
state by default), the gate fails and surfaces through existing
`blocking_conditions` with a new `category: "temporal"` field so agents know to
retry rather than take corrective action. Gate output includes per-child status
in a fixed-shape schema with aggregate fields (`total`, `completed`, `pending`,
`all_complete`) that feed into `gates.*` when-clauses for outcome-dependent
routing.

Parent agents observe children through three commands: `koto workflows
--children <parent>` to discover children, `koto status <child>` (new, read-
only) to check state without side effects, and `koto context get <child> <key>`
to read results children stored. When the parent is cancelled, cleaned up, or
rewound, koto reports affected children in the response JSON but takes no
automatic action -- the agent decides what to do. Orphaned children (parent
cleaned up first) are discoverable via `koto workflows --orphaned`.

Only `"terminal"` completion mode ships initially. The `completion` field's
closed-prefix design (`"state:<name>"`, `"context:<key>"`) reserves
extensibility for follow-up releases without changing the gate struct or output
schema.

### Rationale

The four decisions form a coherent stack: header-based lineage (Decision 2)
enables the gate's parent-pointer discovery (Decision 1), which enables the
parent to wait for children without advance loop changes. `koto status`
(Decision 3) gives agents read-only child inspection that complements the
gate's blocking output. Advisory-only lifecycle (Decision 4) keeps the agent in
control, consistent with koto's contract-layer role.

The key trade-off is implementation simplicity vs future flexibility. We accept
terminal-only completion for MVP, convention-based naming, and no automatic
cascade -- each with a clean upgrade path. The gate struct grows by two
optional fields, `BlockingCondition` by one, and the CLI adds one command
(`status`) and three flags (`--roots`, `--children`, `--orphaned`). No advance
loop changes, no new `StopReason` variants, no new event types.

## Solution Architecture

### Overview

Hierarchical workflows add parent-child awareness to koto's per-workflow state
machine without changing the advance loop. A child workflow registers its
parent at init time via a header field. A new `children-complete` gate type
queries child status through the session backend. A new `koto status` command
provides read-only state inspection. Lifecycle commands gain advisory child
information in their output.

### Components

```
┌─────────────────────────────────────────────────────────┐
│ Template Layer                                          │
│                                                         │
│  Gate struct (src/template/types.rs)                    │
│    + completion: Option<String>                         │
│    + name_filter: Option<String>                        │
│                                                         │
│  Compiler (src/template/compile.rs)                     │
│    + validate children-complete gate fields              │
│    + validate completion prefix ("terminal", "state:*") │
├─────────────────────────────────────────────────────────┤
│ Engine Layer                                            │
│                                                         │
│  Gate evaluator (src/gate.rs)                           │
│    + children-complete match arm                         │
│    + gate_blocking_category() function                  │
│                                                         │
│  Persistence (src/engine/persistence.rs)                │
│    + derive_machine_state() — already exists            │
│                                                         │
│  Types (src/engine/types.rs)                            │
│    + StateFileHeader.parent_workflow: Option<String>     │
├─────────────────────────────────────────────────────────┤
│ Session Layer                                           │
│                                                         │
│  SessionBackend (src/session/)                          │
│    + list() returns parent_workflow in SessionInfo      │
│                                                         │
│  Discovery (src/discover.rs)                            │
│    + find_workflows_with_metadata() threads parent      │
│                                                         │
│  Types (src/engine/types.rs)                            │
│    + WorkflowMetadata gains parent_workflow field       │
├─────────────────────────────────────────────────────────┤
│ CLI Layer (src/cli/)                                    │
│                                                         │
│  init handler                                           │
│    + --parent flag, existence validation                │
│    + writes parent_workflow to header                   │
│                                                         │
│  workflows handler                                     │
│    + --roots, --children, --orphaned filter flags       │
│    + parent_workflow in JSON output                     │
│                                                         │
│  status handler (new)                                   │
│    + read-only state + terminal check                   │
│                                                         │
│  cancel/cleanup/rewind handlers                        │
│    + advisory children array in response               │
│                                                         │
│  next handler                                          │
│    + BlockingCondition gains category field             │
│    + gate evaluator closure captures session backend    │
├─────────────────────────────────────────────────────────┤
│ CLI Output Types (src/cli/next_types.rs)               │
│                                                         │
│  BlockingCondition                                      │
│    + category: String ("temporal" | "corrective")      │
└─────────────────────────────────────────────────────────┘
```

### Key Interfaces

**`koto init <name> --parent <parent-name>`**

Validates parent exists via `backend.exists(parent_name)`. Writes
`parent_workflow: Some(parent_name)` to the child's `StateFileHeader`. Fails
with exit code 1 if parent doesn't exist. No event written to the parent's
log.

**`children-complete` gate evaluation**

Input: gate definition with `completion` and `name_filter` fields, access to
session backend.

Process:
1. Call `backend.list()` to get all sessions with headers
2. Filter to sessions where `header.parent_workflow == Some(current_workflow)`
3. If `name_filter` is set, further filter by name prefix
4. If zero children match, return `Failed` (prevent vacuous pass)
5. For each child, check completion condition against current state
6. Build output JSON with per-child status and aggregates
7. Return `Passed` if `all_complete`, `Failed` otherwise

Output: fixed-shape JSON (see gate contract above).

The gate checks **direct children only** -- workflows where
`parent_workflow == current_workflow`. It does not recurse into grandchildren.
Multi-level convergence (grandparent waiting on parent waiting on grandchild)
works because each level declares its own `children-complete` gate. The
grandparent's gate passes when the parent reaches its terminal state, which
only happens after the parent's own gate passes when its children complete.
This composition is implicit and requires no special handling.

The gate evaluator closure in the CLI handler (`src/cli/mod.rs`) captures the
session backend through the same closure injection pattern used for
`context_store`. The `evaluate_gates` closure gains access to `backend` from
its environment; the `advance_until_stop` signature does not change.

**`koto status <name>`**

Calls `derive_machine_state()` to get current state from event log. Loads
compiled template to check `terminal` flag. Returns JSON with `name`,
`current_state`, `template_path`, `template_hash`, `is_terminal`.

**`BlockingCondition.category`**

New field on `BlockingCondition` struct. Set by `gate_blocking_category()`:
returns `"temporal"` for `children-complete`, `"corrective"` for all others.
Serialized in all `gate_blocked` and `evidence_required` responses.

### Data Flow

```
Parent Agent                    koto                         Child Agent
    │                            │                               │
    │  koto init child           │                               │
    │  --parent parent           │                               │
    │  --template child.md       │                               │
    │ ──────────────────────────>│                               │
    │                            │ validate parent exists        │
    │                            │ write child header            │
    │                            │   (parent_workflow: parent)   │
    │  {name: "child", ...}      │                               │
    │ <──────────────────────────│                               │
    │                            │                               │
    │  [spawn child agent with   │                               │
    │   workflow name "child"]   │                               │
    │ ─────────────────────────────────────────────────────────> │
    │                            │                               │
    │  koto next parent          │          koto next child      │
    │ ──────────────────────────>│ <─────────────────────────────│
    │                            │                               │
    │  action: gate_blocked      │  action: evidence_required   │
    │  blocking_conditions:      │  [child runs its loop...]    │
    │    children-done:          │                               │
    │      category: temporal    │          koto next child      │
    │      output:               │          --with-data ...      │
    │        all_complete: false  │ <─────────────────────────────│
    │        pending: ["child"]  │  [child reaches terminal]     │
    │ <──────────────────────────│                               │
    │                            │                               │
    │  [retry later]             │                               │
    │                            │                               │
    │  koto next parent          │                               │
    │ ──────────────────────────>│                               │
    │                            │ gate evaluates: child done    │
    │  action: evidence_required │                               │
    │  (gates passed, parent     │                               │
    │   can now advance)         │                               │
    │ <──────────────────────────│                               │
    │                            │                               │
    │  koto status child         │                               │
    │ ──────────────────────────>│                               │
    │  {current_state: "done",   │                               │
    │   is_terminal: true}       │                               │
    │ <──────────────────────────│                               │
    │                            │                               │
    │  koto context get child    │                               │
    │    results                 │                               │
    │ ──────────────────────────>│                               │
    │  <child's stored results>  │                               │
    │ <──────────────────────────│                               │
```

## Implementation Approach

### Phase 1: Lineage registration

Add `parent_workflow: Option<String>` to `StateFileHeader`. Add `--parent`
flag to `koto init`. Validate parent existence. Thread `parent_workflow`
through `SessionInfo` and `WorkflowMetadata`. Add `parent_workflow` to
`koto workflows` JSON output.

Deliverables:
- `src/engine/types.rs` -- header field
- `src/session/mod.rs` -- SessionInfo field
- `src/discover.rs` -- WorkflowMetadata field
- `src/cli/mod.rs` -- init handler (`--parent`), workflows output
- Tests for init with/without parent, parent validation

### Phase 2: Workflow discovery flags

Add `--roots`, `--children <name>`, and `--orphaned` filter flags to
`koto workflows`. Filter logic lives in the CLI handler using metadata from
`list()`.

Deliverables:
- `src/cli/mod.rs` -- filter flags on workflows handler
- Tests for each filter mode

### Phase 3: `koto status` command

Add `koto status <name>` command. Call `derive_machine_state()`, load compiled
template, check terminal flag, return JSON.

Deliverables:
- `src/cli/mod.rs` -- status handler
- Tests for status output shape, terminal detection

### Phase 4: `children-complete` gate type

Add gate type to evaluator. Add `completion` and `name_filter` to `Gate`
struct. Add gate type schema. Add compiler validation. Wire session backend
through gate evaluator closure. Add `gate_blocking_category()` and `category`
field to `BlockingCondition`.

Deliverables:
- `src/gate.rs` -- evaluator match arm, category function
- `src/template/types.rs` -- Gate fields, schema
- `src/template/compile.rs` -- validation
- `src/cli/next_types.rs` -- BlockingCondition.category
- `src/cli/mod.rs` -- wire session backend into gate closure
- Integration tests with parent-child workflow lifecycle

### Phase 5: Advisory lifecycle

Add child discovery to cancel, cleanup, and rewind handlers. Include
`children` array in their JSON responses. No cascade behavior.

Deliverables:
- `src/cli/mod.rs` -- cancel, cleanup, rewind handlers
- Tests for advisory output

### Phase 6: Skill updates

Update koto-user and koto-author skills to document hierarchical workflows
(PRD requirements R16 and R17).

**koto-user:** Add `children-complete` gate to action dispatch table and
handling guidance. Document temporal vs corrective `category` field. Add
`koto status`, `--parent` on init, and `--roots`/`--children`/`--orphaned`
on workflows to command reference. Cover overriding `children-complete` gates
in override flow section.

**koto-author:** Add `children-complete` gate type to template authoring guide
with `completion` and `name_filter` fields. Document the single-state fan-out
pattern (directive + gate on same state). Add compiler validation for
`children-complete` fields. Include parent+child template pair example.

Deliverables:
- `plugins/koto-skills/skills/koto-user/SKILL.md` -- hierarchy sections
- `plugins/koto-skills/skills/koto-user/references/` -- updated references
- `plugins/koto-skills/skills/koto-author/SKILL.md` -- gate type docs
- `plugins/koto-skills/skills/koto-author/references/` -- updated references
- Updated evals for both skills covering hierarchy scenarios

## Consequences

### Positive

- Zero advance loop changes. The `children-complete` gate plugs into the
  existing gate evaluation system, reusing blocking_conditions, `gates.*`
  routing, and override infrastructure.
- Backward compatible. Existing workflows ignore the new header field and gate
  type. No schema version bump needed.
- Clean upgrade path. Terminal-only completion ships first; `"state:<name>"`
  and `"context:<key>"` follow without schema changes. Advisory lifecycle can
  evolve to per-child policies.
- Agents use familiar patterns. `gate_blocked` with `blocking_conditions` is
  already handled by koto-user skill agents. The `category` field adds signal
  without changing the dispatch flow.

### Negative

- Session scan for child discovery. Every `children-complete` gate evaluation
  calls `backend.list()` and reads all headers. For large session stores this
  could be slow.
- Orphans are possible. Advisory-only lifecycle means agents that don't clean
  up children leave dangling state files. `--orphaned` makes them discoverable
  but doesn't prevent them.
- No fan-out declaration in templates. The directive tells the agent what to
  spawn, but the template doesn't structurally declare children. This is
  fragile for complex fan-out patterns.

### Mitigations

- Session scan cost is bounded by the number of sessions per repo-id scope
  (expected to be small, under 50). If this becomes a bottleneck, a secondary
  index of parent-child relationships can be added without changing the gate
  contract.
- Orphan accumulation can be addressed by periodic `koto workflows --orphaned`
  checks in agent cleanup routines. Per-child cascade policies can be layered
  on later.
- Template-level children declarations can be added as syntactic sugar that
  compiles down to gates and directives, preserving the simple engine model.

## Security Considerations

**Cross-workflow isolation.** The `parent_workflow` header field is self-
declared by the child at init time. koto validates that the parent exists but
does not authenticate that the initializing agent is authorized by the parent.
Any agent with access to the session backend can create a child claiming any
existing workflow as its parent, which affects the parent's `children-complete`
gate evaluation. In the current trust model (single user per repo-id scope),
this is acceptable. Multi-tenant deployments would require an authorization
mechanism for parent-child registration.

Use `name_filter` on `children-complete` gates to restrict which children
affect evaluation. Without it, any workflow declaring the parent relationship
will be counted.

**Resource bounds.** Child creation is unbounded by default. Each child creates
a session directory and state file. The `children-complete` gate scans all
sessions in the repo-id scope on every evaluation (O(N) in total sessions, not
just children). Keep session counts under 50 per repo-id for predictable
performance. Clean up completed children promptly. If performance degrades, a
secondary parent-child index can be added without changing the gate contract.

**Orphan lifecycle and name recycling.** Parent cleanup does not cascade to
children. Orphaned children retain a `parent_workflow` reference to a deleted
session. If a new workflow reuses the deleted parent's name, orphaned children
will silently appear as its children, corrupting `children-complete` gate
evaluation with unrelated child data. This is exploitable in shared
environments. Use `koto workflows --orphaned` in cleanup routines to prevent
accumulation, and clean up orphaned children before reusing a parent workflow
name.

**Context key trust boundary.** `koto context get <child> <key>` is the
standard mechanism for parent agents to read child results. Child-written
context values should be treated as untrusted input by the parent agent. A
child workflow (or its agent) could craft context values intended to influence
parent behavior. Template authors should validate child-provided data before
using it in parent transition logic or gate evaluations.
