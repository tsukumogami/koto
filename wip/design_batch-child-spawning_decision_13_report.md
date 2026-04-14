<!-- decision:start id="post-completion-observability" status="assumed" -->
### Decision: Post-completion observability

**Context**

Decision 6 extended `koto status <parent>` and `koto workflows --children <parent>` with batch metadata, but only while the parent's current state declares a `materialize_children` hook. Round 1 walkthroughs (cluster F in `wip/walkthrough/round1_synthesis.md`, full transcript in `simulation_round1_pair1c.md`) found five gaps that surface the moment the batch terminates or the parent advances past the batched state:

1. `koto status` drops the `batch` section the instant the parent leaves the batched state — exactly when consumers need it to write a summary, diagnose failures, or decide follow-up action.
2. The minimal `done` response (`{action, state, directive, is_terminal}`) drops `blocking_conditions` and `scheduler`, so batch detail evaporates on the terminal tick.
3. Synthetic skipped children (Decision 5.2) are shape-indistinguishable in `koto status` and `koto workflows --children` from real terminal work, so observers cannot tell "synthesized as skipped" from "author's template deliberately ended in a skipped-like state." CD9 may replace the synthetic-template mechanism with runtime reclassification, but whichever representation survives must still be identifiable.
4. Transitive skip attribution is singular (`skipped_because: X`). For a chain B-failed → D-skipped → E-skipped-because-of-D, the design does not specify whether X is the direct blocker (D) or the root cause (B).
5. Per Decision 6, when `failure_reason` is unset the batch view's `reason` falls back to the opaque terminal state name (e.g., `"done_blocked"`). Templates that route to a `failure: true` state via `default_action` shell scripts get no compile-time guidance to write `failure_reason`, so the fallback engages silently.

All five gaps are observability-only and converge on a single theme: the observer's right-to-know extends past the batched state's lifetime. The fix must be additive (no changes to existing field semantics), survive cloud-sync round trips, and stay read-only on the query paths.

**Assumptions**

- CD9 is deciding the on-disk representation of skipped children in parallel. This decision uses "whichever representation marks a child as synthesized-skipped" as its hook point, without committing to synthetic state files or runtime reclassification. Whatever survives CD9 must expose a single well-known predicate; this decision names that predicate `synthetic: true` and leaves the computation to CD9's chosen mechanism.
- Cloud sync tolerates additional fields in parent state files. Persisting a small `BatchFinalView` alongside context (or as a dedicated event) round-trips through cloud sync the same way existing context data does. No new backend contract is required.
- The `batch_final_view` payload is bounded by the same task-count limit that bounds the in-flight batch view (Decision 1's R6/R7 caps). It does not grow unboundedly.
- Observers read response fields by name. Additive fields (new optional top-level keys, new values in an enum's documented range) do not regress existing consumers keying on `action`, `state`, `directive`, `is_terminal`.
- Compile warnings are advisory, not blocking. W5 surfaces through the same CLI path as W1-W4 (printed on `koto template compile`, suppressible per existing convention).

**Chosen: Persist `batch_final_view`, extend terminal responses, mark synthetic children explicitly, record transitive skip chain, add W5 warning**

Five coordinated sub-decisions, all additive, all read-only on observer paths:

**13.1 Preserve the batch view across terminal transitions — store on a marker event.**

When the `children-complete` gate first evaluates `all_complete: true` on a state with `materialize_children`, the advance loop appends a `BatchFinalized` event to the parent log containing the final `BatchView` snapshot (the same payload Decision 6 already computes via `derive_batch_view`). Subsequent `koto status <parent>` calls — regardless of the parent's current state — emit the `batch` section by replaying from the most recent `BatchFinalized` event, labeled `batch.phase: "final"` to distinguish from `batch.phase: "current"` (the live view emitted by Decision 6 when the current state has the hook). If the parent re-enters a batched state (e.g., after `retry_failed`), the next gate pass appends a new `BatchFinalized` event, superseding the prior one.

- **Why event-based rather than context-key-based.** Event-based storage round-trips through cloud sync identically to the rest of the append-only log. It also survives the `retry_failed` evidence-clearing write (Decision 5.4 step 4) that wipes reserved context keys, because events are never rewritten.
- **Why not carry via context.** Context keys are template-author territory; persisting a reserved key there pollutes the author's namespace and risks collisions. Events are engine-reserved.
- **Why not "leave to the consumer."** The design driver is "observability through existing commands." Forcing consumers to snapshot their own batch view duplicates work koto already does, and every consumer ends up writing the same `derive_batch_view` wrapper.

**13.2 Terminal `done` response carries `batch_final_view` when the workflow was batched.**

The `done` response shape extends to `{action, state, directive, is_terminal, batch_final_view?}`. The field is present (as `Option::is_none` suppresses emission when null) if and only if the parent log contains at least one `BatchFinalized` event. The payload is identical to the `batch` section in `koto status`. This gives agents on the terminal tick — when they're asked to write a summary directive — the full batch snapshot without a second command.

For intermediate `done` responses (non-terminal transitions), `batch_final_view` is also emitted when a `BatchFinalized` event exists on the log. This consistency means the same response shape covers "just transitioned past the batched state" and "reached an actual terminal state," eliminating the gap where `summarize` runs with no batch detail.

**13.3 Synthetic-child marking — explicit `synthetic: true` field.**

Both `koto status <name>` and the per-row shape in `koto workflows --children <parent>` add an explicit boolean field `synthetic: true` when the child was created as a skip-marker by the scheduler (and not by the agent driving the canonical child template). The field is false (or omitted via `skip_serializing_if`) for real work.

The predicate is: "this child's lifecycle was authored by the scheduler, not by the agent." Implementation-wise, this is a one-line check against whatever representation CD9 settles on:
- **If CD9 keeps synthetic state files** (current Decision 5.2): `synthetic: true` when the current state's template is the hardcoded synthetic mini-template (detectable via `template_hash` matching the synthetic's compile-time-known hash, or a `synthetic: bool` flag on `CompiledTemplate`).
- **If CD9 adopts runtime reclassification**: `synthetic: true` when the scheduler's classification reports `Skipped` without a corresponding real-template child state file, or when a stored `skip_marker_only: bool` on the child's header is set.

Observers don't need to know which mechanism is live; they read `synthetic` and branch on it.

**`koto next <synthetic-child>` returns an immediate terminal `done` response** with directive text explaining the synthesis:

```json
{
  "action": "done",
  "state": "skipped_due_to_dep_failure",
  "directive": "This task was skipped because dependency '<skipped_because>' did not succeed. No action required.",
  "is_terminal": true,
  "synthetic": true,
  "skipped_because": "<name>",
  "skipped_because_chain": [...]
}
```

Rationale: an error response on a legitimate-state read would be hostile (agents chase errors). A silent blank directive (current behavior per GAP 5) is worse — the agent gets no signal. The explicit `synthetic: true` plus a templated directive interpolating `skipped_because` gives both machine-readable and human-readable answers. The directive is the same whether CD9 keeps the synthetic mini-template or moves to runtime reclassification — in the latter case, the engine synthesizes the response at `koto next` time rather than pulling it from a hardcoded template.

**13.4 Transitive skip attribution — keep singular, add `skipped_because_chain: [...]`.**

Retain the singular `skipped_because: <name>` field, defined as the **direct** upstream task whose non-success caused this skip. Add a parallel `skipped_because_chain: [<direct>, <grandparent>, ..., <root-failure>]` array recording the full attribution path from direct blocker back to the first failed (not skipped) task in the chain.

For the diamond scenario (B-failed → D-skipped → E-skipped):
- D's fields: `skipped_because: "B"`, `skipped_because_chain: ["B"]`.
- E's fields: `skipped_because: "D"`, `skipped_because_chain: ["D", "B"]`.

The chain is computed at synthesis time by walking upstream through `waits_on` until a failed (non-skipped) ancestor is found. The last element is always the root failure; the first element is always the direct blocker. For diamonds where multiple blocker paths exist, pick the shortest; tie-break on alphabetical order for determinism.

This preserves the existing singular field for consumers that don't care about transitivity while giving diagnostic tools (shirabe's work-on-plan, human debugging) the full context. Both fields land in the batch view (`children[].skipped_because`, `children[].skipped_because_chain`), in `koto status` output, in `--children` rows, and in the synthetic-child's own `koto next` / `koto status` responses.

**13.5 Compile warning W5 — `failure: true` state without `failure_reason` writer.**

Extend the compile-warning list with:

> **W5** — warning — Terminal state with `failure: true` has no path that writes `failure_reason` to context. The batch view's `reason` field will fall back to the state name, which is uninformative for observers.

The compiler detects W5 by checking that for each state S with `failure: true`, at least one of the following is true:
- S's `accepts` block declares a field named `failure_reason`, OR
- S's `default_action` (if present) writes `failure_reason` to context (via koto's context-write frontmatter syntax), OR
- An upstream state's transition into S has a `context_assignments` entry writing `failure_reason`.

If none holds, W5 fires at compile time (printed by `koto template compile`, with the same suppression convention as W1-W4). This is advisory: the template compiles and runs. The warning simply surfaces the case where a template author is about to produce opaque failure diagnostics.

**Additional projection: reason source.** When the batch view's per-child `reason` field engages the fallback (state name), also emit `reason_source: "state_name"`. When `reason` comes from the `failure_reason` context key, emit `reason_source: "failure_reason"`. This lets observers distinguish "author chose to echo the state name" from "author wrote nothing" without a heuristic on the value. Omitted entirely (via `skip_serializing_if`) for successful or not-yet-terminal children.

**Rationale**

Each sub-decision picks the option that preserves the "observability through existing commands" driver while keeping the additions additive and cloud-sync-clean:

- **13.1 (event-based `BatchFinalized`)** beats "carry view through context" on cloud-sync cleanliness (events are append-only and never mutated post-write), beats "leave to the consumer" on the core design driver, and beats "carry through terminal states only" on observability breadth (the view survives arbitrary post-batch states, not just terminal ones).
- **13.2 (`batch_final_view` in `done`)** collapses the two-call pattern (status + next) into one. The alternative — leaving `done` minimal and expecting consumers to call `koto status` for batch detail — works but forces a known high-frequency access pattern to pay a second command's overhead.
- **13.3 (explicit `synthetic: true`)** is the minimum marking that survives CD9's open question. Both "keep synthetic template" and "runtime reclassification" reduce to the same observer-visible predicate. The immediate-terminal response for `koto next <synthetic-child>` avoids the GAP 5 silent-blank-directive problem without introducing error semantics on a legitimate state read.
- **13.4 (keep singular + add chain)** avoids breaking existing consumers while giving new consumers the full picture. An alternative "replace singular with plural" is cleaner architecturally but is a breaking change on a field already documented in Decision 5.2 and Decision 6.
- **13.5 (W5)** is the smallest compile-time change that catches the opaque-reason gap. The alternative "require `failure_reason` as a hard error" would be too strong — some templates legitimately have nothing to say beyond the state name. A warning nudges without blocking.

**Alternatives Considered**

- **13.1(a) Carry last-known batch view through all subsequent states via in-memory cache.** Rejected: doesn't survive process restarts, inconsistent with koto's "pure function of disk state" model, and breaks on cloud sync machine handoffs.
- **13.1(c) Leave batch-view preservation to the consumer via evidence.** Rejected: duplicates work koto already does via `derive_batch_view`, and the consumer's snapshot would have to ride on context keys (polluting template-author namespace) or a sidecar file (violating the "state is the append-only log" invariant).
- **13.2 alt: Keep `done` minimal; expect consumers to call `koto status`.** Rejected: a known-frequent access pattern (write-summary-on-terminal-tick) should not require two commands when one suffices, and terminal responses are the natural place to deliver the final snapshot because transitions into terminal states are typically "once and done."
- **13.3 alt: `kind: "skip_marker"` string field.** Rejected in favor of `synthetic: bool`. A string enum field is more expressive but `synthetic` is the only category observers need to discriminate; a boolean is simpler and the eventual string enum can be added later without breaking anything.
- **13.3 alt: Error on `koto next <synthetic-child>`.** Rejected: errors imply caller malformation. A synthetic child is a legitimate workflow that happens to have nothing to do.
- **13.4 alt: Switch singular field to name the root cause (B) instead of direct blocker (D).** Rejected: breaking change on existing field, and direct blocker is the more locally useful answer for most observer queries (e.g., "which task do I retry to unblock this?" is answered by the direct blocker chain, not the root).
- **13.4 alt: Replace singular with plural array only.** Rejected: breaking change.
- **13.5 alt: Make missing `failure_reason` writer a hard error (E11).** Rejected: overly strong. A template author may legitimately decide the state name is a sufficient reason (e.g., `done_cancelled_by_user`). Warning respects authorial intent while flagging the common mistake.
- **13.5 alt: Add no warning; rely on skill-level documentation.** Rejected: leaves the silent-fallback gap unflagged. Compile warnings are the natural place to catch this at authoring time.

**Consequences**

**What becomes easier:**

- Consumers writing summary directives on terminal transitions receive the batch snapshot in the same response, no second call needed.
- `koto status <parent>` works uniformly across the parent's lifecycle: current-phase view while batched, final-phase view afterward.
- Agents encountering a synthetic skipped child get a directive that explains the situation, instead of a silent blank.
- Diagnostic tools can render both direct-blocker and root-cause views without running their own DAG walks.
- Template authors routing failures through shell-script actions see a compile warning guiding them to write `failure_reason`.
- CD9's on-disk representation choice is decoupled from the observability surface — whichever survives, observers read `synthetic: true`.

**What becomes harder / what changes:**

- `BatchFinalized` is a new engine-reserved event type. Decision 2's event-type registry must learn it, and replay paths (cloud-sync reconciliation, `koto query`, `koto session resolve`) must handle it. The event is append-only and idempotent — same content on re-evaluation, so merge conflicts reduce to "keep latest."
- The `BatchFinalized` payload size grows with batch cardinality. Decision 1's limits (`max_tasks_per_batch`, depth cap) already bound it; no new bound needed. But cloud sync round-trip size increases modestly for batched parents.
- `done` response consumers that strictly validate the response shape (unlikely but possible) would need to accept the new optional `batch_final_view` field. Standard additive-field compatibility applies.
- `synthetic: true` and `skipped_because_chain` become part of the documented response shape in the koto-user skill reference. The koto-author skill gains a W5 entry in its compile-warning table.
- `reason_source` disambiguation adds one more field to the per-task batch view. Observers can ignore it; those that care gain clarity.
- `handle_next` on a synthetic child must recognize the case and return the synthesized terminal response with the interpolated directive. Under CD9-keeps-synthetic-template, this comes from the hardcoded mini-template. Under CD9-runtime-reclassification, the engine constructs the response inline; either is a few lines in `handle_next`.
- Compile-time detection for W5 requires the template compiler to walk `accepts`, `default_action`, and incoming transitions looking for `failure_reason` writes. This is additive to the existing W1-W4 detection passes.

**Cloud-sync compatibility.** `BatchFinalized` events follow the same append-only conflict-resolution rules as other events. Two machines cannot both append a different final view for the same batch epoch — the first gate-pass wins, and the second machine's push either no-ops (same event) or fails with the existing concurrent-write error (caller retries). Synthetic-child identification uses state-file data that syncs unchanged. The `skipped_because_chain` is computed from disk at synthesis time, persisted in context alongside the singular `skipped_because`, and syncs as any other context key.

**Downstream shirabe fit.** Work-on-plan's coordinator gains what cluster F flagged as missing: a post-terminal batch snapshot for writing the summary (from `batch_final_view` on the `done` response), an explicit flag to skip synthetic children in its dispatch loop (`synthetic: true`), a root-cause view for error messages (`skipped_because_chain[last]`), and authoring guidance to write `failure_reason` in its child templates (W5 at compile time). No new commands, no new schemas outside the extensions above.

<!-- decision:end -->
