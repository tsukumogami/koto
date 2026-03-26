<!-- decision:start id="transition-count-response-contract" status="assumed" -->
### Decision: How transition_count integrates into the response contract

**Context**

The `koto next` command returns JSON responses via `NextResponse` variants, each containing an `advanced: bool` field. The auto-advance engine tracks `transition_count` as a local variable but never exposes it. The `advanced` field has inconsistent semantics across code paths: hardcoded `true` for directed transitions, derived from "at least one transition happened" in the auto-advance loop, and documented as "was an event appended before dispatching." This ambiguity makes `advanced` unreliable as a precise signal for how much happened during a `koto next` call.

The design needs `transition_count` exposed for observability so consumers can distinguish "nothing happened" (0) from "one step" (1) from "auto-advanced through several states" (5). The constraint is strict backward compatibility: adding fields is fine, changing existing field semantics is breaking.

**Assumptions**

- No external consumers depend on `advanced` meaning specifically "an event was appended" vs "a transition happened." The field is treated as a rough "did something change" indicator by agent callers.
- Agent consumers ignore unknown JSON fields (standard JSON forward-compatibility). Adding `transition_count` won't break parsers.

**Chosen: Option A -- Add transition_count to AdvanceResult and all NextResponse variants, keep advanced unchanged**

Add `transition_count: usize` to `AdvanceResult`. Add `transition_count: u64` to all six `NextResponse` variants. Serialize it in the JSON output alongside `advanced`. Document `transition_count > 0` as the preferred way to check whether transitions occurred. Deprecate `advanced` in documentation only -- it retains its current semantics and keeps shipping in the JSON.

Implementation touches:
- `AdvanceResult` struct: add field, return it from `advance_until_stop`
- `NextResponse` enum: add field to all 6 variants
- `with_substituted_directive`: pass through in all 6 arms
- Custom `Serialize` impl: serialize in all 6 arms
- `handle_next` in mod.rs: thread transition_count from AdvanceResult into NextResponse construction
- Tests: update all serialization assertions to include transition_count

**Rationale**

Option A is the only alternative that satisfies all four stated constraints simultaneously. It's backward compatible (additive field only), keeps `advanced` in the response, stays lean (one integer, no array of passed-through states), and requires zero new instrumentation since `transition_count` already exists in the engine loop.

The semantic inconsistency in `advanced` becomes irrelevant over time as consumers migrate to the precise `transition_count` signal. There's no need to fix `advanced` -- just make it obsolete by providing something better alongside it.

Option B (aligning advanced = transition_count > 0) was tempting for its cleanliness but directly violates the backward-compatibility constraint. The directed-transition path currently hardcodes `advanced: true`, and changing that is a semantic break that could affect agent behavior in ways we can't audit. The cleanup isn't worth the risk when transition_count already provides the precise signal.

Option C (AdvanceResult only, not in JSON) defeats the purpose. The whole motivation is consumer observability. Telling consumers to read the event log to count transitions adds complexity and latency for information the engine already has.

**Alternatives Considered**

- **Option B: Add transition_count, align advanced = transition_count > 0.** Cleans up the semantic inconsistency but changes `advanced` behavior for directed transitions (currently hardcoded true). This is a subtle breaking change that violates the backward-compatibility constraint. Rejected because the risk of unknown consumer impact outweighs the aesthetic benefit.

- **Option C: Add transition_count to AdvanceResult only, not to NextResponse.** Minimal code change but provides zero consumer value. transition_count stays internal, and consumers still rely on the semantically inconsistent `advanced` bool. Rejected because it doesn't address the observability need that motivated this decision.

**Consequences**

- The JSON response gains one new field (`transition_count`) across all response types. Existing parsers that don't destructure strictly will continue working.
- `advanced` becomes a legacy field. It should not be removed until a major version boundary, but new documentation and examples should reference `transition_count` instead.
- The code change is mechanical but touches ~18 locations in next_types.rs plus the handler. This is a one-time cost with no ongoing maintenance burden.
- Future features (e.g., progress reporting, chain depth limits surfaced to consumers) can build on transition_count rather than inventing new signals.
<!-- decision:end -->
