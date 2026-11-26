//! The question content area, `koto/packets/` (PRD R62 to R64;
//! DESIGN-decision-record.md, Decision 9).
//!
//! An escalate names its question in a short `text`; the content a person
//! is shown, the full question and its option labels, can be longer. It is
//! written once into the raiser's `koto/packets/<escalate id>`, at most
//! [`MAX_PACKET_BYTES`], and the escalate carries its hash in the optional
//! subject field `packet`, so the content can't be swapped after the
//! question is raised: a write whose bytes don't hash to the escalate's
//! `packet` is refused, and so is a second write.
//!
//! koto stores the bytes as they are. A person's answer reads them back as
//! JSON (a `question` string and an `options` array of objects with a
//! `label`), which [`super::prompt::Expected::from_content`] checks when
//! the answer is recorded, not here.

use std::path::PathBuf;

use super::{codes, store, DecisionError, PACKETS_AREA};
use crate::session::Backend;

/// The most bytes one question's content may hold: 16 KiB, the same cap a
/// decision body has, so one bounded reader serves both.
pub const MAX_PACKET_BYTES: usize = super::body::MAX_BODY_BYTES;

/// The prefix of a content hash.
pub const HASH_PREFIX: &str = "sha256:";

/// The context key of question `id`'s content.
pub fn key(id: &str) -> String {
    format!("{PACKETS_AREA}/{id}")
}

/// The hash an escalate's `packet` subject carries for `bytes`.
pub fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{HASH_PREFIX}{}", hex::encode(Sha256::digest(bytes)))
}

/// Whether `s` is a content hash: `sha256:` and 64 lowercase hex characters.
pub fn is_hash(s: &str) -> bool {
    s.strip_prefix(HASH_PREFIX).is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// The refusal for content past the cap.
fn too_large(len: usize) -> DecisionError {
    DecisionError::invalid(
        "content",
        format!(
            "the question content is {len} bytes, past the 16 KiB cap ({MAX_PACKET_BYTES} bytes)"
        ),
    )
}

/// Write question `id`'s content into `session`, once. `packet` is the
/// hash the escalate carries; `bytes` must hash to it and fit the cap, and
/// the session must hold no content for `id` yet (here or, under the cloud
/// backend, in the bucket, whose create-only put refuses other bytes).
pub fn write(
    backend: &Backend,
    session: &str,
    id: &str,
    packet: &str,
    bytes: &[u8],
) -> Result<(), DecisionError> {
    if bytes.len() > MAX_PACKET_BYTES {
        return Err(too_large(bytes.len()));
    }
    if hash(bytes) != packet {
        return Err(DecisionError::invalid(
            "content",
            format!(
                "the content's hash isn't the 'packet' decision {id} carries, so it isn't the \
                 content that question was raised with"
            ),
        ));
    }
    let local = match backend {
        Backend::Local(l) => l,
        Backend::Cloud(c) => c.local(),
    };
    let key = key(id);
    local
        .with_manifest_lock(session, || {
            if std::fs::symlink_metadata(path(backend, session, id)).is_ok() {
                return Ok(Err(exists(session, id)));
            }
            Ok(
                store::publish_both(backend, session, &key, bytes)?.map_err(|e| {
                    if e.code == codes::DECISION_EXISTS {
                        exists(session, id)
                    } else {
                        e
                    }
                }),
            )
        })
        .map_err(|e| DecisionError::new("decision_write_failed", format!("{e:#}")))?
}

fn exists(session: &str, id: &str) -> DecisionError {
    DecisionError::new(
        codes::DECISION_EXISTS,
        format!("question {id} in '{session}' already has its content, which is written once"),
    )
}

/// Where question `id`'s content sits in `session` on this host.
pub fn path(backend: &Backend, session: &str, id: &str) -> PathBuf {
    let local = match backend {
        Backend::Local(l) => l,
        Backend::Cloud(c) => c.local(),
    };
    store::key_path(&local.ctx_path(session), &key(id))
}

/// Question `id`'s content in `session` on this host: `Ok(None)` when it
/// has none, `Err` when the file is there but can't be read as a regular
/// file within the cap.
pub fn read(backend: &Backend, session: &str, id: &str) -> Result<Option<Vec<u8>>, String> {
    let p = path(backend, session, id);
    if std::fs::symlink_metadata(&p).is_err() {
        return Ok(None);
    }
    store::read_file(&p).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_is_sha256_and_sixty_four_lowercase_hex() {
        let h = hash(b"x");
        assert!(is_hash(&h), "{h}");
        assert!(!is_hash(&h.to_uppercase()));
        assert!(!is_hash(&h[..h.len() - 1]));
        assert!(!is_hash(h.trim_start_matches(HASH_PREFIX)));
    }

    #[test]
    fn the_cap_is_sixteen_kib() {
        assert_eq!(MAX_PACKET_BYTES, 16 * 1024);
    }
}
