// Template layer: compiled template loading, compilation, and validation.
// Implemented in Issue 2.
pub use koto_session_reader::template::assignments;
pub mod compile;
pub use koto_session_reader::template::decider;
pub mod decider_check;
pub use koto_session_reader::template::result_map;
#[cfg(test)]
mod result_map_compile_tests;
pub mod types;
mod validate;
pub mod variables;

pub(crate) use compile::split_frontmatter;
