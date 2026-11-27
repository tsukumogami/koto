//! The process's queue of session records, and the flush that sends it.
//!
//! `emit` puts a stamped copy of each record here, with the session
//! directory it belongs to. [`flush`] sends what is queued to the event
//! sink and is the only entry point the flush sites call. Nothing here
//! fails, retries or blocks a command past a fixed bound.
//!
//! ## One flush
//!
//! A flush takes the queue's items and releases the queue before anything
//! slow, so it never holds up another thread's enqueue. With nothing
//! queued it returns at once and calls nothing. Otherwise it sends chunks
//! of up to [`CHUNK`] records, each in its own call, all under one call
//! deadline [`CALL_BUDGET`] from the flush's start. A chunk starts only
//! when at least [`MIN_CALL`], the sink's own per-call bound, is left
//! before that deadline. The time between the call deadline and 3.0 s pays
//! for ending a killed call and for writing the drop counts.
//!
//! What a call's outcome does:
//!
//! - posted, or posted with some records invalid: on to the next chunk;
//! - a failure the contract gives no meaning to: that chunk is counted as
//!   dropped, and the flush goes on;
//! - sending is off at the sink: the process stops calling and counts
//!   nothing, this flush included;
//! - the sink dropped the chunk, couldn't be started, or had to be killed:
//!   the process stops calling, and that chunk and every chunk after it are
//!   counted as dropped.
//!
//! A chunk that can't start because too little time is left is counted as
//! dropped with no call, no kill and no change to the breaker: a sink that
//! is slow but healthy never stops a long-lived process from sending on
//! its next flush.
//!
//! ## The breaker
//!
//! One failure stops later calls for the rest of the process, so a broken
//! sink costs one bounded flush, not one per flush point. After a stop that
//! counts, records are still queued so each later flush can count them.
//! After a stop that doesn't, nothing more is queued.
//!
//! Drop counts go to each record's session directory (see `carry`).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::carry;
use super::sink::{self, Outcome};

/// The most records one call carries.
pub(crate) const CHUNK: usize = 200;

/// How long after a flush starts its last call must have ended.
pub(crate) const CALL_BUDGET: Duration = Duration::from_millis(2850);

/// The least time a call is started with: the sink's own per-call bound.
pub(crate) const MIN_CALL: Duration = Duration::from_millis(2000);

#[cfg(not(test))]
fn call_budget() -> Duration {
    CALL_BUDGET
}

// The budget is the real one in every unit test unless the test widens it.
// A later chunk starts only when the calls before it were quick, so on a
// starved test host a test about something other than timing would
// otherwise fail for a reason it isn't testing.
#[cfg(test)]
thread_local! {
    static TEST_CALL_BUDGET: std::cell::Cell<Duration> = const { std::cell::Cell::new(CALL_BUDGET) };
}

#[cfg(test)]
fn call_budget() -> Duration {
    TEST_CALL_BUDGET.with(|b| b.get())
}

/// One queued record: its line as it will be sent, and the session it
/// belongs to, for the count if it is dropped.
struct Queued {
    line: String,
    session_dir: Arc<Path>,
}

/// What is waiting for the next flush.
#[derive(Default)]
struct Queue {
    items: Vec<Queued>,
    /// Records that were too long to send, by session: dropped before they
    /// were ever queued, counted by the next flush so that enqueueing does
    /// no file work.
    oversize: Vec<(Arc<Path>, u64)>,
}

/// Whether the process still calls the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Breaker {
    /// Calls are made.
    Open,
    /// No more calls; every later record is counted as dropped.
    StoppedCounting,
    /// No more calls, and nothing is counted: sending is off at the sink.
    StoppedSilent,
}

#[cfg(not(test))]
static QUEUE: std::sync::Mutex<Queue> = std::sync::Mutex::new(Queue {
    items: Vec::new(),
    oversize: Vec::new(),
});

#[cfg(not(test))]
static BREAKER: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

// Each unit test has a queue and a breaker of its own: the harness runs
// tests on threads of one process, and a shared queue would hand one test
// another's records.
#[cfg(test)]
thread_local! {
    static QUEUE: std::cell::RefCell<Queue> = std::cell::RefCell::new(Queue::default());
    static BREAKER: std::cell::Cell<Breaker> = const { std::cell::Cell::new(Breaker::Open) };
}

#[cfg(not(test))]
fn with_queue<T>(f: impl FnOnce(&mut Queue) -> T) -> T {
    // A poisoned lock still holds a usable queue: it is only pushed to or
    // taken whole.
    f(&mut QUEUE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

#[cfg(test)]
fn with_queue<T>(f: impl FnOnce(&mut Queue) -> T) -> T {
    QUEUE.with(|q| f(&mut q.borrow_mut()))
}

#[cfg(not(test))]
pub(crate) fn breaker() -> Breaker {
    match BREAKER.load(std::sync::atomic::Ordering::Relaxed) {
        0 => Breaker::Open,
        1 => Breaker::StoppedCounting,
        _ => Breaker::StoppedSilent,
    }
}

#[cfg(not(test))]
fn set_breaker(state: Breaker) {
    let value = match state {
        Breaker::Open => 0,
        Breaker::StoppedCounting => 1,
        Breaker::StoppedSilent => 2,
    };
    BREAKER.store(value, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
pub(crate) fn breaker() -> Breaker {
    BREAKER.with(|b| b.get())
}

#[cfg(test)]
fn set_breaker(state: Breaker) {
    BREAKER.with(|b| b.set(state));
}

/// Whether records are being queued: there is a sink koto may run, and the
/// process hasn't been told that sending is off. The sink is looked for
/// once per process, here, on the first record. Without one this is the
/// only cost of the sender: one cached check.
pub(crate) fn active() -> bool {
    sink::discover().is_some() && breaker() != Breaker::StoppedSilent
}

/// Queue `lines`, the records of the session in `session_dir`, and note
/// `oversize` more of its records that were too long to send.
pub(crate) fn push(session_dir: &Path, lines: Vec<String>, oversize: u64) {
    if lines.is_empty() && oversize == 0 {
        return;
    }
    let dir: Arc<Path> = Arc::from(session_dir);
    with_queue(|q| {
        if oversize > 0 {
            q.oversize.push((Arc::clone(&dir), oversize));
        }
        q.items.extend(lines.into_iter().map(|line| Queued {
            line,
            session_dir: Arc::clone(&dir),
        }));
    });
}

/// Dropped records, by session directory.
#[derive(Default)]
struct Drops(BTreeMap<Arc<Path>, u64>);

impl Drops {
    fn add(&mut self, dir: &Arc<Path>, n: u64) {
        *self.0.entry(Arc::clone(dir)).or_insert(0) += n;
    }

    fn add_all(&mut self, items: &[Queued]) {
        for item in items {
            self.add(&item.session_dir, 1);
        }
    }

    /// Write each session's count beside it.
    fn write(self) {
        for (dir, n) in self.0 {
            carry::add(&dir, n);
        }
    }
}

/// Send what is queued to the event sink. Returns at once, having called
/// nothing, when nothing is queued. See the module documentation for what
/// each outcome does.
pub fn flush() {
    let Queue { items, oversize } = with_queue(std::mem::take);
    if items.is_empty() && oversize.is_empty() {
        return;
    }
    let state = breaker();
    if state == Breaker::StoppedSilent {
        return;
    }
    let mut drops = Drops::default();
    for (dir, n) in &oversize {
        drops.add(dir, *n);
    }
    if state == Breaker::StoppedCounting {
        drops.add_all(&items);
        drops.write();
        return;
    }
    let Some(sink) = sink::discover() else {
        // Nothing is queued without a sink, so there is nothing to count.
        return;
    };

    let deadline = Instant::now() + call_budget();
    let mut rest = items.as_slice();
    while !rest.is_empty() {
        let (chunk, after) = rest.split_at(rest.len().min(CHUNK));
        if deadline.saturating_duration_since(Instant::now()) < MIN_CALL {
            // Too late to give a call its full time: this chunk and the
            // ones after it are dropped with no call, and the breaker is
            // left alone.
            drops.add_all(rest);
            break;
        }
        let lines: Vec<&str> = chunk.iter().map(|q| q.line.as_str()).collect();
        match sink::send(&sink, &lines, 0, deadline) {
            Outcome::Posted | Outcome::PostedSomeInvalid => {}
            Outcome::Usage => drops.add_all(chunk),
            Outcome::Off => {
                // Sending is off: stop, and count nothing, not even what
                // this flush had already set aside.
                set_breaker(Breaker::StoppedSilent);
                return;
            }
            Outcome::Dropped | Outcome::Killed | Outcome::SpawnFailed => {
                set_breaker(Breaker::StoppedCounting);
                drops.add_all(rest);
                break;
            }
        }
        rest = after;
    }
    drops.write();
}

/// The lines waiting for the next flush, in order.
#[cfg(test)]
pub(crate) fn queued_for_test() -> Vec<String> {
    with_queue(|q| q.items.iter().map(|i| i.line.clone()).collect())
}

/// Empty this thread's queue and reopen its breaker.
#[cfg(test)]
pub(crate) fn reset_for_test() {
    with_queue(|q| *q = Queue::default());
    set_breaker(Breaker::Open);
    TEST_CALL_BUDGET.with(|b| b.set(CALL_BUDGET));
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::telemetry::event_sink_stub::Stub;

    /// A stub sink installed for this thread, and two session directories
    /// to queue records for.
    struct Fixture {
        _tmp: tempfile::TempDir,
        stub: Stub,
        a: std::path::PathBuf,
        b: std::path::PathBuf,
    }

    impl Fixture {
        /// A fixture for a test that isn't about the deadline: the call
        /// budget is long enough that no chunk is ever late, however slow
        /// the host is at starting the stub.
        fn unhurried() -> Self {
            let f = Fixture::new();
            TEST_CALL_BUDGET.with(|b| b.set(Duration::from_secs(120)));
            f
        }

        /// A fixture with the real call budget. The tests that use it run
        /// with every core to themselves under the CI test runner
        /// (`.config/nextest.toml`), since they depend on the stub starting
        /// promptly.
        fn new() -> Self {
            reset_for_test();
            let tmp = tempfile::tempdir().unwrap();
            let stub = Stub::install(&tmp.path().join("event-sink"), &tmp.path().join("ctl"));
            sink::set_sink_for_test(Some(sink::SinkPath::for_test(stub.path.clone())));
            let a = tmp.path().join("session-a");
            let b = tmp.path().join("session-b");
            std::fs::create_dir(&a).unwrap();
            std::fs::create_dir(&b).unwrap();
            Fixture {
                _tmp: tmp,
                stub,
                a,
                b,
            }
        }

        /// Queue `n` records for the session in `dir`, numbered from
        /// `from`.
        fn queue(&self, dir: &Path, from: usize, n: usize) {
            let lines = (from..from + n)
                .map(|i| format!("{{\"kind\":\"test\",\"n\":{i}}}"))
                .collect();
            push(dir, lines, 0);
        }

        fn sizes(&self) -> Vec<usize> {
            self.stub.calls().iter().map(|c| c.lines.len()).collect()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            sink::set_sink_for_test(None);
            reset_for_test();
        }
    }

    #[test]
    fn an_empty_queue_makes_no_call() {
        let f = Fixture::unhurried();
        flush();
        flush();
        assert_eq!(f.stub.call_count(), 0);
        assert_eq!(breaker(), Breaker::Open);
    }

    #[test]
    fn nothing_is_queued_or_called_without_a_sink() {
        let f = Fixture::unhurried();
        sink::set_sink_for_test(None);
        assert!(!active());
        // A record pushed anyway is discarded by the flush, uncounted.
        f.queue(&f.a, 0, 3);
        flush();
        assert_eq!(f.stub.call_count(), 0);
        assert_eq!(carry::peek(&f.a), 0);
        assert!(queued_for_test().is_empty());
    }

    #[test]
    fn chunks_200_200_50() {
        let f = Fixture::new();
        f.queue(&f.a, 0, 450);
        let started = Instant::now();
        flush();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        let calls = f.stub.calls();
        assert_eq!(f.sizes(), vec![200, 200, 50]);
        for call in &calls {
            assert_eq!(call.argv, vec!["send", "--scope", "koto-journal"]);
            assert!(call.done);
        }
        // Every record arrived once, in the order it was queued.
        let sent: Vec<String> = calls.into_iter().flat_map(|c| c.lines).collect();
        let expected: Vec<String> = (0..450)
            .map(|i| format!("{{\"kind\":\"test\",\"n\":{i}}}"))
            .collect();
        assert_eq!(sent, expected);
        assert_eq!(carry::peek(&f.a), 0);
        assert_eq!(breaker(), Breaker::Open);
        // The queue is empty: a second flush calls nothing.
        flush();
        assert_eq!(f.stub.call_count(), 3);
    }

    #[test]
    fn exactly_two_hundred_records_make_one_call() {
        let f = Fixture::unhurried();
        f.queue(&f.a, 0, 200);
        flush();
        assert_eq!(f.sizes(), vec![200]);
    }

    #[test]
    fn deadline_drops_unreached_chunk() {
        let f = Fixture::new();
        // The second call sleeps past the call deadline.
        f.stub.sleeps(&["0", "30"]);
        f.queue(&f.a, 0, 450);
        let started = Instant::now();
        flush();
        let took = started.elapsed();
        assert!(took < Duration::from_secs(3), "the flush took {took:?}");
        assert!(took >= CALL_BUDGET, "the call was ended early: {took:?}");
        // Two calls were made; the third chunk never was.
        let calls = f.stub.calls();
        assert_eq!(f.sizes(), vec![200, 200]);
        assert!(calls[0].done);
        assert!(!calls[1].done, "the second call wasn't killed");
        // The killed chunk and the unreached one are both counted.
        assert_eq!(carry::peek(&f.a), 250);
        assert_eq!(breaker(), Breaker::StoppedCounting);
    }

    #[test]
    fn late_chunk_dropped_breaker_stays_open() {
        let f = Fixture::new();
        // Two healthy calls that together leave under 2.0 s for a third.
        f.stub.sleeps(&["0.45", "0.45", "0"]);
        f.queue(&f.a, 0, 450);
        let started = Instant::now();
        flush();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        let calls = f.stub.calls();
        assert_eq!(f.sizes(), vec![200, 200]);
        // Neither call was killed.
        assert!(calls.iter().all(|c| c.done));
        // Only the third chunk is counted, and the breaker is open.
        assert_eq!(carry::peek(&f.a), 50);
        assert_eq!(breaker(), Breaker::Open);
        // The next flush sends.
        f.queue(&f.a, 450, 5);
        flush();
        assert_eq!(f.sizes(), vec![200, 200, 5]);
        assert_eq!(carry::peek(&f.a), 50);
    }

    #[test]
    fn exit_3_trips_breaker_and_counts() {
        let f = Fixture::unhurried();
        f.stub.exit_codes(&[3]);
        f.queue(&f.a, 0, 250);
        f.queue(&f.b, 250, 10);
        flush();
        // One call: the failed chunk and the one after it are counted, each
        // record against its own session.
        assert_eq!(f.sizes(), vec![200]);
        assert_eq!(carry::peek(&f.a), 250);
        assert_eq!(carry::peek(&f.b), 10);
        assert_eq!(breaker(), Breaker::StoppedCounting);
        // Later records are still queued, and every later flush counts
        // them without calling.
        assert!(active());
        f.queue(&f.b, 260, 7);
        flush();
        f.queue(&f.a, 267, 1);
        flush();
        assert_eq!(f.stub.call_count(), 1);
        assert_eq!(carry::peek(&f.a), 251);
        assert_eq!(carry::peek(&f.b), 17);
    }

    #[test]
    fn exit_2_stops_without_counting() {
        let f = Fixture::unhurried();
        f.stub.exit_codes(&[2]);
        f.queue(&f.a, 0, 450);
        flush();
        assert_eq!(f.sizes(), vec![200]);
        assert_eq!(carry::peek(&f.a), 0);
        assert_eq!(breaker(), Breaker::StoppedSilent);
        // Nothing more is queued, called or counted.
        assert!(!active());
        f.queue(&f.a, 450, 5);
        flush();
        assert_eq!(f.stub.call_count(), 1);
        assert_eq!(carry::peek(&f.a), 0);
        assert!(!f.a.join(carry::COUNT_FILE).exists());
    }

    #[test]
    fn exit_1_drops_its_chunk_and_goes_on() {
        let f = Fixture::unhurried();
        f.stub.exit_codes(&[1, 0]);
        f.queue(&f.a, 0, 250);
        flush();
        assert_eq!(f.sizes(), vec![200, 50]);
        assert_eq!(carry::peek(&f.a), 200);
        assert_eq!(breaker(), Breaker::Open);
    }

    #[test]
    fn exit_4_counts_nothing_and_goes_on() {
        let f = Fixture::unhurried();
        f.stub.exit_codes(&[4]);
        f.queue(&f.a, 0, 250);
        flush();
        assert_eq!(f.sizes(), vec![200, 50]);
        assert_eq!(carry::peek(&f.a), 0);
        assert_eq!(breaker(), Breaker::Open);
    }

    #[test]
    fn a_sink_that_stops_passing_its_check_is_not_run_and_trips_the_breaker() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::unhurried();
        std::fs::set_permissions(&f.stub.path, std::fs::Permissions::from_mode(0o770)).unwrap();
        f.queue(&f.a, 0, 3);
        flush();
        assert_eq!(f.stub.call_count(), 0);
        assert_eq!(carry::peek(&f.a), 3);
        assert_eq!(breaker(), Breaker::StoppedCounting);
    }

    #[test]
    fn records_too_long_to_send_are_counted_by_the_next_flush() {
        let f = Fixture::unhurried();
        push(&f.a, vec!["{\"kind\":\"test\"}".to_string()], 2);
        push(&f.b, Vec::new(), 1);
        // Nothing is written until the flush.
        assert_eq!(carry::peek(&f.a), 0);
        flush();
        assert_eq!(f.sizes(), vec![1]);
        assert_eq!(carry::peek(&f.a), 2);
        assert_eq!(carry::peek(&f.b), 1);
    }
}
