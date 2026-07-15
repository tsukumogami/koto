#[cfg(unix)]
pub mod action;
pub mod buildinfo;
pub mod cache;
pub mod cli;
pub mod config;
pub mod discover;
pub mod engine;
pub mod export;
#[cfg(unix)]
pub mod gate;
pub mod session;
pub mod template;
pub mod workflows_surface;

// ===== Stage 1 frozen public surface (Issue 19 / Decision 5) =====
//
// The re-exports below pin the canonical paths bunki BK2 imports
// against. Breaking changes to these names — renames, removals, or
// type-shape changes — require a 6-week deprecation window and a
// migration tool per `docs/STABILITY.md`. Additive evolution
// (new fields on `StateFileHeader`, new variants on `EventPayload`,
// new error variants on `koto::error::Error`) is permitted in minor
// releases per the rules documented in the stability doc.
//
// The eight types are intentionally re-exported AT `koto::engine::types`
// (not under a separate stability module) so a downstream `use
// koto::engine::types::*;` import is the canonical access pattern.
// `derive_state_from_log` is the one exception: it lives in
// `engine::persistence` but is aliased into `engine::types` via
// `pub use crate::engine::persistence::derive_state_from_log;`
// inside `src/engine/types.rs`. The alias decouples the canonical
// export path from the implementation module so a future refactor
// that moves the function does not surface as a visible-to-bunki
// change.

/// Stage 1 frozen error surface — re-exports
/// [`crate::engine::errors::EngineError`] as `koto::error::Error` per
/// Decision 5.
///
/// The `Error` alias is the canonical name in the public stability
/// contract; the internal `EngineError` shape stays inside the engine
/// module so a future refactor can rename it without breaking
/// downstream consumers. New variants on `Error` are permitted in
/// minor releases per `docs/STABILITY.md`.
pub mod error {
    /// Stage 1 frozen error type for the koto crate's public surface.
    ///
    /// See [`crate::engine::errors::EngineError`] for the underlying
    /// definition. This re-export is the canonical name bunki BK2
    /// and other downstream consumers import.
    pub use crate::engine::errors::EngineError as Error;
}
