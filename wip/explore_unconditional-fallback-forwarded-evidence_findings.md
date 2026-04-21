# Exploration Findings: Unconditional Fallback on Forwarded Evidence

Issue #146 — round 1

---

## Is This a Real Bug?

**Yes.** It is reproducible from the issue steps and was observed in production in the
`work-on-plan.md` orchestrator template, where `ci_monitor` and `plan_completion` were
silently skipped in a single `koto next` call.

---

## What Is Actually Happening

There are two slightly different paths that produce the same observable failure:

### Path A — Empty Evidence After auto-advance

After a state transition (skip_if or unconditional), `advance_until_stop` resets
`current_evidence = BTreeMap::new()` at lines 507 and 549 of `src/engine/advance.rs`.
The next state is entered with empty agent evidence. If that state has:
- conditional transitions (that require evidence to match), AND
- an unconditional fallback (a transition with no `when` clause),

then `resolve_transition()` falls through all conditional arms (`_` case, line 738),
finds the unconditional target, checks only `gate_failed` (which is `false`), and fires
the unconditional fallback at line 746 — skipping the state's directive entirely.

### Path B — Forwarded Evidence

At least one code path does not consistently clear `current_evidence` before entering a
newly-reached state. When `current_evidence` still holds evidence from state A and state B
is entered, the evidence from A is assembled into `evidence_value` (lines 453–467). B's
conditional transitions don't match (wrong keys), the unconditional fallback fires.

Both paths produce the same observable result: a state that should ask for agent input is
silently bypassed, and the advance chain continues past it.

---

## Root Cause

`resolve_transition()` has exactly one protection against premature unconditional firing:
the `gate_failed` flag (added in commit dfce0bf, the gate-failed fallback fix). When gates
fail, the unconditional fallback is suppressed and `NeedsEvidence` is returned. No
equivalent protection exists for the "just entered via auto-advance, no evidence yet"
case.

**The fix pattern is already in the codebase.** The `gate_failed` guard at line 740–744
is exactly the model. A second condition of the same form is needed.

---

## Fix Scope

| Dimension | Assessment |
|-----------|------------|
| Files | `src/engine/advance.rs` primary; possibly `src/engine/types.rs` if a flag is added to the loop state |
| Lines changed | ~5–25 depending on approach |
| New design concepts | No — follows existing `gate_failed` guard pattern |
| Regression risk | Low-to-medium: templates that intentionally rely on unconditional fallbacks firing immediately when a state is entered via chaining (e.g., pure routing states with no conditional transitions) must NOT be broken |

### The Key Behavioral Constraint

An unconditional fallback that is the ONLY transition (no conditional transitions on the
state at all) should continue to fire on auto-advance — that is intentional routing.
Only unconditional fallbacks on states that ALSO have conditional transitions need the
guard; those states are meant to gather evidence before proceeding.

---

## Fix Approaches (Three Viable Options)

**Option A — Track advance origin with a flag (recommended)**
Add a boolean `bool fresh_evidence` to the loop (true = agent submitted fresh evidence
this iteration, false = entered via auto-advance). Pass it to `resolve_transition()` as
a second guard alongside `gate_failed`. In the unconditional fallback arm: if
`!fresh_evidence && has_conditional`, return `NeedsEvidence`. ~10–20 lines.

**Option B — Check empty evidence + has_conditional in resolve_transition**
At the unconditional fallback arm (line 739), add: if `evidence.as_object().map_or(true,
|o| o.is_empty()) && has_conditional`, return `NeedsEvidence`. This is simpler (~5–8
lines) but relies on empty evidence as a proxy for "auto-advanced" — which breaks if
future code legitimately submits empty evidence to a state.

**Option C — Clear evidence more aggressively**
Ensure `current_evidence` is always reset at the TOP of the loop iteration for non-initial
states. Requires a guard to preserve initial-state evidence. ~10 lines, but has subtle
first-iteration risk.

Option A is cleanest because the distinction (was this a fresh agent submission?) is the
right invariant to capture, not evidence emptiness.

---

## Test Gaps

- No existing test covers unconditional fallback + auto-advance (skip_if or chaining).
- `skip-if-branch.md` is the closest fixture (skip_if + unconditional fallback + conditional
  transition) but is only exercised in the skip_if-fires path; the fallthrough path is
  untested.
- `gate-with-evidence-fallback.feature` covers gate-failed + unconditional but not the
  advance-chain case.
- Fix should add a regression test in `tests/integration_test.rs` covering the exact
  scenario from the issue: A→B chain where B has conditional + unconditional transitions
  and is entered via skip_if.

---

## Is This Simple or Complex?

**Simple to medium — leans simple.** The approach is known (follow the `gate_failed`
pattern). The code is in one function in one file. The behavioral constraint
(don't break pure-routing states) is clear. No new design infrastructure is needed.

The only non-trivial piece: choosing between Option A and B and ensuring the regression
test captures the exact failure mode.

---

## What Artifact Is Needed?

None. This is a targeted bug fix with a clear cause, a known fix pattern, and no
competing architecture approaches that need design exploration. Proceed directly to
`/work-on #146`.
