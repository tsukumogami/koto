//! Session version tracking and conflict detection.
//!
//! Each cloud-synced session maintains a `version.json` file that tracks:
//! - `version`: monotonically increasing counter, incremented on every sync push
//! - `last_sync_base`: the remote version observed at the last successful sync
//! - `machine_id`: identifies the machine that last wrote this version
//!
//! Three-way conflict detection compares local version, remote version, and
//! the last known sync base to determine whether a push is safe, whether
//! a pull is needed first, or whether both machines have diverged (conflict).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Version metadata for a session, persisted as `version.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionVersion {
    /// Monotonically increasing counter, incremented on each sync push.
    pub version: u64,
    /// The remote version that was observed at the last successful sync.
    pub last_sync_base: u64,
    /// Identifier for the machine that last modified this version file.
    pub machine_id: String,
}

impl SessionVersion {
    /// Create a new version starting at 0 (no syncs yet).
    pub fn new(machine_id: String) -> Self {
        Self {
            version: 0,
            last_sync_base: 0,
            machine_id,
        }
    }

    /// Read a `SessionVersion` from a JSON file. Returns `None` if the file
    /// does not exist.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(path)
            .with_context(|| format!("reading version file: {}", path.display()))?;
        let version: Self = serde_json::from_str(&data)
            .with_context(|| format!("parsing version file: {}", path.display()))?;
        Ok(Some(version))
    }

    /// Write this version to a JSON file, creating parent directories if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        fs::write(path, data)
            .with_context(|| format!("writing version file: {}", path.display()))?;
        Ok(())
    }
}

/// Result of comparing local and remote versions before a sync push.
#[derive(Debug, PartialEq)]
pub enum SyncCheck {
    /// Remote matches last_sync_base: safe to push.
    Safe,
    /// Remote is newer and local hasn't diverged: pull remote first.
    RemoteNewer,
    /// Both local and remote advanced past last_sync_base: conflict.
    Conflict {
        local_version: u64,
        remote_version: u64,
        local_machine: String,
        remote_machine: String,
    },
}

/// Perform three-way conflict detection.
///
/// Arguments:
/// - `local`: the local session version (version=L, last_sync_base=B)
/// - `remote`: the remote session version (version=R), or `None` if no
///   remote version exists (first sync)
///
/// Returns the appropriate `SyncCheck` variant.
pub fn check_sync(local: &SessionVersion, remote: Option<&SessionVersion>) -> SyncCheck {
    let remote = match remote {
        Some(r) => r,
        None => return SyncCheck::Safe, // No remote version means first sync.
    };

    let r = remote.version;
    let l = local.version;
    let b = local.last_sync_base;

    if r <= b {
        // Remote hasn't advanced past our sync base (or regressed). Safe.
        SyncCheck::Safe
    } else if l == b {
        // Remote is newer but we haven't made local changes since last sync.
        SyncCheck::RemoteNewer
    } else {
        // Both sides advanced: conflict.
        SyncCheck::Conflict {
            local_version: l,
            remote_version: r,
            local_machine: local.machine_id.clone(),
            remote_machine: remote.machine_id.clone(),
        }
    }
}

/// Format a conflict error message for display.
pub fn conflict_message(
    local_version: u64,
    remote_version: u64,
    local_machine: &str,
    remote_machine: &str,
) -> String {
    format!(
        "session conflict: local version {} (machine {}), remote version {} (machine {})\n\
         Run `koto session resolve --keep local` or `--keep remote` to resolve.",
        local_version, local_machine, remote_version, remote_machine
    )
}

/// Compute the resolved version after conflict resolution.
/// Sets version to `max(local, remote) + 1` with `last_sync_base` equal to the new version.
pub fn resolved_version(
    local: &SessionVersion,
    remote: &SessionVersion,
    machine_id: &str,
) -> SessionVersion {
    let max_version = std::cmp::max(local.version, remote.version);
    SessionVersion {
        version: max_version + 1,
        last_sync_base: max_version + 1,
        machine_id: machine_id.to_string(),
    }
}

/// Generate a machine ID by hashing the hostname.
///
/// Uses the first 8 characters of the SHA-256 hex digest of the hostname.
/// Falls back to "unknown" if the hostname cannot be determined.
pub fn generate_machine_id() -> String {
    let hostname = get_hostname();
    let hash = crate::cache::sha256_hex(hostname.as_bytes());
    hash[..8].to_string()
}

/// Get the system hostname. Falls back to "unknown" on failure.
fn get_hostname() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        let ret = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if ret == 0 {
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            return String::from_utf8_lossy(&buf[..len]).to_string();
        }
    }
    #[cfg(not(unix))]
    {
        if let Ok(output) = std::process::Command::new("hostname").output() {
            if output.status.success() {
                return String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
        }
    }
    "unknown".to_string()
}

/// Get or generate the machine ID.
///
/// Reads from the user config file at `~/.koto/config.toml`. If the
/// `machine.id` key is not set, generates a new ID from the hostname hash
/// and writes it to the config.
pub fn get_or_create_machine_id() -> Result<String> {
    use crate::config::resolve::{
        ensure_koto_dir, load_toml_value, user_config_path, write_toml_value,
    };

    // Try to read from user config.
    if let Some(config_path) = user_config_path() {
        if config_path.exists() {
            let doc = load_toml_value(&config_path)?;
            if let Some(machine) = doc.get("machine") {
                if let Some(id) = machine.get("id") {
                    if let Some(s) = id.as_str() {
                        if !s.is_empty() {
                            return Ok(s.to_string());
                        }
                    }
                }
            }
        }
    }

    // Generate and persist.
    let machine_id = generate_machine_id();
    let koto_dir = ensure_koto_dir()?;
    let config_path = koto_dir.join("config.toml");

    let mut doc = load_toml_value(&config_path)?;
    let table = doc
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config is not a TOML table"))?;
    let machine = table
        .entry("machine")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let machine_table = machine
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("machine is not a table"))?;
    machine_table.insert("id".to_string(), toml::Value::String(machine_id.clone()));
    write_toml_value(&config_path, &doc)?;

    Ok(machine_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -- SessionVersion read/write --

    #[test]
    fn save_and_load_version() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("version.json");

        let v = SessionVersion {
            version: 5,
            last_sync_base: 3,
            machine_id: "abc123".to_string(),
        };
        v.save(&path).unwrap();

        let loaded = SessionVersion::load(&path).unwrap().unwrap();
        assert_eq!(loaded, v);
    }

    #[test]
    fn load_returns_none_for_missing_file() {
        let result = SessionVersion::load(Path::new("/tmp/nonexistent_koto_version.json")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn new_starts_at_zero() {
        let v = SessionVersion::new("machine1".to_string());
        assert_eq!(v.version, 0);
        assert_eq!(v.last_sync_base, 0);
        assert_eq!(v.machine_id, "machine1");
    }

    // -- Conflict detection --

    #[test]
    fn check_sync_safe_when_no_remote() {
        let local = SessionVersion::new("m1".to_string());
        assert_eq!(check_sync(&local, None), SyncCheck::Safe);
    }

    #[test]
    fn check_sync_safe_when_remote_equals_base() {
        let local = SessionVersion {
            version: 5,
            last_sync_base: 3,
            machine_id: "m1".to_string(),
        };
        let remote = SessionVersion {
            version: 3,
            last_sync_base: 3,
            machine_id: "m2".to_string(),
        };
        assert_eq!(check_sync(&local, Some(&remote)), SyncCheck::Safe);
    }

    #[test]
    fn check_sync_safe_when_remote_less_than_base() {
        let local = SessionVersion {
            version: 5,
            last_sync_base: 4,
            machine_id: "m1".to_string(),
        };
        let remote = SessionVersion {
            version: 2,
            last_sync_base: 2,
            machine_id: "m2".to_string(),
        };
        assert_eq!(check_sync(&local, Some(&remote)), SyncCheck::Safe);
    }

    #[test]
    fn check_sync_remote_newer_when_local_unchanged() {
        let local = SessionVersion {
            version: 3,
            last_sync_base: 3,
            machine_id: "m1".to_string(),
        };
        let remote = SessionVersion {
            version: 5,
            last_sync_base: 5,
            machine_id: "m2".to_string(),
        };
        assert_eq!(check_sync(&local, Some(&remote)), SyncCheck::RemoteNewer);
    }

    #[test]
    fn check_sync_conflict_when_both_advanced() {
        let local = SessionVersion {
            version: 5,
            last_sync_base: 3,
            machine_id: "m1".to_string(),
        };
        let remote = SessionVersion {
            version: 6,
            last_sync_base: 6,
            machine_id: "m2".to_string(),
        };
        assert_eq!(
            check_sync(&local, Some(&remote)),
            SyncCheck::Conflict {
                local_version: 5,
                remote_version: 6,
                local_machine: "m1".to_string(),
                remote_machine: "m2".to_string(),
            }
        );
    }

    // -- Resolution --

    #[test]
    fn resolved_version_uses_max_plus_one() {
        let local = SessionVersion {
            version: 7,
            last_sync_base: 5,
            machine_id: "m1".to_string(),
        };
        let remote = SessionVersion {
            version: 6,
            last_sync_base: 6,
            machine_id: "m2".to_string(),
        };
        let resolved = resolved_version(&local, &remote, "m1");
        assert_eq!(resolved.version, 8);
        assert_eq!(resolved.last_sync_base, 8);
        assert_eq!(resolved.machine_id, "m1");
    }

    #[test]
    fn resolved_version_when_remote_is_higher() {
        let local = SessionVersion {
            version: 3,
            last_sync_base: 2,
            machine_id: "m1".to_string(),
        };
        let remote = SessionVersion {
            version: 10,
            last_sync_base: 10,
            machine_id: "m2".to_string(),
        };
        let resolved = resolved_version(&local, &remote, "m1");
        assert_eq!(resolved.version, 11);
        assert_eq!(resolved.last_sync_base, 11);
    }

    // -- Conflict message --

    #[test]
    fn conflict_message_includes_versions_and_machines() {
        let msg = conflict_message(7, 6, "a1b2c3", "d4e5f6");
        assert!(msg.contains("local version 7"));
        assert!(msg.contains("remote version 6"));
        assert!(msg.contains("machine a1b2c3"));
        assert!(msg.contains("machine d4e5f6"));
        assert!(msg.contains("koto session resolve"));
    }

    // -- Version increment --

    #[test]
    fn version_increment_flow() {
        // Simulate a normal sync: start at 0, increment to 1, update base.
        let mut v = SessionVersion::new("m1".to_string());
        assert_eq!(v.version, 0);

        // Sync push: increment version, update base after success.
        v.version += 1;
        v.last_sync_base = v.version;
        assert_eq!(v.version, 1);
        assert_eq!(v.last_sync_base, 1);

        // Another sync.
        v.version += 1;
        v.last_sync_base = v.version;
        assert_eq!(v.version, 2);
        assert_eq!(v.last_sync_base, 2);
    }

    // -- Machine ID generation --

    #[test]
    fn generate_machine_id_is_8_chars() {
        let id = generate_machine_id();
        assert_eq!(id.len(), 8);
        // All hex chars.
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generate_machine_id_is_deterministic() {
        let id1 = generate_machine_id();
        let id2 = generate_machine_id();
        assert_eq!(id1, id2);
    }
}
