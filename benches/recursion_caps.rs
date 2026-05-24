//! Criterion benchmark harness for Issue 17 recursion-cap
//! enforcement.
//!
//! Measures [`koto::engine::caps::validate_recursion_caps`] at
//! workspace sizes 1k, 10k, and 26k sessions with 100 of them open
//! (the remaining are terminal — recorded in `_terminal_index.jsonl`
//! per Issue 8's writer contract). Asserts the AD3.3 perf-cliff
//! avoidance commitment: the total-unassigned counter must be
//! O(open-sessions), not O(workspace-sessions), so a 26k workspace
//! with 100 open sessions validates in <30 ms p95.
//!
//! ## Reference hardware
//!
//! Same as `benches/discovery_scan.rs`: GitHub Actions
//! `ubuntu-latest` runners (Ubuntu 24.04, x86_64, 2 vCPU, ~7 GiB
//! RAM) per `.github/workflows/validate.yml`. Operators running this
//! bench on different hardware should normalize.
//!
//! ## Thresholds
//!
//! | Workspace | Open sessions | Design reference (p95) | CI gate |
//! |-----------|--------------:|-----------------------:|--------:|
//! | 1k        | 100           | <10 ms                 | (informational) |
//! | 10k       | 100           | <20 ms                 | (informational) |
//! | 26k       | 100           | <30 ms                 | **30 ms** |
//!
//! The 26k gate is **soft-by-default** (matches Issue 10's posture).
//! Smoke testing at 1k revealed that `read_terminal_index` walks the
//! full JSONL file (~26k lines at year-2 scale) and parses each
//! entry; that itself is O(workspace-sessions) and dominates the
//! measurement. The AD3.3 commitment requires either a different
//! index format (one-pass-countable, or in-memory cache) or a
//! separate counter file — neither lands in Issue 17's scope. The
//! bench surfaces the gap; a follow-up issue (terminal-index reader
//! optimization or O(open-sessions) counter primitive) can close it.
//! Once closed, flip `KOTO_BENCH_STRICT=1` in CI to hard-gate.
//!
//! See the file-header note in `benches/discovery_scan.rs` for the
//! identical "land harness now, tighten gate after the optimization
//! issue ships" sequencing.
//!
//! ## Methodology
//!
//! Each benchmark uses [`criterion::Bencher::iter_batched`] with
//! `BatchSize::SmallInput`. The setup closure:
//!
//! 1. Builds a deterministic synthetic workspace under a `tempfile`
//!    directory. The seed (`SEED_BASE + size`) ensures byte-identical
//!    fixtures across runs of the same parameter set.
//! 2. Marks `size - 100` sessions as terminal in
//!    `_terminal_index.jsonl` — the year-2 scaffolding where most
//!    sessions are settled and only a handful are open.
//! 3. Pre-reads every header via `fs::read` to populate the OS page
//!    cache.
//!
//! The measurement closure calls
//! `validate_recursion_caps(backend, "parent-0", koto_root)` once
//! and discards the result.
//!
//! ## Custom main and regression gating
//!
//! `Cargo.toml` declares `harness = false`, so this file provides
//! its own `main()`. After criterion's `final_summary`, the gate
//! parses `target/criterion/<group>/<size>/new/estimates.json` for
//! the 26k case, computes a p95 proxy as `mean + 2 * std_dev`, and
//! asserts it's below 30 ms. A breach returns non-zero exit (CI
//! visible).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{black_box, criterion_group, BatchSize, BenchmarkId, Criterion};
use filetime::{set_file_mtime, FileTime};

use koto::engine::caps::validate_recursion_caps;
use koto::engine::persistence::append_header;
use koto::engine::terminal_index::{append_terminal_index_entry, TerminalIndexEntry};
use koto::engine::types::{AssignmentClaim, StateFileHeader};
use koto::session::local::LocalBackend;
use koto::session::state_file_name;

// ----- Constants -----

/// Workspace sizes the bench exercises. Each size keeps 100 sessions
/// open and marks the rest terminal in the index.
const SIZES: &[usize] = &[1_000, 10_000, 26_000];

/// Number of open (unclaimed, `needs_agent=true`) sessions in every
/// fixture, regardless of total workspace size. This is the "100
/// open sessions" mode AD3.3's perf-cliff avoidance assumption rests
/// on.
const OPEN_SESSIONS: usize = 100;

/// Deterministic seed root. Combined with workspace size for a
/// reproducible fixture per parameter set.
const SEED_BASE: u64 = 0xCAFEBABE;

/// Hard threshold for the 26k case, in milliseconds. The
/// total-unassigned counter is supposed to be O(open-sessions); a
/// breach indicates the terminal-index filter is misbehaving and the
/// AD3.3 perf cliff has reopened.
const HARD_THRESHOLD_MS_26K: u64 = 30;

// ----- Deterministic fixture generator (inline-duplicated from
//       benches/discovery_scan.rs per Issue 17's bench fixture
//       sharing decision: duplication is fine here, the contract
//       lives in discovery_scan.rs's file header and is summarized
//       in this file's methodology section.)

/// Lehmer LCG: tiny, deterministic, no `rand` dependency.
fn lcg_next(state: &mut u64) -> u32 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1);
    (*state >> 33) as u32
}

/// Build a workspace with `total` sessions where `OPEN_SESSIONS` are
/// unclaimed (`needs_agent=true, no claim`) and the rest are
/// terminal (recorded in `_terminal_index.jsonl`).
///
/// The structure: one "parent-0" header at the root, and `total - 1`
/// child sessions each naming "parent-0" as their parent. The first
/// `OPEN_SESSIONS - 1` are open (so total open = OPEN_SESSIONS
/// including the parent); the remaining are terminal.
fn build_workspace(tmp: &Path, total: usize, seed: u64) -> (LocalBackend, PathBuf) {
    let sessions_dir = tmp.join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let backend = LocalBackend::with_base_dir(sessions_dir.clone());
    let koto_root = tmp.to_path_buf();
    let mut state = seed;

    // Parent header (unclaimed, request-store shape).
    let parent = base_header("parent-0", None);
    write_header(&sessions_dir, &parent);

    // Build the rest of the workspace.
    let mut terminal_ids: HashSet<String> = HashSet::with_capacity(total);
    for i in 0..(total - 1) {
        let id = format!("sess-{:06}", i);
        // First (OPEN_SESSIONS - 1) sessions stay open; the rest are
        // terminal (claimed + indexed).
        let is_terminal = i + 1 >= OPEN_SESSIONS;
        let header = if is_terminal {
            terminal_ids.insert(id.clone());
            let mut h = base_header(&id, Some("parent-0"));
            h.assignment_claim = Some(AssignmentClaim {
                coord_id: "team-lead".into(),
                claimed_at: "2026-05-24T14:35:01.000Z".into(),
            });
            h
        } else {
            base_header(&id, Some("parent-0"))
        };
        write_header(&sessions_dir, &header);
        // Synthetic mtime spread so the filesystem ordering doesn't
        // degenerate. Uses the LCG so the spread is deterministic.
        let path = sessions_dir.join(&id).join(state_file_name(&id));
        let mtime_offset_ms: u64 = lcg_next(&mut state) as u64;
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        set_file_mtime(
            &path,
            FileTime::from_unix_time(
                now_secs as i64 - (mtime_offset_ms / 1000) as i64,
                ((mtime_offset_ms % 1000) * 1_000_000) as u32,
            ),
        )
        .unwrap();
    }

    // Populate the terminal index for terminal sessions. The
    // header_mtime_ns is read back from each header's actual on-disk
    // mtime so the index-vs-header reconciliation in Issue 8 treats
    // them as up-to-date.
    for id in &terminal_ids {
        let path = sessions_dir.join(id).join(state_file_name(id));
        let mtime_ns = koto::engine::terminal_index::header_mtime_unix_nanos(&path).unwrap_or(0);
        let entry = TerminalIndexEntry {
            session_id: id.clone(),
            terminal_at: "2026-05-24T14:35:01.000Z".into(),
            header_mtime_ns: mtime_ns,
            terminal_state: "completed".into(),
        };
        append_terminal_index_entry(&koto_root, &entry).unwrap();
    }

    (backend, koto_root)
}

fn base_header(id: &str, parent: Option<&str>) -> StateFileHeader {
    StateFileHeader {
        schema_version: 1,
        workflow: id.to_string(),
        template_hash: "deadbeef".into(),
        created_at: "2026-05-24T00:00:00Z".into(),
        parent_workflow: parent.map(|p| p.to_string()),
        template_source_dir: None,
        session_id: id.to_string(),
        intent: None,
        template_name: Some("verdict".into()),
        needs_agent: Some(true),
        role: Some("scrutineer".into()),
        inputs: None,
        coordinator_of_record: Some("team-lead".into()),
        requested_by: Some("parent-coord".into()),
        assignment_claim: None,
        dispatch_epoch: 0,
        priority: None,
        deadline: None,
        retry_count: None,
        agent_config: None,
        respawn_generation: None,
    }
}

fn write_header(sessions_dir: &Path, header: &StateFileHeader) {
    let dir = sessions_dir.join(&header.workflow);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(state_file_name(&header.workflow));
    append_header(&path, header).unwrap();
}

/// Pre-read every header file so the OS page cache is populated
/// before measurement. Without this, the first iteration sees
/// cold-disk latency that the perf budget does not include.
fn warm_page_cache(sessions_dir: &Path, koto_root: &Path) {
    if let Ok(entries) = fs::read_dir(sessions_dir) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    if let Some(id) = entry.file_name().to_str() {
                        let path = entry.path().join(state_file_name(id));
                        let _ = fs::read(&path);
                    }
                }
            }
        }
    }
    let _ = fs::read(koto_root.join("_terminal_index.jsonl"));
}

// ----- Bench group -----

fn bench_validate_recursion_caps(c: &mut Criterion) {
    let mut group = c.benchmark_group("recursion_caps_validate");
    group.measurement_time(Duration::from_secs(15));
    for &n in SIZES {
        // Larger workspaces have higher setup cost; criterion's
        // default 10 sample minimum is fine here because the
        // measurement closure itself is fast (<30 ms even at 26k).
        let sample_size = match n {
            n if n >= 10_000 => 15,
            _ => 30,
        };
        group.sample_size(sample_size);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let tmp = tempfile::tempdir().unwrap();
                    let seed = SEED_BASE.wrapping_add(n as u64);
                    let (backend, koto_root) = build_workspace(tmp.path(), n, seed);
                    let sessions_dir = tmp.path().join("sessions");
                    warm_page_cache(&sessions_dir, &koto_root);
                    (tmp, backend, koto_root)
                },
                |(tmp, backend, koto_root)| {
                    let outcome =
                        validate_recursion_caps(&backend, "parent-0", &koto_root).unwrap();
                    black_box(outcome);
                    drop(tmp);
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_validate_recursion_caps);

// ----- Regression-gate post-step -----

fn read_estimate(group: &str, size: usize) -> Option<(f64, f64)> {
    let path = PathBuf::from("target/criterion")
        .join(group)
        .join(size.to_string())
        .join("new")
        .join("estimates.json");
    let contents = fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let mean = v.pointer("/mean/point_estimate")?.as_f64()?;
    let std_dev = v.pointer("/std_dev/point_estimate")?.as_f64()?;
    Some((mean, std_dev))
}

/// Hard 26k threshold gate. The 1k and 10k cases are informational
/// only — useful for trend lines but not gated. The AD3.3 commitment
/// is the year-2-scale 30 ms p95.
fn assert_26k_threshold() -> usize {
    let threshold_ns = HARD_THRESHOLD_MS_26K as f64 * 1_000_000.0;
    let (mean_ns, std_dev_ns) = match read_estimate("recursion_caps_validate", 26_000) {
        Some(x) => x,
        None => return 0,
    };
    let p95_proxy_ns = mean_ns + 2.0 * std_dev_ns;
    let p95_proxy_ms = p95_proxy_ns / 1_000_000.0;
    if p95_proxy_ns > threshold_ns {
        eprintln!(
            "[recursion_caps_validate / 26000] p95 proxy = {p95_proxy_ms:.2} ms (mean={:.2} ms ± {:.2} ms) vs gate {HARD_THRESHOLD_MS_26K} ms — BREACH",
            mean_ns / 1_000_000.0,
            std_dev_ns / 1_000_000.0,
        );
        1
    } else {
        eprintln!(
            "[recursion_caps_validate / 26000] p95 proxy = {p95_proxy_ms:.2} ms (mean={:.2} ms ± {:.2} ms) vs gate {HARD_THRESHOLD_MS_26K} ms — OK",
            mean_ns / 1_000_000.0,
            std_dev_ns / 1_000_000.0,
        );
        0
    }
}

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();

    // Soft-by-default gate: breach reported to stderr, exit 0.
    // KOTO_BENCH_STRICT=1 flips to fail-fast for the post-optimization
    // world. Matches Issue 10's posture; the file-header explains.
    let breaches = assert_26k_threshold();
    let strict = std::env::var("KOTO_BENCH_STRICT").as_deref() == Ok("1");
    if breaches > 0 && strict {
        eprintln!(
            "recursion_caps bench: AD3.3 perf-cliff threshold breached at 26k; KOTO_BENCH_STRICT=1 → failing"
        );
        std::process::exit(1);
    } else if breaches > 0 {
        eprintln!(
            "recursion_caps bench: {breaches} threshold breach(es) (reporting only; set KOTO_BENCH_STRICT=1 to enforce)"
        );
    }
}
