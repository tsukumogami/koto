//! Shape guard for the koto-skills plugin's hook file.
//!
//! Claude Code loads a plugin's hooks from `hooks/hooks.json` under the plugin
//! root, in the matcher-group shape: each event maps to a list of matcher
//! groups, and each group carries a `hooks` list of
//! `{"type": "command", "command": ...}` entries. The plugin used to ship a
//! root-level `hooks.json` with the command entries directly under each event,
//! which Claude Code never read, so none of its hooks ran.
//!
//! The file now registers no hook; its empty `hooks` object is where a later
//! SessionStart group goes. The /workflows SessionStart script
//! (`hooks/session-start-workflows.sh`) and the Stop reminder
//! (`hooks/stop-workflows.sh`) stay on disk and stay off: registering them
//! would run a command at every session start and every turn end for every
//! koto-skills user. The test fails on any `Stop` event and on any command
//! naming either script, so turning one on by those names means changing this
//! test too. It matches names, not behaviour: a renamed copy would pass.

use serde_json::Value;
use std::path::Path;

const HOOKS_FILE: &str = "plugins/koto-skills/hooks/hooks.json";

/// Scripts kept on disk but deliberately not registered under any event.
const UNREGISTERED_SCRIPTS: &[&str] = &["session-start-workflows.sh", "stop-workflows.sh"];

/// Checks the hook file at `path`, returning every problem found.
fn check_hooks_path(path: &Path) -> Result<(), Vec<String>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| vec![format!("cannot read {}: {e}", path.display())])?;
    check_hooks_text(&text)
}

/// Checks hook file contents against the matcher-group shape and the
/// unregistered-script rule.
fn check_hooks_text(text: &str) -> Result<(), Vec<String>> {
    let doc: Value =
        serde_json::from_str(text).map_err(|e| vec![format!("not valid JSON: {e}")])?;
    let events = doc
        .get("hooks")
        .and_then(Value::as_object)
        .ok_or_else(|| vec!["top-level `hooks` must be an object".to_string()])?;

    let mut problems = Vec::new();
    for (event, groups) in events {
        if event == "Stop" {
            problems
                .push("registers a `Stop` event; the plugin runs nothing at turn end".to_string());
        }
        let Some(groups) = groups.as_array() else {
            problems.push(format!("`{event}` must map to a list of matcher groups"));
            continue;
        };
        for (i, group) in groups.iter().enumerate() {
            let at = format!("`{event}`[{i}]");
            if group.get("type").is_some() || group.get("command").is_some() {
                problems.push(format!(
                    "{at} carries `type`/`command` directly: the old flat shape, \
                     which Claude Code does not read; wrap it in a `hooks` list"
                ));
            }
            let Some(hooks) = group.get("hooks").and_then(Value::as_array) else {
                problems.push(format!("{at} has no `hooks` list"));
                continue;
            };
            for (j, hook) in hooks.iter().enumerate() {
                let at = format!("{at}.hooks[{j}]");
                if hook.get("type").and_then(Value::as_str) != Some("command") {
                    problems.push(format!("{at} must have `\"type\": \"command\"`"));
                }
                match hook.get("command").and_then(Value::as_str) {
                    None => problems.push(format!("{at} has no string `command`")),
                    Some(cmd) => {
                        for script in UNREGISTERED_SCRIPTS {
                            if cmd.contains(script) {
                                problems.push(format!(
                                    "{at} registers `hooks/{script}`, which stays off"
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

#[test]
fn plugin_hook_file_is_matcher_group_shaped_and_keeps_old_hooks_off() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(HOOKS_FILE);
    if let Err(problems) = check_hooks_path(&path) {
        panic!("{HOOKS_FILE}:\n  {}", problems.join("\n  "));
    }
}

#[test]
fn unregistered_scripts_stay_on_disk() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/koto-skills/hooks");
    for script in UNREGISTERED_SCRIPTS {
        assert!(dir.join(script).is_file(), "hooks/{script} is missing");
    }
}

// The checker itself, against the inputs it must reject.

#[test]
fn rejects_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    assert!(check_hooks_path(&dir.path().join("hooks.json")).is_err());
}

#[test]
fn rejects_invalid_json() {
    let problems = check_hooks_text("{\"hooks\": {").unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains("not valid JSON")),
        "{problems:?}"
    );
}

#[test]
fn rejects_the_old_flat_file() {
    // The root file as it shipped before the move, Stop command shortened.
    let old = r#"{
      "hooks": {
        "SessionStart": [
          {"type": "command", "command": "bash \"${CLAUDE_PLUGIN_ROOT}/hooks/session-start-workflows.sh\""}
        ],
        "Stop": [
          {"type": "command", "command": "WORKFLOWS=$(koto workflows 2>/dev/null); true"}
        ]
      }
    }"#;
    let problems = check_hooks_text(old).unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains("old flat shape")),
        "{problems:?}"
    );
}

#[test]
fn rejects_a_stop_event_in_matcher_group_shape() {
    let text = r#"{"hooks": {"Stop": [{"hooks": [
        {"type": "command", "command": "bash \"${CLAUDE_PLUGIN_ROOT}/hooks/stop-workflows.sh\""}
    ]}]}}"#;
    let problems = check_hooks_text(text).unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains("`Stop` event")),
        "{problems:?}"
    );
}

#[test]
fn rejects_the_workflows_script_under_session_start() {
    let text = r#"{"hooks": {"SessionStart": [{"hooks": [
        {"type": "command", "command": "bash \"${CLAUDE_PLUGIN_ROOT}/hooks/session-start-workflows.sh\""}
    ]}]}}"#;
    let problems = check_hooks_text(text).unwrap_err();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("session-start-workflows.sh")),
        "{problems:?}"
    );
}

#[test]
fn accepts_another_session_start_group() {
    // The shape a later SessionStart hook is added in.
    let text = r#"{"hooks": {"SessionStart": [{"hooks": [
        {"type": "command", "command": "bash \"${CLAUDE_PLUGIN_ROOT}/hooks/other.sh\""}
    ]}]}}"#;
    assert_eq!(check_hooks_text(text), Ok(()));
}
