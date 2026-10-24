//! The short-retention guard on bulk removal under the cloud backend.
//!
//! A retention below an hour releases almost every finished session, and
//! under the cloud backend each removal also deletes the session's copy in
//! the bucket. `koto session prune --apply` without `--plan` and
//! `koto workspace prune --yes` remove what they work out with no preview
//! anyone read, so with such a retention they refuse unless given
//! `--confirm-short-retention`. The preview-then-apply forms are left alone:
//! someone saw the set. The guard reads the backend the command resolved,
//! never where `HOME` points, since the cloud backend can come from the
//! environment alone.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::session::hierarchy::{RetentionSettings, Setting};
use crate::session::{Backend, SessionBackend};

/// Below this, a retention setting, or the age `--older-than` selects from,
/// counts as short.
pub(crate) const FLOOR: Duration = Duration::from_secs(60 * 60);

/// The error code both verbs refuse with.
pub(crate) const CODE: &str = "prune_short_retention";

/// The removal settings below [`FLOOR`]: `retention.keep` and
/// `retention.idle_period`. `retention.stale_after` only reports an anchor
/// stale, and releases nothing.
pub(crate) fn short_settings(settings: &RetentionSettings) -> Vec<&Setting> {
    [&settings.keep, &settings.idle_period]
        .into_iter()
        .filter(|s| s.value < FLOOR)
        .collect()
}

/// `retention.keep is 1s (from KOTO_RETENTION_KEEP)`, joined with `and`.
pub(crate) fn settings_text(short: &[&Setting]) -> String {
    short
        .iter()
        .map(|s| format!("{} is {} (from {})", s.name, s.raw.trim(), s.origin_label()))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// The store a removal acts on, as a refusal and the apply line name it.
pub(crate) struct Store {
    pub kind: String,
    pub bucket: Option<String>,
    pub path: PathBuf,
}

impl Store {
    pub(crate) fn of(backend: &Backend) -> Self {
        match backend.store_identity() {
            Some(id) => Store {
                kind: id.kind,
                bucket: id.bucket,
                path: id.base,
            },
            None => Store {
                kind: if backend.is_cloud() { "cloud" } else { "local" }.to_string(),
                bucket: None,
                path: backend.local_base_dir().to_path_buf(),
            },
        }
    }

    /// `the cloud store at <path> (bucket <name>)` or `the local store at
    /// <path>`.
    pub(crate) fn describe(&self) -> String {
        match &self.bucket {
            Some(bucket) => format!(
                "the {} store at {} (bucket {bucket})",
                self.kind,
                self.path.display()
            ),
            None => format!("the {} store at {}", self.kind, self.path.display()),
        }
    }

    /// The fields a refusal carries: `backend`, `bucket` and `store`.
    pub(crate) fn fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        fields.insert("backend".into(), json!(self.kind));
        fields.insert("bucket".into(), json!(self.bucket));
        fields.insert("store".into(), json!(self.path));
        fields
    }
}

/// The fields a refusal carries for the short settings and the count:
/// `retention` (each setting, its value and where it came from) and
/// `would_remove`.
pub(crate) fn refusal_fields(short: &[&Setting], store: &Store, would_remove: usize) -> Map<String, Value> {
    let mut fields = store.fields();
    fields.insert(
        "retention".into(),
        Value::Array(
            short
                .iter()
                .map(|s| json!({"setting": s.name, "value": s.raw.trim(), "from": s.origin_label()}))
                .collect(),
        ),
    );
    fields.insert("would_remove".into(), json!(would_remove));
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(keep: u64, idle: u64) -> RetentionSettings {
        RetentionSettings::from_durations(
            Duration::from_secs(keep),
            Duration::from_secs(idle),
            Duration::from_secs(keep.max(idle) + 1),
        )
    }

    #[test]
    fn a_setting_is_short_only_below_an_hour() {
        let names = |s: &RetentionSettings| {
            short_settings(s)
                .iter()
                .map(|s| s.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&settings(1, 1)), ["retention.keep", "retention.idle_period"]);
        assert_eq!(names(&settings(3599, 7200)), ["retention.keep"]);
        assert_eq!(names(&settings(7200, 59)), ["retention.idle_period"]);
        assert!(names(&settings(3600, 3600)).is_empty());
    }

    #[test]
    fn the_text_names_each_short_setting_and_its_origin() {
        let s = settings(1, 1);
        assert_eq!(
            settings_text(&short_settings(&s)),
            "retention.keep is 1s (from default) and retention.idle_period is 1s (from default)"
        );
    }
}
