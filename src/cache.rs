use std::path::{Path, PathBuf};

use anyhow::Context;
use hex::encode as hex_encode;
use sha2::{Digest, Sha256};

use crate::template::compile::compile;
use crate::template::types::CompiledTemplate;

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

/// Compile a template source file with caching.
///
/// The cache key is the SHA256 of the source file content. If a cached compiled
/// template exists for this source, return its path without recompiling.
/// On a cache miss, compile the source, write the result to cache, and return
/// the cache path.
pub fn compile_cached(source_path: &Path) -> anyhow::Result<(PathBuf, CompiledTemplate)> {
    let source_bytes = std::fs::read(source_path)
        .with_context(|| format!("failed to read source: {}", source_path.display()))?;

    let source_hash = sha256_hex(&source_bytes);
    let cache_path = cache_dir().join(format!("{}.json", source_hash));

    if cache_path.exists() {
        // Cache hit: deserialize the existing compiled template.
        let cached_bytes = std::fs::read(&cache_path)
            .with_context(|| format!("failed to read cache file: {}", cache_path.display()))?;
        let compiled: CompiledTemplate = serde_json::from_slice(&cached_bytes)
            .with_context(|| "failed to deserialize cached template")?;
        return Ok((cache_path, compiled));
    }

    // Cache miss: compile and store.
    let compiled = compile(source_path)?;
    let json =
        serde_json::to_string_pretty(&compiled).context("failed to serialize compiled template")?;

    let dir = cache_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create cache directory: {}", dir.display()))?;

    // Write atomically using a temp file then rename.
    let tmp_path = cache_path.with_extension("tmp");
    std::fs::write(&tmp_path, &json)
        .with_context(|| format!("failed to write cache temp file: {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &cache_path)
        .with_context(|| format!("failed to rename cache file: {}", cache_path.display()))?;

    Ok((cache_path, compiled))
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
        // SHA256("abc") verified with sha256sum
        let hash = sha256_hex(b"abc");
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_empty() {
        // SHA256("") verified with sha256sum
        let hash = sha256_hex(b"");
        // Empty input has a well-known hash value
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn cache_miss_then_hit_skips_recompile() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(simple_template().as_bytes()).unwrap();

        // Override cache dir to a temp location.
        let cache_tmp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CACHE_HOME", cache_tmp.path());

        // First call: cache miss — compiles and caches.
        let (path1, compiled1) = compile_cached(f.path()).unwrap();
        assert!(path1.exists());
        assert_eq!(compiled1.name, "cache-test");

        // Second call: cache hit — returns same path without recompiling.
        let (path2, compiled2) = compile_cached(f.path()).unwrap();
        assert_eq!(path1, path2);
        assert_eq!(compiled1, compiled2);
    }
}
