//! The dashboard's read of sessions other machines pushed to the bucket.
//!
//! [`RemoteReader::scan`] lists the bucket's top level for workspace
//! prefixes, lists each one with the delimiter `/ctx/`, which returns every
//! session's top-level objects (state file, `summary.json`, markers) with
//! their size, ETag and LastModified and folds its whole context tree into
//! one common prefix, and reads each session's [`SessionSummary`] and its
//! `migrated.json` marker when the listing shows one. It never fetches a
//! state file, so showing a remote row never costs the size of its history,
//! and it reads nothing under `ctx/`. The reader has no write method: it
//! lists and gets, and works with keys that grant only those.
//!
//! What the scan found is then decided by one pure function, [`classify`],
//! which drops removed, internal and locally held sessions, hides migrated
//! sources, and lists a session it can't trust a summary for as `unknown`.
//!
//! Everything read from the bucket is untrusted. Listings are asked in the
//! `url` encoding so a key reaches the reader as stored; a summary or marker
//! GET asks for its first 64 KiB and one byte and reads no further; a
//! summary with a field over its cap is unreadable; and every string bound
//! for a remote row passes through [`sanitize_bucket_text`] first. A time
//! more than [`FUTURE_SKEW`] past the reader's clock is clamped to it when
//! first read, and each pass stops at [`MAX_PREFIXES`] workspace prefixes,
//! [`MAX_PAGES`] pages a listing and [`MAX_SESSIONS`] sessions, saying so
//! in [`RemoteSnapshot::truncated`].
//!
//! The interactive dashboard reads through a [`RemoteFeed`]: one background
//! thread that owns its own reader, scans on a fixed interval, and hands each
//! complete [`RemoteSnapshot`] to the UI thread over a channel, so no key
//! press or redraw ever waits on the bucket. That thread scans through a
//! [`FeedCache`] ([`RemoteReader::scan_cached`]): a summary is fetched again
//! only when its listed ETag changed, and at most [`PREFIX_BUDGET`] prefixes
//! are listed per pass. `koto dashboard --once` scans uncached.
//!
//! Every request the reader makes fails after [`REQUEST_TIMEOUT`], and every
//! failure falls in one [`FailureClass`] ([`failure_class_after`]). A failed
//! top-level listing ends the pass with no further request. The reader asks
//! a listing page again only after a throttle, never after a timeout, but
//! rust-s3 asks every failed request again on its own, a second later, unless the
//! process has called `s3::set_retries(0)`. That setting is process-wide,
//! with no per-bucket form, and the reader can't set it: `koto dashboard`
//! does, in its `run` in `src/cli/dashboard.rs`. Only with it does a bucket that
//! never answers cost a pass one request timeout; without it, each ask is
//! two requests and a timeout costs two.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use s3::error::S3Error;
use s3::serde_types::ListBucketResult;
use s3::Bucket;

use crate::engine::types::{iso8601_at, parse_timestamp_to_systemtime};
use crate::session::cloud::{is_workspace_prefix, marker_target_str, MIGRATED_MARKER};
use crate::session::state_file_name;
use crate::session::summary::{is_internal_name, SessionSummary, MAX_SUMMARY_BYTES, SUMMARY_FILE};
use crate::session::sync_stamp::normalize_etag;
use crate::session_view::sanitize_capped;

/// The delimiter that folds a session's context tree into one entry.
pub const CTX_DELIMITER: &str = "/ctx/";

/// How long one bucket request may take before it fails.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a summary may name a state ETag other than the listed one before
/// the session reads as `unknown`: the ordinary gap between a push's state
/// PUT and its summary PUT stays inside it, a release that pushed the state
/// without refreshing the summary doesn't.
pub const STALE_SUMMARY_WINDOW: Duration = Duration::from_secs(60);

/// The largest `migrated.json` the reader fetches, by listed size and body.
pub const MAX_MARKER_BYTES: usize = 64 * 1024;

/// How many workspace listings are in flight at once.
const FANOUT: usize = 64;

/// Whether `c` is a character the bucket-text sanitizer replaces: every C0
/// and C1 control, DEL, the line and paragraph separators, the
/// bidirectional embeddings, overrides and isolates, the zero-width and
/// directional marks, the Arabic letter mark, the byte-order mark and the
/// tag characters.
fn is_display_hazard(c: char) -> bool {
    matches!(
        c,
        '\u{0}'..='\u{1F}'
            | '\u{7F}'..='\u{9F}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200B}'..='\u{200F}'
            | '\u{061C}'
            | '\u{FEFF}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// Text read from the bucket, made safe to show: each character
/// [`is_display_hazard`] names becomes exactly one space, and every other
/// character is kept as it is. Every string another machine wrote passes
/// through this before it reaches a remote row, in `koto dashboard --once`
/// and in the interactive dashboard alike, so no such text can move the
/// cursor, retitle the terminal, break a tab-separated line or reorder what
/// is drawn around it.
pub fn sanitize_bucket_text(text: &str) -> String {
    text.chars()
        .map(|c| if is_display_hazard(c) { ' ' } else { c })
        .collect()
}

/// The `encoding-type` the reader asks its listings in. Without it a key's
/// carriage return, U+0085 or U+2028 reaches the client raw inside the
/// listing's XML, and rust-s3's parser turns each into a newline, so the
/// listed name is no longer the stored key and the session can't be read.
const LIST_ENCODING: &str = "url";

/// A listed key or prefix in the `url` encoding S3 answers an
/// `encoding-type=url` listing with: `+` is a space and `%XX` a byte.
/// `None` when it isn't well formed or doesn't decode to UTF-8.
fn url_decode(encoded: &str) -> Option<String> {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = encoded.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Decode, in place, the keys and prefixes of each page that says it was
/// answered in [`LIST_ENCODING`]. A key that doesn't decode is dropped from
/// its page; a page in no encoding is left as it came.
fn decode_listing(pages: &mut [ListBucketResult]) {
    for page in pages {
        if page.encoding_type.as_deref() != Some(LIST_ENCODING) {
            continue;
        }
        page.contents.retain_mut(|obj| match url_decode(&obj.key) {
            Some(key) => {
                obj.key = key;
                true
            }
            None => false,
        });
        if let Some(prefixes) = page.common_prefixes.as_mut() {
            prefixes.retain_mut(|cp| match url_decode(&cp.prefix) {
                Some(prefix) => {
                    cp.prefix = prefix;
                    true
                }
                None => false,
            });
        }
    }
}

/// The most workspace prefixes one scan lists: the first ones the top-level
/// listing returns, in its order. A top-level listing says nothing of when
/// a prefix last changed, so it can't choose by recency.
pub const MAX_PREFIXES: usize = 5_000;

/// The most pages one scan asks of one listing, the top-level one included.
pub const MAX_PAGES: usize = 100;

/// The most remote sessions one scan keeps, chosen by recency
/// ([`keep_most_recent`]). This machine's own sessions, which show as their
/// local rows, are never among those it drops.
pub const MAX_SESSIONS: usize = 20_000;

/// What a scan that reached [`MAX_PREFIXES`], [`MAX_PAGES`] or
/// [`MAX_SESSIONS`] says, on `--once`'s stderr and the TUI's status line.
pub const TRUNCATED_NOTE: &str =
    "listing truncated: a listing ceiling was reached, so some of other machines' sessions \
     aren't shown";

/// How many times one listing page is asked, the first included, while
/// the bucket throttles it. The same as the sync path's listing.
const PAGE_ATTEMPTS: u32 = 3;

/// The wait before a page's second ask, doubled before each ask after that.
const PAGE_BACKOFF: Duration = Duration::from_millis(250);

/// A listing of `prefix`, split at `delimiter` when there is one, read one
/// page at a time and no further than `max_pages` pages: the pages read,
/// and whether the listing went on past them. Each page is asked up to
/// [`PAGE_ATTEMPTS`] times while `retry` accepts its failure, waiting
/// [`PAGE_BACKOFF`], doubled each time, between asks; a failure carries how
/// long its last ask took. Only the dashboard's reader lists this way; the
/// sync path follows every page.
fn list_pages(
    bucket: &Bucket,
    prefix: &str,
    delimiter: Option<&str>,
    max_pages: usize,
    retry: impl Fn(&S3Error) -> bool,
) -> Result<(Vec<ListBucketResult>, bool), (S3Error, Duration)> {
    let delimiter = delimiter.map(str::to_string);
    let mut pages: Vec<ListBucketResult> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let ask = || {
            let started = Instant::now();
            bucket
                .list_page(
                    prefix.to_string(),
                    delimiter.clone(),
                    token.clone(),
                    None,
                    None,
                )
                .map(|(page, _)| page)
                .map_err(|e| (e, started.elapsed()))
        };
        let mut wait = PAGE_BACKOFF;
        let mut attempt = 1;
        let page = loop {
            match ask() {
                Err((e, _)) if attempt < PAGE_ATTEMPTS && retry(&e) => {
                    std::thread::sleep(wait);
                    wait *= 2;
                    attempt += 1;
                }
                result => break result?,
            }
        };
        token = page
            .next_continuation_token
            .clone()
            .filter(|t| !t.is_empty());
        pages.push(page);
        match token {
            None => return Ok((pages, false)),
            Some(_) if pages.len() >= max_pages => return Ok((pages, true)),
            Some(_) => {}
        }
    }
}

/// How far past the reader's clock a time another machine wrote may lie
/// and still be read as written: ordinary clock skew between two machines.
/// A later one is clamped to the reader's clock.
pub const FUTURE_SKEW: Duration = Duration::from_secs(5 * 60);

/// Whether `t` lies more than [`FUTURE_SKEW`] past `now`.
fn is_far_future(t: SystemTime, now: SystemTime) -> bool {
    now.checked_add(FUTURE_SKEW).is_some_and(|edge| t > edge)
}

/// Clamp a summary's `created_at` and `last_event_at` to `now`, the
/// reader's clock when it read the summary, when either lies more than
/// [`FUTURE_SKEW`] past it, so a forged future time can't hold a row's idle
/// at zero. A time is read as the dashboard reads it, after the bucket-text
/// sanitizer; one that doesn't parse is left as it is.
///
/// The clamp is applied once, when the summary is read: a cached scan
/// reuses the clamped summary while its ETag is unchanged, so the row's
/// idle grows from the first read, as it would from a real time.
fn clamp_summary_times(summary: &mut SessionSummary, now: SystemTime) {
    let ahead = |text: &str| {
        parse_timestamp_to_systemtime(&sanitize_bucket_text(text))
            .is_some_and(|t| is_far_future(t, now))
    };
    if ahead(&summary.created_at) {
        summary.created_at = iso8601_at(now);
    }
    if summary.last_event_at.as_deref().is_some_and(ahead) {
        summary.last_event_at = Some(iso8601_at(now));
    }
}

/// Clamp each listed LastModified of `listing` that lies more than
/// [`FUTURE_SKEW`] past `now` to `now`, or, when `before` (the session as
/// the cache last listed it) lists the same file with the same non-empty
/// ETag, to the time it was given then, so an unchanged object keeps the
/// time it was first clamped to and its row ages from there.
fn clamp_listing(listing: &mut SessionListing, before: Option<&SessionListing>, now: SystemTime) {
    for (file, object) in listing.objects.iter_mut() {
        if !object.last_modified.is_some_and(|t| is_far_future(t, now)) {
            continue;
        }
        let earlier = before
            .and_then(|b| b.objects.get(file))
            .filter(|was| !object.etag.is_empty() && was.etag == object.etag)
            .and_then(|was| was.last_modified)
            .filter(|t| !is_far_future(*t, now));
        object.last_modified = Some(earlier.unwrap_or(now));
    }
}

/// A session a scan may keep or drop at [`MAX_SESSIONS`]: its workspace
/// prefix, its name and its state file's listed LastModified.
pub type SessionCandidate<'a> = (&'a str, &'a str, Option<SystemTime>);

/// The sessions a scan keeps out of `candidates` when there are more than
/// `limit`: prefixes in order of their most recent state change (the
/// newest LastModified among their sessions, newest first, a prefix with no
/// time last, then by prefix), and within a prefix its sessions newest
/// first, then by name, until `limit` are taken. `None` when there are no
/// more than `limit`, and every candidate is kept.
pub fn keep_most_recent(
    candidates: &[SessionCandidate<'_>],
    limit: usize,
) -> Option<HashSet<(String, String)>> {
    if candidates.len() <= limit {
        return None;
    }
    let mut newest: HashMap<&str, Option<SystemTime>> = HashMap::new();
    for &(workspace, _, modified) in candidates {
        let at = newest.entry(workspace).or_insert(None);
        *at = (*at).max(modified);
    }
    let mut order: Vec<&SessionCandidate<'_>> = candidates.iter().collect();
    order.sort_by(|a, b| {
        newest[b.0]
            .cmp(&newest[a.0])
            .then_with(|| a.0.cmp(b.0))
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.1.cmp(b.1))
    });
    Some(
        order
            .into_iter()
            .take(limit)
            .map(|&(workspace, name, _)| (workspace.to_string(), name.to_string()))
            .collect(),
    )
}

/// Run `f` on every item, [`FANOUT`] at a time, and give the results in the
/// items' order.
fn fan_out<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|scope| {
        for _ in 0..items.len().min(FANOUT) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(i) else {
                    return;
                };
                let result = f(item);
                results
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((i, result));
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, r)| r).collect()
}

/// What a bounded GET gave.
#[derive(Debug, PartialEq, Eq)]
enum Fetched {
    /// The whole body, no longer than the cap.
    Body(Vec<u8>),
    /// The body ran past the cap and was read no further.
    Oversized,
    /// The GET failed, or answered neither 200 nor 206.
    Failed,
}

/// A writer that keeps at most `limit` bytes and refuses any past them, so
/// a body streamed into it is read no further than that.
pub(crate) struct BoundedSink {
    pub(crate) bytes: Vec<u8>,
    pub(crate) limit: usize,
}

impl std::io::Write for BoundedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let room = self.limit - self.bytes.len();
        if room == 0 && !buf.is_empty() {
            return Err(std::io::Error::other("body past the read bound"));
        }
        let n = buf.len().min(room);
        self.bytes.extend_from_slice(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// One top-level object of a session directory, as the listing reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedObject {
    /// The listed size in bytes.
    pub size: u64,
    /// The listed ETag, without its quotes.
    pub etag: String,
    /// The listed LastModified; `None` when it didn't parse.
    pub last_modified: Option<SystemTime>,
}

/// A session directory as one `/ctx/`-delimited listing shows it: its
/// top-level objects by file name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionListing {
    /// Top-level file name to what the listing says about it.
    pub objects: BTreeMap<String, ListedObject>,
}

impl SessionListing {
    /// The state file of session `name`, when the listing holds one.
    pub fn state(&self, name: &str) -> Option<&ListedObject> {
        self.objects.get(&state_file_name(name))
    }
}

/// Group a workspace listing's objects into session directories.
///
/// `keys` are `(key, object)` pairs from listings of `<workspace>/` with the
/// [`CTX_DELIMITER`]; only objects directly inside a session directory
/// (`<workspace>/<name>/<file>`) count, so nothing under a deeper path
/// becomes a row fact.
pub fn group_sessions<'a>(
    workspace: &str,
    keys: impl IntoIterator<Item = (&'a str, ListedObject)>,
) -> BTreeMap<String, SessionListing> {
    let prefix = format!("{workspace}/");
    let mut sessions: BTreeMap<String, SessionListing> = BTreeMap::new();
    for (key, object) in keys {
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
            .insert(file.to_string(), object);
    }
    sessions
}

/// What reading a session's `summary.json` gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryRead {
    /// The listing shows no summary (or none was asked for).
    Absent,
    /// Listed but not usable; the reason (`oversized`, `unfetched`,
    /// `malformed`, `schema`, `field-cap`, `sources-cap`, `key-mismatch`).
    Unreadable(&'static str),
    /// Read and checked against its key.
    Read(Box<SessionSummary>),
}

/// Where a source's `migrated.json` says its copy went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedTo {
    /// The copy's workspace prefix.
    pub prefix: String,
    /// The copy's session name.
    pub session: String,
    /// The copy's `session_id`, when the marker records one.
    pub session_id: Option<String>,
}

/// One session directory a scan found, with what was read for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedSession {
    /// The workspace prefix the directory is under.
    pub workspace: String,
    /// The session name (the directory's name).
    pub name: String,
    /// Its top-level objects.
    pub listing: SessionListing,
    /// Its summary.
    pub summary: SummaryRead,
    /// Its `migrated.json`, when one was listed and read.
    pub marker: Option<MigratedTo>,
}

/// A source a local session was imported from, by its latest
/// `session_imported` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSource {
    /// The source's workspace prefix; `None` when its workspace path
    /// couldn't be turned into one.
    pub prefix: Option<String>,
    /// The source's session name.
    pub name: String,
    /// The source's `session_id`.
    pub session_id: String,
}

/// What a scan needs to know about this machine's own sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalView {
    /// The `session_id` of every local session.
    pub session_ids: HashSet<String>,
    /// `(workspace prefix, name)` of every local session whose home prefix
    /// is known.
    pub homes: HashSet<(String, String)>,
    /// The source each imported local session names.
    pub imports: Vec<ImportedSource>,
}

/// How a remote row is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowStatus {
    /// From its summary.
    Shown(Box<SessionSummary>),
    /// With status `unknown`: no summary, an unreadable one, or one that no
    /// longer describes the listed state file. Nothing but the listing's own
    /// facts is shown.
    Unknown {
        /// Why (`no-summary`, `stale`, or the summary's unreadable reason).
        reason: &'static str,
        /// The state file's listed LastModified, which the row's idle time
        /// is measured from.
        state_modified: Option<SystemTime>,
        /// The `session_id` a stale summary named; `None` otherwise.
        session_id: Option<String>,
    },
}

/// One session another machine pushed, as the dashboard lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRow {
    /// The workspace id (16-hex prefix) the session's copy lives under.
    pub workspace: String,
    /// The session name.
    pub name: String,
    /// What the row shows.
    pub status: RowStatus,
    /// Another remote row's summary names this session among its `sources`
    /// (by workspace prefix, name and `session_id`), so the row recedes: it
    /// leaves the default view and the needs-you band, and shows with
    /// `--all` and the `a` key. A `sources` entry never hides a row.
    pub receded_by_source: bool,
}

/// What [`classify`] decided for a scan.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Classified {
    /// The rows to list, in workspace then name order.
    pub rows: Vec<RemoteRow>,
    /// Migrated sources left out in favour of their copy.
    pub hidden_sources: usize,
    /// Rows listed as `unknown`.
    pub unreadable: usize,
}

/// Decide every session directory of a scan, in this order (the first rule
/// that matches decides):
///
/// 1. no state object: dropped, the session was removed;
/// 2. an internal name: dropped;
/// 3. a session this machine holds a copy of: dropped, it shows as its local
///    row. Matched by `session_id` when its summary was read, else by its
///    workspace prefix and name;
/// 4. a migrated source: hidden and counted, when its own `migrated.json`
///    names another session directory present in this scan (and, when that
///    copy's summary was read, by the copy's `session_id`), or a local
///    session's latest `session_imported` names it (by `session_id` when its
///    summary was read, else by workspace prefix and name);
/// 5. no summary: `unknown`, counted;
/// 6. an unreadable summary: `unknown`, counted;
/// 7. a summary whose `state_etag` isn't the listed state file's ETag, once
///    that file is more than [`STALE_SUMMARY_WINDOW`] old by `now` (or its
///    LastModified didn't parse): `unknown`, counted;
/// 8. otherwise shown from its summary.
///
/// Then each row a `sources` entry of another shown row's summary names
/// recedes ([`RemoteRow::receded_by_source`]): only that row, matched by
/// workspace prefix, name and the `session_id` its summary records. An entry
/// naming its own summary's session, a local session (never a remote row
/// here) or a session by prefix and name with another `session_id` recedes
/// nothing. `sources` never hides or counts a row: hiding takes rule 4.
pub fn classify(scan: &[ScannedSession], local: &LocalView, now: SystemTime) -> Classified {
    let present: HashMap<(&str, &str), &ScannedSession> = scan
        .iter()
        .filter(|s| s.listing.state(&s.name).is_some())
        .map(|s| ((s.workspace.as_str(), s.name.as_str()), s))
        .collect();
    let mut out = Classified::default();
    for session in scan {
        let Some(state) = session.listing.state(&session.name) else {
            continue;
        };
        if is_internal_name(&session.name) {
            continue;
        }
        let read = match &session.summary {
            SummaryRead::Read(summary) => Some(summary.as_ref()),
            _ => None,
        };
        let here = (session.workspace.clone(), session.name.clone());
        let held = match read {
            Some(summary) => local.session_ids.contains(&summary.session_id),
            None => local.homes.contains(&here),
        };
        if held {
            continue;
        }
        if is_migrated(session, read, &present, local) {
            out.hidden_sources += 1;
            continue;
        }
        let unknown = |reason, session_id: Option<&str>| RowStatus::Unknown {
            reason,
            state_modified: state.last_modified,
            session_id: session_id.map(str::to_string),
        };
        let status = match &session.summary {
            SummaryRead::Absent => unknown("no-summary", None),
            SummaryRead::Unreadable(reason) => unknown(reason, None),
            SummaryRead::Read(summary) if is_stale(summary, state, now) => {
                unknown("stale", Some(&summary.session_id))
            }
            SummaryRead::Read(summary) => RowStatus::Shown(summary.clone()),
        };
        if matches!(status, RowStatus::Unknown { .. }) {
            out.unreadable += 1;
        }
        out.rows.push(RemoteRow {
            workspace: session.workspace.clone(),
            name: session.name.clone(),
            status,
            receded_by_source: false,
        });
    }
    recede_named_sources(&mut out.rows);
    out.rows.sort_by(|a, b| {
        (a.workspace.as_str(), a.name.as_str()).cmp(&(b.workspace.as_str(), b.name.as_str()))
    });
    out
}

/// The `sources` rule of [`classify`]: mark each row another shown row's
/// summary names as a source, by workspace prefix, name and `session_id`.
fn recede_named_sources(rows: &mut [RemoteRow]) {
    let named: HashSet<(&str, &str, &str)> = rows
        .iter()
        .filter_map(|row| match &row.status {
            RowStatus::Shown(summary) => Some((row, summary)),
            RowStatus::Unknown { .. } => None,
        })
        .flat_map(|(row, summary)| {
            summary.sources.iter().filter(move |source| {
                let is_self = (source.prefix == row.workspace && source.name == row.name)
                    || source.session_id == summary.session_id;
                !is_self
            })
        })
        .map(|source| {
            (
                source.prefix.as_str(),
                source.name.as_str(),
                source.session_id.as_str(),
            )
        })
        .collect();
    if named.is_empty() {
        return;
    }
    let receded: Vec<bool> = rows
        .iter()
        .map(|row| {
            let session_id = match &row.status {
                RowStatus::Shown(summary) => Some(summary.session_id.as_str()),
                RowStatus::Unknown { session_id, .. } => session_id.as_deref(),
            };
            session_id
                .is_some_and(|id| named.contains(&(row.workspace.as_str(), row.name.as_str(), id)))
        })
        .collect();
    for (row, receded) in rows.iter_mut().zip(receded) {
        row.receded_by_source = receded;
    }
}

/// Rule 4 of [`classify`]: whether `session` is a source whose copy this
/// scan or this machine holds.
fn is_migrated(
    session: &ScannedSession,
    read: Option<&SessionSummary>,
    present: &HashMap<(&str, &str), &ScannedSession>,
    local: &LocalView,
) -> bool {
    let by_marker = session.marker.as_ref().is_some_and(|to| {
        let is_self = to.prefix == session.workspace && to.session == session.name;
        !is_self
            && present
                .get(&(to.prefix.as_str(), to.session.as_str()))
                .is_some_and(|copy| match (&copy.summary, &to.session_id) {
                    (SummaryRead::Read(summary), Some(id)) => &summary.session_id == id,
                    _ => true,
                })
    });
    by_marker
        || local.imports.iter().any(|source| match read {
            Some(summary) => source.session_id == summary.session_id,
            None => {
                source.prefix.as_deref() == Some(session.workspace.as_str())
                    && source.name == session.name
            }
        })
}

/// Rule 7 of [`classify`]: the summary names another state ETag and the
/// listed state file is older than [`STALE_SUMMARY_WINDOW`].
fn is_stale(summary: &SessionSummary, state: &ListedObject, now: SystemTime) -> bool {
    if normalize_etag(&summary.state_etag) == state.etag {
        return false;
    }
    match state.last_modified {
        Some(modified) => now
            .duration_since(modified)
            .is_ok_and(|age| age > STALE_SUMMARY_WINDOW),
        None => true,
    }
}

/// One workspace's listing: its session directories by name, and whether
/// it stopped at [`MAX_PAGES`].
type Listed = (BTreeMap<String, SessionListing>, bool);

/// How a bucket request the reader made failed. Every failure is one of
/// these four; [`failure_class_after`] decides which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// The connection was refused, the host didn't resolve, or the answer
    /// wasn't a usable reply: a 5xx other than 503, or a body that isn't
    /// the listing asked for.
    Unreachable,
    /// HTTP 401 or 403.
    Denied,
    /// No answer within [`REQUEST_TIMEOUT`].
    Timeout,
    /// HTTP 429 or 503, after koto's usual retries.
    Throttled,
}

impl FailureClass {
    /// The class as the status line and the stderr note name it.
    pub fn name(self) -> &'static str {
        match self {
            FailureClass::Unreachable => "unreachable",
            FailureClass::Denied => "denied",
            FailureClass::Timeout => "timeout",
            FailureClass::Throttled => "throttled",
        }
    }
}

/// The class of a failed bucket request: 401 and 403 are `denied`, 429 and
/// 503 `throttled`, a connection that timed out (the request deadline, or a
/// read that waited out its socket timeout) `timeout`, and anything else,
/// a refused connection, a host that didn't resolve, another 5xx or a reply
/// that didn't parse, `unreachable`.
///
/// It parts from [`crate::session::cloud`]'s `is_answered_failure` on 500:
/// a sync asks a 500 again because the next ask may answer, while here a
/// 500 is a reply the reader can only report, so it isn't `throttled`.
pub fn failure_class(e: &S3Error) -> FailureClass {
    match e {
        S3Error::HttpFailWithBody(401 | 403, _) => FailureClass::Denied,
        S3Error::HttpFailWithBody(429 | 503, _) => FailureClass::Throttled,
        S3Error::Atto(e) => match e.kind() {
            attohttpc::ErrorKind::Io(io) if is_timed_out(io) => FailureClass::Timeout,
            _ => FailureClass::Unreachable,
        },
        S3Error::Io(io) if is_timed_out(io) => FailureClass::Timeout,
        _ => FailureClass::Unreachable,
    }
}

/// [`failure_class`] for a request that failed after `took`: a transport
/// failure (no HTTP answer) that arrives once [`REQUEST_TIMEOUT`] has
/// passed is `timeout`, whatever error the connection's end gave. The
/// request deadline ends a stalled TLS handshake by shutting its socket,
/// which the handshake reports as an unexpected end of file, not a timeout.
pub fn failure_class_after(e: &S3Error, took: Duration) -> FailureClass {
    match failure_class(e) {
        FailureClass::Unreachable
            if took >= REQUEST_TIMEOUT && matches!(e, S3Error::Atto(_) | S3Error::Io(_)) =>
        {
            FailureClass::Timeout
        }
        class => class,
    }
}

/// Whether an I/O error is a wait that ran out: the request deadline
/// (`TimedOut`) or a socket read timeout (`WouldBlock` on Unix).
fn is_timed_out(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

/// A listing the reader gave up on: the last ask's error, and how long that
/// ask took (all its pages, and rust-s3's own retry when the process has
/// one).
#[derive(Debug)]
pub struct ListFailure {
    /// The last ask's error.
    pub error: S3Error,
    /// How long the last ask took.
    pub took: Duration,
}

impl ListFailure {
    /// The failure's class ([`failure_class_after`]).
    pub fn class(&self) -> FailureClass {
        failure_class_after(&self.error, self.took)
    }
}

impl std::fmt::Display for ListFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for ListFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// The longest a host is shown, in characters.
const HOST_CAP: usize = 256;

/// The host of an endpoint as [`s3::Region::host`] gives it: no scheme, no
/// userinfo, no path, no port. Unsanitized. Everything up to the last `@`
/// goes before the path is cut, so a secret holding `/`, `?` or `#` in the
/// userinfo never shows.
fn host_of(endpoint: &str) -> String {
    let rest = endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(rest) => match rest.find(']') {
            Some(end) => &authority[..end + 2],
            None => authority,
        },
        None => match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
            _ => authority,
        },
    };
    host.to_string()
}

/// Why a pass couldn't read some or all of the bucket: the failure's class,
/// the endpoint's host, and which workspaces it left unread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadFailure {
    /// The top-level listing's class, or the first failed workspace's (in
    /// workspace order).
    pub class: FailureClass,
    /// The endpoint's host, as configured. Shown only through
    /// [`ReadFailure::line`], which sanitizes it.
    pub host: String,
    /// The top-level listing failed, so no workspace was read; a holder
    /// keeps every remote row it last had.
    pub top_level: bool,
    /// Workspaces whose listing failed, sorted; a holder keeps the rows it
    /// last had for them. Empty when `top_level`.
    pub workspaces: Vec<String>,
}

impl ReadFailure {
    /// Whether a holder keeps the rows it last had for `workspace`.
    pub fn keeps(&self, workspace: &str) -> bool {
        self.top_level || self.workspaces.iter().any(|w| w == workspace)
    }

    /// The failure on one line, its class and host passed through the
    /// dashboard's sanitizer: `<class> at <host>: the bucket couldn't be
    /// listed` when the top-level listing failed, else `<n> workspace(s)
    /// couldn't be read; the first: <class> at <host>`.
    pub fn line(&self) -> String {
        let class = sanitize_capped(self.class.name(), HOST_CAP);
        let host = sanitize_capped(&self.host, HOST_CAP);
        if self.top_level {
            return format!("{class} at {host}: the bucket couldn't be listed");
        }
        let n = self.workspaces.len();
        format!(
            "{n} workspace{} couldn't be read; the first: {class} at {host}",
            if n == 1 { "" } else { "s" }
        )
    }
}

impl std::fmt::Display for ReadFailure {
    /// [`ReadFailure::line`].
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.line())
    }
}

impl std::error::Error for ReadFailure {}

/// One pass over the bucket.
#[derive(Debug, Default)]
pub struct RemoteSnapshot {
    /// Every row listed, in workspace then name order.
    pub rows: Vec<RemoteRow>,
    /// Why the pass couldn't read some or all of the bucket, when it
    /// couldn't.
    pub failure: Option<ReadFailure>,
    /// Rows listed as `unknown`: no summary, or one that couldn't be read
    /// or no longer describes its state file.
    pub unreadable: usize,
    /// Migrated sources left out in favour of their copy.
    pub hidden_sources: usize,
    /// The pass reached a ceiling ([`MAX_PREFIXES`], [`MAX_PAGES`] or
    /// [`MAX_SESSIONS`]), so some of the bucket isn't in `rows`. A cached
    /// pass also counts a prefix whose last answered listing reached
    /// [`MAX_PAGES`], until a listing of it doesn't.
    pub truncated: bool,
}

impl RemoteSnapshot {
    /// Whether a holder keeps the rows it last had for `workspace`: its
    /// listing, or the top-level one, failed.
    pub fn keeps(&self, workspace: &str) -> bool {
        self.failure.as_ref().is_some_and(|f| f.keeps(workspace))
    }
}

/// The most workspace prefixes one cached scan lists.
pub const PREFIX_BUDGET: usize = 200;

/// A prefix whose newest listed object is younger than this is listed on
/// every cached scan, whatever the budget's rotation says.
pub const ACTIVE_WINDOW: Duration = Duration::from_secs(30 * 60);

/// What the feed thread remembers between scans, in memory only.
///
/// Per workspace prefix: the sessions its last answered listing found, with
/// the summaries and markers read for them, each held against the ETag the
/// listing gave its object, so a later scan reuses a read whose object's
/// listed ETag is unchanged instead of fetching it again. Only a summary
/// that was read and parsed is reused; anything else is asked again. The
/// cache also records when each prefix was last listed and the newest
/// LastModified its listing showed, which [`choose_prefixes`] reads to
/// spend the [`PREFIX_BUDGET`].
#[derive(Debug, Default)]
pub struct FeedCache {
    workspaces: HashMap<String, CachedWorkspace>,
    /// How many cached scans have started; a prefix's `listed_at` is the
    /// scan that last listed it.
    scans: u64,
}

#[derive(Debug, Default)]
struct CachedWorkspace {
    /// The scan that last asked for this prefix's listing.
    listed_at: u64,
    /// The newest LastModified the last answered listing showed.
    newest: Option<SystemTime>,
    /// The last answered listing stopped at [`MAX_PAGES`].
    truncated: bool,
    /// The sessions the last answered listing found, by name.
    sessions: BTreeMap<String, ScannedSession>,
}

impl FeedCache {
    /// An empty cache: the first scan through it reads everything.
    pub fn new() -> Self {
        Self::default()
    }

    /// The sessions cached for `workspace`, by name.
    pub fn sessions(&self, workspace: &str) -> Option<&BTreeMap<String, ScannedSession>> {
        self.workspaces.get(workspace).map(|w| &w.sessions)
    }
}

/// The prefixes a cached scan lists out of `all`, the top-level listing's
/// prefixes: every one while there are at most `budget`; above that, every
/// prefix the cache has never listed, then every prefix whose newest listed
/// object is younger than [`ACTIVE_WINDOW`] at `now`, then the rest in
/// rotation, longest since listed first, until `budget` are chosen.
pub fn choose_prefixes(
    all: &[String],
    cache: &FeedCache,
    now: SystemTime,
    budget: usize,
) -> Vec<String> {
    if all.len() <= budget {
        return all.to_vec();
    }
    let active = |w: &CachedWorkspace| {
        w.newest
            .is_some_and(|t| now.duration_since(t).unwrap_or_default() < ACTIVE_WINDOW)
    };
    let mut ranked: Vec<(u8, u64, &String)> = all
        .iter()
        .map(|ws| match cache.workspaces.get(ws) {
            None => (0, 0, ws),
            Some(w) if active(w) => (1, 0, ws),
            Some(w) => (2, w.listed_at, ws),
        })
        .collect();
    ranked.sort();
    ranked
        .into_iter()
        .take(budget)
        .map(|(_, _, ws)| ws.clone())
        .collect()
}

/// A bucket reader for the dashboard: a copy of the configured bucket
/// client with a [`REQUEST_TIMEOUT`] on every request, reached only through
/// listings and GETs.
pub struct RemoteReader {
    bucket: Box<Bucket>,
    /// The same client, asking its listings in [`LIST_ENCODING`].
    lister: Box<Bucket>,
    host: String,
}

impl RemoteReader {
    /// A reader of `bucket`, the client koto already resolved from its
    /// configuration. Constructing it makes no request. A client that can't
    /// be copied with the timeout is a failure of the whole bucket, classed
    /// `unreachable` at the endpoint's host, so it reads like any other.
    ///
    /// The reader bounds a timed-out listing at one request only in a
    /// process that has called `s3::set_retries(0)`, as `koto dashboard`
    /// does; elsewhere rust-s3 asks each failed request once more on its
    /// own (see the module documentation).
    pub fn new(bucket: &Bucket) -> Result<Self, ReadFailure> {
        let host = host_of(&bucket.region().host());
        let mut query = bucket.extra_query.clone();
        query.insert("encoding-type".to_string(), LIST_ENCODING.to_string());
        let built = bucket
            .with_request_timeout(REQUEST_TIMEOUT)
            .and_then(|plain| {
                let lister = plain.with_extra_query(query)?;
                Ok((plain, Box::new(lister)))
            });
        match built {
            Ok((bucket, lister)) => Ok(Self {
                bucket,
                lister,
                host,
            }),
            Err(_) => Err(ReadFailure {
                class: FailureClass::Unreachable,
                host,
                top_level: true,
                workspaces: Vec::new(),
            }),
        }
    }

    /// A listing of `prefix`, at most [`MAX_PAGES`] pages ([`list_pages`]),
    /// and whether it went on past them. Each page is asked again only
    /// while the bucket throttles it (koto's usual retries), never after a
    /// timeout or a refusal. It is asked in [`LIST_ENCODING`] and its keys
    /// decoded, so a listed key is the key as stored, whatever characters
    /// it holds.
    fn list(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<(Vec<ListBucketResult>, bool), ListFailure> {
        let (mut pages, truncated) = list_pages(&self.lister, prefix, delimiter, MAX_PAGES, |e| {
            failure_class(e) == FailureClass::Throttled
        })
        .map_err(|(error, took)| ListFailure { error, took })?;
        decode_listing(&mut pages);
        Ok((pages, truncated))
    }

    /// A failure of this reader's bucket.
    fn failure(&self, class: FailureClass, workspaces: Vec<String>) -> ReadFailure {
        ReadFailure {
            class,
            host: self.host.clone(),
            top_level: workspaces.is_empty(),
            workspaces,
        }
    }

    /// The bucket's workspace prefixes, from one top-level listing, at
    /// most the first [`MAX_PREFIXES`] in the listing's order, and whether
    /// any were left out or the listing stopped at [`MAX_PAGES`].
    fn workspaces(&self) -> Result<(Vec<String>, bool), ListFailure> {
        let (pages, mut truncated) = self.list("", Some("/"))?;
        let mut all: Vec<String> = pages
            .iter()
            .filter_map(|page| page.common_prefixes.as_ref())
            .flatten()
            .filter_map(|cp| cp.prefix.strip_suffix('/'))
            .filter(|p| is_workspace_prefix(p))
            .map(str::to_string)
            .collect();
        if all.len() > MAX_PREFIXES {
            all.truncate(MAX_PREFIXES);
            truncated = true;
        }
        Ok((all, truncated))
    }

    /// The session directories under `workspace`, from its
    /// `/ctx/`-delimited listing, its first [`MAX_PAGES`] pages followed.
    pub fn list_workspace(
        &self,
        workspace: &str,
    ) -> Result<BTreeMap<String, SessionListing>, ListFailure> {
        self.list_sessions(workspace).map(|(sessions, _)| sessions)
    }

    /// [`RemoteReader::list_workspace`], and whether the listing stopped at
    /// [`MAX_PAGES`].
    fn list_sessions(&self, workspace: &str) -> Result<Listed, ListFailure> {
        let (pages, truncated) = self.list(&format!("{workspace}/"), Some(CTX_DELIMITER))?;
        let sessions = group_sessions(
            workspace,
            pages
                .iter()
                .flat_map(|page| page.contents.iter())
                .map(|obj| {
                    (
                        obj.key.as_str(),
                        ListedObject {
                            size: obj.size,
                            etag: obj.e_tag.as_deref().map(normalize_etag).unwrap_or_default(),
                            last_modified: crate::engine::wake::parse_rfc3339_millis(
                                &obj.last_modified,
                            ),
                        },
                    )
                }),
        );
        Ok((sessions, truncated))
    }

    /// GET `<workspace>/<name>/<file>`, reading no more than `cap` bytes
    /// and one past them. The request asks for that range
    /// (`bytes=0-<cap>`), and a body that runs past `cap` anyway (the
    /// endpoint ignored the range) stops being read there, so an object
    /// replaced by a huge one after it was listed costs no more than that.
    fn get(&self, workspace: &str, name: &str, file: &str, cap: usize) -> Fetched {
        let mut sink = BoundedSink {
            bytes: Vec::new(),
            limit: cap + 1,
        };
        let answered = self.bucket.get_object_range_to_writer(
            format!("{workspace}/{name}/{file}"),
            0,
            Some(cap as u64),
            &mut sink,
        );
        if sink.bytes.len() > cap {
            return Fetched::Oversized;
        }
        match answered {
            Ok(200 | 206) => Fetched::Body(sink.bytes),
            _ => Fetched::Failed,
        }
    }

    /// Read one session's summary, its far-future times clamped to `now`
    /// ([`clamp_summary_times`]).
    fn read_summary(&self, workspace: &str, name: &str, now: SystemTime) -> SummaryRead {
        match self.get(workspace, name, SUMMARY_FILE, MAX_SUMMARY_BYTES) {
            Fetched::Failed => SummaryRead::Unreadable("unfetched"),
            Fetched::Oversized => SummaryRead::Unreadable("oversized"),
            Fetched::Body(bytes) => match SessionSummary::parse(&bytes, workspace, name) {
                Ok(mut summary) => {
                    clamp_summary_times(&mut summary, now);
                    SummaryRead::Read(Box::new(summary))
                }
                Err(reason) => SummaryRead::Unreadable(reason),
            },
        }
    }

    /// Read one session's `migrated.json`; `None` when it can't be read,
    /// runs past [`MAX_MARKER_BYTES`] or doesn't name a copy.
    fn read_marker(&self, workspace: &str, name: &str) -> Option<MigratedTo> {
        let Fetched::Body(bytes) = self.get(workspace, name, MIGRATED_MARKER, MAX_MARKER_BYTES)
        else {
            return None;
        };
        Some(MigratedTo {
            prefix: marker_target_str(&bytes, "prefix")?,
            session: marker_target_str(&bytes, "session")?,
            session_id: marker_target_str(&bytes, "session_id"),
        })
    }

    /// The summary and marker of one listed session, when it has a state
    /// object and isn't internal. A summary over [`MAX_SUMMARY_BYTES`] by
    /// its listed size isn't fetched.
    ///
    /// `before` is the session as the last answered listing of its
    /// workspace found it, when the caller keeps a [`FeedCache`]: a summary
    /// that was read and parsed, or a marker that named a copy, is reused
    /// without a GET while its object's listed ETag is the one it was read
    /// under.
    fn read_session(
        &self,
        session: &ScannedSession,
        before: Option<&ScannedSession>,
        now: SystemTime,
    ) -> (SummaryRead, Option<MigratedTo>) {
        let (workspace, name, listing) = (&session.workspace, &session.name, &session.listing);
        let wanted = listing.state(name).is_some() && !is_internal_name(name);
        let unchanged = |file: &str| match (
            before.and_then(|b| b.listing.objects.get(file)),
            listing.objects.get(file),
        ) {
            (Some(was), Some(now)) => !now.etag.is_empty() && was.etag == now.etag,
            _ => false,
        };
        let summary = match listing.objects.get(SUMMARY_FILE) {
            Some(_) if !wanted => SummaryRead::Absent,
            Some(obj) if obj.size > MAX_SUMMARY_BYTES as u64 => {
                SummaryRead::Unreadable("oversized")
            }
            Some(_) => match before.map(|b| &b.summary) {
                Some(read @ SummaryRead::Read(_)) if unchanged(SUMMARY_FILE) => read.clone(),
                _ => self.read_summary(workspace, name, now),
            },
            None => SummaryRead::Absent,
        };
        let marker = match listing.objects.get(MIGRATED_MARKER) {
            Some(obj) if wanted && obj.size <= MAX_MARKER_BYTES as u64 => {
                match before.and_then(|b| b.marker.as_ref()) {
                    Some(m) if unchanged(MIGRATED_MARKER) => Some(m.clone()),
                    _ => self.read_marker(workspace, name),
                }
            }
            _ => None,
        };
        (summary, marker)
    }

    /// One pass over the bucket: the first [`MAX_PREFIXES`] workspace
    /// prefixes, each listed with the `/ctx/` delimiter, [`FANOUT`] at a
    /// time and no further than [`MAX_PAGES`] pages, then, of the sessions
    /// listed, at most [`MAX_SESSIONS`] kept by recency
    /// ([`keep_most_recent`]) and only those read, then [`classify`]d as a
    /// whole against `local`. A session `local` holds by its home prefix
    /// and name is never among those dropped. A pass that reached any of
    /// the ceilings says so in `truncated`. A failed top-level listing ends
    /// the pass with no further request. A failed workspace leaves the
    /// others' rows standing and is named in `failure`, which carries the
    /// class of the first one in workspace order. Nothing is kept: every
    /// summary of a kept session is fetched, as `koto dashboard --once`
    /// needs. A summary's or state file's time more than [`FUTURE_SKEW`]
    /// past the reader's clock is clamped to it.
    pub fn scan(&self, local: &LocalView) -> RemoteSnapshot {
        self.scan_with(local, None)
    }

    /// [`RemoteReader::scan`] through `cache`, as the feed thread runs it.
    /// At most [`PREFIX_BUDGET`] prefixes are listed ([`choose_prefixes`]);
    /// a prefix left out this pass keeps what the cache holds, and its
    /// cached sessions are classified with the rest so its rows stand. The
    /// session ceiling chooses among the listed sessions and the cached ones
    /// together, and the cache keeps only the sessions it chose. A
    /// workspace whose listing failed keeps its cache entry and is left to
    /// the holder's kept rows, as without a cache. A summary or marker whose
    /// listed ETag is unchanged is reused, not fetched, and so is the time a
    /// far-future summary time or LastModified was clamped to, so the row
    /// ages from its first read. A session gone from an answered listing,
    /// and a prefix gone from the top level, leave the cache.
    pub fn scan_cached(&self, local: &LocalView, cache: &mut FeedCache) -> RemoteSnapshot {
        self.scan_with(local, Some(cache))
    }

    fn scan_with(&self, local: &LocalView, cache: Option<&mut FeedCache>) -> RemoteSnapshot {
        let (all, mut truncated) = match self.workspaces() {
            Ok(w) => w,
            Err(e) => {
                return RemoteSnapshot {
                    failure: Some(self.failure(e.class(), Vec::new())),
                    ..RemoteSnapshot::default()
                }
            }
        };
        let now = SystemTime::now();
        let workspaces = match cache.as_deref() {
            Some(cache) => choose_prefixes(&all, cache, now, PREFIX_BUDGET),
            None => all.clone(),
        };
        let listed = fan_out(&workspaces, |ws| self.list_sessions(ws));

        // The listed sessions, their far-future times clamped, each held
        // against what the cache last had for it.
        let previous = cache.as_deref();
        let mut failed: Vec<(String, FailureClass)> = Vec::new();
        let mut fresh: Vec<FreshWorkspace> = Vec::new();
        for (ws, result) in workspaces.iter().zip(listed) {
            let (listings, cut) = match result {
                Ok(listed) => listed,
                Err(e) => {
                    failed.push((ws.clone(), e.class()));
                    continue;
                }
            };
            truncated |= cut;
            let before = previous.and_then(|c| c.sessions(ws));
            let sessions: Vec<ScannedSession> = listings
                .into_iter()
                .map(|(name, mut listing)| {
                    let was = before.and_then(|b| b.get(&name)).map(|s| &s.listing);
                    clamp_listing(&mut listing, was, now);
                    ScannedSession {
                        workspace: ws.clone(),
                        name,
                        listing,
                        summary: SummaryRead::Absent,
                        marker: None,
                    }
                })
                .collect();
            let newest = sessions
                .iter()
                .flat_map(|s| s.listing.objects.values())
                .filter_map(|o| o.last_modified)
                .max();
            fresh.push(FreshWorkspace {
                workspace: ws.clone(),
                newest,
                truncated: cut,
                sessions,
            });
        }

        // The session ceiling, over the listed sessions and, through a
        // cache, the cached ones of every prefix not listed this pass.
        let counted = |s: &ScannedSession| {
            s.listing.state(&s.name).is_some()
                && !is_internal_name(&s.name)
                && !local.homes.contains(&(s.workspace.clone(), s.name.clone()))
        };
        let listed_now: HashSet<&str> = workspaces.iter().map(String::as_str).collect();
        let cached_elsewhere = previous.into_iter().flat_map(|c| {
            c.workspaces
                .iter()
                .filter(|(ws, _)| !listed_now.contains(ws.as_str()))
                .flat_map(|(_, w)| w.sessions.values())
        });
        let candidates: Vec<SessionCandidate<'_>> = fresh
            .iter()
            .flat_map(|w| w.sessions.iter())
            .chain(cached_elsewhere)
            .filter(|s| counted(s))
            .map(|s| {
                let modified = s.listing.state(&s.name).and_then(|o| o.last_modified);
                (s.workspace.as_str(), s.name.as_str(), modified)
            })
            .collect();
        let keep = keep_most_recent(&candidates, MAX_SESSIONS);
        truncated |= keep.is_some();
        let kept = |s: &ScannedSession| {
            !counted(s)
                || keep
                    .as_ref()
                    .is_none_or(|k| k.contains(&(s.workspace.clone(), s.name.clone())))
        };

        // Read what the kept listed sessions need, a workspace per thread.
        let reads = fan_out(&fresh, |w| {
            let before = previous.and_then(|c| c.sessions(&w.workspace));
            w.sessions
                .iter()
                .map(|s| {
                    kept(s).then(|| self.read_session(s, before.and_then(|b| b.get(&s.name)), now))
                })
                .collect::<Vec<_>>()
        });
        for (w, reads) in fresh.iter_mut().zip(reads) {
            let sessions = std::mem::take(&mut w.sessions);
            w.sessions = sessions
                .into_iter()
                .zip(reads)
                .filter_map(|(mut s, read)| {
                    (s.summary, s.marker) = read?;
                    Some(s)
                })
                .collect();
        }

        let mut snapshot = RemoteSnapshot::default();
        let mut scanned = Vec::new();
        match cache {
            None => {
                for w in fresh {
                    scanned.extend(w.sessions);
                }
            }
            Some(cache) => {
                cache.scans += 1;
                let scan = cache.scans;
                let present: HashSet<&str> = all.iter().map(String::as_str).collect();
                cache
                    .workspaces
                    .retain(|ws, _| present.contains(ws.as_str()));
                for (ws, _) in &failed {
                    cache.workspaces.entry(ws.clone()).or_default().listed_at = scan;
                }
                for w in fresh {
                    let entry = cache.workspaces.entry(w.workspace).or_default();
                    entry.listed_at = scan;
                    entry.newest = w.newest;
                    entry.truncated = w.truncated;
                    entry.sessions = w
                        .sessions
                        .into_iter()
                        .map(|s| (s.name.clone(), s))
                        .collect();
                }
                for (ws, entry) in cache.workspaces.iter_mut() {
                    if !listed_now.contains(ws.as_str()) {
                        entry.sessions.retain(|_, s| kept(s));
                    }
                }
                let failed_now: HashSet<&str> = failed.iter().map(|(ws, _)| ws.as_str()).collect();
                for (ws, entry) in &cache.workspaces {
                    if !failed_now.contains(ws.as_str()) {
                        truncated |= entry.truncated;
                        scanned.extend(entry.sessions.values().cloned());
                    }
                }
                scanned.sort_by(|a, b| (&a.workspace, &a.name).cmp(&(&b.workspace, &b.name)));
            }
        }
        failed.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(&(_, class)) = failed.first() {
            let workspaces = failed.into_iter().map(|(ws, _)| ws).collect();
            snapshot.failure = Some(self.failure(class, workspaces));
        }
        let classified = classify(&scanned, local, now);
        snapshot.rows = classified.rows;
        snapshot.unreadable = classified.unreadable;
        snapshot.hidden_sources = classified.hidden_sources;
        snapshot.truncated = truncated;
        snapshot
    }
}

/// One workspace a pass listed: its sessions, the newest LastModified the
/// listing showed and whether it stopped at [`MAX_PAGES`].
struct FreshWorkspace {
    workspace: String,
    newest: Option<SystemTime>,
    truncated: bool,
    sessions: Vec<ScannedSession>,
}

/// How long the feed thread waits between the end of one scan and the
/// start of the next.
pub const RESCAN_INTERVAL: Duration = Duration::from_secs(55);

/// How often a waiting feed thread looks at its stop flag.
const STOP_POLL: Duration = Duration::from_millis(100);

/// The interactive dashboard's view of the bucket: a background thread that
/// scans at start and then every `interval`, and sends each complete
/// snapshot to the thread holding this handle.
///
/// The thread owns its own [`RemoteReader`]; the holder never touches a
/// bucket. Dropping the handle tells the thread to stop, and nothing waits
/// for it: a thread stuck in a request that never answers ends with the
/// process.
pub struct RemoteFeed {
    snapshots: Receiver<RemoteSnapshot>,
    local: Arc<Mutex<LocalView>>,
    stop: Arc<AtomicBool>,
}

impl RemoteFeed {
    /// Start the feed thread on a copy of `bucket`. `local` describes the
    /// sessions this machine holds, which every scan leaves to their local
    /// rows; [`RemoteFeed::set_local`] replaces it. Starting makes no
    /// request on the calling thread. When the reader can't be built
    /// ([`RemoteReader::new`]) the error is its [`ReadFailure`].
    pub fn spawn(bucket: &Bucket, local: LocalView, interval: Duration) -> anyhow::Result<Self> {
        let reader = RemoteReader::new(bucket)?;
        let (tx, snapshots) = mpsc::channel();
        let local = Arc::new(Mutex::new(local));
        let stop = Arc::new(AtomicBool::new(false));
        let (view, stopped) = (Arc::clone(&local), Arc::clone(&stop));
        let mut cache = FeedCache::new();
        std::thread::Builder::new()
            .name("koto-remote-feed".to_string())
            .spawn(move || loop {
                if stopped.load(Ordering::Relaxed) {
                    return;
                }
                let view = view.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let snapshot = reader.scan_cached(&view, &mut cache);
                if stopped.load(Ordering::Relaxed) || tx.send(snapshot).is_err() {
                    return;
                }
                let until = Instant::now() + interval;
                while Instant::now() < until {
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(
                        STOP_POLL.min(until.saturating_duration_since(Instant::now())),
                    );
                }
            })?;
        Ok(Self {
            snapshots,
            local,
            stop,
        })
    }

    /// The newest snapshot the thread has sent since the last call, if any;
    /// older ones still queued are dropped. Never blocks.
    pub fn latest(&self) -> Option<RemoteSnapshot> {
        let mut latest = None;
        loop {
            match self.snapshots.try_recv() {
                Ok(snapshot) => latest = Some(snapshot),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return latest,
            }
        }
    }

    /// Replace what the next scan knows about this machine's sessions.
    pub fn set_local(&self, local: LocalView) {
        *self.local.lock().unwrap_or_else(|e| e.into_inner()) = local;
    }
}

impl Drop for RemoteFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WS: &str = "0123456789abcdef";
    const OTHER: &str = "fedcba9876543210";

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn obj(etag: &str, age_secs: u64) -> ListedObject {
        ListedObject {
            size: 10,
            etag: etag.to_string(),
            last_modified: Some(now() - Duration::from_secs(age_secs)),
        }
    }

    fn summary(ws: &str, name: &str, id: &str, state_etag: &str) -> SessionSummary {
        SessionSummary {
            schema: crate::session::summary::SUMMARY_SCHEMA,
            session_id: id.to_string(),
            name: name.to_string(),
            home: ws.to_string(),
            kind: None,
            parent: None,
            created_at: "2026-10-31T00:00:00Z".to_string(),
            template_name: Some("t".to_string()),
            current_state: Some("check".to_string()),
            is_terminal: false,
            is_blocked: true,
            failed: false,
            intent: None,
            salient_var: None,
            is_batch_task: false,
            is_request_leg: false,
            last_event_at: None,
            last_seq: 2,
            state_etag: state_etag.to_string(),
            machine: Some("machine-a".to_string()),
            sources: Vec::new(),
        }
    }

    /// A session directory with a state file (ETag `s`, 600 s old) and,
    /// when `read` is given, a summary describing it.
    fn session(ws: &str, name: &str, summary_read: SummaryRead) -> ScannedSession {
        let mut listing = SessionListing::default();
        listing.objects.insert(state_file_name(name), obj("s", 600));
        if !matches!(summary_read, SummaryRead::Absent) {
            listing
                .objects
                .insert(SUMMARY_FILE.to_string(), obj("x", 600));
        }
        ScannedSession {
            workspace: ws.to_string(),
            name: name.to_string(),
            listing,
            summary: summary_read,
            marker: None,
        }
    }

    fn shown(ws: &str, name: &str, id: &str) -> ScannedSession {
        session(
            ws,
            name,
            SummaryRead::Read(Box::new(summary(ws, name, id, "s"))),
        )
    }

    fn names(c: &Classified) -> Vec<(&str, &str, bool)> {
        c.rows
            .iter()
            .map(|r| {
                (
                    r.workspace.as_str(),
                    r.name.as_str(),
                    matches!(r.status, RowStatus::Shown(_)),
                )
            })
            .collect()
    }

    #[test]
    fn group_sessions_keeps_only_top_level_objects_with_their_facts() {
        let o = |size| ListedObject {
            size,
            etag: "e".to_string(),
            last_modified: None,
        };
        let keys = [
            ("0123456789abcdef/review/koto-review.state.jsonl", o(10)),
            ("0123456789abcdef/review/summary.json", o(5)),
            ("0123456789abcdef/review/backrefs/x.json", o(1)),
            ("0123456789abcdef/other/koto-other.state.jsonl", o(3)),
            ("fedcba9876543210/review/summary.json", o(5)),
            ("0123456789abcdef/loose.json", o(1)),
        ];
        let sessions = group_sessions(WS, keys);
        assert_eq!(sessions.keys().collect::<Vec<_>>(), vec!["other", "review"]);
        let review = &sessions["review"].objects;
        assert_eq!(review.len(), 2);
        assert_eq!(review.get("summary.json"), Some(&o(5)));
        assert!(sessions["review"].state("review").is_some());
    }

    #[test]
    fn rule_1_a_directory_without_a_state_object_is_dropped() {
        let mut s = shown(WS, "gone", "id");
        s.listing.objects.remove(&state_file_name("gone"));
        let c = classify(&[s], &LocalView::default(), now());
        assert_eq!(c, Classified::default());
    }

    #[test]
    fn rule_2_an_internal_name_is_dropped_even_without_a_summary() {
        let scan = [
            session(WS, "wf~1", SummaryRead::Absent),
            session(WS, "anchor-77d6", SummaryRead::Absent),
            session(WS, "anchor-77d6-closed", SummaryRead::Absent),
        ];
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(c, Classified::default());
    }

    #[test]
    fn rule_3_a_local_copy_is_dropped_by_session_id() {
        let local = LocalView {
            session_ids: HashSet::from(["id".to_string()]),
            ..LocalView::default()
        };
        let c = classify(&[shown(WS, "wf", "id")], &local, now());
        assert_eq!(c, Classified::default());
    }

    #[test]
    fn rule_3_a_local_copy_without_a_readable_summary_is_dropped_by_home_and_name() {
        let local = LocalView {
            homes: HashSet::from([(WS.to_string(), "wf".to_string())]),
            ..LocalView::default()
        };
        let scan = [
            session(WS, "wf", SummaryRead::Absent),
            session(WS, "wf2", SummaryRead::Absent),
            session(OTHER, "wf", SummaryRead::Absent),
        ];
        let c = classify(&scan, &local, now());
        assert_eq!(names(&c), vec![(WS, "wf2", false), (OTHER, "wf", false)]);

        let truncated = [session(WS, "wf", SummaryRead::Unreadable("malformed"))];
        assert_eq!(classify(&truncated, &local, now()), Classified::default());

        // A readable summary is matched by session_id only.
        let c = classify(&[shown(WS, "wf", "other-id")], &local, now());
        assert_eq!(names(&c), vec![(WS, "wf", true)]);
    }

    #[test]
    fn rule_4_a_marker_naming_a_copy_in_the_scan_hides_the_source() {
        // The source has no summary, so rule 5 would list it as unknown.
        let mut source = session(WS, "wf", SummaryRead::Absent);
        source.marker = Some(MigratedTo {
            prefix: OTHER.to_string(),
            session: "copy".to_string(),
            session_id: Some("copy-id".to_string()),
        });
        let scan = [source, shown(OTHER, "copy", "copy-id")];
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(c.hidden_sources, 1);
        assert_eq!(c.unreadable, 0);
        assert_eq!(names(&c), vec![(OTHER, "copy", true)]);
    }

    #[test]
    fn rule_4_a_marker_whose_copy_is_not_in_the_scan_lists_the_source() {
        let mut source = shown(WS, "wf", "id");
        source.marker = Some(MigratedTo {
            prefix: OTHER.to_string(),
            session: "copy".to_string(),
            session_id: None,
        });
        // A copy directory with no state object isn't a copy in the scan.
        let mut stateless = shown(OTHER, "copy", "copy-id");
        stateless.listing.objects.remove(&state_file_name("copy"));
        let c = classify(&[source, stateless], &LocalView::default(), now());
        assert_eq!(c.hidden_sources, 0);
        assert_eq!(names(&c), vec![(WS, "wf", true)]);
    }

    #[test]
    fn rule_4_a_marker_naming_itself_or_a_copy_with_another_id_hides_nothing() {
        let mut source = shown(WS, "wf", "id");
        source.marker = Some(MigratedTo {
            prefix: OTHER.to_string(),
            session: "copy".to_string(),
            session_id: Some("copy-id".to_string()),
        });
        let mut selfish = shown(WS, "self", "self-id");
        selfish.marker = Some(MigratedTo {
            prefix: WS.to_string(),
            session: "self".to_string(),
            session_id: None,
        });
        let scan = [source, selfish, shown(OTHER, "copy", "someone-else")];
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(c.hidden_sources, 0);
        assert_eq!(c.rows.len(), 3);
    }

    #[test]
    fn rule_4_a_local_import_hides_its_source() {
        let local = LocalView {
            imports: vec![
                ImportedSource {
                    prefix: Some("1111111111111111".to_string()),
                    name: "elsewhere".to_string(),
                    session_id: "src-id".to_string(),
                },
                ImportedSource {
                    prefix: Some(WS.to_string()),
                    name: "legacy".to_string(),
                    session_id: "unknown-to-us".to_string(),
                },
            ],
            ..LocalView::default()
        };
        let scan = [
            shown(WS, "wf", "src-id"),
            session(WS, "legacy", SummaryRead::Absent),
            shown(WS, "legacy2", "other"),
        ];
        let c = classify(&scan, &local, now());
        assert_eq!(c.hidden_sources, 2);
        assert_eq!(names(&c), vec![(WS, "legacy2", true)]);
    }

    #[test]
    fn rule_5_no_summary_lists_unknown_with_the_state_files_time() {
        let c = classify(
            &[session(WS, "old", SummaryRead::Absent)],
            &LocalView::default(),
            now(),
        );
        assert_eq!(c.unreadable, 1);
        assert_eq!(
            c.rows[0].status,
            RowStatus::Unknown {
                reason: "no-summary",
                state_modified: Some(now() - Duration::from_secs(600)),
                session_id: None,
            }
        );
    }

    #[test]
    fn rule_6_an_unreadable_summary_lists_unknown() {
        let scan = [
            session(WS, "a", SummaryRead::Unreadable("malformed")),
            session(WS, "b", SummaryRead::Unreadable("oversized")),
        ];
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(c.unreadable, 2);
        assert_eq!(names(&c), vec![(WS, "a", false), (WS, "b", false)]);
    }

    #[test]
    fn rule_7_a_stale_summary_reads_unknown_only_past_60_seconds() {
        let at = |age: u64, etag: &str| {
            let mut s = shown(WS, "wf", "id");
            s.listing
                .objects
                .insert(state_file_name("wf"), obj(etag, age));
            classify(&[s], &LocalView::default(), now())
        };
        let fresh = at(10, "new");
        assert_eq!(
            (fresh.unreadable, names(&fresh)),
            (0, vec![(WS, "wf", true)])
        );
        let edge = at(60, "new");
        assert_eq!(edge.unreadable, 0, "60 s is still inside the window");
        let old = at(120, "new");
        assert_eq!((old.unreadable, names(&old)), (1, vec![(WS, "wf", false)]));
        assert!(matches!(
            &old.rows[0].status,
            RowStatus::Unknown { reason: "stale", session_id: Some(id), .. } if id == "id"
        ));
        let same = at(120, "s");
        assert_eq!(same.unreadable, 0, "a matching ETag is never stale");

        // A quoted ETag in the summary matches its unquoted listing.
        let mut s = session(
            WS,
            "q",
            SummaryRead::Read(Box::new(summary(WS, "q", "id", "\"s\""))),
        );
        s.listing
            .objects
            .insert(state_file_name("q"), obj("s", 120));
        assert_eq!(classify(&[s], &LocalView::default(), now()).unreadable, 0);

        // A LastModified that didn't parse counts as old.
        let mut s = shown(WS, "wf", "id");
        s.listing.objects.insert(
            state_file_name("wf"),
            ListedObject {
                size: 1,
                etag: "new".to_string(),
                last_modified: None,
            },
        );
        assert_eq!(classify(&[s], &LocalView::default(), now()).unreadable, 1);
    }

    #[test]
    fn rule_8_otherwise_the_row_is_shown_from_its_summary_in_order() {
        let scan = [
            shown(OTHER, "b", "1"),
            shown(WS, "z", "2"),
            shown(WS, "a", "3"),
        ];
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(
            names(&c),
            vec![(WS, "a", true), (WS, "z", true), (OTHER, "b", true)]
        );
        assert_eq!((c.unreadable, c.hidden_sources), (0, 0));
    }

    // -- sources ------------------------------------------------------------

    /// A shown session `name` whose summary names each of `sources`
    /// (`prefix`, `name`, `session_id`).
    fn naming(ws: &str, name: &str, id: &str, sources: &[(&str, &str, &str)]) -> ScannedSession {
        let mut s = summary(ws, name, id, "s");
        s.sources = sources
            .iter()
            .map(
                |&(prefix, name, session_id)| crate::session::summary::SummarySource {
                    relation: "imported".to_string(),
                    prefix: prefix.to_string(),
                    name: name.to_string(),
                    session_id: session_id.to_string(),
                },
            )
            .collect();
        session(ws, name, SummaryRead::Read(Box::new(s)))
    }

    /// `(workspace, name, receded_by_source)` of each row.
    fn receded(c: &Classified) -> Vec<(&str, &str, bool)> {
        c.rows
            .iter()
            .map(|r| (r.workspace.as_str(), r.name.as_str(), r.receded_by_source))
            .collect()
    }

    #[test]
    fn sources_an_entry_recedes_only_the_row_it_names() {
        let scan = [
            naming(WS, "x", "x-id", &[(OTHER, "y", "y-id")]),
            shown(OTHER, "y", "y-id"),
            shown(OTHER, "z", "z-id"),
        ];
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(
            receded(&c),
            vec![(WS, "x", false), (OTHER, "y", true), (OTHER, "z", false)]
        );
        // Receded, not hidden: Y is listed from its summary and not counted.
        assert_eq!(names(&c)[1], (OTHER, "y", true));
        assert_eq!((c.unreadable, c.hidden_sources), (0, 0));
    }

    #[test]
    fn sources_an_entry_naming_itself_or_another_session_id_recedes_nothing() {
        let scan = [
            naming(WS, "x", "x-id", &[(WS, "x", "x-id"), (OTHER, "y", "not-y")]),
            naming(WS, "w", "w-id", &[(WS, "w", "w-other-id")]),
            shown(OTHER, "y", "y-id"),
        ];
        let c = classify(&scan, &LocalView::default(), now());
        assert!(c.rows.iter().all(|r| !r.receded_by_source), "{c:?}");
        assert_eq!((c.unreadable, c.hidden_sources), (0, 0));
    }

    #[test]
    fn sources_an_entry_naming_a_local_session_leaves_it_to_its_local_row() {
        let local = LocalView {
            session_ids: ["mine-id".to_string()].into_iter().collect(),
            ..LocalView::default()
        };
        let scan = [
            naming(WS, "x", "x-id", &[(OTHER, "mine", "mine-id")]),
            shown(OTHER, "mine", "mine-id"),
        ];
        let c = classify(&scan, &local, now());
        assert_eq!(receded(&c), vec![(WS, "x", false)]);
        assert_eq!(c.hidden_sources, 0);
    }

    #[test]
    fn sources_never_hide_a_session_without_its_own_marker() {
        let mut scan: Vec<ScannedSession> = (0..16)
            .map(|i| {
                naming(
                    WS,
                    &format!("x{i:02}"),
                    &format!("x{i}-id"),
                    &[(OTHER, "y", "y-id")],
                )
            })
            .collect();
        scan.push(shown(OTHER, "y", "y-id"));
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(c.rows.len(), 17);
        assert_eq!(c.hidden_sources, 0);
        assert!(receded(&c).contains(&(OTHER, "y", true)));

        // Only a marker in Y's own directory, naming a copy in the scan,
        // hides it, and it is counted.
        let mut y = shown(OTHER, "y", "y-id");
        y.marker = Some(MigratedTo {
            prefix: WS.to_string(),
            session: "x00".to_string(),
            session_id: Some("x0-id".to_string()),
        });
        *scan.last_mut().unwrap() = y;
        let c = classify(&scan, &LocalView::default(), now());
        assert_eq!(c.rows.len(), 16);
        assert_eq!(c.hidden_sources, 1);
    }

    #[test]
    fn sources_of_an_unreadable_or_stale_summary_recede_nothing() {
        let mut stale = naming(WS, "x", "x-id", &[(OTHER, "y", "y-id")]);
        if let SummaryRead::Read(s) = &mut stale.summary {
            s.state_etag = "older".to_string();
        }
        let scan = [
            stale,
            session(WS, "u", SummaryRead::Unreadable("sources-cap")),
            shown(OTHER, "y", "y-id"),
        ];
        let c = classify(&scan, &LocalView::default(), now());
        assert!(c.rows.iter().all(|r| !r.receded_by_source), "{c:?}");
        assert_eq!(c.unreadable, 2);
    }

    // -- failure classes ----------------------------------------------------

    fn atto(kind: std::io::ErrorKind) -> S3Error {
        S3Error::Atto(attohttpc::Error::from(std::io::Error::from(kind)))
    }

    /// A client of a bucket at `endpoint` whose requests fail after
    /// `timeout`.
    fn bucket_at(endpoint: &str, timeout: Duration) -> Box<Bucket> {
        let region = s3::Region::Custom {
            region: "us-east-1".to_string(),
            endpoint: endpoint.to_string(),
        };
        let credentials =
            s3::creds::Credentials::new(Some("k"), Some("s"), None, None, None).unwrap();
        Bucket::new("b", region, credentials)
            .unwrap()
            .with_path_style()
            .with_request_timeout(timeout)
            .unwrap()
    }

    fn list_error(bucket: &Bucket) -> S3Error {
        bucket
            .list(String::new(), Some("/".to_string()))
            .expect_err("the listing fails")
    }

    #[test]
    fn failure_class_unreachable_for_a_refused_connection_dns_a_5xx_or_a_bad_body() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let refused = list_error(&bucket_at(
            &format!("http://127.0.0.1:{port}"),
            Duration::from_secs(5),
        ));
        assert!(matches!(refused, S3Error::Atto(_)), "{refused:?}");
        for e in [
            refused,
            atto(std::io::ErrorKind::ConnectionRefused),
            S3Error::Atto(attohttpc::Error::from(std::io::Error::other(
                "failed to lookup address information",
            ))),
            S3Error::HttpFailWithBody(500, String::new()),
            S3Error::HttpFailWithBody(502, String::new()),
            S3Error::HttpFailWithBody(504, String::new()),
            S3Error::HttpFail,
            S3Error::Io(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
        ] {
            assert_eq!(failure_class(&e), FailureClass::Unreachable, "{e:?}");
        }
        assert_eq!(FailureClass::Unreachable.name(), "unreachable");
    }

    #[test]
    fn failure_class_denied_for_401_and_403() {
        for status in [401, 403] {
            let e = S3Error::HttpFailWithBody(status, "<Error/>".to_string());
            assert_eq!(failure_class(&e), FailureClass::Denied, "{status}");
        }
        assert_eq!(FailureClass::Denied.name(), "denied");
    }

    #[test]
    fn failure_class_timeout_for_a_request_that_gets_no_answer() {
        // A listener that accepts and never answers: the request deadline
        // ends the request.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let held: Vec<_> = listener.incoming().take(4).collect();
            std::thread::sleep(Duration::from_secs(30));
            drop(held);
        });
        let silent = list_error(&bucket_at(&endpoint, Duration::from_millis(200)));
        for e in [
            silent,
            atto(std::io::ErrorKind::TimedOut),
            atto(std::io::ErrorKind::WouldBlock),
            S3Error::Io(std::io::Error::from(std::io::ErrorKind::TimedOut)),
        ] {
            assert_eq!(failure_class(&e), FailureClass::Timeout, "{e:?}");
        }
        assert_eq!(FailureClass::Timeout.name(), "timeout");
    }

    #[test]
    fn a_transport_failure_once_the_request_timeout_passed_is_a_timeout() {
        // A stalled TLS handshake the deadline ends by shutting its socket.
        let eof = || atto(std::io::ErrorKind::UnexpectedEof);
        assert_eq!(
            failure_class_after(&eof(), REQUEST_TIMEOUT),
            FailureClass::Timeout
        );
        assert_eq!(
            failure_class_after(&eof(), REQUEST_TIMEOUT + Duration::from_secs(11)),
            FailureClass::Timeout
        );
        assert_eq!(
            failure_class_after(
                &S3Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
                REQUEST_TIMEOUT
            ),
            FailureClass::Timeout
        );
        // Before the deadline the same failure is unreachable.
        assert_eq!(
            failure_class_after(&eof(), REQUEST_TIMEOUT - Duration::from_millis(1)),
            FailureClass::Unreachable
        );
        // An answer is classed by its status however long it took.
        for (e, class) in [
            (
                S3Error::HttpFailWithBody(500, String::new()),
                FailureClass::Unreachable,
            ),
            (
                S3Error::HttpFailWithBody(403, String::new()),
                FailureClass::Denied,
            ),
            (
                S3Error::HttpFailWithBody(503, String::new()),
                FailureClass::Throttled,
            ),
            (S3Error::HttpFail, FailureClass::Unreachable),
        ] {
            assert_eq!(failure_class_after(&e, REQUEST_TIMEOUT * 2), class, "{e:?}");
        }
        let failure = ListFailure {
            error: eof(),
            took: REQUEST_TIMEOUT,
        };
        assert_eq!(failure.class(), FailureClass::Timeout);
    }

    #[test]
    fn failure_class_throttled_for_429_and_503() {
        for status in [429, 503] {
            let e = S3Error::HttpFailWithBody(status, "<Error/>".to_string());
            assert_eq!(failure_class(&e), FailureClass::Throttled, "{status}");
        }
        assert_eq!(FailureClass::Throttled.name(), "throttled");
    }

    #[test]
    fn host_of_drops_userinfo_port_and_path() {
        assert_eq!(host_of("127.0.0.1:9000"), "127.0.0.1");
        assert_eq!(
            host_of("user:secret@minio.example:9000/base"),
            "minio.example"
        );
        assert_eq!(host_of("s3.amazonaws.com"), "s3.amazonaws.com");
        assert_eq!(host_of("[::1]:9000"), "[::1]");
        assert_eq!(
            host_of(
                &bucket_at("http://k:s@10.0.0.7:9000/", REQUEST_TIMEOUT)
                    .region()
                    .host()
            ),
            "10.0.0.7"
        );
    }

    #[test]
    fn host_of_never_shows_a_userinfo_fragment_holding_a_path_delimiter() {
        for endpoint in [
            "AKIA:abc/def@host/",
            "AKIA:abc?def@host:9000/x",
            "AKIA:abc#def@host",
            "https://AKIA:abc/def@host/",
            "AKIA:a@b/c@host:9000",
        ] {
            assert_eq!(host_of(endpoint), "host", "{endpoint}");
        }
        let host = host_of(
            &bucket_at("https://AKIA:abc/def@10.0.0.7:9000/", REQUEST_TIMEOUT)
                .region()
                .host(),
        );
        assert!(
            !host.contains("AKIA") && !host.contains("abc") && !host.contains("def"),
            "{host}"
        );
    }

    #[test]
    fn a_reader_failure_reads_as_its_line() {
        let failure = ReadFailure {
            class: FailureClass::Unreachable,
            host: "10.0.0.7".to_string(),
            top_level: true,
            workspaces: Vec::new(),
        };
        assert_eq!(failure.to_string(), failure.line());
    }

    #[test]
    fn a_read_failure_line_names_the_class_and_the_sanitized_host() {
        let top = ReadFailure {
            class: FailureClass::Denied,
            host: "evil\u{1b}[2J\u{202e}host\nx".to_string(),
            top_level: true,
            workspaces: Vec::new(),
        };
        let line = top.line();
        assert_eq!(
            line,
            "denied at evil\\u{1B}[2J\\u{202E}host x: the bucket couldn't be listed"
        );
        assert!(!line.chars().any(|c| c.is_control()), "{line:?}");
        assert!(top.keeps(WS) && top.keeps(OTHER));

        let some = ReadFailure {
            class: FailureClass::Unreachable,
            host: "127.0.0.1".to_string(),
            top_level: false,
            workspaces: vec![WS.to_string()],
        };
        assert_eq!(
            some.line(),
            "1 workspace couldn't be read; the first: unreachable at 127.0.0.1"
        );
        assert!(some.keeps(WS) && !some.keeps(OTHER));
        let two = ReadFailure {
            workspaces: vec![WS.to_string(), OTHER.to_string()],
            ..some
        };
        assert!(two.line().starts_with("2 workspaces couldn't be read"));
    }

    /// A cache that last listed each `(prefix, scan, newest age in secs)`.
    fn cache_of(entries: &[(&str, u64, u64)]) -> FeedCache {
        let mut cache = FeedCache::new();
        for &(ws, listed_at, age) in entries {
            cache.workspaces.insert(
                ws.to_string(),
                CachedWorkspace {
                    listed_at,
                    newest: Some(now() - Duration::from_secs(age)),
                    truncated: false,
                    sessions: BTreeMap::new(),
                },
            );
        }
        cache.scans = entries.iter().map(|e| e.1).max().unwrap_or(0);
        cache
    }

    fn prefixes(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{i:016x}")).collect()
    }

    #[test]
    fn remote_feed_budget_lists_every_prefix_at_or_under_it() {
        let all = prefixes(3);
        let cache = cache_of(&[(&all[0], 1, 7200)]);
        assert_eq!(choose_prefixes(&all, &cache, now(), 3), all);
    }

    #[test]
    fn remote_feed_budget_puts_new_then_active_then_longest_unlisted() {
        let all = prefixes(6);
        // 0 and 1 never listed; 2 active (changed 60 s ago); 3, 4, 5 idle,
        // listed at scans 3, 1 and 2.
        let cache = cache_of(&[
            (&all[2], 3, 60),
            (&all[3], 3, 7200),
            (&all[4], 1, 7200),
            (&all[5], 2, 7200),
        ]);
        let chosen = choose_prefixes(&all, &cache, now(), 4);
        assert_eq!(
            chosen,
            vec![
                all[0].clone(),
                all[1].clone(),
                all[2].clone(),
                all[4].clone()
            ]
        );
        let chosen = choose_prefixes(&all, &cache, now(), 5);
        assert_eq!(chosen[4], all[5]);
    }

    #[test]
    fn remote_feed_budget_never_exceeds_it_and_rotates_idle_prefixes() {
        let all = prefixes(250);
        let mut cache = FeedCache::new();
        let mut seen: Vec<HashSet<String>> = Vec::new();
        for scan in 1..=3u64 {
            let chosen = choose_prefixes(&all, &cache, now(), PREFIX_BUDGET);
            assert_eq!(chosen.len(), PREFIX_BUDGET, "scan {scan}");
            for ws in &chosen {
                let entry = cache.workspaces.entry(ws.clone()).or_default();
                entry.listed_at = scan;
                entry.newest = Some(now() - Duration::from_secs(7200));
            }
            seen.push(chosen.into_iter().collect());
        }
        // The 50 left out of the first scan are listed on the second, and
        // every prefix is listed in any two consecutive scans.
        for pair in seen.windows(2) {
            for ws in &all {
                assert!(
                    pair[0].contains(ws) || pair[1].contains(ws),
                    "{ws} unlisted for two scans"
                );
            }
        }
    }

    #[test]
    fn remote_feed_budget_lists_a_changed_idle_prefix_every_scan() {
        let mut all = prefixes(201);
        let cache = cache_of(
            &all.iter()
                .enumerate()
                .map(|(i, ws)| (ws.as_str(), 5, if i == 200 { 10 } else { 7200 }))
                .collect::<Vec<_>>(),
        );
        assert!(choose_prefixes(&all, &cache, now(), 200).contains(&all[200]));
        all.push("ffffffffffffffff".to_string());
        assert!(choose_prefixes(&all, &cache, now(), 200).contains(&all[201]));
    }

    /// Each character of `range`, between two letters, comes out of the
    /// sanitizer as those letters with one space between them.
    fn assert_spaced(range: impl IntoIterator<Item = char>) {
        let mut seen = 0;
        for c in range {
            assert_eq!(
                sanitize_bucket_text(&format!("a{c}b")),
                "a b",
                "U+{:04X}",
                c as u32
            );
            seen += 1;
        }
        assert!(seen > 0);
    }

    #[test]
    fn remote_feed_sanitizer_spaces_each_c0_control() {
        assert_spaced('\u{0}'..='\u{1F}');
        assert_eq!(sanitize_bucket_text("a\u{1B}b"), "a b");
        assert_eq!(sanitize_bucket_text("a\t\n\rb"), "a   b");
    }

    #[test]
    fn remote_feed_sanitizer_spaces_del() {
        assert_spaced(['\u{7F}']);
    }

    #[test]
    fn remote_feed_sanitizer_spaces_each_c1_control() {
        assert_spaced('\u{80}'..='\u{9F}');
    }

    #[test]
    fn remote_feed_sanitizer_spaces_the_line_and_paragraph_separators() {
        assert_spaced(['\u{2028}', '\u{2029}']);
    }

    #[test]
    fn remote_feed_sanitizer_spaces_each_bidi_embedding_and_override() {
        assert_spaced('\u{202A}'..='\u{202E}');
    }

    #[test]
    fn remote_feed_sanitizer_spaces_each_bidi_isolate() {
        assert_spaced('\u{2066}'..='\u{2069}');
    }

    #[test]
    fn remote_feed_sanitizer_spaces_each_zero_width_and_directional_mark() {
        assert_spaced('\u{200B}'..='\u{200F}');
    }

    #[test]
    fn remote_feed_sanitizer_spaces_the_arabic_letter_mark() {
        assert_spaced(['\u{061C}']);
    }

    #[test]
    fn remote_feed_sanitizer_spaces_the_byte_order_mark() {
        assert_spaced(['\u{FEFF}']);
    }

    #[test]
    fn remote_feed_sanitizer_spaces_each_tag_character() {
        assert_spaced('\u{E0000}'..='\u{E007F}');
    }

    #[test]
    fn remote_feed_sanitizer_keeps_other_text_as_it_is() {
        let kept = "caf\u{e9} \u{65e5}\u{672c} \\u{1B} a-b_c.d ~ @ \u{2030} \u{2070} \u{E0080}";
        assert_eq!(sanitize_bucket_text(kept), kept);
        // Each hazard is one space, never an escape, and the text around
        // it is untouched.
        assert_eq!(
            sanitize_bucket_text("caf\u{e9}\u{1B}[2J\u{1B}]0;owned\u{7}"),
            "caf\u{e9} [2J ]0;owned "
        );
    }

    #[test]
    fn remote_feed_url_decode_reads_the_listing_encoding() {
        assert_eq!(url_decode("a+b%09c%1Bd").as_deref(), Some("a b\tc\u{1B}d"));
        assert_eq!(
            url_decode("x%C2%85y%E2%80%A8z").as_deref(),
            Some("x\u{85}y\u{2028}z")
        );
        assert_eq!(url_decode("ws%2Fname%2B").as_deref(), Some("ws/name+"));
        assert_eq!(
            url_decode("plain-name_1.2~3").as_deref(),
            Some("plain-name_1.2~3")
        );
        assert_eq!(url_decode("bad%G1"), None);
        assert_eq!(url_decode("cut%4"), None);
        assert_eq!(url_decode("%FF"), None, "not UTF-8");
    }

    #[test]
    fn remote_feed_bounded_sink_refuses_bytes_past_its_limit() {
        use std::io::Write;
        let mut sink = BoundedSink {
            bytes: Vec::new(),
            limit: 4,
        };
        assert!(sink.write_all(b"abc").is_ok());
        assert!(sink.write_all(b"defg").is_err());
        assert_eq!(sink.bytes, b"abcd");
        let mut exact = BoundedSink {
            bytes: Vec::new(),
            limit: 4,
        };
        assert!(std::io::copy(&mut &b"abcd"[..], &mut exact).is_ok());
        assert_eq!(exact.bytes, b"abcd");
    }

    // --- the clamp of far-future times ---

    fn at(offset: Duration, ahead: bool) -> SystemTime {
        if ahead {
            now() + offset
        } else {
            now() - offset
        }
    }

    fn clamped(created: SystemTime, last: SystemTime) -> SessionSummary {
        let mut s = summary(WS, "s", "id", "e");
        s.created_at = iso8601_at(created);
        s.last_event_at = Some(iso8601_at(last));
        clamp_summary_times(&mut s, now());
        s
    }

    #[test]
    fn remote_feed_clamp_keeps_a_time_up_to_five_minutes_ahead() {
        for offset in [Duration::ZERO, Duration::from_secs(240), FUTURE_SKEW] {
            let t = at(offset, true);
            let s = clamped(t, t);
            assert_eq!(s.created_at, iso8601_at(t), "{offset:?}");
            assert_eq!(s.last_event_at, Some(iso8601_at(t)), "{offset:?}");
        }
        let past = at(Duration::from_secs(3600), false);
        assert_eq!(clamped(past, past).created_at, iso8601_at(past));
    }

    #[test]
    fn remote_feed_clamp_brings_a_time_past_five_minutes_to_the_reader_clock() {
        for offset in [
            FUTURE_SKEW + Duration::from_millis(1),
            Duration::from_secs(3600),
            Duration::from_secs(400 * 365 * 86400),
        ] {
            let t = at(offset, true);
            let s = clamped(t, t);
            assert_eq!(s.created_at, iso8601_at(now()), "{offset:?}");
            assert_eq!(s.last_event_at, Some(iso8601_at(now())), "{offset:?}");
        }
        // Each field on its own.
        let (ahead, past) = (
            at(Duration::from_secs(3600), true),
            at(Duration::ZERO, false),
        );
        let s = clamped(past, ahead);
        assert_eq!(s.created_at, iso8601_at(past));
        assert_eq!(s.last_event_at, Some(iso8601_at(now())));
    }

    #[test]
    fn remote_feed_clamp_leaves_an_unparsed_or_absent_time_alone() {
        let mut s = summary(WS, "s", "id", "e");
        s.created_at = "18446744073709551615-01-01T00:00:00Z".to_string();
        s.last_event_at = Some("yesterday".to_string());
        clamp_summary_times(&mut s, now());
        assert_eq!(s.created_at, "18446744073709551615-01-01T00:00:00Z");
        assert_eq!(s.last_event_at.as_deref(), Some("yesterday"));
        s.last_event_at = None;
        clamp_summary_times(&mut s, now());
        assert_eq!(s.last_event_at, None);
    }

    fn listing_at(etag: &str, modified: SystemTime) -> SessionListing {
        let mut listing = SessionListing::default();
        listing.objects.insert(
            state_file_name("s"),
            ListedObject {
                size: 10,
                etag: etag.to_string(),
                last_modified: Some(modified),
            },
        );
        listing
    }

    fn state_time(listing: &SessionListing) -> Option<SystemTime> {
        listing.state("s").and_then(|o| o.last_modified)
    }

    #[test]
    fn remote_feed_clamp_of_a_listed_time_holds_while_its_etag_is_unchanged() {
        let ahead = now() + Duration::from_secs(3600);
        let edge = now() + FUTURE_SKEW;
        let mut near = listing_at("s", edge);
        clamp_listing(&mut near, None, now());
        assert_eq!(state_time(&near), Some(edge), "five minutes ahead stands");

        let mut first = listing_at("s", ahead);
        clamp_listing(&mut first, None, now());
        assert_eq!(
            state_time(&first),
            Some(now()),
            "clamped to the reader's clock"
        );

        // A later scan lists the same object, as far ahead: it keeps the
        // time it was first clamped to, so its idle grows.
        let later = now() + Duration::from_secs(120);
        let mut again = listing_at("s", ahead);
        clamp_listing(&mut again, Some(&first), later);
        assert_eq!(state_time(&again), Some(now()));

        // A new object (another ETag) is clamped afresh.
        let mut changed = listing_at("t", ahead);
        clamp_listing(&mut changed, Some(&first), later);
        assert_eq!(state_time(&changed), Some(later));
    }

    // --- the session ceiling's choice ---

    fn candidates(spec: &[(&'static str, &'static str, u64)]) -> Vec<SessionCandidate<'static>> {
        spec.iter()
            .map(|&(ws, name, age)| (ws, name, Some(now() - Duration::from_secs(age))))
            .collect()
    }

    fn kept_names(keep: &HashSet<(String, String)>) -> Vec<(String, String)> {
        let mut names: Vec<_> = keep.iter().cloned().collect();
        names.sort();
        names
    }

    #[test]
    fn remote_feed_ceiling_keeps_every_session_at_or_under_it() {
        let all = candidates(&[(WS, "a", 5), (OTHER, "b", 1)]);
        assert_eq!(keep_most_recent(&all, 2), None);
        assert_eq!(keep_most_recent(&[], 0), None);
    }

    #[test]
    fn remote_feed_ceiling_takes_prefixes_by_their_newest_state_then_sessions_by_recency() {
        // OTHER holds the newest state file though it sorts last by key;
        // WS's newest session is older, so it is cut first.
        let all = candidates(&[
            (WS, "a", 100),
            (WS, "b", 50),
            (WS, "c", 300),
            (OTHER, "x", 10),
            (OTHER, "y", 900),
        ]);
        let keep = keep_most_recent(&all, 3).unwrap();
        let pair = |ws: &str, n: &str| (ws.to_string(), n.to_string());
        assert_eq!(
            kept_names(&keep),
            vec![pair(WS, "b"), pair(OTHER, "x"), pair(OTHER, "y")]
        );
        let keep = keep_most_recent(&all, 4).unwrap();
        assert!(keep.contains(&pair(WS, "a")) && !keep.contains(&pair(WS, "c")));
    }

    #[test]
    fn remote_feed_ceiling_puts_a_session_with_no_time_last_and_breaks_ties_by_name() {
        let mut all = candidates(&[(WS, "b", 10), (WS, "a", 10)]);
        all.push((OTHER, "z", None));
        let keep = keep_most_recent(&all, 1).unwrap();
        assert_eq!(kept_names(&keep), vec![(WS.to_string(), "a".to_string())]);
        let keep = keep_most_recent(&all, 2).unwrap();
        assert!(!keep.contains(&(OTHER.to_string(), "z".to_string())));
    }

    #[test]
    fn remote_feed_ceiling_keeps_exactly_its_limit_of_20000() {
        let names: Vec<String> = (0..MAX_SESSIONS + 50).map(|i| format!("s{i:05}")).collect();
        let all: Vec<SessionCandidate<'_>> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (WS, n.as_str(), Some(now() - Duration::from_secs(i as u64))))
            .collect();
        let keep = keep_most_recent(&all, MAX_SESSIONS).unwrap();
        assert_eq!(keep.len(), MAX_SESSIONS);
        assert!(keep.contains(&(WS.to_string(), names[0].clone())));
        assert!(!keep.contains(&(WS.to_string(), names[MAX_SESSIONS].clone())));
    }
}
