//! `koto pool status [--json]` (DESIGN-host-resource-pools.md, "Interfaces").
//!
//! Lists every pool the user config defines, with who holds its slots and
//! who waits for one, in queue order. It reads and never writes: it takes no
//! session lock and no `queue.lock`, reads `queue.json` as it stands, and
//! probes slot files with non-blocking tries it releases at once, so it
//! answers while a holder is mid-run or another process holds a session's
//! lock, and changes no file under the koto root. A pool whose config entry
//! is invalid is reported with `pool_config_invalid`, and the command then
//! exits 1.

use std::path::Path;

use serde_json::{json, Value};

use crate::pool::config::{self, PoolLookup};
use crate::pool::store;

/// The exit code when any configured pool is invalid.
pub const EXIT_INVALID: i32 = 1;

/// Run `koto pool status`. Returns the process exit code.
pub fn status(json_out: bool) -> anyhow::Result<i32> {
    let root = crate::koto_home::koto_home_or_err()?;
    let report = report(&root, store::now_secs());
    if json_out {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", plain(&report));
    }
    let invalid = report["pools"]
        .as_array()
        .is_some_and(|pools| pools.iter().any(|p| p["status"] == "invalid"));
    Ok(if invalid { EXIT_INVALID } else { 0 })
}

/// The status of every pool `<root>/config.toml` defines, as of `now`.
pub fn report(root: &Path, now: u64) -> Value {
    let config_path = root.join("config.toml");
    let config_file = config_path.display().to_string();
    let invalid = |name: &str, key: &str, reason: String| {
        json!({
            "name": name,
            "status": "invalid",
            "error_code": "pool_config_invalid",
            "config_file": config_file,
            "key": key,
            "reason": reason,
        })
    };
    let text = std::fs::read_to_string(&config_path).unwrap_or_default();
    let mut pools = Vec::new();
    if text.contains("pools") {
        match text.parse::<toml::Value>() {
            Err(e) => pools.push(invalid(
                "",
                "pools",
                format!("the config file does not parse: {e}"),
            )),
            Ok(value) => match value.get("pools") {
                None => {}
                Some(toml::Value::Table(table)) => {
                    for name in table.keys() {
                        if !crate::template::types::is_valid_pool_name(name) {
                            pools.push(invalid(
                                name,
                                &format!("pools.{name}"),
                                "is not a valid pool name".to_string(),
                            ));
                            continue;
                        }
                        pools.push(match config::lookup_in(&text, name) {
                            PoolLookup::Defined(spec) => pool(root, name, spec.size, now),
                            PoolLookup::Invalid { key, reason } => invalid(name, &key, reason),
                            // Listed in the table, so never undefined.
                            PoolLookup::Undefined => continue,
                        });
                    }
                }
                Some(_) => pools.push(invalid("", "pools", "must be a table".to_string())),
            },
        }
    }
    json!({ "config_file": config_file, "pools": pools })
}

/// One defined pool's holders and waiters. Nothing is created: a pool no
/// session has used yet has no directory and reads as all free.
fn pool(root: &Path, name: &str, size: u32, now: u64) -> Value {
    let dir = root.join("pools").join(name);
    let exists = std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_dir());
    let (held, queue) = if exists {
        (store::holders(&dir), store::read_queue(&dir))
    } else {
        (Vec::new(), store::Queue::default())
    };
    let holders: Vec<Value> = held
        .iter()
        .map(|(slot, holder)| match holder {
            Some(h) => json!({
                "slot": slot,
                "session": h.session,
                "workspace": h.workspace,
                "state": h.state,
                "held_secs": now.saturating_sub(h.since),
            }),
            // Held, but its record is being written or was cut.
            None => json!({
                "slot": slot,
                "session": "",
                "workspace": "",
                "state": "",
                "held_secs": 0,
            }),
        })
        .collect();
    let waiters: Vec<Value> = queue
        .entries
        .iter()
        .map(|e| {
            json!({
                "session": e.session,
                "workspace": e.workspace,
                "waited_secs": now.saturating_sub(e.joined_at),
            })
        })
        .collect();
    json!({
        "name": name,
        "status": "ok",
        "size": size,
        "held": holders.len(),
        "free": (size as usize).saturating_sub(holders.len()),
        "waiting": waiters.len(),
        "holders": holders,
        "waiters": waiters,
    })
}

/// `s` with every control character escaped, for a value printed to a
/// terminal: another session's name can't move the cursor or recolour it.
fn esc(value: &Value) -> String {
    let s = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    s.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// The plain form: one block per pool.
fn plain(report: &Value) -> String {
    let pools = report["pools"].as_array().cloned().unwrap_or_default();
    if pools.is_empty() {
        return format!(
            "no pools are configured in {}\n",
            esc(&report["config_file"])
        );
    }
    let mut out = String::new();
    for p in &pools {
        if p["status"] == "invalid" {
            out.push_str(&format!(
                "pool {}: pool_config_invalid\n  {} {} (in {})\n",
                esc(&p["name"]),
                esc(&p["key"]),
                esc(&p["reason"]),
                esc(&p["config_file"]),
            ));
            continue;
        }
        out.push_str(&format!(
            "pool {}: size {}, held {}, free {}, waiting {}\n",
            esc(&p["name"]),
            p["size"],
            p["held"],
            p["free"],
            p["waiting"],
        ));
        for h in p["holders"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "  holder: session {}, workspace {}, state {}, held {}s\n",
                esc(&h["session"]),
                esc(&h["workspace"]),
                esc(&h["state"]),
                h["held_secs"],
            ));
        }
        for (i, w) in p["waiters"].as_array().into_iter().flatten().enumerate() {
            out.push_str(&format!(
                "  waiter {}: session {}, workspace {}, waited {}s\n",
                i + 1,
                esc(&w["session"]),
                esc(&w["workspace"]),
                w["waited_secs"],
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_config_lists_no_pools_and_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        let r = report(root.path(), 0);
        assert_eq!(r["pools"], json!([]));
        assert!(plain(&r).starts_with("no pools are configured"));
        assert!(!root.path().join("pools").exists());
    }

    #[test]
    fn an_unused_pool_reads_as_all_free_and_an_invalid_one_names_its_key() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("config.toml"),
            "[pools.a]\nsize = 2\n[pools.b]\nsize = 0\n",
        )
        .unwrap();
        let r = report(root.path(), 0);
        let pools = r["pools"].as_array().unwrap();
        assert_eq!(pools[0]["free"], 2);
        assert_eq!(pools[0]["held"], 0);
        assert_eq!(pools[1]["error_code"], "pool_config_invalid");
        assert_eq!(pools[1]["key"], "pools.b.size");
        assert!(!root.path().join("pools").exists());
    }

    #[test]
    fn control_characters_are_escaped_in_the_plain_form() {
        assert_eq!(esc(&json!("a\u{1b}[31mb\n")), "a\\u{1b}[31mb\\n");
    }
}
