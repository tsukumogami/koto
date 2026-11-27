//! Waiters served in order, with a claim period and a lapse
//! (DESIGN-host-resource-pools.md, "Taking a slot: the queue and the claim";
//! PLAN step 3).
//!
//! Sessions waiting on a full pool take freed slots first come, first
//! served: an arrival queues behind existing waiters even at a free slot, a
//! freed slot is held for the head waiter for `claim_secs` and then passes
//! on, a waiter that stops ticking for `lapse_secs` loses its place, and a
//! session that leaves the pool state leaves the queue. These cases run real
//! `koto next` processes against one koto root.

#![cfg(unix)]

#[path = "support/koto_command.rs"]
mod koto_command;

use assert_cmd::Command;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// One koto root shared by several workspaces, one per session.
struct Host {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Host {
    fn new(config: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("koto-root");
        std::fs::create_dir_all(&root).unwrap();
        let host = Host { _tmp: tmp, root };
        host.write_config(config);
        host
    }

    fn base(&self) -> &Path {
        self._tmp.path()
    }

    fn write_config(&self, text: &str) {
        std::fs::write(self.root.join("config.toml"), text).unwrap();
    }

    /// The workspace of session `name`, `ws-<name>`.
    fn ws(&self, name: &str) -> PathBuf {
        let dir = self.base().join(format!("ws-{name}"));
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        dir
    }

    fn env(&self, cmd: &mut std::process::Command, name: &str) {
        cmd.env_remove("CLAUDE_CODE_SESSION_ID")
            .current_dir(self.ws(name))
            .env("HOME", self.base())
            .env("KOTO_HOME", &self.root)
            .env("KOTO_SESSIONS_BASE", self.ws(name).join("sessions"));
    }

    fn koto(&self, name: &str) -> Command {
        let mut std_cmd = koto_command::koto_std();
        self.env(&mut std_cmd, name);
        Command::from_std(std_cmd)
    }

    fn init(&self, name: &str, template: &str) {
        let src = self.ws(name).join(format!("{name}.md"));
        std::fs::write(&src, template).unwrap();
        let out = self
            .koto(name)
            .args(["init", name, "--template", src.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn run(&self, name: &str, args: &[&str]) -> serde_json::Value {
        let out = self.koto(name).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?} should exit 0: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("koto prints JSON")
    }

    /// Tick `name`, keeping its session when it finishes so its log can
    /// be read.
    fn next(&self, name: &str) -> serde_json::Value {
        self.run(name, &["next", name, "--no-cleanup"])
    }

    /// Start `koto next` without waiting for it, in its own process group.
    fn spawn_next(&self, name: &str) -> std::process::Child {
        let mut cmd = koto_command::koto_std();
        self.env(&mut cmd, name);
        cmd.args(["next", name, "--no-cleanup"])
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    }

    /// The session's log, header left out.
    fn events(&self, name: &str) -> Vec<serde_json::Value> {
        let file = format!("koto-{name}.state.jsonl");
        let path = find(&self.ws(name).join("sessions"), &file).expect("the session log");
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// The sessions in the pool's queue file, in order.
    fn queue(&self) -> Vec<String> {
        let path = self.root.join("pools/test-suite/queue.json");
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let q: serde_json::Value = serde_json::from_str(&text).unwrap();
        q["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["session"].as_str().unwrap().to_string())
            .collect()
    }

    fn flag(&self, name: &str) -> PathBuf {
        self.base().join(name)
    }
}

/// The first file named `name` below `dir`.
fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find(&path, name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}

fn of_type<'a>(events: &'a [serde_json::Value], ty: &str) -> Vec<&'a serde_json::Value> {
    events.iter().filter(|e| e["type"] == ty).collect()
}

/// The reasons of the log's `pool_released` events, in order.
fn released(events: &[serde_json::Value]) -> Vec<String> {
    of_type(events, "pool_released")
        .iter()
        .map(|e| e["payload"]["reason"].as_str().unwrap().to_string())
        .collect()
}

fn wait_for(path: &Path) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A template whose `test` state runs `command` on pool `test-suite`, then
/// goes to `done`.
fn template(command: &str) -> String {
    format!(
        r#"---
name: pool-queue
version: "1.0"
initial_state: test
states:
  test:
    default_action:
      command: {command:?}
      pool: test-suite
    transitions:
      - target: done
  done:
    terminal: true
---

## test

Run the suite.

## done

Done.
"#
    )
}

/// A command that writes its pid to `started`, then waits for `release`.
/// koto runs it in a process group of its own, led by that pid.
fn held(started: &Path, release: &Path) -> String {
    format!(
        "echo $$ > {}.tmp && mv {}.tmp {} && while [ ! -f {} ]; do sleep 0.05; done && echo held",
        started.display(),
        started.display(),
        started.display(),
        release.display()
    )
}

/// A session holding the pool's one slot until its release file appears.
struct Holder {
    child: std::process::Child,
    release: PathBuf,
    /// The command's pid, which leads its process group.
    command_pid: i32,
}

impl Holder {
    fn start(host: &Host, name: &str) -> Holder {
        let started = host.flag(&format!("started-{name}"));
        let release = host.flag(&format!("release-{name}"));
        host.init(name, &template(&held(&started, &release)));
        let child = host.spawn_next(name);
        wait_for(&started);
        let command_pid = std::fs::read_to_string(&started)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        Holder {
            child,
            release,
            command_pid,
        }
    }

    /// Let its command finish and wait for its tick to end.
    fn release(self) -> serde_json::Value {
        std::fs::write(&self.release, "").unwrap();
        let out = self.child.wait_with_output().unwrap();
        assert!(out.status.success());
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// `kill -9` the tick and its command's process group, and wait until
    /// they are gone.
    fn kill(mut self) {
        let tick = self.child.id() as i32;
        // SAFETY: kill sends a signal to process groups this test started.
        unsafe {
            assert_eq!(libc::kill(-self.command_pid, libc::SIGKILL), 0);
            assert_eq!(libc::kill(-tick, libc::SIGKILL), 0);
        }
        let _ = self.child.wait();
        // A killed `sleep` is reaped a moment after its group is signalled.
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// The `__pool__` condition's output of a queued response.
fn queued(resp: &serde_json::Value) -> &serde_json::Value {
    assert_eq!(resp["action"], "gate_blocked", "got {resp}");
    let conditions = resp["blocking_conditions"].as_array().unwrap();
    assert_eq!(conditions.len(), 1, "got {conditions:?}");
    assert_eq!(conditions[0]["name"], "__pool__");
    assert_eq!(conditions[0]["status"], "queued");
    &conditions[0]["output"]
}

fn assert_done(resp: &serde_json::Value) {
    assert_eq!(resp["state"], "done", "got {resp}");
}

const QUICK: &str = "echo ran";

// ---------------------------------------------------------------------------
// first come, first served
// ---------------------------------------------------------------------------

/// Tick `name` every `every` until its action runs, and return when.
fn tick_until_done(host: &Host, name: &str, every: Duration, since: Instant) -> Duration {
    loop {
        let resp = host.next(name);
        if resp["state"] == "done" {
            return since.elapsed();
        }
        queued(&resp);
        std::thread::sleep(every);
    }
}

#[test]
fn waiters_take_freed_slots_in_arrival_order_whatever_their_tick_rate() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let h = Holder::start(&host, "h");
    for (i, w) in ["w1", "w2", "w3"].iter().enumerate() {
        host.init(w, &template(QUICK));
        assert_eq!(queued(&host.next(w))["position"], i + 1);
    }

    let start = Instant::now();
    std::thread::scope(|s| {
        let host = &host;
        let w1 = s.spawn(move || tick_until_done(host, "w1", Duration::from_secs(10), start));
        let w2 = s.spawn(move || tick_until_done(host, "w2", Duration::from_secs(10), start));
        let w3 = s.spawn(move || tick_until_done(host, "w3", Duration::from_secs(1), start));
        std::thread::sleep(Duration::from_millis(500));
        h.release();
        let (t1, t2, t3) = (w1.join().unwrap(), w2.join().unwrap(), w3.join().unwrap());
        println!(
            "acquired: w1 {:.1}s, w2 {:.1}s, w3 {:.1}s",
            t1.as_secs_f64(),
            t2.as_secs_f64(),
            t3.as_secs_f64()
        );
        assert!(t1 < t2 && t2 < t3, "w1 {t1:?}, w2 {t2:?}, w3 {t3:?}");
    });
    for w in ["w1", "w2", "w3"] {
        let events = host.events(w);
        // Joined once, not on every queued tick.
        assert_eq!(of_type(&events, "pool_queued").len(), 1, "{w}");
        assert_eq!(of_type(&events, "pool_acquired").len(), 1, "{w}");
        assert_eq!(released(&events), ["done"], "{w}");
    }
}

#[test]
fn an_arrival_at_a_free_slot_queues_behind_a_waiter() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let h = Holder::start(&host, "h");
    host.init("w1", &template(QUICK));
    assert_eq!(queued(&host.next("w1"))["position"], 1);
    assert_done(&h.release());

    let ran = host.flag("ran-c");
    host.init("c", &template(&format!("touch {}", ran.display())));
    let out = queued(&host.next("c")).clone();
    assert_eq!(out["position"], 2, "got {out}");
    assert_eq!(out["waiting"], 2, "got {out}");
    assert!(!ran.exists(), "the arrival ran ahead of the waiter");

    assert_done(&host.next("w1"));
    assert_done(&host.next("c"));
}

// ---------------------------------------------------------------------------
// the claim
// ---------------------------------------------------------------------------

/// W1 and W2 wait on a slot `h` holds; `free` frees it. W1 doesn't tick for
/// six seconds, so W2's next tick takes the slot, and W1 takes the one
/// after.
fn an_unused_claim_passes_on(free: impl FnOnce(Holder)) {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\nclaim_secs = 5\n");
    let h = Holder::start(&host, "h");
    host.init("w1", &template(QUICK));
    let w2_started = host.flag("started-w2");
    let w2_release = host.flag("release-w2");
    host.init("w2", &template(&held(&w2_started, &w2_release)));
    assert_eq!(queued(&host.next("w1"))["position"], 1);
    assert_eq!(queued(&host.next("w2"))["position"], 2);

    free(h);
    // The first edit that sees the slot free reserves it for W1.
    assert_eq!(queued(&host.next("w2"))["position"], 2);
    std::thread::sleep(Duration::from_secs(6));

    let w2 = host.spawn_next("w2");
    wait_for(&w2_started);
    // W1 keeps first place, behind nobody.
    assert_eq!(queued(&host.next("w1"))["position"], 1);
    std::fs::write(&w2_release, "").unwrap();
    let out = w2.wait_with_output().unwrap();
    assert_done(&serde_json::from_slice(&out.stdout).unwrap());
    assert_done(&host.next("w1"));
}

#[test]
fn an_unused_claim_passes_to_the_next_waiter_after_a_clean_exit() {
    an_unused_claim_passes_on(|h| assert_done(&h.release()));
}

#[test]
fn an_unused_claim_passes_to_the_next_waiter_after_a_kill() {
    an_unused_claim_passes_on(Holder::kill);
}

// ---------------------------------------------------------------------------
// the lapse
// ---------------------------------------------------------------------------

#[test]
fn a_waiter_that_stops_ticking_loses_its_place_and_queues_at_the_back() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\nlapse_secs = 5\n");
    let h = Holder::start(&host, "h");
    for (i, w) in ["w1", "w2", "w3"].iter().enumerate() {
        host.init(w, &template(QUICK));
        assert_eq!(queued(&host.next(w))["position"], i + 1);
    }
    // W2 and W3 keep ticking; W1 doesn't for over six seconds.
    let idle = Instant::now();
    while idle.elapsed() < Duration::from_millis(6500) {
        host.next("w2");
        host.next("w3");
        std::thread::sleep(Duration::from_secs(1));
    }
    assert_eq!(queued(&host.next("w2"))["position"], 1);
    assert_eq!(queued(&host.next("w3"))["position"], 2);
    assert_eq!(host.queue(), ["w2", "w3"]);

    let back = queued(&host.next("w1")).clone();
    assert_eq!(back["position"], 3, "got {back}");
    assert_eq!(back["waiting"], 3, "got {back}");
    let events = host.events("w1");
    assert_eq!(released(&events), ["lapsed"]);
    // Queued, lapsed, queued again, in that order.
    let order: Vec<&str> = events
        .iter()
        .filter_map(|e| e["type"].as_str())
        .filter(|t| t.starts_with("pool_"))
        .collect();
    assert_eq!(order, ["pool_queued", "pool_released", "pool_queued"]);
    h.release();
}

// ---------------------------------------------------------------------------
// what a queued tick reports, and the in-tick wait
// ---------------------------------------------------------------------------

#[test]
fn a_queued_tick_reports_its_retry_and_wait_and_an_in_tick_wait_takes_the_slot() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\nclaim_secs = 7\n");
    let h = Holder::start(&host, "h");
    host.init("w", &template(QUICK));
    let first = queued(&host.next("w")).clone();
    assert_eq!(first["retry_after_secs"], 7, "min(15, claim_secs)");
    std::thread::sleep(Duration::from_secs(3));
    let second = queued(&host.next("w")).clone();
    let (a, b) = (
        first["waited_secs"].as_u64().unwrap(),
        second["waited_secs"].as_u64().unwrap(),
    );
    assert!(b >= a + 2, "waited_secs {a} then {b}");

    // The default claim period caps the retry at 15.
    host.write_config("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    assert_eq!(queued(&host.next("w"))["retry_after_secs"], 15);

    // With wait_secs = 5, a holder finishing 2 seconds into the tick hands
    // the slot over inside it.
    host.write_config("[pools.test-suite]\nsize = 1\nwait_secs = 5\n");
    let release = h.release.clone();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        std::fs::write(release, "").unwrap();
    });
    let started = Instant::now();
    let resp = host.next("w");
    let took = started.elapsed();
    assert_done(&resp);
    assert!(took >= Duration::from_millis(1500), "took {took:?}");
    assert!(took < Duration::from_secs(5), "took {took:?}");
    releaser.join().unwrap();
    let _ = h.child.wait_with_output();
}

// ---------------------------------------------------------------------------
// the race for the last slot
// ---------------------------------------------------------------------------

#[test]
fn twenty_sessions_racing_for_the_last_slot_produce_one_holder() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let release = host.flag("release");
    let names: Vec<String> = (0..20).map(|i| format!("r{i}")).collect();
    for name in &names {
        let started = host.flag(&format!("started-{name}"));
        host.init(name, &template(&held(&started, &release)));
    }
    let mut children: Vec<(String, std::process::Child)> = names
        .iter()
        .map(|n| (n.clone(), host.spawn_next(n)))
        .collect();

    // Every session but the holder returns; the holder's tick waits on its
    // command.
    let started = Instant::now();
    let mut finished = Vec::new();
    while finished.len() < 19 {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "only {} of 20 ticks returned",
            finished.len()
        );
        let mut i = 0;
        while i < children.len() {
            if children[i].1.try_wait().unwrap().is_some() {
                finished.push(children.remove(i));
            } else {
                i += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(children.len(), 1);
    let holder = children.pop().unwrap();
    wait_for(&host.flag(&format!("started-{}", holder.0)));
    let ran: Vec<&String> = names
        .iter()
        .filter(|n| host.flag(&format!("started-{n}")).exists())
        .collect();
    assert_eq!(ran, [&holder.0], "more than one session ran");
    for (name, child) in finished {
        let out = child.wait_with_output().unwrap();
        let resp: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        queued(&resp);
        assert!(of_type(&host.events(&name), "pool_acquired").is_empty());
    }
    std::fs::write(&release, "").unwrap();
    let out = holder.1.wait_with_output().unwrap();
    assert_done(&serde_json::from_slice(&out.stdout).unwrap());
}

// ---------------------------------------------------------------------------
// resize and removal
// ---------------------------------------------------------------------------

#[test]
fn a_shrunk_pool_keeps_its_holders_and_admits_after_both_release() {
    let host = Host::new("[pools.test-suite]\nsize = 2\nwait_secs = 0\n");
    let a = Holder::start(&host, "a");
    let b = Holder::start(&host, "b");
    host.write_config("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    host.init("c", &template(QUICK));
    let out = queued(&host.next("c")).clone();
    assert_eq!(out["holders"].as_array().unwrap().len(), 2, "got {out}");

    assert_done(&a.release());
    queued(&host.next("c"));
    assert_done(&b.release());
    assert_done(&host.next("c"));
}

#[test]
fn removing_the_pool_lets_a_waiter_run_unthrottled() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let h = Holder::start(&host, "h");
    host.init("w", &template(QUICK));
    queued(&host.next("w"));

    host.write_config("");
    // The holder still holds; the waiter runs anyway.
    assert_done(&host.next("w"));
    assert_eq!(released(&host.events("w")), ["pool_removed"]);
    assert!(host.queue().is_empty());
    assert_done(&h.release());
}

// ---------------------------------------------------------------------------
// leaving the pool state
// ---------------------------------------------------------------------------

/// `start` leads to the pooled `test`, which completes on `finish`.
fn leaving_template() -> String {
    r#"---
name: pool-leave
version: "1.0"
initial_state: start
states:
  start:
    transitions:
      - target: test
  test:
    accepts:
      verdict:
        type: enum
        required: true
        values: [finish]
    default_action:
      command: "echo ran"
      pool: test-suite
    transitions:
      - target: done
        when:
          verdict: finish
  done:
    terminal: true
---

## start

Start.

## test

Run the suite.

## done

Done.
"#
    .to_string()
}

#[test]
fn a_waiter_rewound_out_of_the_pool_state_leaves_the_queue_at_once() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let h = Holder::start(&host, "h");
    host.init("w", &leaving_template());
    let resp = host.next("w");
    assert_eq!(resp["state"], "test");
    queued(&resp);
    assert_eq!(host.queue(), ["w"]);

    let rewound = host.run("w", &["rewind", "w"]);
    assert_eq!(rewound["state"], "start");
    assert!(host.queue().is_empty(), "still queued: {:?}", host.queue());
    let events = host.events("w");
    assert_eq!(released(&events), ["left_state"]);
    assert_eq!(
        of_type(&events, "pool_released")[0]["payload"]["state"],
        "test"
    );
    h.release();
}

#[test]
fn a_waiter_that_completes_leaves_the_queue_at_once() {
    let host = Host::new("[pools.test-suite]\nsize = 1\nwait_secs = 0\n");
    let h = Holder::start(&host, "h");
    host.init("w", &leaving_template());
    queued(&host.next("w"));
    assert_eq!(host.queue(), ["w"]);

    let resp = host.run(
        "w",
        &[
            "next",
            "w",
            "--no-cleanup",
            "--with-data",
            r#"{"verdict":"finish"}"#,
        ],
    );
    assert_done(&resp);
    assert!(host.queue().is_empty(), "still queued: {:?}", host.queue());
    assert_eq!(released(&host.events("w")), ["left_state"]);
    h.release();
}
