//! Probe: mount koto's own reader source files unchanged in a crate that
//! declares no storage or network dependency, and let the compiler say
//! which items cross the boundary.
#![allow(dead_code, unused_imports)]

pub mod engine {
    #[path = "../../../src/engine/errors.rs"]
    pub mod errors;
    #[path = "../../../src/engine/jsonl_append.rs"]
    pub mod jsonl_append;
    #[path = "../../../src/engine/leg_pointer.rs"]
    pub mod leg_pointer;
    #[path = "../../../src/engine/persistence.rs"]
    pub mod persistence;
    #[path = "../../../src/engine/terminal_result.rs"]
    pub mod terminal_result;
    #[path = "../../../src/engine/types.rs"]
    pub mod types;
}

#[path = "../../src/findings.rs"]
pub mod findings;
#[path = "../../src/redact.rs"]
pub mod redact;

pub mod decider {
    #[path = "../../../src/decider/check.rs"]
    pub mod check;
    #[path = "../../../src/decider/evaluate.rs"]
    pub mod evaluate;
    #[path = "../../../src/decider/record.rs"]
    pub mod record;
    #[path = "../../../src/decider/request.rs"]
    pub mod request;
    #[path = "../../../src/decider/types.rs"]
    pub mod types;
}

pub mod template {
    #[path = "../../../src/template/assignments.rs"]
    pub mod assignments;
    #[path = "../../../src/template/decider.rs"]
    pub mod decider;
    #[path = "../../../src/template/decider_check.rs"]
    pub mod decider_check;
    #[path = "../../../src/template/result_map.rs"]
    pub mod result_map;
    #[path = "../../../src/template/types.rs"]
    pub mod types;
}

pub mod session {
    #[path = "../../../src/session/row.rs"]
    pub mod row;
    #[path = "../../../src/session/validate.rs"]
    pub mod validate;
    #[path = "../../../src/session/version.rs"]
    pub mod version;
}

pub mod source;

pub mod cli {
    #[path = "../../../src/cli/dashboard_data.rs"]
    pub mod dashboard_data;
}

/// Visibility seam: koto keeps `compute_elapsed_since` crate-private; the
/// trial exposes it through a wrapper rather than widen koto's own items.
pub fn elapsed_since(timestamp: &str) -> std::time::Duration {
    crate::cli::dashboard_data::compute_elapsed_since(timestamp)
}
