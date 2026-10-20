use serde::Serialize;

#[derive(Serialize)]
pub struct BuildInfo {
    pub version: &'static str,
    pub commit: &'static str,
    pub built_at: &'static str,
    /// The decision-record line format this build writes and reads. A koto
    /// without the host-level decision store (`koto decisions record
    /// --kind`, `show`, `reverse`, `import`, the host-wide `list` filters)
    /// leaves the field out, so a plugin can detect the store with `koto
    /// version --json` before calling it.
    pub decision_store: u32,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("KOTO_VERSION"),
        commit: env!("KOTO_GIT_HASH"),
        built_at: env!("KOTO_BUILD_DATE"),
        decision_store: crate::decision_store::DECISION_FORMAT_VERSION,
    }
}
