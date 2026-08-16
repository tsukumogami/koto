# Decision D3: unifying the delivery rule across both construction sites

## Question

`NextResponse` is built at exactly two sites: `dispatch_next` (`src/cli/next.rs:32-124`), called only from the `--to` branch of `handle_next` (`src/cli/mod.rs:3355`), and the `StopReason` match inside `handle_next`'s main advance path (`src/cli/mod.rs:4040-4196`). Only the second site applies a delivery check today, computed once at `src/cli/mod.rs:3995-4014` and reused across six `StopReason` arms. R4 requires one rule to govern both paths. This decision is about the *plumbing* — how the (already-decided-elsewhere, D1's job) delivery decision reaches both sites and gets applied uniformly — not about how the decision itself is computed.

## Decision drivers

- R4: one rule, evaluated identically on the natural-advancement and directed-transition paths.
- R5: `--full` must still force-include details on both paths.
- R18: no new file read on `koto next` beyond what the pre-change binary performs.
- The PRD's own Out of Scope list forbids "changing the shared visit-count derivation's own semantics" — so `derive_visit_counts` and its second consumer (`visited_states` in `src/workflows_surface/project.rs:284-286`) must come out untouched.
- `dispatch_next`'s doc comment (`src/cli/next.rs:16-30`) frames it as a pure, I/O-free classifier; its ~20 unit tests (`src/cli/next.rs:162-819`) call it positionally with today's four-argument signature.
- Minimize blast radius on passing tests while still landing R1-R4 as one rule.

## Considered options

### A. Thread the check into `dispatch_next`

Add a parameter to `dispatch_next` — either a resolved `Option<String>` override or a `suppress_details: bool` — and apply it inside the function's existing `details = if template_state.details.is_empty() { None } else { ... }` block (`next.rs:50-54`, reused at lines 64, 83, 99, 114).

This has exactly one production call site to update (`mod.rs:3355`), but breaks all ~20 unit tests in `next.rs` at compile time since they call `dispatch_next(state, &ts, advanced, &gates)` positionally — every one needs a fifth argument added, even though none of those tests are about the delivery rule at all; they test classification (Terminal vs GateBlocked vs EvidenceRequired vs IntegrationUnavailable). That's pure churn with no signal.

Worse, the decision itself — "has this occupancy already received these details" — needs the event log, which `dispatch_next` doesn't take and whose doc comment explicitly says the function does no I/O. So under A, the caller (`mod.rs:3355`) still has to compute the boolean externally before calling `dispatch_next`, using whatever D1's decision function turns out to be. A doesn't eliminate that computation — it just adds a second place (inside `dispatch_next`) where the boolean gets *applied*. The only thing A buys over C is that the "already-decided" fields (`details`) are set correctly at construction time instead of being cleared afterward — a marginal stylistic difference that costs a signature change and 20 test edits.

- Preserves `derive_visit_counts`'s second consumer: yes, untouched either way — A doesn't call `derive_visit_counts` from `dispatch_next` itself, only from the caller.
- Rule evaluated once per response: yes, if the caller computes the boolean once and passes it in.
- Rewind/respawn-awareness: lives in whatever function computes the boolean before the call; orthogonal to A itself.
- Test blast radius: ~20 unit tests in `next.rs` need a new argument; `dispatch_next`'s "no I/O" contract is watered down by taking a delivery-related bool that only makes sense in light of session history.

### B. Rewire the `--to` handler to share the advance path's computation

Compute the decision in the caller (`mod.rs`, inside the `--to` branch, between the event append at `3341-3349` and the `dispatch_next` call at `3355`) using the same function/inputs the main path uses at `3995-4014`, then either (a) pass it into `dispatch_next` as in A, or (b) apply it to the response `dispatch_next` returns, after the fact.

B-(a) collapses into A (same signature change, same test churn). B-(b) is materially the same design as C below — the difference between B and C in the brief's framing is really "does the shared computation live in the caller vs. in a named shared helper," which isn't a fork in outcome so much as a naming question. I fold B-(b) into C's evaluation rather than treat it as a fourth independent design.

### C. Hoist the decision into a shared helper both sites call

A single function — call it `should_suppress_details(state: &str, events: &[Event]) -> bool` (its actual name and internal computation are D1's decision, not this one) — computes the boolean from the state name and the event log. Both `mod.rs:3355`'s branch and `mod.rs:4001-4015` call it, each with its own post-append event list.

The codebase already has the exact combinator shape this calls for: `NextResponse::with_substituted_directive` (`src/cli/next_types.rs:159-251`) is a consuming method that pattern-matches all six variants and applies a uniform transform to `directive` and `details`, and it is *already called at both construction sites* — `mod.rs:3357` (the `--to` path) and `mod.rs:4198` (the main path) — immediately after each site builds its raw `NextResponse`. `with_directive_prefix` (`next_types.rs:268-...`) is the same pattern again, for the abandonment notice, also called at both sites.

So C isn't introducing a new architectural idiom; it's adding a third combinator alongside two that already exist and are already threaded through both sites:

```rust
// next_types.rs, next to with_substituted_directive
pub fn with_details_suppressed_unless_full(self, already_delivered: bool, full: bool) -> Self {
    let suppress = already_delivered && !full;
    // match every variant; if suppress, set details: None; Terminal/Error pass through unchanged
}
```

Call sites:
- `mod.rs:4198`, chained right where `.with_substituted_directive(...)` already runs. This *replaces* the inline `if full || count <= 1 { Some(..) } else { None }` logic currently baked into each of the six `StopReason` arms (`4001-4015`, reused at `4052`, `4073`, `4096`, `4106`, `4124`, `4174/4184`) with: build `details` unconditionally in every arm (just `Some(final_template_state.details.clone())` when non-empty, matching what `dispatch_next` already does), compute `already_delivered` once, and let the combinator clear it uniformly.
- `mod.rs:3357`, chained right where `.with_substituted_directive(...)` already runs for the `--to` path. `dispatch_next` itself is untouched — it keeps building `details` unconditionally exactly as it does today (`next.rs:50-54`). The `--to` branch computes `already_delivered` right before or after the `dispatch_next` call, using the post-append event list, and applies `.with_details_suppressed_unless_full(already_delivered, full)` to the response before the abandonment-notice splice.

**On R18 (no new file read):** the naive way to get the post-append event list on the `--to` path is `backend.read_events(&name)` after the `append_event` call at `3341` — an extra read the pre-change binary doesn't do. But the `--to` branch already holds the exact `DirectedTransition` payload it just wrote (the `payload` variable at `3336-3340`) and already has `events` in scope from before the branch (used to derive `machine_state`). `Event.seq` and `Event.timestamp` (`src/engine/types.rs:1035-1040`) aren't consumed by any visit/occupancy-style computation — `derive_visit_counts` only matches `.payload` — so the branch can clone `events`, push a synthetic `Event` wrapping the same `payload` (any placeholder `seq`, the same `now_iso8601()` timestamp already computed for the real append), and feed that in-memory list to the shared decision function. Zero added disk reads. This is a concrete implementation requirement this decision should carry forward, not an incidental optimization — R18 is a hard requirement and the naive version violates it.

**On R5 (`--full` override):** `full` is already an in-scope parameter of `handle_next`, threaded into the `--to` branch's closure the same as everywhere else in the function, so baking `!full` into the combinator's own signature (`with_details_suppressed_unless_full(already_delivered, full)`) rather than trusting each call site to remember `!full && already_delivered` removes one way the two sites could drift apart. Small point, but it's exactly the kind of thing that turns into a silent divergence six months from now if left as caller-side boolean algebra.

- Preserves `derive_visit_counts`'s second consumer: yes. The shared decision function is new; it may call `derive_visit_counts` internally or may not, but nothing about C requires touching `derive_visit_counts`'s matching logic or its return semantics, which is what `project.rs:284-286` depends on (it only reads the key set, not count values, but the PRD's Out of Scope list forbids touching the function's semantics outright regardless).
- Rule evaluated once per response: yes, at both sites — same property the main path already has today, preserved and, if anything, clarified: the decision becomes a single value computed once and a single combinator call, instead of being re-embedded in each `StopReason` arm's `details: details.clone()` line.
- Rewind/respawn-awareness: lives entirely inside the shared decision function (D1's territory) and inside R7-R14's retrieval surface (D2's territory) respectively. D3 only has to guarantee both call sites feed the decision function the correct (state, post-append events) pair — which the in-memory-append trick above does correctly for a `--to` self-transition too, since the synthetic event is byte-for-byte what would be on disk.
- Test blast radius: `dispatch_next`'s ~20 unit tests: **zero changes** — its signature and body are untouched. New tests needed: unit tests for the new combinator in `next_types.rs` (one per variant plus the Terminal/Error no-op cases, small and additive, same shape as whatever tests exist for `with_substituted_directive` if any) and the integration-level ACs the PRD already requires regardless of which option is picked (non-advancing repeat, rewind arrival, both directed-transition cases — R19's own list).
- Regression risk on `--to`: real, but it's the fix, not a side effect — R2/AC explicitly requires "two consecutive directed transitions into the same phase: the first carries instructions, the second does not," which is a deliberate behavior change from today's "`--to` always includes details" defect. The grep across `tests/integration_test.rs`'s eleven `--to` call sites shows none of them assert on the `details` field's presence against a template that declares non-empty details content, so the existing suite doesn't regress — but R20/R23 (skills docs + evals) must move in the same PR or a caller reading `response-shapes.md` gets surprised by a real behavior change with stale documentation.

### D. Collapse the two construction sites into one

Rewrite `handle_next` so the `--to` branch stops early-returning at `mod.rs:3396` and instead feeds its target state into the same `StopReason`-shaped match the main path uses at `4040-4196`, or extract a single `fn build_next_response(state, template_state, advanced, gate_results, events, full) -> NextResponse` that both branches call and that owns 100% of variant construction, replacing both `dispatch_next` and the `StopReason` match's response-building tail.

This is a much larger refactor: it has to reconcile `dispatch_next`'s five-variant classification (which never evaluates gates for `--to`, by design — `mod.rs:3353` passes an empty `gate_results` map, "skip gate evaluation") with the main path's `StopReason`-driven six-variant match, which is fed by `advance_until_stop`'s actual gate evaluation. The two paths differ in more than just the details rule: `--to` skips gate evaluation entirely, never produces `Integration` or `ActionRequiresConfirmation`, and doesn't run the advance loop's auto-advance chaining. Actually unifying construction would either have to preserve those differences as parameters (in which case the "unified" function still branches internally on which path called it, which is A's signature-pollution problem at a larger scale) or change `--to`'s behavior to evaluate gates — which the PRD explicitly puts Out of Scope ("Requiring the directed-transition path to evaluate gates... a materially larger behavior change than this problem statement supports").

- Preserves `derive_visit_counts`'s second consumer: yes, same as C, if done carefully — but the larger the refactor, the larger the surface for an incidental touch.
- Rule evaluated once: yes, trivially, since there'd be one construction path.
- Blast radius: large. Every one of `dispatch_next`'s ~20 tests and a meaningful fraction of the `StopReason` match's implicit coverage (the eleven `--to` integration tests plus whatever exercises the main path) would need re-validation against a restructured control flow, for a decision the PRD scoped as "one rule, not two response builders."
- This is the option the PRD's Decisions section warns against by name (the gate-evaluation carve-out), and R4's own text says "There is one rule, not two" — rule, not response-construction-site. D over-delivers relative to what R4 asks for.

## Recommendation

**C**, concretely as: add `NextResponse::with_details_suppressed_unless_full(self, already_delivered: bool, full: bool) -> Self` to `next_types.rs` alongside `with_substituted_directive` and `with_directive_prefix`, call it at both existing post-construction call sites (`mod.rs:3357` for `--to`, `mod.rs:4198` for the main path), and compute `already_delivered` at each site from a D1-owned decision function fed the post-append event list — for `--to`, built in-memory from the already-appended `payload` rather than re-read from disk, to hold R18.

This is the smallest true unification available: `dispatch_next` is untouched (zero signature change, zero of its 20 existing tests touched), the main path's per-arm `details.clone()` duplication collapses into one combinator call, and the mechanism is not new — it's the same "compute once, transform every variant uniformly, apply at both call sites" idiom the codebase already uses twice for `directive`. A and B both require a `dispatch_next` signature change that produces test churn for no behavioral gain over C. D is a larger refactor than R4 asks for and the PRD explicitly rules out the one thing that would make it clean (gate evaluation on `--to`).

## Case against the recommendation

The strongest objection: C leaves `dispatch_next`'s `details` field populated unconditionally at construction time, then relies on a *second* function elsewhere clearing it — two functions have to agree about a field's final value, and a future reader who only looks at `dispatch_next` will see `details: details.clone()` on every branch and reasonably conclude the delivery rule doesn't apply there, because nothing in `next.rs` says otherwise. `with_substituted_directive` sets a precedent that this kind of two-step construction is normal in this codebase, but it's still an extra hop for anyone tracing the field by grep rather than by reading `next_types.rs` end to end. A doc comment on `dispatch_next` noting that its `details` field is provisional and gets filtered by a caller-applied combinator is a cheap mitigation, but it's a real cost against A/B's more visible (if more expensive) alternative of making the suppression visible at the point details gets set.

I attacked the in-memory-event-construction detail specifically (the R18 mitigation) looking for a reason it doesn't survive: could a decision function plausibly need `seq` ordering relative to *other* event types (e.g., an `EvidenceSubmitted` interleaved between the last entry and now) to compute delivery status correctly, in a way a synthetic max-`seq` placeholder would get wrong? Checked `derive_visit_counts`'s current implementation (`persistence.rs:981-993`): it iterates in list order and only inspects `.payload`, never `.seq` — so list order, not the `seq` field's value, is what matters, and the in-memory list (`events.clone()` + push) preserves list order correctly since the just-appended `DirectedTransition` is genuinely the last event chronologically. If D1's decision function turns out to need genuine `seq` values for some other reason (unlikely given the existing precedent, but not something this decision can rule out sight-unseen), the mitigation degrades gracefully to "one extra `read_events` call on the `--to` path only," which is a small, isolated R18 exception rather than a reason to abandon C itself.

The recommendation survives both attacks: the first is a documentation cost, not a correctness one; the second has a concrete fallback that only costs R18 cleanliness, not the overall design.

## Consequences

- `next_types.rs` gains one new combinator and its tests; `next.rs`'s `dispatch_next` and its ~20 tests are unmodified.
- `mod.rs:4001-4015`'s per-arm `if full || count <= 1` computation is replaced by an unconditional `details` build (matching `dispatch_next`'s existing pattern) plus one `already_delivered` computation and one combinator call at `4198`.
- `mod.rs:3355`'s `--to` branch gains: a clone of `events` with the just-appended payload pushed on (or, if D1's decision function needs a real disk read for other reasons, one `backend.read_events` call), one `already_delivered` computation, and one combinator call at `3357`.
- The `--to` path's long-standing "always includes details" behavior changes for repeat directed transitions into an already-occupied phase — this is R2/R4's intended fix and must ship together with R20/R23's doc and eval updates so agents relying on the old contract aren't left with stale documentation describing a behavior that no longer exists.
- Whatever D1 decides for the occupancy/delivery computation must be exposed as a function taking `(state, events)` or equivalent — not baked into `derive_visit_counts` itself — so this design's "second consumer untouched" property holds. D3 imposes that shape constraint on D1's output, not the reverse.

## Open questions for cross-validation

- D1 needs to confirm its decision function's actual signature accepts a plain `events: &[Event]` slice (not something backend-coupled) so the `--to` path's in-memory-constructed list can be passed without a real read — if D1's function needs backend access directly, the R18 mitigation in this decision doesn't apply as described.
- D2 (the read-only retrieval) should confirm it does not also need to call whatever combinator or decision function this design introduces in a way that would risk counting the retrieval itself as a delivery — R10 requires it not to.
