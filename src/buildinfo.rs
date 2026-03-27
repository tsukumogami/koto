use serde::Serialize;

#[derive(Serialize)]
pub struct BuildInfo {
    pub version: &'static str,
    pub commit: &'static str,
    pub built_at: &'static str,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("KOTO_VERSION"),
        commit: env!("KOTO_GIT_HASH"),
        built_at: env!("KOTO_BUILD_DATE"),
    }
}
