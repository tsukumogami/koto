# Maintainer review — request-store-converge Issue 1 (walking skeleton), commit 8f1aa9a

Lens: can the next developer understand this and change it with confidence?
Files touched: `src/engine/types.rs`, `src/cli/mod.rs`, `src/cli/batch.rs`
(+ inline tests in types.rs and batch.rs).

## Blocking

### B1. Merged doc-comment blocks orphan `append_child_completed_to_parent`'s docs onto `synthesize_workflow_result`
`src/cli/mod.rs:2103-2137`

Two `///` doc blocks run together with no blank line and no item between them:

- Lines 2103-2123: a doc block opening "Issue #134: append a `ChildCompleted`
  event to the parent's log..." — clearly meant for
  `append_child_completed_to_parent`.
- Lines 2124-2137: a doc block opening "Synthesize a `WorkflowResult`..." —
  meant for `synthesize_workflow_result`.

Because Rust attaches a contiguous `///` run to the *next* item, the whole
2103-2137 block binds to `synthesize_workflow_result` (line 2139). The actual
function `append_child_completed_to_parent` (line 2175) ends up with **no doc
comment at all**.

What the next developer misreads:
- rustdoc / hover for `synthesize_workflow_result` shows a two-headed
  description that opens by talking about appending events to the parent log —
  the wrong function entirely. They form a wrong mental model of what
  `synthesize_workflow_result` does and where its side effects are.
- `append_child_completed_to_parent` — the function that actually performs the
  parent-log append and returns the cleanup-safety `ChildCompletedAppend`
  signal — looks undocumented, so the carefully written contract about when
  cleanup may proceed (NoParent / Notified / AppendFailed) is invisible at the
  function it describes.

Fix: insert a blank line (end the first doc block) so 2103-2123 attaches to
`append_child_completed_to_parent` and 2124-2137 to `synthesize_workflow_result`.
The likely cause is that `synthesize_workflow_result` was inserted *above*
`append_child_completed_to_parent` after its doc block was written, splitting
the function from its documentation.

## Advisory

### A1. `WorkflowResult.summary` is documented as "Bounded" but nothing bounds it
`src/engine/types.rs:702-703` ("Bounded human-readable end-of-work statement"),
synthesized at `src/cli/mod.rs:2152-2161`.

The design treats the summary bound as a stated property — "the `summary` is
length-bounded" (DESIGN §Security, lines 562-566), "keep the summary bounded"
(line 591), "The summary bound is enforced and documented" (line 628).
`synthesize_workflow_result` reads the raw `"summary"` evidence string verbatim
with no truncation, and the default-fallback strings are unbounded
(`format!("completed at {}", final_state)`).

The next developer reading the field doc ("Bounded") will assume truncation
already happens and may build on that assumption (e.g., skip a length check at a
new write site, or trust the field is safe to inline into a directive). Either
this is deferred to a later Issue (walking-skeleton scope) — in which case the
word "Bounded" should be qualified ("intended to be bounded; enforcement lands
in Issue N") — or the bound belongs here. As written the comment describes
behavior the code does not yet exhibit. Flagging as advisory because Issue 1 is
explicitly a skeleton; the security reviewer may rate the missing bound higher.

### A2. `RequestStoreResult` doc claims it is "Emitted on a child's own session log" but no producer exists in this commit
`src/engine/types.rs:633-646`.

The variant doc states as present-tense fact: "Emitted on a child's own session
log when it reaches a terminal state, carrying the auto-promoted `WorkflowResult`
envelope." In this commit `RequestStoreResult` is only *defined*, *deserialized*
(types.rs:1043-1047), and *round-tripped in a test* (types.rs:2645-2672). Grep
confirms no append site — the child-log append is Phase 3 of the design, not
yet landed. Only the parent-side `ChildCompleted.result` copy is wired.

A maintainer grepping "who emits `RequestStoreResult`?" finds nothing and may
conclude the feature is half-broken or that they deleted the producer. A one-line
"Producer lands in Issue N (Phase 3); Issue 1 wires only the parent-side
`ChildCompleted.result` copy" would prevent the detour. The neighboring
`ChildCompleted.result` doc (types.rs:617-625) does cite Decision 3 and reads
correctly, so the asymmetry is the trap.

### A3. Dual-keyed result maps in `build_children_complete_output` — keying rule is correct but easy to break
`src/cli/batch.rs:2309` (`event_snapshots`, keyed by short `task_name`) vs
`src/cli/batch.rs:2316` (`result_by_child`, keyed by composed `child_name`),
consumed at 2435-2443.

Two maps built in the same loop (2317-2337) from the same `ChildCompleted`
event use *different* keys: `event_snapshots` keys on `task_name` (short),
`result_by_child` keys on `child_name` (composed `<parent>.<task>`). The lookup
at 2437-2440 then does `result_by_child.get(&entry.name).or_else(composed)`.
For the hook path `entry.name` is the short task name, so the direct `get` always
misses and the `.or_else(composed)` branch is what actually fires; the direct
`get` only matches the no-hook/legacy cleaned-up case.

The comments at 2310-2316 and 2428-2434 do explain this, and the test
`gate_inlines_child_result_without_replaying_child_log` (batch.rs:4320) exercises
the composed-name path — so this is documented, not silently wrong. The advisory
is that the two maps look like twins built side by side but key differently; a
future change that "unifies" them, or swaps the lookup order assuming the direct
match is the common case, would silently drop results. Consider keying both maps
the same way, or adding a one-line "NOTE: these two maps key differently on
purpose" at the build site so the divergence is impossible to miss.

### A4. `synthesize_workflow_result` has no direct unit test
`src/cli/mod.rs:2139`.

The summary-synthesis logic (the part the task description specifically asks
about: evidence `summary` field vs final-state-derived default, the
empty-fields → `payload: None` filter at 2163-2165) is only exercised
indirectly through `gate_inlines_child_result_without_replaying_child_log`,
which feeds a pre-built `ChildCompleted.result` rather than running synthesis.
The three default-summary branches (Success/Failure/Skipped) and the
"prefer evidence `summary`" override are untested as behavior, so the next
developer editing the default strings or the field-lookup convention has no
test that would catch a regression. The types-level round-trips are good but
they test the envelope, not the synthesis.

## What reads well (not exhaustive)

- `WorkflowResult` naming and field docs (status/summary/payload) are clear, and
  reusing `TerminalOutcome` for `status` (no new enum) is well-justified in the
  doc comment (types.rs:685-707).
- The composed-name-vs-short-name fallback in `append_child_completed_to_parent`
  (mod.rs:2207-2215) is explicitly commented, including the legacy
  `koto init --parent` case.
- The serde-optional rationale ("additive field... pre-feature logs round-trip
  unchanged") is stated at every new optional field and matches the project's
  documented additive-field idiom.
- Tests are honestly named and their assertions match their names; the
  backward-compat test `pre_feature_child_completed_without_result_deserializes_to_none`
  (types.rs:2704) documents the additive-field contract precisely.
- The "why a copy on the parent" rationale (survive child auto-cleanup, converge
  is a read without opening the child log) is stated at both the field
  (types.rs:617-625) and the read site (batch.rs:2294-2316, 2428-2434), citing
  Decision 3/4.
