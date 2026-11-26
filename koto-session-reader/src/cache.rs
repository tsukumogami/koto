//! Content hashing shared with koto's compiled-template cache.

use hex::encode as hex_encode;
use sha2::{Digest, Sha256};

/// Compute the SHA256 hex digest of a byte slice.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(hasher.finalize())
}
