//! Session listing records and the state file's name. koto's `session`
//! module holds the backends that list and write sessions.

pub mod links;
pub mod local;
pub mod row;
pub mod validate;
pub mod version;

use std::path::PathBuf;

use crate::engine::template_source_status::TemplateSourceStatus;

/// Information about an existing session.
#[derive(serde::Serialize)]
pub struct SessionInfo {
    /// Session identifier (same as the workflow name).
    pub id: String,

    /// RFC 3339 UTC timestamp of session creation (from state file header).
    pub created_at: String,

    /// SHA-256 hash of the compiled template (from state file header).
    pub template_hash: String,

    /// Name of the parent workflow, if this workflow was created as a child.
    pub parent_workflow: Option<String>,

    /// Whether the session's recorded `template_source_dir` (if any) still
    /// resolves on this machine, and on whose.
    ///
    /// Populated from the state file header already read into memory while
    /// building this `SessionInfo` -- backends never perform extra I/O just
    /// to fill this field.
    ///
    /// `None` is ambiguous by design and conflates two distinct cases that
    /// this field does not (and is not required to) distinguish between:
    ///
    /// - The session's header never recorded a `template_source_dir` --
    ///   there is nothing to check.
    /// - No header was available to check at all. This is the case for
    ///   `CloudBackend`'s remote-only placeholder rows (a session that
    ///   exists in S3 but has not been synced locally): there is no header
    ///   in scope, so the field is unconditionally `None` there regardless
    ///   of what the session's real (remote) header might say once synced.
    ///
    /// This ambiguity is a known, accepted limitation -- not a bug to fix
    /// here. See DESIGN-orphaned-session-detection.md for the rationale.
    pub template_source_status: Option<TemplateSourceStatus>,

    /// The directory the session's ticks run in, from the header
    /// (`execution_dir`). `None` when the header records none, and for the
    /// cloud backend's remote-only placeholder rows, which have no header.
    pub execution_dir: Option<PathBuf>,

    /// The header's `session_id`. `None` for a header written before the
    /// field existed, and for remote-only placeholder rows.
    pub session_id: Option<String>,

    /// The header's `kind` (`anchor`, `anchor-close`), omitted for an
    /// ordinary session. `koto session list` shows it; the workflow views
    /// leave out every session that carries a kind koto knows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl SessionInfo {
    /// What the session's header says it is.
    pub fn session_kind(&self) -> crate::engine::types::SessionKind {
        crate::engine::types::SessionKind::of(self.kind.as_deref())
    }
}

/// Return the state file name for a given session ID.
///
/// The naming convention (`koto-<id>.state.jsonl`) is a free function
/// because it doesn't vary across backends.
pub fn state_file_name(id: &str) -> String {
    format!("koto-{}.state.jsonl", id)
}
