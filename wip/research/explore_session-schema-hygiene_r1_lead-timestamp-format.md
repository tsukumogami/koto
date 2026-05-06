# Lead: Timestamp format

## Findings

`now_iso8601()` in `src/engine/types.rs` (lines 731-756) generates timestamps as `YYYY-MM-DDTHH:MM:SSZ` using only integer arithmetic, calling `.as_secs()` on `SystemTime::now().duration_since(UNIX_EPOCH)`. No external time crate is used.

The `Event` struct (line 380-393) has `pub timestamp: String`. All event payloads that embed a timestamp (e.g., `GateEvaluated`, `SchedulerRan`, `BatchFinalized`) also use `String`. The `StateFileHeader.created_at` field is also a `String`.

Sub-second access is available via `duration.subsec_millis()` (u32, 0-999) from the same `SystemTime` call — no external dependency required.

RFC 3339 fractional seconds (`2026-05-06T14:30:00.123Z`) are a strict superset of the current format; readers that parse RFC 3339 tolerate both forms.

## Implications

The change is to `now_iso8601()` — a single function. All callers automatically get millisecond precision. No field renames required. The PRD should specify: format is RFC 3339 with 3-digit fractional second, field name `timestamp` unchanged.

## Surprises

None. The implementation is simpler than expected — one function to change, no external crate needed.

## Open Questions

None.

## Summary

Timestamps are whole-second RFC 3339 strings from `now_iso8601()` using `as_secs()`. Millisecond precision requires adding `subsec_millis()` to the same call — no new dependencies. All consumers tolerate RFC 3339 with or without fractional seconds.
