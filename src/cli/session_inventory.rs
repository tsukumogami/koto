//! The CLI's path to a session's standing. The inventory itself lives in
//! [`crate::session::inventory`], so the retention engine in
//! `src/session` can use it without depending on the CLI; this module
//! re-exports it for the CLI callers that already name it here.

pub use crate::session::inventory::{
    inspect, inspect_full, normalize_filter, under_dir, HeaderFields, Inspection, SessionStatus,
    TemplateCache,
};
