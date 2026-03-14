use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("state not found: {0}")]
    StateNotFound(String),

    #[error("empty event log")]
    EmptyLog,

    #[error("parse error: {0}")]
    ParseError(String),
}
