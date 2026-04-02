# Architecture Review: DESIGN-gate-backward-compat

Phase 6 architecture review — Feature 4 (backward compatibility) of the gate-transition contract.
Date: 2026-04-02

## Scope

Review of DESIGN-gate-backward-compat.md against:
- `src/template/compile.rs` (`SourceFrontmatter`, `compile()`)
- `src/template/types.rs` (`CompiledTemplate`, `validate()`, `validate_gate_reachability()`)
- `src/engine/advance.rs` (gate evaluation and evidence merge, lines 298–445)
- `src/cli/mod.rs` (`handle_init()`, lines 951–1070)

---

## Current state baseline

`legacy_gates` does not exist in any form in the codebase. The design is purely additive.

`SourceFrontmatter` (compile.rs:14–27): no `legacy_gates` field.

`CompiledTemplate` (types.rs:22–32): no `legacy_gates` field.

`compile()` constructs `CompiledTemplate` at lines 265–273 with a struct literal. `validate()` is called immediately after construction (line 277). Both run on every `compile_cached()` cache miss.

`validate()` runs a per-state loop (lines 295–526) covering D2 (override_default validation), accepts-field validation, evidence routing (D3), variable references, and default_action checks. After the per-state loop, a separate loop runs D4 via `validate_gate_reachability()` (lines 531–533).

`has_gates_routing` is computed at advance.rs:395–403, inside `if any_failed { ... }`, which is itself inside `if !template_state.gates.is_empty() { ... }` (line 304). The evidence merge (lines 437–442) runs unconditionally — it is not guarded.

`handle_init()` calls `compile_cached()` then `load_compiled_template()`. On a cache hit, `compile_cached()` returns the cached path; `load_compiled_template()` does a bare `serde_json::from_str()` with no `validate()` call (cli/mod.rs:358–363). On a cache miss, `compile_cached()` calls `compile()` which calls `validate()`.

---

## Findings

### Finding 1 — Three-site change described as two (Blocking: implementation completeness)

The design says: add `legacy_gates: Option<bool>` to `SourceFrontmatter`, add `legacy_gates: bool` to `CompiledTemplate`, and have `compile()` read and store it.

In practice this is three co-located changes:
1. `SourceFrontmatter` struct field (compile.rs:14–27)
2. `CompiledTemplate` struct field (types.rs:22–32)
3. The `CompiledTemplate` constructor at compile.rs:265–273 — the struct literal must include `legacy_gates: fm.legacy_gates.unwrap_or(false)`

The Rust compiler will catch the omission of (3) as a missing-field error, so this cannot silently slip through, but an implementer working from the design doc alone might not anticipate needing to touch the constructor.

**No code fix needed to the design; the implementation plan should call out all three sites explicitly in Phase 1 deliverables.**

---

### Finding 2 — D5 check location within validate() is unambiguous and correct (No gap)

The design says D5 fires "after D2/D3 checks, before D4."

In the current code structure, D2/D3 run inside the per-state loop (lines 295–526) and D4 runs in a separate loop after (lines 531–533). D5 — scanning each state for gates without `gates.*` routing — naturally belongs inside the per-state loop, consistent with D2/D3. Any D5 error will cause `validate()` to return early before reaching line 531, which is what the design intends.

The design's phrasing is accurate. Implementers should place D5 inside the per-state loop, after the existing gate-type checks and D2 override validation (around line 430), before or after `validate_evidence_routing()` — either position is correct since D5 is orthogonal to evidence routing validation.

**No gap.**

---

### Finding 3 — D4 vacuous pass for legacy states: early return is required for AC10, not the reachability check (Advisory)

The design says D4 suppression is achieved via an early return in `validate_gate_reachability()` when `self.legacy_gates` is true.

A legacy state (gates present, no `gates.*` when-clause references) will have zero pure-gate transitions. `validate_gate_reachability()` already returns `Ok(())` early at line 570–573 when `pure_gate_transitions.is_empty()`. So the reachability check itself already passes vacuously for legacy states.

The real motivation for the early return is the AC10 warning loop (lines 591–607), which iterates all gate schema fields and emits `eprintln!` warnings for any field not referenced in a `when` clause. For a legacy state, no fields are referenced, so AC10 fires for every field of every gate. The early return suppresses this correctly.

The design's stated outcome is right ("suppress D4 unreferenced-field warnings") but the mechanism explanation omits that the reachability check itself is already vacuously safe. This matters for code review: reviewers may question why the early return is needed if the reachability check "already passes." A comment in the code should note that the early return is specifically to suppress AC10 warnings, not to prevent a reachability failure.

**Advisory: add a code comment explaining the early return targets AC10 suppression.**

---

### Finding 4 — `koto init` warning: reachable on both cache paths, but "init always succeeds" framing is incorrect (Blocking: documentation accuracy)

The design says: "`koto init` always succeeds — it doesn't run the strict gate validation."

This is incorrect. On a cache miss, `compile_cached()` calls `compile()` which calls `validate()`. If a template has gates without `gates.*` routing and no `legacy_gates: true`, D5 fires and `compile_cached()` returns an error, causing `handle_init()` to exit with an error. This is the first time a user points `koto init` at such a template.

On a cache hit, `load_compiled_template()` does only a `serde_json::from_str()` — no `validate()`. But a cache hit means the template was already successfully compiled (D5 passed on the miss), so the cache-hit path cannot encounter a D5 error.

The correct statement: `koto init` fails at compile time if D5 fires. After that, repeated inits against the same template hit the cache and never re-run validation. The "init always succeeds" framing describes the cache-hit path, not the general case.

**Impact for the Phase 2 integration test:** The test spec says "koto init with a `legacy_gates: true` template exits 0 and emits the warning." This is correct only because the template carries `legacy_gates: true`, which suppresses D5. A template without the field and with legacy gates will fail `koto init` on first use — which is the intended behavior. The test should not accidentally test with an undecorated legacy template expecting exit 0.

**Fix:** Correct the design text to: "`koto init` succeeds when the template is valid; a template without `legacy_gates: true` that uses legacy gate behavior will fail compilation (D5) whether invoked via `koto init` or `koto template compile`."

---

### Finding 5 — `legacy_gates` field readable in `handle_init()` after cache deserialization (No gap)

The `CompiledTemplate` struct uses serde for both serialization (to cache) and deserialization (from cache). With `#[serde(default)]` on `legacy_gates: bool`, existing cached JSON files (which lack the field) will deserialize with `legacy_gates = false`. Templates compiled after the field is added will serialize `legacy_gates: true` into the JSON, and `load_compiled_template()` will deserialize it correctly.

The warning in `handle_init()` that checks `compiled.legacy_gates` is reachable and correct on both the miss path (freshly compiled) and the hit path (deserialized from cache). **No gap.**

The `eprintln!` warning should fire before the final `println!` JSON response in `handle_init()` (currently the last output, line 1062–1068). Sending to stderr before stdout avoids interleaving issues for callers that capture both streams. The design does not specify ordering. **Advisory: note the ordering in the implementation.**

---

### Finding 6 — `has_gates_routing` hoist: correct, one initialization detail to watch (Advisory)

The design says `has_gates_routing` is currently computed at lines 395–403 inside the `if any_failed` block and must be hoisted to before the evidence merge (~line 437).

Confirmed. After hoisting, `has_gates_routing` must be initialized to `false` before the outer `if !template_state.gates.is_empty()` block (line 304), so it is defined at the evidence merge even when the state has no gates. If hoisted only to the top of the gate block (inside line 304), the variable would be uninitialized at the merge step for gateless states. The merge guard `if !gate_evidence_map.is_empty() && has_gates_routing` handles this correctly in any case — an empty `gate_evidence_map` already prevents the insert for gateless states — but Rust requires the variable to be initialized regardless.

The design's code snippet is correct; the prose does not mention the `false` default initialization. **Advisory: call this out explicitly.**

The behavioral change to the existing `if any_failed` path is zero — the boolean is read at the same point (line 404) with the same value. The hoist is safe.

---

### Finding 7 — Evidence exclusion: `GateBlocked` and `GateEvaluated` paths unaffected (No gap)

The design claims gate output for `GateBlocked` and `GateEvaluated` events uses `gate_results` directly, not the merged evidence map.

Confirmed. `gate_results` is populated at lines 352–369 (inside the evaluate loop). `GateEvaluated` events are appended at lines 360–368, before the evidence merge. `GateBlocked` uses `gate_results` (via `failed_gate_results`) at lines 405–409 and 478–485. Neither path touches `merged` or `gate_evidence_map` after the proposed guard. **No gap.**

---

## Summary

### Blocking findings

| # | Finding | Impact |
|---|---------|--------|
| B1 | Three-site change: `SourceFrontmatter`, `CompiledTemplate`, and the `CompiledTemplate` constructor in `compile()` all need updating. Phase 1 deliverables should name all three. | Implementation may miss the constructor if working only from the struct change list. |
| B2 | "init always succeeds" is incorrect: `koto init` runs full compilation including D5 on cache misses. Phase 2 test spec must use a `legacy_gates: true` template, not a bare legacy template. | Phase 2 integration test would be misspecified; a bare legacy template fails `koto init`. |

### Advisory findings

| # | Finding |
|---|---------|
| A1 | D4 early return targets AC10 warning suppression, not reachability correctness; add a code comment to preempt reviewer confusion. |
| A2 | `has_gates_routing` needs a `false` default before the gate block after hoisting; design code snippet is correct but prose omits this. |
| A3 | `eprintln!` warning in `handle_init()` should fire before the final `println!` JSON; specify ordering in the design. |

### Assessment

The design is implementable as written. The structural decisions (frontmatter field, evidence exclusion guard, D4 suppression) are all correct and map cleanly to the existing code structure. No proposed change bypasses an existing layer or introduces a parallel pattern. The two blocking findings are documentation and test-spec accuracy issues, not architectural flaws — fixing them requires a sentence correction in the design doc and an explicit three-site list in the Phase 1 deliverables, not redesign.
