//! koto's native Claude Code `/workflows` surface (Feature 1: walking skeleton).
//!
//! koto renders a session as a native entry in Claude Code's `/workflows`
//! screen by producing the artifacts that screen already reads -- there is no
//! skill, reader, or parallel surface (see
//! `docs/decisions/ADR-koto-native-workflows-rendering.md`, upstream). On each
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

pub use contract::{workflow_filename, RenderStatus, WorkflowFile, CONTRACT_VERSION};
pub use discover::{publish_location, resolve_publish_location, PUBLISH_LOCATION_KEY};
pub use materialize::{materialize_after_commit, WORKFLOWS_DIR_ENV};
pub use project::{derive_minimal_projection, Projection};
