//! The id and name shapes koto's records and validators share.
//!
//! An id is a value koto or a harness minted (a session id, a tool-call
//! id); a name is one an author chose (a template, a state, a check). Both
//! shapes are short and ASCII-only, so a value that has one can be written
//! to a record as it is. A value with neither is written as its digest.

/// The longest id or name, in bytes. Both shapes are ASCII-only, so this
/// is also the character limit.
const MAX_VALUE_BYTES: usize = 128;

/// Whether `value` has the id shape: 1 to 128 characters from
/// `A-Z a-z 0-9 . _ : -`.
pub(crate) fn is_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_VALUE_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// Whether `value` has the name shape: 1 to 128 characters from
/// `A-Z a-z 0-9 . _ : / @ -`, not starting with `/` or `~`, so an absolute
/// path can't pass as a template or state name.
pub(crate) fn is_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_VALUE_BYTES
        && !value.starts_with('/')
        && !value.starts_with('~')
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'@' | b'-')
        })
}

/// `value` when it has the name shape, otherwise `sha256:` and the first 32
/// lowercase hex characters of the SHA-256 of its UTF-8 bytes. Used for
/// rule ids and decider criterion ids, which koto doesn't restrict to the
/// name shape: the same id gives the same value
/// in every session, and the result always has the name shape, so an odd
/// id keeps its identity without its text reaching the journal.
pub(crate) fn name_or_digest(value: &str) -> String {
    if is_name(value) {
        return value.to_string();
    }
    digest(value)
}

/// `sha256:` and the first 32 lowercase hex characters of the SHA-256 of
/// `value`'s UTF-8 bytes, whatever its shape: how [`name_or_digest`] writes
/// an odd id, and how a unit's key, and a document or section an extraction
/// printed, are always written, so nothing an extraction printed reaches the
/// journal. The result has the id shape.
pub(crate) fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let hex = hex::encode(Sha256::digest(value.as_bytes()));
    format!("sha256:{}", &hex[..32])
}
