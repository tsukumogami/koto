//! A scratch home that koto's sink discovery accepts, with a stub event
//! sink at the fixed path under it.
//!
//! Include it from a test file, after `koto_command`, with
//!
//! ```ignore
//! #[path = "support/koto_command.rs"]
//! mod koto_command;
//! #[path = "support/event_sink.rs"]
//! mod event_sink;
//! ```
//!
//! No test reaches a real sink. Every koto run a test starts has a `HOME`
//! of its own (`tests/support/test_home.rs`), which holds no sink, so a
//! test that installs none has none. A test that wants one builds a
//! [`SinkHome`], which installs the stub of `event_sink_stub.rs`, and runs
//! koto through [`SinkHome::koto`], which always sets `HOME` to the scratch
//! home. `tests/koto_command_env.rs` checks that it does.
//!
//! ## Where the home lives
//!
//! Discovery refuses a sink under a directory that group or others can
//! write, and on most hosts the system temporary directory is exactly that
//! (`/tmp` is mode 1777). A home under it would fail the check, and every
//! test would quietly exercise the no-sink path. So the home is made under
//! `CARGO_TARGET_TMPDIR`, the build directory's scratch space, with its own
//! directories mode 0700, and [`SinkHome::new`] checks the whole chain of
//! ancestors up to `/` before the test goes on. When the build directory
//! itself sits under a path discovery refuses, the check fails with a
//! message naming the directory and what is wrong with it, instead of
//! letting the tests pass without a sink.

#![allow(dead_code)]

#[path = "event_sink_stub.rs"]
pub mod event_sink_stub;

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

pub use event_sink_stub::{Call, Stub};

/// The sink's fixed path under a home directory.
pub const SINK_DIR: &str = ".local/libexec/fleet";
pub const SINK_FILE: &str = "event-sink";

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A scratch home under the build directory, removed when dropped.
pub struct SinkHome {
    root: PathBuf,
    /// The `HOME` koto runs with.
    pub home: PathBuf,
    /// A working directory for the test's templates and scripts.
    pub work: PathBuf,
}

fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Why discovery would refuse a sink under `dir`, if it would: the first
/// ancestor that isn't owned by this user or root, or that group or others
/// can write.
fn untrusted_ancestor(dir: &Path) -> Option<String> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    let mut ancestor = Some(dir);
    while let Some(d) = ancestor {
        let meta = std::fs::symlink_metadata(d).unwrap();
        let mode = meta.permissions().mode() & 0o7777;
        if meta.uid() != uid && meta.uid() != 0 {
            return Some(format!("{} is owned by uid {}", d.display(), meta.uid()));
        }
        if mode & 0o022 != 0 {
            return Some(format!(
                "{} is mode {:o}, writable by group or others",
                d.display(),
                mode
            ));
        }
        ancestor = d.parent();
    }
    None
}

impl SinkHome {
    /// A home with the sink's directory in place and no sink in it.
    ///
    /// Panics, naming the offending directory, when the build directory's
    /// own ancestors would make discovery refuse any sink under it.
    pub fn new(label: &str) -> SinkHome {
        let base = std::fs::canonicalize(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let root = base.join(format!(
            "event-sink-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let work = root.join("work");
        std::fs::create_dir_all(home.join(SINK_DIR)).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        // Every directory the harness made is owner-only, whatever the
        // umask.
        let mut d = Some(home.join(SINK_DIR));
        while let Some(dir) = d {
            set_mode(&dir, 0o700);
            if dir == root {
                break;
            }
            d = dir.parent().map(Path::to_path_buf);
        }
        set_mode(&work, 0o700);
        if let Some(why) = untrusted_ancestor(&home.join(SINK_DIR)) {
            let _ = std::fs::remove_dir_all(&root);
            panic!(
                "the build directory can't hold a sink that koto's discovery accepts: {why}. \
                 Every directory from the sink up to / must be owned by this user or root and \
                 writable by nobody else; build into a directory where that holds \
                 (CARGO_TARGET_DIR) or fix the mode."
            );
        }
        SinkHome { root, home, work }
    }

    /// A home with a stub sink installed at the fixed path.
    pub fn with_stub(label: &str) -> (SinkHome, Stub) {
        let home = SinkHome::new(label);
        let stub = home.install_stub();
        (home, stub)
    }

    /// The directory the sink lives in.
    pub fn sink_dir(&self) -> PathBuf {
        self.home.join(SINK_DIR)
    }

    /// The sink's fixed path.
    pub fn sink_path(&self) -> PathBuf {
        self.sink_dir().join(SINK_FILE)
    }

    /// Install the stub sink at the fixed path. Its control directory is
    /// outside the home, so nothing koto writes lands beside it.
    pub fn install_stub(&self) -> Stub {
        self.install_stub_at(&self.sink_path())
    }

    /// Install the stub somewhere else, for a test that then links or
    /// moves it.
    pub fn install_stub_at(&self, path: &Path) -> Stub {
        Stub::install(path, &self.root.join("sink-ctl"))
    }

    /// A koto command that runs under this home, from the working
    /// directory, with none of the developer's variables that would move
    /// the store, the config or the driver. `HOME` is always set here: the
    /// sink is found through it and nothing else.
    pub fn koto(&self) -> assert_cmd::Command {
        let mut cmd = crate::koto_command::koto();
        cmd.current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", self.root.join("cache"));
        for unset in [
            "KOTO_SESSIONS_BASE",
            "KOTO_HOME",
            "XDG_CONFIG_HOME",
            "KOTO_WORKFLOWS_DIR",
            "KOTO_ANCHOR",
            "CLAUDE_CODE_SESSION_ID",
            "FLEET_EVENT_SINK",
            "NIWA_DISPATCH_ID",
            "NIWA_WORKSPACE",
        ] {
            cmd.env_remove(unset);
        }
        cmd
    }

    /// The koto home under this home.
    pub fn koto_home(&self) -> PathBuf {
        self.home.join(".koto")
    }

    /// The directory of session `name`.
    pub fn session_dir(&self, name: &str) -> PathBuf {
        self.koto_home().join("sessions").join(name)
    }

    /// The count of dropped records the session's `telemetry.json` holds, 0
    /// when the file isn't there.
    pub fn dropped(&self, name: &str) -> u64 {
        std::fs::read_to_string(self.session_dir(name).join("telemetry.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| v["dropped"].as_u64())
            .unwrap_or(0)
    }

    /// The run journal's lines, parsed; empty when there is no journal.
    pub fn journal(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.koto_home().join("_run_journal.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

impl Drop for SinkHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Every record the stub received across `calls`, parsed, in order.
pub fn sent(calls: &[Call]) -> Vec<serde_json::Value> {
    calls
        .iter()
        .flat_map(|c| c.lines.iter())
        .map(|l| {
            serde_json::from_str(l)
                .unwrap_or_else(|e| panic!("the sink read a bad line ({e}): {l}"))
        })
        .collect()
}
