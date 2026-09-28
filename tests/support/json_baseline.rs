//! Compare a JSON value against a recorded baseline while allowing new
//! optional fields.
//!
//! Include it from a test file with
//!
//! ```ignore
//! #[path = "support/json_baseline.rs"]
//! mod json_baseline;
//! ```
//!
//! The compatibility rule for `koto next` responses and log events is that a
//! later koto may add optional keys but must never change or remove one an
//! older reader already sees. [`strip_added_keys`] turns that rule into a
//! plain equality check: delete from the current value every object key the
//! baseline lacks, then compare. A changed value, a removed key, a changed
//! type or a changed array length all survive the strip and fail the
//! comparison; only a key the baseline never had disappears.

#![allow(dead_code)]

use serde_json::Value;

/// Delete from `actual`, recursively, every object key that is absent from
/// the matching object in `baseline`.
///
/// Objects are matched key by key and arrays element by element (up to the
/// shorter length, so an added or dropped element still shows up in the
/// comparison that follows). Where the two values have different types
/// nothing is stripped below that point. Scalars are left alone.
pub fn strip_added_keys(actual: &mut Value, baseline: &Value) {
    match (actual, baseline) {
        (Value::Object(a), Value::Object(b)) => {
            a.retain(|k, _| b.contains_key(k));
            for (k, v) in a.iter_mut() {
                strip_added_keys(v, &b[k]);
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (av, bv) in a.iter_mut().zip(b.iter()) {
                strip_added_keys(av, bv);
            }
        }
        _ => {}
    }
}

/// Replace every occurrence of each `(needle, token)` pair inside every
/// string in `value`, recursively. Used to swap a tempdir root for a fixed
/// token so a recorded fixture holds no machine-specific path. Longer
/// needles should come first when one is a prefix of another.
pub fn tokenize_strings(value: &mut Value, pairs: &[(String, &str)]) {
    match value {
        Value::String(s) => {
            for (needle, token) in pairs {
                if !needle.is_empty() && s.contains(needle.as_str()) {
                    *s = s.replace(needle.as_str(), token);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                tokenize_strings(item, pairs);
            }
        }
        Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                tokenize_strings(v, pairs);
            }
        }
        _ => {}
    }
}
