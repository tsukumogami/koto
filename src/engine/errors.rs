use thiserror::Error;

/// Typed engine errors for use at the CLI boundary.
///
/// Persistence functions return `anyhow::Result` for internal flexibility.
/// CLI commands should convert `anyhow::Error` to an `EngineError`
/// variant when they need to present a specific user-facing message.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("state not found: {0}")]
    StateNotFound(String),

    #[error("empty event log")]
    EmptyLog,

    #[error("parse error: {0}")]
    ParseError(String),

    #[error("state file corrupted: {0}")]
    StateFileCorrupted(String),
}
