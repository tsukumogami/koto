//! Known-credential redaction for captured command output
//! (DESIGN-koto-failure-reporting.md, Decision 5).
//!
//! A tick builds one [`Redactor`] beside its command environment. It holds
//! the values koto knows are secret -- the live credential-carrying
//! variables, the `pass_env:` values that reach a command, and koto's own
//! decider and cloud keys -- each tagged with the name of where it came from.
//! [`redact_capture`] runs once per stream, on raw bytes, before anything is
//! decoded, cut or handed to a consumer, and replaces every occurrence with
//! a marker `[REDACTED:<source>]`. The only way to get a [`RedactedText`] is
//! through this module, so code outside it can't reach raw capture.
//!
//! Values shorter than [`MIN_VALUE_LEN`] bytes are never searched for: they
//! match ordinary text too easily. With an empty known set every step is
//! skipped and the output is byte-identical to a plain capture.

use aho_corasick::{AhoCorasick, MatchKind};

/// Shortest value searched for. A fragment shorter than this that survives
/// a cut is accepted; anything this long or longer never does.
pub const MIN_VALUE_LEN: usize = 8;

/// Opening of every marker. ASCII, safe inside a JSON string, and outside
/// the variable-value allowlist, so a capture holding one can't be delivered.
const MARKER_OPEN: &str = "[REDACTED:";

/// Longest source name a marker carries, in bytes. A longer run of name
/// bytes after `[REDACTED:` isn't a marker, so text a command printed to
/// look like one can't make a cut back off over most of a stream.
pub const MAX_SOURCE_LEN: usize = 64;

/// `source` cut to at most [`MAX_SOURCE_LEN`] bytes, at a character
/// boundary, so every marker koto writes is one it recognizes.
fn bounded_source(source: &str) -> &str {
    if source.len() <= MAX_SOURCE_LEN {
        return source;
    }
    let mut end = MAX_SOURCE_LEN;
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    &source[..end]
}

/// The marker that replaces a value from `source`.
fn marker(source: &str) -> String {
    format!("{}{}]", MARKER_OPEN, bounded_source(source))
}

/// Whether `b` may appear in a source name: a variable name, or a
/// configuration setting name (which contains a dot).
fn is_source_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

/// Byte spans `[start, end)` of every marker in `bytes`, in order.
fn marker_spans(bytes: &[u8]) -> Vec<(usize, usize)> {
    let open = MARKER_OPEN.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i + open.len() < bytes.len() {
        if &bytes[i..i + open.len()] != open {
            i += 1;
            continue;
        }
        let name_start = i + open.len();
        let name_limit = bytes.len().min(name_start + MAX_SOURCE_LEN);
        let mut j = name_start;
        while j < name_limit && is_source_byte(bytes[j]) {
            j += 1;
        }
        if j > name_start && j < bytes.len() && bytes[j] == b']' {
            spans.push((i, j + 1));
            i = j + 1;
        } else {
            i += 1;
        }
    }
    spans
}

/// The source named by the first marker in `text`, if it holds one.
///
/// Used to refuse a capture that would store a marker as a variable value.
pub fn first_marker_source(text: &str) -> Option<&str> {
    marker_spans(text.as_bytes())
        .first()
        .map(|&(s, e)| &text[s + MARKER_OPEN.len()..e - 1])
}

/// The length of the longest prefix of `bytes`, at most `max` bytes long,
/// that neither splits a marker nor a UTF-8 character.
///
/// This is the one cut every bounded copy of redacted text goes through:
/// the capture bound itself, the log copies and the per-field caps. A cut
/// that would land inside a marker backs off to the marker's start, so a
/// reader never sees half a marker; it then backs off over continuation
/// bytes to the start of a character.
pub fn safe_cut_len(bytes: &[u8], max: usize) -> usize {
    if bytes.len() <= max {
        return bytes.len();
    }
    let mut cut = max;
    for (s, e) in marker_spans(bytes) {
        if s >= cut {
            break;
        }
        if cut < e {
            cut = s;
            break;
        }
    }
    // A UTF-8 character has at most three continuation bytes, so backing off
    // more than three can only mean the bytes aren't UTF-8; stop there.
    let mut backed = 0;
    while cut > 0 && backed < 3 && (bytes[cut] & 0xC0) == 0x80 {
        cut -= 1;
        backed += 1;
    }
    cut
}

/// Text that has been through redaction, or that koto wrote itself.
///
/// The field is private and the only constructors are this module's
/// redaction functions and the crate-private [`RedactedText::koto_note`]
/// (with [`RedactedText::push_koto_note`] to append), so a consumer that
/// holds one knows every known value in it has been replaced, and code
/// outside koto can't wrap text of its own.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedactedText(String);

impl RedactedText {
    /// Text koto composed itself -- a timeout, polling or `working_dir` note --
    /// that holds no captured output.
    pub(crate) fn koto_note(text: impl Into<String>) -> Self {
        RedactedText(text.into())
    }

    /// Append koto-written `note` exactly as given.
    pub(crate) fn push_koto_note(&mut self, note: &str) {
        self.0.push_str(note);
    }

    /// The text as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The text as an owned `String`, for a consumer that stores it.
    pub fn into_string(self) -> String {
        self.0
    }

    /// At most `max_bytes` bytes of this text, cut at a character boundary
    /// and never inside a marker, with whether anything was dropped.
    pub fn cut_bytes(&self, max_bytes: usize) -> (RedactedText, bool) {
        let n = safe_cut_len(self.0.as_bytes(), max_bytes);
        (RedactedText(self.0[..n].to_string()), n < self.0.len())
    }

    /// At most `max_chars` characters of this text, never inside a marker,
    /// with whether anything was dropped.
    pub fn cut_chars(&self, max_chars: usize) -> (RedactedText, bool) {
        match self.0.char_indices().nth(max_chars) {
            Some((byte, _)) => self.cut_bytes(byte),
            None => (self.clone(), false),
        }
    }
}

impl std::ops::Deref for RedactedText {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RedactedText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for RedactedText {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl PartialEq<str> for RedactedText {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for RedactedText {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// The known values for one tick, and the automaton that finds them.
///
/// Holds credentials, so it is never serialized and its `Debug` form prints
/// source names only. Nothing that contains it derives `Debug`.
pub struct Redactor {
    /// Every searched spelling, as bytes, parallel to `pattern_source`.
    values: Vec<Vec<u8>>,
    /// Index into `sources` for each entry of `values`.
    pattern_source: Vec<usize>,
    /// Source names, in first-seen order.
    sources: Vec<String>,
    /// `None` when the set is empty.
    automaton: Option<AhoCorasick>,
    /// Length of the longest searched spelling.
    longest: usize,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor")
            .field("sources", &self.sources)
            .finish()
    }
}

/// The JSON string spellings of `value` that differ from it: as `serde_json`
/// escapes it, and again with `/` written as `\/`.
fn json_spellings(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let quoted = serde_json::to_string(value).unwrap_or_default();
    if quoted.len() < 2 {
        return out;
    }
    let escaped = quoted[1..quoted.len() - 1].to_string();
    let slashed = escaped.replace('/', "\\/");
    if escaped != value {
        out.push(escaped.clone());
    }
    if slashed != escaped && slashed != value {
        out.push(slashed);
    }
    out
}

impl Redactor {
    /// A redactor that knows nothing and changes nothing.
    pub fn empty() -> Self {
        Redactor {
            values: Vec::new(),
            pattern_source: Vec::new(),
            sources: Vec::new(),
            automaton: None,
            longest: 0,
        }
    }

    /// Build from `(source, value)` pairs in priority order.
    ///
    /// A value shorter than [`MIN_VALUE_LEN`] bytes is dropped. Each value is
    /// also searched in its JSON string spellings. When two sources carry
    /// the same value, the first one names it.
    pub fn new<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut r = Self::empty();
        for (source, value) in entries {
            if value.len() < MIN_VALUE_LEN {
                continue;
            }
            let mut spellings = vec![value.clone()];
            spellings.extend(json_spellings(&value));
            for spelling in spellings {
                if r.values.iter().any(|v| v.as_slice() == spelling.as_bytes()) {
                    continue;
                }
                let idx = match r.sources.iter().position(|s| *s == source) {
                    Some(i) => i,
                    None => {
                        r.sources.push(source.clone());
                        r.sources.len() - 1
                    }
                };
                r.longest = r.longest.max(spelling.len());
                r.values.push(spelling.into_bytes());
                r.pattern_source.push(idx);
            }
        }
        if !r.values.is_empty() {
            r.automaton = AhoCorasick::builder()
                .match_kind(MatchKind::Standard)
                .build(&r.values)
                .ok();
        }
        r
    }

    /// True when there is nothing to search for.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The source names this redactor knows, for diagnostics.
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// How many bytes a capture reader must keep for a `limit`-byte bound so
    /// that a value starting before `limit` is seen whole:
    /// `limit + max(L, 1) - 1`, where L is the longest searched spelling.
    pub fn retention(&self, limit: usize) -> usize {
        limit + self.longest.max(1) - 1
    }

    /// Every occurrence of a known value in `hay`, overlapping ones included,
    /// as `(start, end, source index)`.
    fn matches(&self, hay: &[u8]) -> Vec<(usize, usize, usize)> {
        match &self.automaton {
            Some(ac) => ac
                .find_overlapping_iter(hay)
                .map(|m| {
                    (
                        m.start(),
                        m.end(),
                        self.pattern_source[m.pattern().as_usize()],
                    )
                })
                .collect(),
            // The automaton only fails to build past internal size limits a
            // handful of values can't reach. It isn't provably unreachable
            // (a config file can hold a value of any size), so fail closed:
            // search directly rather than letting a value through.
            None => {
                let mut out = Vec::new();
                for (p, v) in self.values.iter().enumerate() {
                    if v.is_empty() || v.len() > hay.len() {
                        continue;
                    }
                    for start in 0..=hay.len() - v.len() {
                        if &hay[start..start + v.len()] == v.as_slice() {
                            out.push((start, start + v.len(), self.pattern_source[p]));
                        }
                    }
                }
                out
            }
        }
    }

    /// The longest suffix of `hay`, at least [`MIN_VALUE_LEN`] bytes, that is
    /// a prefix of a known value: what a stream cut off mid-value leaves.
    fn kill_tail(&self, hay: &[u8]) -> Option<(usize, usize, usize)> {
        let mut best: Option<(usize, usize)> = None;
        for (p, v) in self.values.iter().enumerate() {
            let max_k = v.len().min(hay.len());
            let floor = best.map_or(MIN_VALUE_LEN, |(k, _)| k + 1);
            let mut k = max_k;
            while k >= floor {
                if hay.ends_with(&v[..k]) {
                    best = Some((k, p));
                    break;
                }
                k -= 1;
            }
        }
        best.map(|(k, p)| (hay.len() - k, hay.len(), self.pattern_source[p]))
    }

    /// Matches merged into disjoint spans, each named by its earliest match.
    fn spans(&self, hay: &[u8], killed: bool) -> Vec<(usize, usize, usize)> {
        let mut found = self.matches(hay);
        if killed {
            found.extend(self.kill_tail(hay));
        }
        found.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        let mut spans: Vec<(usize, usize, usize)> = Vec::new();
        for m in found {
            match spans.last_mut() {
                Some(last) if m.0 < last.1 => last.1 = last.1.max(m.1),
                _ => spans.push(m),
            }
        }
        spans
    }
}

/// Decode captured bytes, dropping a trailing partial UTF-8 sequence.
///
/// A cut at a byte count can split a multi-byte character. An incomplete
/// tail is dropped; any other invalid byte is replaced, so a command
/// emitting binary still yields readable output rather than nothing.
fn decode_capture(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        // `error_len() == None` means the input ended mid-character.
        Err(e) if e.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Redact one captured stream and bound it to `limit` bytes.
///
/// `raw` is what the reader kept (at most [`Redactor::retention`] bytes),
/// `raw_total` how many bytes the stream carried in all, and `killed`
/// whether koto killed the process, so the stream may end mid-value.
/// Returns the text and whether it was cut: the stream carried more than
/// `limit` bytes, or its markers pushed it past `limit`.
///
/// Every known value is found on the raw bytes, overlapping matches merge
/// into one span, and each span becomes one marker. When the whole stream
/// was kept and koto killed the process, a trailing suffix of at least
/// [`MIN_VALUE_LEN`] bytes that begins a known value is masked too. Output
/// is emitted by raw offset up to `limit`, a span that starts before `limit`
/// emitted whole as its marker; a final cut at `limit` backs off to the
/// start of any marker it would split and to a character boundary. The
/// result equals redacting the whole stream and then cutting it in raw
/// coordinates.
pub fn redact_capture(
    raw: &[u8],
    raw_total: usize,
    killed: bool,
    limit: usize,
    redactor: &Redactor,
) -> (RedactedText, bool) {
    if redactor.is_empty() {
        let kept = &raw[..raw.len().min(limit)];
        return (RedactedText(decode_capture(kept)), raw_total > limit);
    }

    // The stream's true end is inside `raw` only when nothing was dropped.
    // The tail is masked only when koto killed the process: then the stream
    // may end mid-value. A stream that ended on its own and happens to end
    // with the start of a known value (`ghp_...`, `https://...`) printed
    // ordinary text, and masking it would put a marker where no credential
    // was. The condition is on what the reader kept (`raw.len()`), not on
    // `limit`: past the retention bound the true end isn't seen at all.
    let whole = raw_total <= raw.len();
    let spans = redactor.spans(raw, killed && whole);

    let mut out: Vec<u8> = Vec::with_capacity(raw.len().min(limit));
    let mut pos = 0;
    for &(start, end, source) in &spans {
        if start >= limit {
            break;
        }
        out.extend_from_slice(&raw[pos..start]);
        out.extend_from_slice(marker(&redactor.sources[source]).as_bytes());
        pos = end;
    }
    let end = raw.len().min(limit);
    if pos < end {
        out.extend_from_slice(&raw[pos..end]);
    }

    let cut = safe_cut_len(&out, limit);
    let truncated = raw_total > limit || cut < out.len();
    (RedactedText(decode_capture(&out[..cut])), truncated)
}

/// Replace every known value in text koto composed from other inputs.
pub fn redact_str(text: &str, redactor: &Redactor) -> RedactedText {
    if redactor.is_empty() {
        return RedactedText(text.to_string());
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for (start, end, source) in redactor.spans(bytes, false) {
        out.push_str(&String::from_utf8_lossy(&bytes[pos..start]));
        out.push_str(&marker(&redactor.sources[source]));
        pos = end;
    }
    out.push_str(&String::from_utf8_lossy(&bytes[pos..]));
    RedactedText(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(pairs: &[(&str, &str)]) -> Redactor {
        Redactor::new(pairs.iter().map(|(s, v)| (s.to_string(), v.to_string())))
    }

    /// Capture `stream` as the reader would: keep `retention` bytes, count
    /// the rest.
    fn capture(stream: &[u8], killed: bool, limit: usize, red: &Redactor) -> (RedactedText, bool) {
        let keep = stream.len().min(red.retention(limit));
        redact_capture(&stream[..keep], stream.len(), killed, limit, red)
    }

    /// Every run of `MIN_VALUE_LEN` or more bytes of `value` found in `text`.
    fn fragment_of(text: &str, value: &str) -> Option<String> {
        let v = value.as_bytes();
        for len in (MIN_VALUE_LEN..=v.len()).rev() {
            for start in 0..=v.len() - len {
                let frag = &value[start..start + len];
                if text.contains(frag) {
                    return Some(frag.to_string());
                }
            }
        }
        None
    }

    #[test]
    fn a_value_is_replaced_by_a_marker_naming_its_source() {
        let red = r(&[("GH_TOKEN", "ghp_secretvalue123")]);
        let (text, cut) = capture(b"token=ghp_secretvalue123 ok\n", false, 1024, &red);
        assert_eq!(text, "token=[REDACTED:GH_TOKEN] ok\n");
        assert!(!cut);
    }

    #[test]
    fn seven_bytes_is_not_searched_and_eight_is() {
        let red = r(&[("GH_TOKEN", "abcdefg")]);
        assert!(red.is_empty());
        let (text, _) = capture(b"x abcdefg y", false, 64, &red);
        assert_eq!(text, "x abcdefg y");
        let red = r(&[("GH_TOKEN", "abcdefgh")]);
        let (text, _) = capture(b"x abcdefgh y", false, 64, &red);
        assert_eq!(text, "x [REDACTED:GH_TOKEN] y");
    }

    #[test]
    fn overlapping_values_merge_into_one_marker() {
        let red = r(&[("A_TOKEN", "aaaabbbbcccc"), ("B_TOKEN", "bbbbccccdddd")]);
        let (text, _) = capture(b"<aaaabbbbccccdddd>", false, 64, &red);
        assert_eq!(text, "<[REDACTED:A_TOKEN]>");
        // A value inside another is covered by the outer one.
        let red = r(&[("OUTER", "0123456789abcdef"), ("INNER", "456789ab")]);
        let (text, _) = capture(b"-0123456789abcdef-", false, 64, &red);
        assert_eq!(text, "-[REDACTED:OUTER]-");
    }

    #[test]
    fn the_first_source_wins_a_duplicate_value() {
        let red = r(&[
            ("GH_TOKEN", "same-value-123"),
            ("GITHUB_TOKEN", "same-value-123"),
        ]);
        assert_eq!(red.sources(), &["GH_TOKEN".to_string()]);
        assert_eq!(redact_str("same-value-123", &red), "[REDACTED:GH_TOKEN]");
    }

    #[test]
    fn json_escaped_spellings_are_caught() {
        let value = "pa\"ss/wo\\rd-1";
        let red = r(&[("GH_DB", value)]);
        let escaped = serde_json::to_string(value).unwrap();
        let inner = &escaped[1..escaped.len() - 1];
        let text = redact_str(&format!("{{\"m\":\"{}\"}}", inner), &red);
        assert_eq!(text, "{\"m\":\"[REDACTED:GH_DB]\"}");
        let slashed = inner.replace('/', "\\/");
        let text = redact_str(&format!("x {} y", slashed), &red);
        assert_eq!(text, "x [REDACTED:GH_DB] y");
        let text = redact_str(value, &red);
        assert_eq!(text, "[REDACTED:GH_DB]");
    }

    #[test]
    fn an_empty_set_is_byte_identical_including_exactly_the_bound() {
        let red = Redactor::empty();
        assert_eq!(red.retention(65_536), 65_536);
        let stream = vec![b'x'; 65_536];
        let (text, cut) = capture(&stream, false, 65_536, &red);
        assert_eq!(text.len(), 65_536);
        assert!(!cut);
        let stream = vec![b'x'; 65_537];
        let (text, cut) = capture(&stream, false, 65_536, &red);
        assert_eq!(text.len(), 65_536);
        assert!(cut);
        // Invalid and split characters decode as a plain capture does.
        let (text, _) = capture(&[b'a', 0xFF, b'b', 0xC3], false, 64, &red);
        assert_eq!(text.as_str(), "a\u{FFFD}b\u{FFFD}");
    }

    #[test]
    fn a_value_at_every_offset_near_the_bound_leaves_no_fragment() {
        let value = "ghp_0123456789abcdefghij";
        let red = r(&[("GH_TOKEN", value)]);
        let limit = 256;
        let l = value.len();
        for offset in (limit - l)..=limit {
            let mut stream = "x".repeat(offset);
            stream.push_str(value);
            stream.push_str(&"y".repeat(300));
            let (text, cut) = capture(stream.as_bytes(), false, limit, &red);
            assert!(cut, "offset {offset}");
            assert!(text.len() <= limit, "offset {offset}");
            assert_eq!(fragment_of(&text, value), None, "offset {offset}: {text}");
            assert!(!text.contains("[REDACTED:GH_TOKE") || text.contains("[REDACTED:GH_TOKEN]"));
        }
    }

    #[test]
    fn a_kill_mid_value_masks_its_head() {
        let value = "ghp_0123456789abcdefghij";
        let red = r(&[("GH_TOKEN", value)]);
        let stream = format!("partial {}", &value[..12]);
        let (text, cut) = capture(stream.as_bytes(), true, 1024, &red);
        assert_eq!(text, "partial [REDACTED:GH_TOKEN]");
        assert!(!cut);
        // Not killed: an ordinary ending is left alone.
        let (text, _) = capture(stream.as_bytes(), false, 1024, &red);
        assert_eq!(text.as_str(), stream);
        // Seven bytes of head is below the floor.
        let stream = format!("partial {}", &value[..7]);
        let (text, _) = capture(stream.as_bytes(), true, 1024, &red);
        assert_eq!(text.as_str(), stream);
    }

    #[test]
    fn a_marker_is_never_split_by_the_final_cut() {
        let red = r(&[("A_VERY_LONG_SOURCE_NAME_FOR_THE_TOKEN", "short123")]);
        // The value starts just before the bound; its marker is longer than
        // the room left, so the cut drops it whole rather than splitting it.
        let stream = format!("{}short123", "x".repeat(60));
        let (text, cut) = capture(stream.as_bytes(), false, 64, &red);
        assert_eq!(text.as_str(), "x".repeat(60));
        assert!(cut);
    }

    #[test]
    fn cut_helpers_respect_markers_and_characters() {
        let red = r(&[("GH_TOKEN", "ghp_secretvalue123")]);
        let text = redact_str("ab ghp_secretvalue123 é", &red);
        let (cut, dropped) = text.cut_bytes(8);
        assert_eq!(cut, "ab ");
        assert!(dropped);
        let (whole, dropped) = text.cut_bytes(1000);
        assert_eq!(whole, text);
        assert!(!dropped);
        let (cut, _) = text.cut_bytes(text.len() - 1);
        assert_eq!(cut, "ab [REDACTED:GH_TOKEN] ");
        let (cut, dropped) = text.cut_chars(4);
        assert_eq!(cut, "ab ");
        assert!(dropped);
        assert_eq!(first_marker_source(&text), Some("GH_TOKEN"));
        assert_eq!(first_marker_source("[REDACTED:] [REDACTED:x"), None);
        assert_eq!(
            first_marker_source("a [REDACTED:session.cloud.access_key] b"),
            Some("session.cloud.access_key")
        );
    }

    #[test]
    fn a_marker_source_name_is_bounded() {
        // A fake marker with a huge source name, printed across the bound,
        // isn't a marker: the cut stays at the bound instead of backing off
        // to its start.
        let fake = format!("[REDACTED:{}]", "A".repeat(10_000));
        let text = redact_str(
            &format!("head {}", fake),
            &r(&[("GH_TOKEN", "unused-value")]),
        );
        let (cut, dropped) = text.cut_bytes(1000);
        assert_eq!(cut.len(), 1000);
        assert!(dropped);
        assert_eq!(first_marker_source(&fake), None);
        // One byte over the bound isn't a marker; exactly the bound is.
        let over = format!("[REDACTED:{}]", "B".repeat(MAX_SOURCE_LEN + 1));
        assert_eq!(first_marker_source(&over), None);
        let at = "C".repeat(MAX_SOURCE_LEN);
        assert_eq!(
            first_marker_source(&format!("x [REDACTED:{}] y", at)),
            Some(at.as_str())
        );
        // A source name koto knows that is longer than the bound is written
        // shortened, so its marker is still recognized and never split.
        let long = format!("LONG_{}", "N".repeat(100));
        let red = r(&[(long.as_str(), "long-source-secret")]);
        let text = redact_str("a long-source-secret b", &red);
        let shown = &long[..MAX_SOURCE_LEN];
        assert_eq!(text, format!("a [REDACTED:{}] b", shown).as_str());
        assert_eq!(first_marker_source(&text), Some(shown));
        let (cut, _) = text.cut_bytes(10);
        assert_eq!(cut, "a ");
    }

    #[test]
    fn debug_prints_source_names_only() {
        let red = r(&[("GH_TOKEN", "ghp_secretvalue123")]);
        let shown = format!("{:?}", red);
        assert!(shown.contains("GH_TOKEN"));
        assert!(!shown.contains("secretvalue"));
    }

    /// Redact the whole stream with an independent search, then cut in raw
    /// coordinates: the definition `redact_capture` must match.
    fn reference(
        stream: &[u8],
        killed: bool,
        limit: usize,
        values: &[(&str, &str)],
    ) -> (String, bool) {
        let mut spellings: Vec<(Vec<u8>, String)> = Vec::new();
        for (s, v) in values {
            if v.len() < MIN_VALUE_LEN {
                continue;
            }
            let mut all = vec![v.to_string()];
            all.extend(json_spellings(v));
            for sp in all {
                if !spellings.iter().any(|(b, _)| b == sp.as_bytes()) {
                    spellings.push((sp.into_bytes(), s.to_string()));
                }
            }
        }
        let mut found: Vec<(usize, usize, String)> = Vec::new();
        for (v, s) in &spellings {
            for start in 0..stream.len() {
                if stream[start..].starts_with(v) {
                    found.push((start, start + v.len(), s.clone()));
                }
            }
        }
        if killed {
            let mut best: Option<(usize, String)> = None;
            for (v, s) in &spellings {
                for k in (MIN_VALUE_LEN..=v.len().min(stream.len())).rev() {
                    if stream.ends_with(&v[..k]) {
                        if best.as_ref().is_none_or(|(bk, _)| k > *bk) {
                            best = Some((k, s.clone()));
                        }
                        break;
                    }
                }
            }
            if let Some((k, s)) = best {
                found.push((stream.len() - k, stream.len(), s));
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        let mut spans: Vec<(usize, usize, String)> = Vec::new();
        for m in found {
            match spans.last_mut() {
                Some(last) if m.0 < last.1 => last.1 = last.1.max(m.1),
                _ => spans.push(m),
            }
        }
        let mut out = Vec::new();
        let mut pos = 0;
        for (s, e, src) in &spans {
            if *s >= limit {
                break;
            }
            out.extend_from_slice(&stream[pos..*s]);
            out.extend_from_slice(marker(src).as_bytes());
            pos = *e;
        }
        let end = stream.len().min(limit);
        if pos < end {
            out.extend_from_slice(&stream[pos..end]);
        }
        let cut = safe_cut_len(&out, limit);
        let truncated = stream.len() > limit || cut < out.len();
        (decode_capture(&out[..cut]), truncated)
    }

    #[test]
    fn redact_capture_equals_redacting_the_whole_stream_then_cutting() {
        // A small fixed-seed generator: the cases are the same on every run.
        let mut seed: u64 = 0x5eed_c0de_1234_5678;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let values: &[(&str, &str)] = &[
            ("GH_TOKEN", "abcabcabXY"),
            ("HTTPS_PROXY", "bcabXYzz/q"),
            ("GH_DB", "pw\"q/ab12"),
            ("SHORT", "abc"),
            ("decider.api_key", "zzzzzzzzzzzz"),
        ];
        let red = r(values);
        let alphabet: &[&[u8]] = &[
            b"a",
            b"b",
            b"c",
            b"X",
            b"Y",
            b"z",
            b"/",
            b"\\",
            b"\"",
            b"q",
            b"\n",
            "é".as_bytes(),
        ];
        for case in 0..3000 {
            let mut stream: Vec<u8> = Vec::new();
            let len = next(90) as usize;
            while stream.len() < len {
                if next(6) == 0 {
                    let (_, v) = values[next(values.len() as u64) as usize];
                    let mut sp = vec![v.to_string()];
                    sp.extend(json_spellings(v));
                    let pick = &sp[next(sp.len() as u64) as usize];
                    // Sometimes only a head of the value, as a kill leaves.
                    let take = if next(3) == 0 {
                        1 + next(pick.len() as u64) as usize
                    } else {
                        pick.len()
                    };
                    stream.extend_from_slice(&pick.as_bytes()[..take]);
                } else {
                    stream.extend_from_slice(alphabet[next(alphabet.len() as u64) as usize]);
                }
            }
            let limit = 1 + next(60) as usize;
            let killed = next(2) == 0;
            let (got, got_cut) = capture(&stream, killed, limit, &red);
            let (want, want_cut) = reference(&stream, killed, limit, values);
            assert_eq!(
                (got.as_str(), got_cut),
                (want.as_str(), want_cut),
                "case {case}: stream {:?} limit {limit} killed {killed}",
                String::from_utf8_lossy(&stream)
            );
        }
    }
}
