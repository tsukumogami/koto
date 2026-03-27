pub mod check;
pub mod html;
pub mod mermaid;

pub use check::{check_freshness, CheckResult};
pub use html::generate_html;
pub use mermaid::to_mermaid;
