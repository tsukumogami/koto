use std::path::{Path, PathBuf};

use anyhow::Context;
use hex::encode as hex_encode;
use sha2::{Digest, Sha256};

use crate::template::compile::compile;

/// Return the koto cache directory: `$XDG_CACHE_HOME/koto` or `~/.cache/koto`.
fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into());
            PathBuf::from(home).join(".cache")
        })
        .join("koto")
}

/// Compute the SHA256 hex digest of a byte slice.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(hasher.finalize())
}

/// Core cache logic with an explicit cache directory.
///
/// The cache key is SHA256 of the compiled JSON. Returns `(cache_path, hash)`.
fn compile_cached_into(source_path: &Path, dir: &Path) -> anyhow::Result<(PathBuf, String)> {
    let compiled = compile(source_path)?;
    let json =
        serde_json::to_string_pretty(&compiled).context("failed to serialize compiled template")?;
    let json_bytes = json.as_bytes();

    let hash = sha256_hex(json_bytes);
    let cache_path = dir.join(format!("{}.json", hash));

    if cache_path.exists() {
        return Ok((cache_path, hash));
    }

    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create cache directory: {}", dir.display()))?;

    let tmp_path = cache_path.with_extension("tmp");
    std::fs::write(&tmp_path, json_bytes)
        .with_context(|| format!("failed to write cache temp file: {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &cache_path)
        .with_context(|| format!("failed to rename cache file: {}", cache_path.display()))?;

    Ok((cache_path, hash))
}

/// Compile a template source file with caching.
///
/// The cache key is the SHA256 of the compiled JSON output. On a cache miss,
/// the source is compiled, the result is serialized to JSON, and the SHA256
/// of that JSON is used as both the cache filename and the returned hash.
/// On a cache hit, the cached file is returned without recompilation.
///
/// Returns `(cache_path, template_hash)` where `template_hash` is the
/// SHA256 hex of the compiled JSON — suitable for use as `template_hash`
/// in the JSONL init event.
pub fn compile_cached(source_path: &Path) -> anyhow::Result<(PathBuf, String)> {
    compile_cached_into(source_path, &cache_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn simple_template() -> &'static str {
        r#"---
name: cache-test
version: "1.0"
initial_state: only
states:
  only:
    terminal: true
---

## only

Cache test directive.
"#
    }

    #[test]
    fn sha256_known_vector() {
        let hash = sha256_hex(b"abc");
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_empty() {
        let hash = sha256_hex(b"");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn cache_miss_then_hit_skips_recompile() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(simple_template().as_bytes()).unwrap();

        let cache_tmp = tempfile::tempdir().unwrap();
        let cache_dir = cache_tmp.path().join("koto");

        let (path1, hash1) = compile_cached_into(f.path(), &cache_dir).unwrap();
        assert!(path1.exists());
        assert_eq!(hash1.len(), 64);
        assert!(hash1.chars().all(|c| c.is_ascii_hexdigit()));

        let (path2, hash2) = compile_cached_into(f.path(), &cache_dir).unwrap();
        assert_eq!(path1, path2);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn hash_is_sha256_of_compiled_json() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(simple_template().as_bytes()).unwrap();

        let cache_tmp = tempfile::tempdir().unwrap();
        let cache_dir = cache_tmp.path().join("koto");

        let (cache_path, hash) = compile_cached_into(f.path(), &cache_dir).unwrap();

        let written_bytes = std::fs::read(&cache_path).unwrap();
        let expected = sha256_hex(&written_bytes);
        assert_eq!(hash, expected);
    }
}
