//! koto's native Claude Code `/workflows` surface (the initial single-session render).
//!
//! koto renders a session as a native entry in Claude Code's `/workflows`
//! screen by producing the artifacts that screen already reads -- there is no
//! skill, reader, or parallel surface (the settled surface decision). On each
//! state-commit a participating session writes its own `koto-<uuid>.json` into
//! the `/workflows` directory a hosting Claude Code session published.
//!
//! Module layout:
//! - [`contract`]  -- the extensible on-disk file shape.
//! - [`project`]   -- the minimal projection derived from koto's read seam.
//! - [`discover`]  -- context-store publish + the nearest-published-ancestor walk.
//! - [`materialize`] -- the commit-funnel entry point tying it together.

pub mod contract;
pub mod discover;
pub mod materialize;
pub mod project;

pub use contract::{
    workflow_filename, Phase, ProgressNode, RenderStatus, WorkflowFile, CONTRACT_VERSION,
};
pub use discover::{publish_location, resolve_publish_location, PUBLISH_LOCATION_KEY};
pub use materialize::{materialize_after_commit, WORKFLOWS_DIR_ENV};
pub use project::{
    derive_enriched_projection, derive_minimal_projection, ordered_phases, per_state_outcomes,
    EnrichedProjection, PhaseEntry, PhaseStatus, Projection,
};
