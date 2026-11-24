//! The result of checking a session's recorded `template_source_dir`.
//! koto's `engine::template_source_status` runs the check.

use std::path::PathBuf;

/// Result of checking whether a recorded `template_source_dir` still
/// resolves on the current machine.
///
/// `machine_id` is a best-effort, non-authoritative label (see
/// [`current_machine_id`]) identifying which machine performed the
/// check -- it is not compared against any stored "creator machine"
/// value, since none exists in [`StateFileHeader`] today.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TemplateSourceStatus {
    /// The recorded `template_source_dir` path, unchanged from the
    /// header (or from whatever `Option<&Path>` the caller supplied).
    pub path: PathBuf,

    /// Whether `path` exists on this machine, per `Path::exists()`.
    ///
    /// This is a bare existence check, not an "is this a directory"
    /// check: a `template_source_dir` that resolves to a regular file
    /// still reports `true` here. Stronger validation is out of scope
    /// for this read-only, informational signal -- see
    /// DESIGN-orphaned-session-detection.md's Decisions Already Made.
    pub exists: bool,

    /// Best-effort identifier for the machine the check ran on. `None`
    /// when no usable identifier could be derived. See
    /// [`current_machine_id`] for how this is computed.
    pub machine_id: Option<String>,
}
