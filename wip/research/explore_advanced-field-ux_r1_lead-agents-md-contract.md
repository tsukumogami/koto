# Lead: What does the AGENTS.md caller documentation promise about `koto next` responses?

## Findings

### Three documents contain caller-facing `koto next` documentation

1. **`plugins/koto-skills/AGENTS.md`** -- The primary caller doc, designed for AI agents to read directly. Contains a "Response Shapes" section with five `action` variants and an error handling section.
2. **`docs/guides/cli-usage.md`** -- The CLI reference guide. Contains the most detailed documentation with a field-presence matrix table and dispatcher classification order.
3. **`README.md`** -- Brief overview. Mentions `advanced` once: "The `advanced` flag is `true` when the call itself caused a state change (via `--with-data` or `--to`)."

### What each doc says about `advanced`

**AGENTS.md** (lines 143-219):
- Shows `"advanced": false` in all non-terminal examples
- Shows `"advanced": true` in the Terminal/done example
- Line 203-205: "The state auto-advances. The engine runs gates, and if they pass, transitions automatically. You'll see `"advanced": true` in the response when this happens."
- The "action: execute (no expects, no blocking)" subsection (lines 203-205) is the only place that directly explains `advanced: true` for auto-advancement
- Does NOT define what `advanced` means in a general, standalone definition

**cli-usage.md** (lines 60-66):
- Line 65: "--with-data ... On success, appends an `evidence_submitted` event and sets `advanced: true` in the response."
- The field-presence table (lines 90-99) shows `advanced` is present in all five variants but never explains its semantics
- Does NOT define what `advanced: false` means or what callers should do differently based on the value

**README.md** (line 94):
- "The `advanced` flag is `true` when the call itself caused a state change (via `--with-data` or `--to`)."
- This is the closest thing to a standalone definition, but it's incomplete -- it omits auto-advancement as a cause of `advanced: true`

### What the implementation actually does

The `advanced` field comes from `AdvanceResult.advanced` (in `src/engine/advance.rs`, line 89), which is set to `true` when at least one `transitioned` event is appended during the advancement loop (line 336: `advanced = true`). This happens in three scenarios:

1. **Evidence submission** (`--with-data`): The evidence event is appended, then the advancement loop runs and may transition, setting `advanced: true`.
2. **Directed transition** (`--to`): The handler sets `advanced: true` directly when calling `dispatch_next` (in `src/cli/mod.rs`, line 1344).
3. **Auto-advancement**: Gates pass on a state with no accepts block and an unconditional transition. The loop transitions automatically, setting `advanced: true`.

The key semantic: `advanced: true` means "the engine moved to a new state during THIS call." It says nothing about whether the current state's work has been done. It means "you are now in a state you weren't in when you called me."

### What each doc tells callers to DO with each response shape

**AGENTS.md** provides clear action guidance:
- `action: "execute"` with `expects` present -> Execute directive, submit evidence matching schema
- `action: "execute"` with `blocking_conditions` -> Fix conditions, call `koto next` again (don't submit evidence)
- `action: "execute"` no expects, no blocking -> "auto-advances" (briefly mentioned, no explicit caller action)
- `action: "done"` -> Workflow finished, stop
- `action: "confirm"` -> Review `action_output`, submit evidence if state accepts it
- Error responses -> Table with exit codes and agent actions

**cli-usage.md** provides the same five variants with a sixth (IntegrationUnavailable) split out from Integration. It also documents the dispatcher classification order (lines 176-181), which is useful for understanding priority. But it focuses on response structure, not caller behavior.

### Coverage gaps in documentation

1. **No standalone `advanced` definition**: None of the docs provide a clear, authoritative definition of what `advanced` means across all variants. The README comes closest but omits auto-advancement.

2. **`advanced: true` on EvidenceRequired is undocumented**: When the engine auto-advances through several states and lands on one requiring evidence, the response is `action: "execute"` with `advanced: true` AND `expects` populated. AGENTS.md never shows this combination. Callers may be confused -- "it advanced, but it still wants evidence?"

3. **`advanced: false` on Terminal is undocumented**: When calling `koto next` on an already-terminal workflow (without `--with-data`), the response is `action: "done"` with `advanced: false`. The code confirms this (`src/cli/next.rs` line 36, `src/engine/advance.rs` line 219). AGENTS.md only shows Terminal with `advanced: true`.

4. **Gate-with-evidence fallback is undocumented in AGENTS.md**: When gates fail on a state that has an `accepts` block, the dispatcher returns `EvidenceRequired` instead of `GateBlocked` (`src/engine/advance.rs` lines 298-310, `src/cli/next.rs` lines 61-73). AGENTS.md doesn't mention this behavior at all. `cli-usage.md` doesn't mention it either. Callers seeing `EvidenceRequired` with failed gates won't know they can submit override evidence.

5. **Auto-advance chaining is barely documented**: The engine can chain through up to 100 states in a single `koto next` call. AGENTS.md mentions auto-advancement in one sentence but doesn't explain that `advanced: true` can mean the engine jumped through 5 states, not just 1.

6. **CycleDetected and ChainLimitReached stop reasons have no documented response shape**: These produce error responses in the handler but aren't covered in any doc.

7. **`--with-data` response shape not documented**: After submitting evidence, the response is whatever the engine lands on after advancement. But AGENTS.md doesn't show what a successful `--with-data` response looks like -- it only documents the bare `koto next` responses.

### Mismatches between docs and code

1. **README says `advanced` is caused by `--with-data` or `--to`**: This is incomplete. Auto-advancement through gate-passing states also sets `advanced: true`. The README definition would lead callers to think `advanced: true` only happens when they explicitly triggered it.

2. **AGENTS.md shows `action: "confirm"` but code has a variant called `ActionRequiresConfirmation`**: The names differ but the behavior matches. No mismatch in output shape.

3. **Fallback variant gap**: The dispatcher's fallback case (step 5/6 in `dispatch_next`, lines 108-117) returns `EvidenceRequired` with empty expects. This "auto-advance candidate" shape is not documented in any doc. AGENTS.md's "no expects, no blocking" section (line 203) vaguely alludes to it but shows no example response.

4. **`cli-usage.md` dispatcher order doesn't mention gate-with-evidence fallback**: The documented order (line 176-181) says step 2 is "Any gate failed -> GateBlocked", but the actual code skips GateBlocked when the state has an accepts block. The docs omit this nuance.

## Implications

1. **The `advanced` field needs a formal definition** that covers all three causes (evidence submission, directed transition, auto-advancement) and explicitly states what it does NOT mean (it doesn't mean the current state's work is done or pre-cleared).

2. **Issue #102's concern is validated**: The docs never explain that `advanced: true` means "you just arrived here" rather than "this phase is already handled." Callers reasonably misread it as the latter.

3. **A response contract specification (PRD) should define**:
   - Canonical definition of `advanced`
   - Complete set of response shapes with example JSON for each
   - Explicit caller action for each shape, including `advanced: true` + `expects` present
   - Gate-with-evidence fallback behavior
   - What happens after `--with-data` success
   - Edge cases: terminal with advanced=false, chaining, cycle detection

4. **AGENTS.md is the most impactful doc to fix** since it's what agents actually read at runtime. cli-usage.md is secondary (human reference). README is tertiary (overview).

## Surprises

1. **Gate-with-evidence fallback is completely undocumented**: This is a significant behavioral nuance -- when gates fail but the state accepts evidence, the engine falls through to EvidenceRequired instead of GateBlocked. This means callers can submit override/recovery evidence. Neither AGENTS.md nor cli-usage.md mentions this, yet it's implemented in both the dispatcher and the advancement engine.

2. **The dispatcher (`dispatch_next` in `src/cli/next.rs`) and the advancement engine (`advance_until_stop` in `src/engine/advance.rs`) have overlapping classification logic**: Both check terminal, gates, integration, and accepts in similar order. The dispatcher is used for `--to` directed transitions (which skip the advancement loop), while `advance_until_stop` handles the normal path. The handler in `mod.rs` maps `AdvanceResult.stop_reason` to `NextResponse` directly, bypassing `dispatch_next` for the normal path. This dual-path design means the response contract must account for both code paths.

3. **`advanced: false` on Terminal is a real case**: When you call `koto next` on an already-done workflow, you get `action: "done"` with `advanced: false`. But AGENTS.md only shows `advanced: true` for Terminal. This could confuse callers who check `advanced` to determine if they caused the completion.

## Open Questions

1. **Should `advanced` be renamed?** Issue #102 suggests it misleads callers. Alternatives like `transitioned`, `state_changed`, or `moved` might be clearer. This is a breaking API change.

2. **Should the gate-with-evidence fallback behavior be exposed in the response?** Currently, callers seeing `EvidenceRequired` after a gate failure have no way to know gates failed. Should the response include both `expects` and `blocking_conditions` in this case?

3. **Should `--with-data` responses have a distinct action type?** Currently, after evidence submission, callers get a standard response for whatever state the engine lands on. There's no indication in the response that evidence was accepted.

4. **Are there other callers besides AGENTS.md consumers?** If other tools parse `koto next` output programmatically, the contract specification affects them too.

5. **Should the response contract be versioned separately from the CLI version?** This would let the contract evolve independently of feature additions.

## Summary

AGENTS.md documents five response shapes with reasonable action guidance, but it never provides a standalone definition of `advanced` -- the closest attempt (in README.md) incorrectly limits it to `--with-data` and `--to`, omitting auto-advancement. The gate-with-evidence fallback, `advanced: true` combined with `expects` present, and `advanced: false` on Terminal are all real code paths with zero documentation, confirming that the response contract has meaningful gaps that could lead to the caller misinterpretation described in issue #102. The biggest open question is whether `advanced` should be renamed to something less ambiguous (like `transitioned`) or whether better documentation alone can fix the confusion.
