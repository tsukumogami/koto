//! A read-only, human-legible view of one session's contents.
//!
//! This module is the data layer behind the dashboard's per-session detail
//! and its one-shot `--detail` mode (DESIGN-session-view). It derives the
//! session's facts, lists every context key with its recorded metadata, and
//! reads bounded, terminal-safe previews of key content. Everything here is
//! a read: no context write, no event append, no migration-marker touch, and
//! no remote object fetch beyond the manifest and marker reads the backend's
//! metadata paths already make.
//!
//! Untrusted text is the module's whole input: context content, manifest key
//! names and fields, and header strings all come from stores another writer
//! may control. Every string headed for a surface leaves here as
//! [`Sanitized`], so a renderer that takes the type cannot print a raw
//! control character by omission.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use crate::engine::persistence::derive_machine_state;
use crate::engine::substitute::Variables;
use crate::session::context::KeyMeta;
use crate::session::validate::validate_context_key;
use crate::session::{Backend, SessionBackend, SessionMigrated};

/// Longest source excerpt of a text value, in bytes, before the cut.
/// The same figure as the session feed's secondary capture cut.
pub const EXCERPT_BOUND: usize = 4096;

/// Most keys a snapshot lists before it stops and counts the rest. A
/// visible, documented exception to listing completeness for manifests no
/// honest session produces.
pub const METADATA_CEILING: usize = 100_000;

/// Most keys the one-shot `--detail` mode reads content previews for;
/// keys past it render metadata only.
pub const DETAIL_PREVIEW_CEILING: usize = 1000;

/// Display cap, in characters, for a migration banner value.
pub const BANNER_VALUE_CAP: usize = 128;

/// Display cap, in characters, for a key name.
pub const KEY_DISPLAY_CAP: usize = 256;

/// Display cap, in characters, for a header fact (intent, anchor, ...).
pub const FACT_CAP: usize = 1024;

/// Marker appended where a cap or an excerpt bound cut text.
pub const CUT_MARKER: &str = "…[cut]";

// ---------------------------------------------------------------------------
// Sanitizer
// ---------------------------------------------------------------------------

/// A string every control and invisible-formatting character has been
/// escaped out of. Renderers accept this type, not `&str`, so an
/// unsanitized field is a compile error rather than a test gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sanitized(String);

impl Sanitized {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for Sanitized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether `c` must leave the sanitizer as a visible escape.
fn needs_escape(c: char) -> bool {
    match c {
        '\n' | '\t' => false,
        c if (c as u32) < 0x20 => true,    // C0 (less \n \t)
        '\u{7F}' => true,                  // DEL
        '\u{80}'..='\u{9F}' => true,       // C1, 8-bit CSI included
        '\u{202A}'..='\u{202E}' => true,   // bidi overrides/embeddings
        '\u{2066}'..='\u{2069}' => true,   // bidi isolates
        '\u{061C}' => true,                // Arabic letter mark
        '\u{200B}'..='\u{200F}' => true,   // zero-width + marks
        '\u{2060}'..='\u{2064}' => true,   // word joiner + invisibles
        '\u{180E}' => true,                // Mongolian vowel separator
        '\u{2028}' | '\u{2029}' => true,   // line/paragraph separator
        '\u{00AD}' => true,                // soft hyphen
        '\u{FFF9}'..='\u{FFFB}' => true,   // interlinear annotation
        '\u{FEFF}' => true,                // BOM / ZWNBSP
        '\u{E0000}'..='\u{E007F}' => true, // Unicode tag block
        _ => false,
    }
}

fn sanitize_impl(input: &str, fold_whitespace: bool) -> Sanitized {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if needs_escape(c) {
            out.push_str(&format!("\\u{{{:X}}}", c as u32));
        } else if fold_whitespace && (c == '\n' || c == '\t') {
            out.push(' ');
        } else if c == '\\' {
            // Keep the escape form unambiguous: a literal backslash that
            // would read as the start of a `\u{...}` escape is itself
            // escaped.
            let mut ahead = chars.clone();
            if ahead.next() == Some('u') && ahead.next() == Some('{') {
                out.push_str("\\u{5C}");
            } else {
                out.push(c);
            }
        } else {
            out.push(c);
        }
    }
    Sanitized(out)
}

/// Sanitize multi-line text (excerpts): newline and tab survive, every
/// other control or invisible-formatting character becomes `\u{XXXX}`.
pub fn sanitize(input: &str) -> Sanitized {
    sanitize_impl(input, false)
}

/// Sanitize for one-line contexts (table cells, the migration banner):
/// as [`sanitize`], with newline and tab folded to a space.
pub fn sanitize_one_line(input: &str) -> Sanitized {
    sanitize_impl(input, true)
}

/// [`sanitize_one_line`], capped at `cap` characters with a visible marker.
pub fn sanitize_capped(input: &str, cap: usize) -> Sanitized {
    let s = sanitize_one_line(input);
    if s.0.chars().count() <= cap {
        return s;
    }
    let cut: String = s.0.chars().take(cap).collect();
    Sanitized(format!("{cut}{CUT_MARKER}"))
}

// ---------------------------------------------------------------------------
// Size formatting
// ---------------------------------------------------------------------------

/// Render a byte count in binary units with one decimal: `0 B`, `512 B`,
/// `4.0 KiB`, `1.2 MiB`. Total over `u64::MAX` is impossible by type; the
/// unit walk never indexes past its table.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 1024.0 {
        // u64 cannot exceed 16 EiB, but stay total anyway.
        value = 1024.0;
    }
    format!("{:.1} {}", value, UNITS[unit])
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Why a snapshot could not be taken. A migrated session is NOT an error:
/// it comes back as a snapshot whose `migrated` is `Some`.
#[derive(Debug)]
pub enum SnapshotError {
    /// No session of that name exists in this store.
    NotFound,
    /// The session exists but its record cannot be derived (unparseable
    /// log, no derivable state).
    Corrupt(String),
    /// Reading failed for an environmental reason (I/O, remote).
    Infrastructure(String),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotError::NotFound => write!(f, "session not found"),
            SnapshotError::Corrupt(m) => write!(f, "corrupt session record: {m}"),
            SnapshotError::Infrastructure(m) => write!(f, "read failed: {m}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

/// Where the migration marker says the session went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedTo {
    pub target: Sanitized,
    pub workspace: Sanitized,
}

/// Whether a key's content is in the local store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locality {
    Local,
    RemoteOnly,
}

/// One context key as the listing shows it.
#[derive(Debug, Clone)]
pub struct KeyEntry {
    /// Display form: sanitized, capped at [`KEY_DISPLAY_CAP`].
    pub display: Sanitized,
    /// The raw key, present only when it passed `validate_context_key`;
    /// the only form ever joined to a path or read.
    pub raw: Option<String>,
    /// Recorded size in bytes, from the manifest.
    pub size: Option<u64>,
    /// Recorded writer, sanitized.
    pub writer: Option<Sanitized>,
    /// Recorded creation time, kept only when it parses as RFC 3339.
    pub created_at: Option<String>,
    pub locality: Locality,
    /// The local and remote manifests disagree on this key's hash.
    pub stale: bool,
}

/// The session facts a reader needs first. Every field a session might not
/// record is an `Option`, and absence is data, not an error.
#[derive(Debug, Clone)]
pub struct SessionFacts {
    pub name: String,
    pub session_id: Option<Sanitized>,
    /// Kept only when it parses as RFC 3339.
    pub created_at: Option<String>,
    pub intent: Option<Sanitized>,
    pub template_name: Option<Sanitized>,
    pub current_state: Option<Sanitized>,
    /// The current state's directive, interpolated the way `koto status`
    /// interpolates it. `None` for a terminal state or when the compiled
    /// template is unavailable.
    pub directive: Option<Sanitized>,
    pub execution_dir: Option<Sanitized>,
    /// The store origin: kind (`local` or `cloud`) and base path.
    pub store: Option<(Sanitized, Sanitized)>,
}

/// Everything [`snapshot`] answers.
#[derive(Debug, Clone)]
pub struct SessionViewSnapshot {
    pub facts: SessionFacts,
    /// `Some` when the session was imported elsewhere; `keys` is then
    /// empty and the facts are what the local copy still shows.
    pub migrated: Option<MigratedTo>,
    pub keys: Vec<KeyEntry>,
    /// Keys past [`METADATA_CEILING`] that were not listed.
    pub unlisted_keys: usize,
}

/// A bounded, classified rendering of one key's content.
#[derive(Debug, Clone)]
pub enum KeyPreview {
    /// Valid UTF-8 text, excerpted to the bound.
    Text { excerpt: Sanitized, truncated: bool },
    /// Not text. Size and hash are the manifest's RECORDED values, never
    /// computed here: hashing would be a full read. A hash that is not 64
    /// hex characters arrives as `None` and renders as invalid.
    Binary { size: u64, hash: Option<Sanitized> },
    /// No local content; the recorded size and hash describe the remote
    /// copy this view deliberately does not fetch.
    NotLocal { size: u64, hash: Option<Sanitized> },
    /// The manifest names the key but its content cannot be read.
    Unreadable { reason: Sanitized },
    /// The manifest entry's name fails the key grammar; nothing was read
    /// and no path was built from it.
    InvalidName,
}

// ---------------------------------------------------------------------------
// Directive rendering
// ---------------------------------------------------------------------------

/// Interpolate a state's directive (or details) text exactly the way
/// `koto status` does: runtime `SESSION_DIR` / `SESSION_NAME` first, then
/// the log's variables. `handle_status` and [`snapshot`] share this one
/// implementation.
pub fn render_directive(
    raw: &str,
    session_dir: &Path,
    session_name: &str,
    variables: &Variables,
) -> String {
    let mut runtime_vars = std::collections::HashMap::new();
    runtime_vars.insert(
        "SESSION_DIR".to_string(),
        session_dir.to_string_lossy().to_string(),
    );
    runtime_vars.insert("SESSION_NAME".to_string(), session_name.to_string());
    let with_runtime = crate::cli::vars::substitute_vars(raw, &runtime_vars);
    variables.substitute(&with_runtime)
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// Keep a recorded timestamp only when it has the RFC 3339 shape koto
/// writes (`YYYY-MM-DDTHH:MM:SS`, optional fraction, `Z` or an offset).
/// Anything else came from a writer koto did not control and is dropped
/// rather than echoed.
fn parse_timestamp(raw: &str) -> Option<String> {
    let b = raw.as_bytes();
    if b.len() < 20 || b.len() > 64 {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(|c| c.is_ascii_digit());
    if !(digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && (b[10] == b'T' || b[10] == b't')
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19))
    {
        return None;
    }
    let mut i = 19;
    if b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    let zone_ok = match b.get(i) {
        Some(b'Z') | Some(b'z') => i + 1 == b.len(),
        Some(b'+') | Some(b'-') => {
            b.len() == i + 6 && digits(i + 1..i + 3) && b[i + 3] == b':' && digits(i + 4..i + 6)
        }
        _ => false,
    };
    zone_ok.then(|| raw.to_string())
}

fn fact(raw: &str) -> Sanitized {
    sanitize_capped(raw, FACT_CAP)
}

/// 64 lowercase/uppercase hex characters, the only hash shape rendered.
fn valid_hash(raw: &str) -> Option<Sanitized> {
    if raw.len() == 64 && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(Sanitized(raw.to_string()))
    } else {
        None
    }
}

fn facts_from_header(
    name: &str,
    header: Option<&crate::engine::types::StateFileHeader>,
) -> SessionFacts {
    let mut facts = SessionFacts {
        name: name.to_string(),
        session_id: None,
        created_at: None,
        intent: None,
        template_name: None,
        current_state: None,
        directive: None,
        execution_dir: None,
        store: None,
    };
    if let Some(h) = header {
        if !h.session_id.is_empty() {
            facts.session_id = Some(fact(&h.session_id));
        }
        facts.created_at = parse_timestamp(&h.created_at);
        facts.intent = h.intent.as_deref().map(fact);
        facts.template_name = h.template_name.as_deref().map(fact);
        facts.execution_dir = h
            .execution_dir
            .as_deref()
            .map(|p| fact(&p.to_string_lossy()));
        facts.store = h
            .origin
            .as_ref()
            .map(|o| (fact(&o.store.kind), fact(&o.store.base.to_string_lossy())));
    }
    facts
}

/// Take the read-only snapshot of one session: facts, the migration
/// answer, and every manifest key with its metadata.
///
/// The migration check runs before any key read, so a migrated session
/// never has its keys listed; its facts come from the local copy, best
/// effort, through the local-only read that makes no remote request.
pub fn snapshot(backend: &Backend, name: &str) -> Result<SessionViewSnapshot, SnapshotError> {
    if !backend.exists(name) {
        return Err(SnapshotError::NotFound);
    }

    if let Err(err) = backend.check_not_migrated(name) {
        if let Some(m) = err.downcast_ref::<SessionMigrated>() {
            let facts = match backend.read_events_local(name) {
                Ok((header, _)) => facts_from_header(name, Some(&header)),
                Err(_) => facts_from_header(name, None),
            };
            return Ok(SessionViewSnapshot {
                facts,
                migrated: Some(MigratedTo {
                    target: sanitize_capped(&m.target, BANNER_VALUE_CAP),
                    workspace: sanitize_capped(&m.workspace, BANNER_VALUE_CAP),
                }),
                keys: Vec::new(),
                unlisted_keys: 0,
            });
        }
        return Err(SnapshotError::Infrastructure(err.to_string()));
    }

    let (header, events) = match backend.read_events(name) {
        Ok(ok) => ok,
        Err(err) => {
            if let Some(m) = err.downcast_ref::<SessionMigrated>() {
                // A marker that appeared between the check and the read.
                let facts = facts_from_header(name, None);
                return Ok(SessionViewSnapshot {
                    facts,
                    migrated: Some(MigratedTo {
                        target: sanitize_capped(&m.target, BANNER_VALUE_CAP),
                        workspace: sanitize_capped(&m.workspace, BANNER_VALUE_CAP),
                    }),
                    keys: Vec::new(),
                    unlisted_keys: 0,
                });
            }
            return Err(SnapshotError::Corrupt(err.to_string()));
        }
    };

    let mut facts = facts_from_header(name, Some(&header));

    // A session that never advanced derives no current state; that is
    // absence to show, not corruption (an unparseable log already failed
    // at `read_events`).
    let session_dir = backend.session_dir(name);
    if let Some(machine) = derive_machine_state(&header, &events, &session_dir) {
        facts.current_state = Some(fact(&machine.current_state));
        // Best-effort directive: a missing or unparseable template leaves
        // the directive absent rather than failing the snapshot.
        if let Ok(bytes) = std::fs::read(&machine.template_path) {
            if let Ok(compiled) =
                serde_json::from_slice::<crate::template::types::CompiledTemplate>(&bytes)
            {
                if let Some(state) = compiled.states.get(&machine.current_state) {
                    if !state.terminal {
                        if let Ok(variables) = Variables::from_events(&events) {
                            let text =
                                render_directive(&state.directive, &session_dir, name, &variables);
                            facts.directive = Some(sanitize(&text));
                        }
                    }
                }
            }
        }
    }

    let (local_manifest, remote_manifest) = backend.manifest_pair(name);

    // Union of key names, in the manifests' lexicographic order.
    let mut names: BTreeMap<&str, ()> = BTreeMap::new();
    for k in local_manifest.keys.keys() {
        names.insert(k.as_str(), ());
    }
    if let Some(remote) = remote_manifest.as_ref() {
        for k in remote.keys.keys() {
            names.insert(k.as_str(), ());
        }
    }

    let total = names.len();
    let mut keys = Vec::with_capacity(total.min(METADATA_CEILING));
    for (raw_key, _) in names.into_iter().take(METADATA_CEILING) {
        let local_meta = local_manifest.keys.get(raw_key);
        let remote_meta = remote_manifest.as_ref().and_then(|m| m.keys.get(raw_key));
        let meta: Option<&KeyMeta> = local_meta.or(remote_meta);
        let valid = validate_context_key(raw_key).is_ok();
        let stale = match (local_meta, remote_meta) {
            (Some(l), Some(r)) => l.hash != r.hash,
            _ => false,
        };
        keys.push(KeyEntry {
            display: sanitize_capped(raw_key, KEY_DISPLAY_CAP),
            raw: valid.then(|| raw_key.to_string()),
            size: meta.map(|m| m.size),
            writer: meta.and_then(|m| m.writer.as_deref().map(|w| sanitize_capped(w, 64))),
            created_at: meta.and_then(|m| parse_timestamp(&m.created_at)),
            locality: if local_meta.is_some() {
                Locality::Local
            } else {
                Locality::RemoteOnly
            },
            stale,
        });
    }

    Ok(SessionViewSnapshot {
        facts,
        migrated: None,
        keys,
        unlisted_keys: total.saturating_sub(METADATA_CEILING),
    })
}

// ---------------------------------------------------------------------------
// Key preview
// ---------------------------------------------------------------------------

/// Longest cut of `bytes` at most `max` that does not split a UTF-8
/// character. Byte-driven and total: on non-UTF-8 input it backs off at
/// most three bytes and cuts there.
fn char_boundary_cut(bytes: &[u8], max: usize) -> usize {
    if bytes.len() <= max {
        return bytes.len();
    }
    let mut cut = max;
    let mut backed = 0;
    while cut > 0 && backed < 3 && bytes[cut] & 0b1100_0000 == 0b1000_0000 {
        cut -= 1;
        backed += 1;
    }
    cut
}

/// Classify and bound one key's content for display.
///
/// Reads only the local store, through the backend's bounded prefix
/// accessor: no pull, no event, no remote request. `bound` is the source
/// excerpt bound in bytes ([`EXCERPT_BOUND`] for both surfaces).
pub fn read_key_preview(backend: &Backend, name: &str, key: &str, bound: usize) -> KeyPreview {
    if validate_context_key(key).is_err() {
        return KeyPreview::InvalidName;
    }

    use crate::session::context::ContextStore;
    let meta = backend.meta(name, key);
    let recorded_size = meta.as_ref().map(|m| m.size).unwrap_or(0);
    let recorded_hash = meta.as_ref().and_then(|m| valid_hash(&m.hash));

    match backend.read_context_prefix(name, key, bound) {
        Ok(Some(read)) => {
            let bytes: &[u8] = &read.bytes;
            let sample = &bytes[..bytes.len().min(bound)];
            let is_binary = sample.contains(&0)
                || match std::str::from_utf8(sample) {
                    Ok(_) => false,
                    Err(e) => {
                        // A clean cut can split a character at the sample's
                        // end; only an error before the tail means binary.
                        let tail_start = sample.len().saturating_sub(3);
                        e.valid_up_to() < tail_start || e.error_len().is_some()
                    }
                };
            if is_binary {
                return KeyPreview::Binary {
                    size: recorded_size,
                    hash: recorded_hash,
                };
            }
            let cut = char_boundary_cut(sample, bound);
            let text = std::str::from_utf8(&sample[..cut]).unwrap_or_default();
            KeyPreview::Text {
                excerpt: sanitize(text),
                truncated: !read.complete || cut < bytes.len(),
            }
        }
        Ok(None) => match meta {
            // On the cloud backend a manifest-listed key with no local
            // file is remote content this view deliberately does not
            // fetch; on the local backend the same shape is a missing
            // file, which is unreadable, not elsewhere.
            Some(_) if backend.is_cloud() => KeyPreview::NotLocal {
                size: recorded_size,
                hash: recorded_hash,
            },
            Some(_) => KeyPreview::Unreadable {
                reason: sanitize_one_line("content file missing for a manifest-listed key"),
            },
            None => KeyPreview::Unreadable {
                reason: sanitize_one_line("absent: no local content and no manifest entry"),
            },
        },
        Err(err) => KeyPreview::Unreadable {
            reason: sanitize_capped(&err.to_string(), 512),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_escapes_each_class_with_the_exact_form() {
        assert_eq!(sanitize("a\u{1B}b").as_str(), "a\\u{1B}b"); // ESC
        assert_eq!(sanitize("\u{9B}").as_str(), "\\u{9B}"); // 8-bit CSI
        assert_eq!(sanitize("\u{7F}").as_str(), "\\u{7F}"); // DEL
        assert_eq!(sanitize("\u{202E}").as_str(), "\\u{202E}"); // bidi override
        assert_eq!(sanitize("\u{200B}").as_str(), "\\u{200B}"); // zero width
        assert_eq!(sanitize("\u{E0041}").as_str(), "\\u{E0041}"); // tag block
        assert_eq!(sanitize("\u{2028}").as_str(), "\\u{2028}");
        assert_eq!(sanitize("\u{FEFF}").as_str(), "\\u{FEFF}");
        assert_eq!(sanitize("\r").as_str(), "\\u{D}");
        // Newline and tab survive multi-line sanitization.
        assert_eq!(sanitize("a\nb\tc").as_str(), "a\nb\tc");
    }

    #[test]
    fn sanitize_keeps_the_escape_form_unambiguous() {
        // A literal backslash that would read as an escape is escaped.
        assert_eq!(sanitize("\\u{41}").as_str(), "\\u{5C}u{41}");
        // Any other backslash is left alone.
        assert_eq!(sanitize("a\\b").as_str(), "a\\b");
    }

    #[test]
    fn sanitize_one_line_folds_newline_and_tab() {
        assert_eq!(sanitize_one_line("a\nb\tc").as_str(), "a b c");
    }

    #[test]
    fn sanitize_capped_cuts_with_a_visible_marker() {
        let s = sanitize_capped("abcdef", 4);
        assert_eq!(s.as_str(), format!("abcd{CUT_MARKER}"));
        assert_eq!(sanitize_capped("abcd", 4).as_str(), "abcd");
    }

    #[test]
    fn format_size_covers_zero_and_the_extremes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(4096), "4.0 KiB");
        assert_eq!(format_size(1024 * 1024 + 200 * 1024), "1.2 MiB");
        assert_eq!(format_size(u64::MAX), "16.0 EiB");
    }

    #[test]
    fn timestamps_are_validated_not_echoed() {
        assert!(parse_timestamp("2026-01-01T00:00:00Z").is_some());
        assert!(parse_timestamp("2026-01-01T00:00:00.123+02:00").is_some());
        assert!(parse_timestamp("yesterday\u{1B}[2J").is_none());
        assert!(parse_timestamp("2026-01-01 00:00:00").is_none());
        assert!(parse_timestamp("").is_none());
    }

    #[test]
    fn char_boundary_cut_never_splits_a_character() {
        let s = "aé€"; // 1 + 2 + 3 bytes
        let b = s.as_bytes();
        assert_eq!(char_boundary_cut(b, 6), 6);
        assert_eq!(char_boundary_cut(b, 5), 3); // inside €
        assert_eq!(char_boundary_cut(b, 2), 1); // inside é
    }

    #[test]
    fn hashes_must_be_64_hex_characters() {
        assert!(valid_hash(&"a".repeat(64)).is_some());
        assert!(valid_hash("deadbeef").is_none());
        assert!(valid_hash(&"g".repeat(64)).is_none());
    }
}
