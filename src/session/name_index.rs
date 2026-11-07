//! The bucket's name-first index of session copies (koto#597).
//!
//! Beside the workspace prefixes, a bucket holds one empty object per
//! prefix that ever held a session name, `koto-names/<name>/<prefix>`, so a
//! lookup of a name can ask which prefixes held it with one listing instead
//! of one per prefix the bucket has ever had. A host writes a copy's entry
//! before the copy's first state object and no session command removes one,
//! so every state object a host running this release wrote has its entry
//! from the moment the object exists.
//!
//! `koto-names/_complete`, the declaration, says an audit found every older
//! copy indexed too, so a miss in the index can be trusted. Nothing here
//! writes it; the write path only reads it, to decide whether a refused
//! entry PUT may be skipped.
//!
//! The index's keys are built only from a name that passed
//! [`validate_session_id`] and a prefix that passed [`is_workspace_prefix`],
//! and a listing's keys are kept only when they parse back the same way, so
//! nothing a bucket lists can steer a lookup outside the workspace prefixes.

use std::time::SystemTime;

use s3::serde_types::ListBucketResult;
use s3::Bucket;
use serde::{Deserialize, Deserializer};

use crate::session::continues_bucket::is_workspace_prefix;
use crate::session::validate::validate_session_id;

/// The key prefix every index object lives under.
pub const ENTRY_ROOT: &str = "koto-names/";

/// The declaration's key. A session name starts with a letter, so no entry
/// listing for a name ever includes it.
pub const DECLARATION_KEY: &str = "koto-names/_complete";

/// How many entries a lookup reads for one name before it treats the name
/// as polluted and stops paginating.
pub const MAX_ENTRIES_PER_LOOKUP: usize = 256;

/// The most bytes of the declaration a host reads; a larger body reads as
/// undeclared.
pub const MAX_DECLARATION_BYTES: usize = 4096;

/// `koto-names/<name>/<prefix>`, the entry that says `prefix` held `name`.
/// Refuses a name [`validate_session_id`] rejects and a prefix that isn't
/// 16 lowercase hex digits.
pub fn entry_key(name: &str, prefix: &str) -> anyhow::Result<String> {
    validate_session_id(name)?;
    if !is_workspace_prefix(prefix) {
        anyhow::bail!(
            "{:?} is not a workspace prefix (16 lowercase hex digits)",
            prefix
        );
    }
    Ok(format!("{}{}/{}", ENTRY_ROOT, name, prefix))
}

/// `koto-names/<name>/`, the listing prefix of `name`'s entries. It always
/// ends in `/`, so a listing for `foo` never returns `foobar`'s entries.
pub fn listing_prefix(name: &str) -> anyhow::Result<String> {
    validate_session_id(name)?;
    Ok(format!("{}{}/", ENTRY_ROOT, name))
}

/// The workspace prefixes `pages`, listings of [`listing_prefix`]`(name)`,
/// name as having held `name`, other than `own`, each once and in listed
/// order. A key that isn't exactly `koto-names/<name>/<prefix>` with a
/// valid prefix is ignored.
pub fn prefixes_from(pages: &[ListBucketResult], name: &str, own: &str) -> Vec<String> {
    let Ok(listed) = listing_prefix(name) else {
        return Vec::new();
    };
    let mut prefixes: Vec<String> = Vec::new();
    for obj in pages.iter().flat_map(|page| page.contents.iter()) {
        let Some(prefix) = obj.key.strip_prefix(&listed) else {
            continue;
        };
        if !is_workspace_prefix(prefix) || prefix == own {
            continue;
        }
        if !prefixes.iter().any(|p| p == prefix) {
            prefixes.push(prefix.to_string());
        }
    }
    prefixes
}

/// The declaration at [`DECLARATION_KEY`]. Every field is required, and
/// fields it doesn't know are ignored, so a newer writer can add some.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Declaration {
    /// The declaration format, as an integer.
    pub version: u64,
    /// When the bucket was declared.
    #[serde(deserialize_with = "timestamp")]
    pub declared_at: SystemTime,
    /// The machine name of the host that declared it.
    pub machine: String,
    /// The release that declared it. Informational only.
    pub first_release: String,
    /// The audit the declaration rests on.
    pub audit: Audit,
}

/// The audit a [`Declaration`] records.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Audit {
    #[serde(deserialize_with = "timestamp")]
    pub started_at: SystemTime,
    #[serde(deserialize_with = "timestamp")]
    pub finished_at: SystemTime,
    /// How many workspace prefixes it walked.
    pub prefixes: u64,
    /// How many session copies it found.
    pub sessions: u64,
    pub entries_written: u64,
    pub entries_pruned: u64,
}

/// A timestamp field, parsed rather than kept as text, so a declaration
/// whose times don't parse reads as undeclared.
fn timestamp<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SystemTime, D::Error> {
    let text = String::deserialize(deserializer)?;
    crate::engine::types::parse_timestamp_to_systemtime(&text)
        .ok_or_else(|| serde::de::Error::custom(format!("not a timestamp: {:?}", text)))
}

/// The declaration in `bytes`; `None`, undeclared, for a body over
/// [`MAX_DECLARATION_BYTES`] or one that doesn't parse strictly.
pub fn parse_declaration(bytes: &[u8]) -> Option<Declaration> {
    if bytes.len() > MAX_DECLARATION_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// What one read of [`DECLARATION_KEY`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeclarationRead {
    /// No object, or one [`parse_declaration`] rejects.
    Absent,
    /// A declaration that parses.
    Present,
    /// The bucket didn't answer whether there is one; the text says how.
    Unreadable(String),
}

/// Read the declaration in `bucket`, never keeping more than
/// [`MAX_DECLARATION_BYTES`] and one byte of it.
///
/// The object is asked for by listing its key first: rust-s3 retries a
/// 404 GET after a one-second sleep, so an absent declaration, the usual
/// case, costs one listing. The GET asks for the capped range, and a body
/// that runs past it anyway (an endpoint ignoring the range, or an object
/// replaced after the listing) stops being read at the cap.
pub(crate) fn read_declaration(bucket: &Bucket) -> DeclarationRead {
    use crate::session::cloud::without_url_userinfo;
    use crate::session::remote_feed::BoundedSink;

    let pages = match bucket.list(DECLARATION_KEY.to_string(), None) {
        Ok(pages) => pages,
        Err(e) => {
            return DeclarationRead::Unreadable(format!(
                "listing {} failed: {}",
                DECLARATION_KEY,
                without_url_userinfo(&e.to_string())
            ))
        }
    };
    let Some(listed) = pages
        .iter()
        .flat_map(|page| page.contents.iter())
        .find(|obj| obj.key == DECLARATION_KEY)
    else {
        return DeclarationRead::Absent;
    };
    if listed.size > MAX_DECLARATION_BYTES as u64 {
        return DeclarationRead::Absent;
    }
    let mut sink = BoundedSink {
        bytes: Vec::new(),
        limit: MAX_DECLARATION_BYTES + 1,
    };
    let answered = bucket.get_object_range_to_writer(
        DECLARATION_KEY,
        0,
        Some(MAX_DECLARATION_BYTES as u64),
        &mut sink,
    );
    if sink.bytes.len() > MAX_DECLARATION_BYTES {
        return DeclarationRead::Absent;
    }
    match answered {
        Ok(200 | 206) => match parse_declaration(&sink.bytes) {
            Some(_) => DeclarationRead::Present,
            None => DeclarationRead::Absent,
        },
        Ok(404) | Err(s3::error::S3Error::HttpFailWithBody(404, _)) => DeclarationRead::Absent,
        Ok(status) => DeclarationRead::Unreadable(format!(
            "reading {} answered status {}",
            DECLARATION_KEY, status
        )),
        Err(e) => DeclarationRead::Unreadable(format!(
            "reading {} failed: {}",
            DECLARATION_KEY,
            without_url_userinfo(&e.to_string())
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "0123456789abcdef";
    const Q: &str = "fedcba9876543210";

    #[test]
    fn an_entry_key_names_the_name_and_the_prefix() {
        assert_eq!(entry_key("wf", P).unwrap(), format!("koto-names/wf/{P}"));
        assert_eq!(
            entry_key("a.b_c-d~e", P).unwrap(),
            format!("koto-names/a.b_c-d~e/{P}")
        );
    }

    #[test]
    fn an_entry_key_refuses_a_bad_name() {
        for name in ["", "1wf", "a/b", ".", "..", "_complete"] {
            assert!(entry_key(name, P).is_err(), "name {name:?}");
        }
    }

    #[test]
    fn an_entry_key_refuses_a_bad_prefix() {
        for prefix in [
            "0123456789abcde",
            "0123456789abcdef0",
            "0123456789ABCDEF",
            "0123456789abcdeF",
            "01234567/9abcdef",
            "",
        ] {
            assert!(entry_key("wf", prefix).is_err(), "prefix {prefix:?}");
        }
    }

    #[test]
    fn a_listing_prefix_ends_in_a_slash_and_refuses_a_bad_name() {
        assert_eq!(listing_prefix("foo").unwrap(), "koto-names/foo/");
        for name in ["", "1wf", "a/b", ".", ".."] {
            assert!(listing_prefix(name).is_err(), "name {name:?}");
        }
        let other = entry_key("foobar", P).unwrap();
        assert!(
            !other.starts_with(&listing_prefix("foo").unwrap()),
            "{other}"
        );
    }

    fn page(keys: &[String]) -> ListBucketResult {
        ListBucketResult {
            name: "b".to_string(),
            delimiter: None,
            max_keys: None,
            prefix: None,
            continuation_token: None,
            encoding_type: None,
            is_truncated: false,
            next_continuation_token: None,
            contents: keys
                .iter()
                .map(|key| s3::serde_types::Object {
                    last_modified: String::new(),
                    e_tag: None,
                    storage_class: None,
                    key: key.clone(),
                    owner: None,
                    size: 0,
                })
                .collect(),
            common_prefixes: None,
        }
    }

    #[test]
    fn prefixes_from_keeps_only_other_valid_prefixes_of_the_name() {
        let own = "1111111111111111";
        let keys = vec![
            format!("koto-names/wf/{P}"),
            format!("koto-names/wf/{own}"),
            format!("koto-names/wf/{Q}/x"),
            "koto-names/wf/0123456789ABCDEF".to_string(),
            "koto-names/wf/0123456789abcde".to_string(),
            "koto-names/wf/0123456789abcdef0".to_string(),
            "koto-names/wf/not-hex-at-all!!".to_string(),
            format!("koto-names/other/{Q}"),
            format!("koto-names/wfx/{Q}"),
            "koto-names/_complete".to_string(),
            format!("koto-names/wf/{Q}"),
        ];
        assert_eq!(
            prefixes_from(&[page(&keys)], "wf", own),
            vec![P.to_string(), Q.to_string()]
        );
    }

    #[test]
    fn prefixes_from_names_each_prefix_once() {
        let first = page(&[format!("koto-names/wf/{P}"), format!("koto-names/wf/{P}")]);
        let second = page(&[format!("koto-names/wf/{Q}"), format!("koto-names/wf/{P}")]);
        assert_eq!(
            prefixes_from(&[first, second], "wf", ""),
            vec![P.to_string(), Q.to_string()]
        );
    }

    #[test]
    fn prefixes_from_an_invalid_name_is_empty() {
        let keys = vec![format!("koto-names/a/b/{P}")];
        assert!(prefixes_from(&[page(&keys)], "a/b", "").is_empty());
    }

    fn valid() -> serde_json::Value {
        serde_json::json!({
            "version": 1,
            "declared_at": "2026-11-01T10:00:00Z",
            "machine": "host-a",
            "first_release": "v0.35.0",
            "audit": {
                "started_at": "2026-10-25T09:00:00.123Z",
                "finished_at": "2026-10-25T09:05:00Z",
                "prefixes": 12,
                "sessions": 40,
                "entries_written": 30,
                "entries_pruned": 2
            }
        })
    }

    fn bytes(value: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    #[test]
    fn a_valid_declaration_parses() {
        let declared = parse_declaration(&bytes(&valid())).expect("a valid body parses");
        assert_eq!(declared.version, 1);
        assert_eq!(declared.machine, "host-a");
        assert_eq!(declared.first_release, "v0.35.0");
        assert_eq!(declared.audit.sessions, 40);
        assert_eq!(
            declared.declared_at,
            crate::engine::types::parse_timestamp_to_systemtime("2026-11-01T10:00:00Z").unwrap()
        );
        assert!(declared.audit.started_at < declared.audit.finished_at);

        // A field a newer writer added is ignored.
        let mut more = valid();
        more["note"] = "later".into();
        assert!(parse_declaration(&bytes(&more)).is_some());
    }

    #[test]
    fn a_declaration_over_the_cap_is_undeclared() {
        let mut padded = bytes(&valid());
        padded.resize(MAX_DECLARATION_BYTES + 1, b' ');
        assert!(serde_json::from_slice::<Declaration>(&padded).is_ok());
        assert_eq!(parse_declaration(&padded), None);

        let mut at_cap = bytes(&valid());
        at_cap.resize(MAX_DECLARATION_BYTES, b' ');
        assert!(parse_declaration(&at_cap).is_some());
    }

    #[test]
    fn a_body_that_is_not_json_is_undeclared() {
        assert_eq!(parse_declaration(b"declared"), None);
        assert_eq!(parse_declaration(b""), None);
        assert_eq!(parse_declaration(b"[1]"), None);
    }

    #[test]
    fn a_declaration_missing_a_field_is_undeclared() {
        for field in [
            "version",
            "declared_at",
            "machine",
            "first_release",
            "audit",
        ] {
            let mut body = valid();
            body.as_object_mut().unwrap().remove(field);
            assert_eq!(parse_declaration(&bytes(&body)), None, "without {field}");
        }
        for field in [
            "started_at",
            "finished_at",
            "prefixes",
            "sessions",
            "entries_written",
            "entries_pruned",
        ] {
            let mut body = valid();
            body["audit"].as_object_mut().unwrap().remove(field);
            assert_eq!(
                parse_declaration(&bytes(&body)),
                None,
                "without audit.{field}"
            );
        }
    }

    #[test]
    fn a_version_that_is_not_an_integer_is_undeclared() {
        for version in [serde_json::json!("1"), serde_json::json!(1.5)] {
            let mut body = valid();
            body["version"] = version.clone();
            assert_eq!(parse_declaration(&bytes(&body)), None, "{version}");
        }
    }

    #[test]
    fn a_timestamp_that_does_not_parse_is_undeclared() {
        for (path, value) in [
            ("declared_at", "yesterday"),
            ("declared_at", "2026-13-01T00:00:00Z"),
            ("started_at", "2026-10-25"),
            ("finished_at", "2026-10-25T09:05:00.x1Z"),
        ] {
            let mut body = valid();
            if path == "declared_at" {
                body[path] = value.into();
            } else {
                body["audit"][path] = value.into();
            }
            assert_eq!(parse_declaration(&bytes(&body)), None, "{path} {value:?}");
        }
        let mut body = valid();
        body["declared_at"] = 1_700_000_000.into();
        assert_eq!(parse_declaration(&bytes(&body)), None, "a number");
    }
}
