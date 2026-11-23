//! What host resource pools cost a machine that doesn't use them, and what
//! they leave unchanged (PRD-host-resource-pools.md R28 and the compile and
//! compatibility criteria; PLAN step 6).
//!
//! - **Tick cost.** A tick on a state whose action names a pool the user
//!   config doesn't define is timed against a tick on the same state with no
//!   `pool` field: the median of 50 each, taken in one run with the two
//!   interleaved, may differ by at most 5 ms. The test prints both medians
//!   and the difference. It runs in the debug suite and, in release mode, in
//!   CI's `Pool Tick Cost (release)` job on the Linux runner.
//! - **The compiled hash of a template that uses none of the new fields.**
//!   `tests/fixtures/template-hash/no-pool.md` compiles to the hash the
//!   released koto v0.38.0 gave it. `compat_baseline_test` holds its
//!   compiled bytes with every other fixture; this pins the hash itself.
//!
//! That the previous release runs a pooled template and reads a log holding
//! the five new event types needs that release's binary, and lives in
//! `test/compat/pool-v0_38_0.sh`.

#![cfg(unix)]

#[path = "support/host_session.rs"]
mod host_session;
#[path = "support/koto_command.rs"]
mod koto_command;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Ticks timed for each of the two states.
const TICKS: usize = 50;

/// The most the undefined pool's median tick may exceed the plain one's.
const BOUND: Duration = Duration::from_millis(5);

/// A state whose action runs on every tick and whose gate never passes, so
/// each tick re-enters the same work: the action, then the gate. `POOL` is
/// replaced by the `pool` line or by nothing.
const TEMPLATE: &str = r#"---
name: tick-cost
version: "1.0"
initial_state: test
states:
  test:
    default_action:
      command: "true"
POOL    gates:
      ready:
        type: context-exists
        key: ready
    transitions:
      - target: done
        when:
          gates.ready.exists: true
  done:
    terminal: true
---

## test

Wait for ready.

## done

Done.
"#;

struct Host {
    tmp: tempfile::TempDir,
    root: PathBuf,
    ws: PathBuf,
}

impl Host {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("koto-root");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(ws.join("sessions")).unwrap();
        // A config that defines pools, just not this one, so the tick reads
        // and parses the `[pools]` table the way a configured machine's does.
        std::fs::write(
            root.join("config.toml"),
            "[pools.other]\nsize = 2\n\n[pools.another]\nsize = 1\n",
        )
        .unwrap();
        Host { tmp, root, ws }
    }

    fn koto(&self) -> std::process::Command {
        let mut cmd = koto_command::koto_std();
        cmd.env_remove("CLAUDE_CODE_SESSION_ID")
            .current_dir(&self.ws)
            .env("HOME", self.tmp.path())
            .env("KOTO_HOME", &self.root)
            .env("KOTO_SESSIONS_BASE", self.ws.join("sessions"));
        cmd
    }

    fn init(&self, name: &str, pool_line: &str) {
        let src = self.ws.join(format!("{name}.md"));
        std::fs::write(&src, TEMPLATE.replace("POOL", pool_line)).unwrap();
        let out = self
            .koto()
            .args(["init", name, "--template", src.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "init {name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// One `koto next`, timed. The state's gate keeps it in place, so every
    /// tick runs the action again.
    fn timed_tick(&self, name: &str) -> Duration {
        let mut cmd = self.koto();
        cmd.args(["next", name]);
        let started = Instant::now();
        let out = cmd.output().unwrap();
        let took = started.elapsed();
        assert!(
            out.status.success(),
            "next {name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let response: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(response["action"], "gate_blocked", "{response}");
        assert_eq!(response["state"], "test", "{response}");
        took
    }
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    let mid = samples.len() / 2;
    if samples.len().is_multiple_of(2) {
        (samples[mid - 1] + samples[mid]) / 2
    } else {
        samples[mid]
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[test]
fn an_undefined_pool_adds_at_most_five_ms_to_a_tick() {
    let host = Host::new();
    host.init("plain", "");
    host.init("pooled", "      pool: test-suite\n");

    // One untimed tick each, so the compile cache and the page cache are
    // warm for both before anything is measured.
    host.timed_tick("plain");
    host.timed_tick("pooled");

    // Interleaved, so drift on the machine (another job, a cache flush, the
    // logs growing a line per tick) lands on both sides alike.
    let mut plain = Vec::with_capacity(TICKS);
    let mut pooled = Vec::with_capacity(TICKS);
    for _ in 0..TICKS {
        plain.push(host.timed_tick("plain"));
        pooled.push(host.timed_tick("pooled"));
    }

    let plain = median(plain);
    let pooled = median(pooled);
    let difference = pooled.saturating_sub(plain);
    println!(
        "tick cost over {TICKS} ticks each: no pool field median {:.3} ms, \
         undefined pool median {:.3} ms, difference {:+.3} ms (bound {} ms)",
        ms(plain),
        ms(pooled),
        ms(pooled) - ms(plain),
        BOUND.as_millis()
    );
    // The bound is judged on a release build only, as retention_cost does: a
    // debug build, and the coverage-instrumented suite in CI, runs each tick
    // several times slower, and on a loaded host the two medians drift apart
    // by more than 5 ms with no change to koto. The release-mode
    // `pool-tick-cost` CI job is the measurement of record.
    if !cfg!(debug_assertions) {
        assert!(
            difference <= BOUND,
            "an undefined pool costs {:.3} ms a tick over no pool, more than {} ms",
            ms(difference),
            BOUND.as_millis()
        );
    }
    assert!(
        !host.root.join("pools").exists(),
        "an undefined pool creates nothing under the koto root"
    );
}

/// The hash koto v0.38.0, the release before pools shipped, gave
/// `tests/fixtures/template-hash/no-pool.md` (the name of the file
/// `koto template compile` wrote to its cache).
const NO_POOL_RELEASED_HASH: &str =
    "c5fa2de21fea83384ba35153edbae50549b1779f1a44de2e5a71712b1c71e0b8";

#[test]
fn a_template_using_none_of_the_new_fields_keeps_its_released_hash() {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/template-hash/no-pool.md");
    // The fixture's gate has a `poll.timeout_secs` of its own, so the
    // action fields are checked on the compiled template, not the text.
    let compiled = koto::template::compile::compile(&source, false).unwrap();
    for (name, state) in &compiled.states {
        if let Some(action) = &state.default_action {
            assert!(
                action.pool.is_none() && !action.background && action.timeout_secs.is_none(),
                "state {name}'s action uses a pool field"
            );
        }
    }
    let json = serde_json::to_string_pretty(&compiled).unwrap();
    assert_eq!(
        koto::cache::sha256_hex(json.as_bytes()),
        NO_POOL_RELEASED_HASH,
        "the template's compiled hash differs from the one koto v0.38.0 computed"
    );
}
