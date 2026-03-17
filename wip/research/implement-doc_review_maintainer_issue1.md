# Maintainer Review: Issue 1 -- Response Types and Serialization

## File: `src/cli/next_types.rs`

### Finding 1: Magic strings for `condition_type` and `status` (Advisory)

**Lines throughout the file and tests.**

`BlockingCondition.condition_type` is always `"command"` and `BlockingCondition.status` is one of `"failed"`, `"timed_out"`, or `"error"`. These are bare strings with no constants or enum. The gate evaluator in Issue 3 will independently produce these same strings. If someone types `"timedout"` instead of `"timed_out"` in gate.rs, the bug is silent -- tests pass in both modules but the contract breaks.

`condition_type` could be a unit enum (it's always `"command"` today, but the design says so explicitly). `status` maps directly to `GateResult` variants and could be an enum with `Serialize`. This is advisory because Issue 3 hasn't landed yet and the implementer can address it there -- but worth flagging as a coordination risk.

### Finding 2: `serialize_map` size hints are cosmetic but could mislead (Advisory)

**Lines 55, 70, 87, 104, 115.**

The `Some(N)` argument to `serialize_map` is a hint to the serializer about how many entries to expect. For `GateBlocked` (line 70) and `Integration` (line 87), the hint is `Some(7)`, which matches the field count. For `EvidenceRequired` (line 55), it's `Some(6)`. These are correct today. But if someone adds a field to a variant and forgets to bump the hint, behavior is still correct (serde handles it gracefully) -- but the mismatch signals "something is wrong" to a reader who notices. Not blocking because serde ignores inaccurate hints, but worth a comment like `// hint only, not enforced` to prevent someone from treating it as a correctness constraint.

### Finding 3: Code is clear and well-structured (Positive)

The custom `Serialize` implementation follows the established `Event` pattern in `src/engine/types.rs`. The doc comment on `NextResponse` (lines 6-11) explicitly references the design's field presence table, giving the next developer a direct pointer to the specification. Test names describe the variant and scenario being tested. Field-level `serde` annotations (`rename`, `skip_serializing_if`) are idiomatic and match the rest of the codebase.

The enum-per-variant approach means adding a sixth response type produces a compiler error at the `match self` in the serializer -- the next developer can't forget to handle it.

### Finding 4: Tests verify absence of fields thoroughly (Positive)

Each test checks both that expected fields are present AND that fields marked "no" in the design are absent (e.g., `assert!(json.get("blocking_conditions").is_none())`). This catches the specific bug class the design doc warns about: field omission/inclusion errors in manual serialization.

## File: `src/cli/mod.rs`

### Finding 5: Module declaration is minimal (Positive)

Line 1: `pub mod next_types;` -- clean module wiring with no unnecessary re-exports.

## Summary

No blocking findings. The implementation is clean, follows established patterns, and has thorough tests that verify the field presence contract. Two advisory items: bare strings for `BlockingCondition.status` and `condition_type` create a coordination risk with Issue 3's gate evaluator (the two modules will need to agree on exact string values without a shared type), and the `serialize_map` size hints could get a brief comment to prevent misinterpretation.
