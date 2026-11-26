//! The `decider-check` gate's spec and its compile-time checks, which live
//! in koto-session-reader's `template::decider_check` and are re-exported
//! here at their old path.
//!
//! The checks there refuse a spec with E-DECIDER-CHECK-FIELD,
//! E-DECIDER-CHECK-UNITS, E-DECIDER-CHECK-RULE, E-DECIDER-CHECK-LABEL,
//! E-DECIDER-CHECK-BUDGET and E-DECIDER-CHECK-THRESHOLD, each documented in
//! docs/reference/error-codes.md.

pub use koto_session_reader::template::decider_check::*;
