// Template layer: compiled template loading, compilation, and validation.
// Implemented in Issue 2.
pub mod assignments;
pub mod compile;
pub mod result_map;
pub mod types;
pub mod variables;

pub(crate) use compile::split_frontmatter;
