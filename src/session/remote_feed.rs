//! The dashboard's read of sessions other machines pushed to the bucket.
//!
//! [`RemoteReader::scan`] lists the bucket's top level for workspace
//! prefixes, lists each one with the delimiter `/ctx/`, which returns every
//! session's top-level objects (state file, `summary.json`, markers) and
//! folds its whole context tree into one common prefix, and reads each
//! session's [`SessionSummary`]. It never fetches a state file, so showing
//! a remote row never costs the size of its history, and it reads nothing
//! under `ctx/`. The reader has no write method: it lists and gets, and
//! works with keys that grant only those.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use s3::Bucket;

use crate::session::cloud::{is_workspace_prefix, list_retrying};
use crate::session::state_file_name;
use crate::session::summary::{is_internal_name, SessionSummary, MAX_SUMMARY_BYTES, SUMMARY_FILE};

/// The delimiter that folds a session's context tree into one entry.
pub const CTX_DELIMITER: &str = "/ctx/";

/// How long one bucket request may take before it fails.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How many workspace listings are in flight at once.
const FANOUT: usize = 64;

/// One session read from another machine's summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRow {
    /// The workspace id (16-hex prefix) the session's copy lives under.
    pub workspace: String,
    /// The summary, checked against the key it was read from.
    pub summary: SessionSummary,
}

/// One pass over the bucket.
#[derive(Debug, Default)]
pub struct RemoteSnapshot {
    /// Every session read, in workspace then name order.
    pub rows: Vec<RemoteRow>,
    /// Why the pass couldn't read some or all of the bucket, when it
    /// couldn't.
    pub failure: Option<String>,
    /// Sessions whose summary was listed but couldn't be read or parsed.
    pub unreadable: usize,
}

/// A session directory as one `/ctx/`-delimited listing shows it: its
/// top-level objects by file name, with each one's listed size.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SessionListing {
    /// Top-level file name to listed size.
    pub objects: BTreeMap<String, u64>,
}

/// Group a workspace listing's objects into session directories.
///
/// `keys` are `(key, size)` pairs from listings of `<workspace>/` with the
/// [`CTX_DELIMITER`]; only objects directly inside a session directory
/// (`<workspace>/<name>/<file>`) count, so nothing under a deeper path
/// becomes a row fact.
pub fn group_sessions<'a>(
    workspace: &str,
    keys: impl IntoIterator<Item = (&'a str, u64)>,
) -> BTreeMap<String, SessionListing> {
    let prefix = format!("{workspace}/");
    let mut sessions: BTreeMap<String, SessionListing> = BTreeMap::new();
    for (key, size) in keys {
        let Some(rest) = key.strip_prefix(&prefix) else {
            continue;
        };
        let Some((name, file)) = rest.split_once('/') else {
            continue;
        };
        if name.is_empty() || file.is_empty() || file.contains('/') {
            continue;
        }
        sessions
            .entry(name.to_string())
            .or_default()
            .objects
            .insert(file.to_string(), size);
    }
    sessions
}

/// A bucket reader for the dashboard: a copy of the configured bucket
/// client with a [`REQUEST_TIMEOUT`] on every request, reached only through
/// listings and GETs.
pub struct RemoteReader {
    bucket: Box<Bucket>,
}

impl RemoteReader {
    /// A reader of `bucket`, the client koto already resolved from its
    /// configuration. Constructing it makes no request.
    pub fn new(bucket: &Bucket) -> anyhow::Result<Self> {
        Ok(Self {
            bucket: bucket.with_request_timeout(REQUEST_TIMEOUT)?,
        })
    }

    /// The bucket's workspace prefixes, from one top-level listing.
    fn workspaces(&self) -> anyhow::Result<Vec<String>> {
        let pages = list_retrying(&self.bucket, String::new(), Some("/")).map_err(|e| {
            anyhow::anyhow!(
                "listing the bucket's workspace prefixes failed: {}",
                crate::session::cloud::without_url_userinfo(&e.to_string())
            )
        })?;
        Ok(pages
            .iter()
            .filter_map(|page| page.common_prefixes.as_ref())
            .flatten()
            .filter_map(|cp| cp.prefix.strip_suffix('/'))
            .filter(|p| is_workspace_prefix(p))
            .map(str::to_string)
            .collect())
    }

    /// The session directories under `workspace`, from its
    /// `/ctx/`-delimited listing, every page followed.
    pub fn list_workspace(
        &self,
        workspace: &str,
    ) -> anyhow::Result<BTreeMap<String, SessionListing>> {
        let prefix = format!("{workspace}/");
        let pages = list_retrying(&self.bucket, prefix.clone(), Some(CTX_DELIMITER))
            .map_err(|e| anyhow::anyhow!("listing {} failed: {}", prefix, e))?;
        Ok(group_sessions(
            workspace,
            pages
                .iter()
                .flat_map(|page| page.contents.iter())
                .map(|obj| (obj.key.as_str(), obj.size)),
        ))
    }

    /// Read one session's summary; `Err` names why it's unreadable.
    fn read_summary(&self, workspace: &str, name: &str) -> Result<SessionSummary, String> {
        let key = format!("{workspace}/{name}/{SUMMARY_FILE}");
        let response = self
            .bucket
            .get_object(&key)
            .map_err(|e| format!("GET {} failed: {}", key, e))?;
        if response.status_code() != 200 {
            return Err(format!("GET {} returned {}", key, response.status_code()));
        }
        SessionSummary::parse(response.bytes(), workspace, name).map_err(str::to_string)
    }

    /// The rows of one workspace: every session directory with a state
    /// object and a readable summary, skipping internal sessions and those
    /// whose `session_id` is in `local_ids`. Returns the rows and how many
    /// summaries couldn't be read.
    fn scan_workspace(
        &self,
        workspace: &str,
        local_ids: &HashSet<String>,
    ) -> anyhow::Result<(Vec<RemoteRow>, usize)> {
        let mut rows = Vec::new();
        let mut unreadable = 0;
        for (name, listing) in self.list_workspace(workspace)? {
            if is_internal_name(&name) || !listing.objects.contains_key(&state_file_name(&name)) {
                continue;
            }
            let Some(&size) = listing.objects.get(SUMMARY_FILE) else {
                continue;
            };
            if size > MAX_SUMMARY_BYTES as u64 {
                unreadable += 1;
                continue;
            }
            match self.read_summary(workspace, &name) {
                Ok(summary) if local_ids.contains(&summary.session_id) => {}
                Ok(summary) => rows.push(RemoteRow {
                    workspace: workspace.to_string(),
                    summary,
                }),
                Err(_) => unreadable += 1,
            }
        }
        Ok((rows, unreadable))
    }

    /// One pass over the bucket: every workspace prefix, each listed with
    /// the `/ctx/` delimiter, [`FANOUT`] at a time. Sessions this machine
    /// holds a copy of (their `session_id` is in `local_ids`) are left out:
    /// they show as local rows. A failed workspace leaves the others'
    /// rows standing and is named in `failure`.
    pub fn scan(&self, local_ids: &HashSet<String>) -> RemoteSnapshot {
        let workspaces = match self.workspaces() {
            Ok(w) => w,
            Err(e) => {
                return RemoteSnapshot {
                    failure: Some(e.to_string()),
                    ..RemoteSnapshot::default()
                }
            }
        };
        let next = AtomicUsize::new(0);
        let results: Mutex<Vec<anyhow::Result<(Vec<RemoteRow>, usize)>>> = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..workspaces.len().min(FANOUT) {
                scope.spawn(|| loop {
                    let Some(ws) = workspaces.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let result = self.scan_workspace(ws, local_ids);
                    results
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(result);
                });
            }
        });
        let mut snapshot = RemoteSnapshot::default();
        let mut failed = 0usize;
        for result in results.into_inner().unwrap_or_else(|e| e.into_inner()) {
            match result {
                Ok((rows, unreadable)) => {
                    snapshot.rows.extend(rows);
                    snapshot.unreadable += unreadable;
                }
                Err(e) => {
                    failed += 1;
                    snapshot.failure.get_or_insert_with(|| e.to_string());
                }
            }
        }
        if failed > 0 {
            snapshot.failure = snapshot
                .failure
                .map(|first| format!("{failed} workspace(s) couldn't be read; the first: {first}"));
        }
        snapshot.rows.sort_by(|a, b| {
            (a.workspace.as_str(), a.summary.name.as_str())
                .cmp(&(b.workspace.as_str(), b.summary.name.as_str()))
        });
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_sessions_keeps_only_top_level_objects() {
        let ws = "0123456789abcdef";
        let keys = [
            ("0123456789abcdef/review/koto-review.state.jsonl", 10),
            ("0123456789abcdef/review/summary.json", 5),
            ("0123456789abcdef/review/backrefs/x.json", 1),
            ("0123456789abcdef/other/koto-other.state.jsonl", 3),
            ("fedcba9876543210/review/summary.json", 5),
            ("0123456789abcdef/loose.json", 1),
        ];
        let sessions = group_sessions(ws, keys);
        assert_eq!(sessions.keys().collect::<Vec<_>>(), vec!["other", "review"]);
        let review = &sessions["review"].objects;
        assert_eq!(review.len(), 2);
        assert_eq!(review.get("summary.json"), Some(&5));
    }
}
