//! Integration test for the batch-scoped parent flock.
//!
//! Issue #2 introduces `SessionBackend::lock_state_file` plus the
//! `BatchError::ConcurrentTick` envelope and wires `handle_next` to
//! use them only when the current state is batch-scoped. The
//! batch-scoped detection helper (`state_is_batch_scoped` in
//! `src/cli/mod.rs`) currently returns `false` in every case because
//! the template fields and log events it will inspect do not exist
//! yet (Issues #7, #16, #17 introduce them).
//!
//! That leaves no realistic end-to-end fixture for the "two concurrent
//! `koto next` on the same batch-scoped parent" scenario at this
//! revision. To still exercise the contract Issue #2 is responsible
//! for -- lock acquisition, translation to `BatchError::ConcurrentTick`,
//! and envelope shape -- this test drives `lock_state_file` directly
//! from two threads and translates the resulting `SessionError::Locked`
//! into a `BatchError::ConcurrentTick` exactly the way `handle_next`
//! does.
//!
//! When real batch plumbing lands, this file can be replaced (or
//! augmented) with a cross-process `koto next` test against a template
//! that carries a `materialize_children` hook. The helper call site in
//! `handle_next` will not change -- only the helper's return value
//! will -- so the integration-level contract encoded here stays valid.

use assert_fs::TempDir;
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::thread;

use koto::cli::batch_error::BatchError;
use koto::engine::types::{Event, EventPayload, StateFileHeader};
use koto::session::local::LocalBackend;
use koto::session::{SessionBackend, SessionError};

/// Build a `LocalBackend` rooted at `dir` and pre-initialise a state
/// file for `id` so `lock_state_file` has something to acquire.
fn init_backend(dir: &std::path::Path, id: &str) -> LocalBackend {
    let backend = LocalBackend::with_base_dir(dir.to_path_buf());
    backend.create(id).expect("create session dir");

    let header = StateFileHeader {
        schema_version: 1,
        workflow: id.to_string(),
        template_hash: "0".repeat(64),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        parent_workflow: None,
        template_source_dir: None,
    };
    let events = vec![Event {
        seq: 1,
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        event_type: "workflow_initialized".to_string(),
        payload: EventPayload::WorkflowInitialized {
            template_path: "/tmp/unused-template.json".to_string(),
            variables: std::collections::HashMap::new(),
            spawn_entry: None,
        },
    }];
    backend
        .init_state_file(id, header, events)
        .expect("init state file");

    backend
}

/// Two concurrent attempts to take the batch-scoped parent lock: the
/// first wins, the second is translated into the
/// `BatchError::ConcurrentTick` envelope that `handle_next` writes.
///
/// Cross-thread (not cross-process) because the unit-level test
/// `session::local::tests::lock_state_file_cross_process_contention`
/// already exercises the kernel's cross-PID release semantics; what
/// this test is responsible for is the CLI-level *translation* from
/// `SessionError::Locked` to `BatchError::ConcurrentTick` and the
/// envelope shape that emerges on stdout.
#[cfg(unix)]
#[test]
fn two_concurrent_acquisitions_produce_batch_concurrent_tick() {
    let tmp = TempDir::new().unwrap();
    let backend = Arc::new(init_backend(tmp.path(), "wf"));

    // Thread A grabs the lock, signals the barrier, then waits on the
    // condvar so the main thread can run the contention attempt with
    // the lock still held. Using a condvar instead of a sleep avoids
    // flaky timing on loaded CI runners.
    let barrier = Arc::new(Barrier::new(2));
    let release = Arc::new(Mutex::new(false));
    let cvar = Arc::new(Condvar::new());

    let a_backend = Arc::clone(&backend);
    let a_bar = Arc::clone(&barrier);
    let a_release = Arc::clone(&release);
    let a_cvar = Arc::clone(&cvar);
    let a = thread::spawn(move || {
        let _guard = a_backend.lock_state_file("wf").expect("thread A acquire");
        a_bar.wait();
        let mut done = a_release.lock().unwrap();
        while !*done {
            done = a_cvar.wait(done).unwrap();
        }
    });

    // Wait until thread A has the lock.
    barrier.wait();

    // Thread B's acquire must fail with SessionError::Locked. Mirror
    // the translation `handle_next` performs on that variant.
    let err = backend
        .lock_state_file("wf")
        .expect_err("second acquire must contend");
    let batch_err = match err {
        SessionError::Locked { holder_pid } => BatchError::ConcurrentTick { holder_pid },
        other => panic!("expected SessionError::Locked, got {:?}", other),
    };

    // The envelope written to stdout by handle_next must match the
    // contract documented in batch_error.rs.
    let envelope = batch_err.to_envelope();
    assert_eq!(envelope["action"], "error");
    assert_eq!(envelope["batch"]["kind"], "concurrent_tick");
    assert!(
        envelope["batch"]["holder_pid"].is_null(),
        "holder_pid is best-effort and currently always null; got {:?}",
        envelope["batch"]["holder_pid"]
    );
    assert_eq!(batch_err.exit_code(), 1, "transient error, exit 1");

    // Let thread A drop its guard and join.
    *release.lock().unwrap() = true;
    cvar.notify_all();
    a.join().unwrap();
}

/// Once the holder drops its guard, a subsequent acquire succeeds.
/// Not strictly an Issue #2 acceptance criterion, but captures the
/// other half of the contract: the lock is not sticky, so a failed
/// tick can retry after the holder finishes.
#[cfg(unix)]
#[test]
fn lock_becomes_available_after_holder_drops() {
    let tmp = TempDir::new().unwrap();
    let backend = init_backend(tmp.path(), "wf");

    {
        let _first = backend.lock_state_file("wf").expect("first acquire");
        // Hold briefly then drop at end of scope.
    }

    // After the first guard drops, a new acquire must eventually
    // succeed. The kernel's per-OFD release is asynchronous in some
    // scheduling windows, so mirror the retry loop the unit tests
    // use.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match backend.lock_state_file("wf") {
            Ok(_guard) => break,
            Err(SessionError::Locked { .. }) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => panic!("second acquire must succeed after first drop: {:?}", e),
        }
    }
}
