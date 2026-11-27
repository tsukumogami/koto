//! A stub event sink: a shell script that records every call it gets.
//!
//! [`Stub::install`] writes the script to a path and gives it a control
//! directory. The script logs each call there (when it started, its
//! arguments, its environment, the lines it read) and takes its exit code
//! and a sleep from files a test writes, so nothing about a test's sink
//! travels through koto's environment. `event_sink_stub.sh` is the script's
//! body and documents the files.
//!
//! This file is included from two places: `tests/support/event_sink.rs`,
//! which puts a stub at the fixed path under a scratch home for tests that
//! run the koto binary, and koto's own unit tests, which hand the path
//! straight to the sender. It uses nothing but the standard library.

#![allow(dead_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const BODY: &str = include_str!("event_sink_stub.sh");

/// An installed stub sink and its control directory.
pub struct Stub {
    /// The script's own path.
    pub path: PathBuf,
    ctl: PathBuf,
}

/// One call the stub received.
#[derive(Debug, Clone)]
pub struct Call {
    /// When the call started, in seconds since the epoch.
    pub at: f64,
    /// The arguments after the program name.
    pub argv: Vec<String>,
    /// The environment the sink saw, as `NAME=value` lines.
    pub env: Vec<String>,
    /// The lines read from stdin.
    pub lines: Vec<String>,
    /// Whether stdin was read to its end.
    pub read: bool,
    /// Whether the call ran to its own exit, rather than being killed.
    pub done: bool,
    /// What the `require_dir` check found, when one was set.
    pub dir_exists: Option<bool>,
    /// The background child's pid, when `fork` was set.
    pub child_pid: Option<i32>,
}

impl Call {
    /// The names of the variables the sink was started with.
    pub fn env_names(&self) -> Vec<String> {
        self.env
            .iter()
            .filter_map(|l| l.split_once('=').map(|(name, _)| name.to_string()))
            .collect()
    }

    /// The value of `--carry`, when the call had one.
    pub fn carry(&self) -> Option<u64> {
        let at = self.argv.iter().position(|a| a == "--carry")?;
        self.argv.get(at + 1)?.parse().ok()
    }
}

impl Stub {
    /// Write the stub to `path`, mode 0700, with `ctl` as its control
    /// directory. `ctl` is created, with an empty call log.
    pub fn install(path: &Path, ctl: &Path) -> Stub {
        std::fs::create_dir_all(ctl.join("calls")).unwrap();
        let quoted = ctl.to_str().unwrap();
        assert!(
            !quoted.contains('\''),
            "the stub's control directory can't hold a quote: {quoted}"
        );
        let script = format!("#!/bin/sh\nCTL='{quoted}'\n{BODY}");
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Stub {
            path: path.to_path_buf(),
            ctl: ctl.to_path_buf(),
        }
    }

    /// Write setting `name` (`exit`, `sleep`, `require_dir`, `fork`).
    pub fn set(&self, name: &str, value: &str) {
        std::fs::write(self.ctl.join(name), format!("{value}\n")).unwrap();
    }

    /// The exit code of each call in turn; the last one repeats.
    pub fn exit_codes(&self, codes: &[i32]) {
        let lines: Vec<String> = codes.iter().map(|c| c.to_string()).collect();
        self.set("exit", &lines.join("\n"));
    }

    /// Seconds each call sleeps after reading stdin; the last one repeats.
    pub fn sleeps(&self, seconds: &[&str]) {
        self.set("sleep", &seconds.join("\n"));
    }

    /// How many calls have started.
    pub fn call_count(&self) -> usize {
        std::fs::read_dir(self.ctl.join("calls")).unwrap().count()
    }

    /// Every call, in the order they started.
    pub fn calls(&self) -> Vec<Call> {
        let read = |dir: &Path, name: &str| std::fs::read_to_string(dir.join(name)).ok();
        let lines = |text: Option<String>| -> Vec<String> {
            text.unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        };
        (0..self.call_count())
            .map(|n| {
                let dir = self.ctl.join("calls").join(n.to_string());
                Call {
                    at: read(&dir, "at")
                        .and_then(|t| t.trim().parse().ok())
                        .unwrap_or(0.0),
                    argv: lines(read(&dir, "argv")),
                    env: lines(read(&dir, "env")),
                    lines: lines(read(&dir, "stdin")),
                    read: dir.join("read").exists(),
                    done: dir.join("done").exists(),
                    dir_exists: read(&dir, "dir_exists").map(|v| v.trim() == "yes"),
                    child_pid: read(&dir, "child_pid").and_then(|p| p.trim().parse().ok()),
                }
            })
            .collect()
    }
}
