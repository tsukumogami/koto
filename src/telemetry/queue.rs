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
//! ## Drop counts and the carry
//!
//! Drop counts go to each record's session directory (see `carry`). They
//! leave with the first call of a later flush: before that call a flush
//! takes the count of every session it has records for, and the orphan
//! count of their stores, and passes the sum as `--carry N`. Only the first
//! call of a flush carries, and its outcome alone decides what happens to
//! what was taken. Posted, the counts stay out. Anything else (the call was
//! refused, failed, was killed, or the sink said sending is off) and they
//! are given back, whatever the flush's later calls do, so the next flush
//! carries them again.

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

/// Give this thread's flushes a call budget other than the real one, until
/// the next [`reset_for_test`].
#[cfg(test)]
pub(crate) fn set_call_budget_for_test(budget: Duration) {
    TEST_CALL_BUDGET.with(|b| b.set(budget));
}

// How much of a flush's budget a test says each call used up, on top of
// the time it really took. A test that needs a call to leave too little
// time for the next one sets this and a long budget, so the outcome is
// decided by the numbers it chose and not by how fast the host ran the
// stub.
#[cfg(test)]
thread_local! {
    static TEST_CALL_COST: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::ZERO) };
}

#[cfg(test)]
fn call_cost() -> Duration {
    TEST_CALL_COST.with(|c| c.get())
}

#[cfg(not(test))]
fn call_cost() -> Duration {
    Duration::ZERO
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

    /// Write each session's count beside it. Every lock wait is measured
    /// from `started`, so a flush that drops records of many sessions
    /// spends no longer here than one that drops records of one.
    fn write(self, started: Instant) {
        for (dir, n) in self.0 {
            carry::add_within(&dir, n, started);
        }
    }
}

/// The counts a flush took to carry out with its first call: each queued
/// session's own, and the orphans of each store those sessions belong to.
/// Each was zeroed under its lock as it was taken, so no other process can
/// carry the same count. If the call that carries them isn't delivered,
/// they are given back.
#[derive(Default)]
struct Taken {
    sessions: Vec<(Arc<Path>, u64)>,
    orphans: Vec<(std::path::PathBuf, u64)>,
}

impl Taken {
    /// Take the counts of every session `items` belong to, and of their
    /// stores. The lock waits are measured from `started`, together.
    fn take(items: &[Queued], started: Instant) -> Taken {
        let mut taken = Taken::default();
        let mut dirs: Vec<&Arc<Path>> = Vec::new();
        let mut bases: Vec<&Path> = Vec::new();
        for item in items {
            if !dirs
                .iter()
                .any(|d| Arc::ptr_eq(d, &item.session_dir) || **d == item.session_dir)
            {
                dirs.push(&item.session_dir);
                if let Some(base) = item.session_dir.parent() {
                    if !bases.contains(&base) {
                        bases.push(base);
                    }
                }
            }
        }
        for dir in dirs {
            let n = carry::take(dir, started);
            if n > 0 {
                taken.sessions.push((Arc::clone(dir), n));
            }
        }
        for base in bases {
            let n = carry::take_orphans(base, started);
            if n > 0 {
                taken.orphans.push((base.to_path_buf(), n));
            }
        }
        taken
    }

    /// What the first call carries.
    fn total(&self) -> u64 {
        self.sessions
            .iter()
            .map(|(_, n)| *n)
            .chain(self.orphans.iter().map(|(_, n)| *n))
            .fold(0u64, u64::saturating_add)
            .min(carry::MAX_COUNT)
    }

    /// Put every taken count back where it came from. A session whose
    /// directory has gone since gets its count in its store's orphans.
    fn give_back(self, started: Instant) {
        for (dir, n) in self.sessions {
            carry::give_back(&dir, n, started);
        }
        for (base, n) in self.orphans {
            carry::give_back_orphans(&base, n, started);
        }
    }
}

/// Send what is queued to the event sink. Returns at once, having called
/// nothing, when nothing is queued. See the module documentation for what
/// each outcome does.
///
/// It is called at fixed points, so a long command doesn't hold its
/// records until it ends: before a gate's or an action's command is
/// started (`run_shell_command`), before a decider is called, before each
/// wait (a polling gate's, a polling action's, `koto request`'s, the
/// decider lock's), on the terminal tick before the session directory can
/// be removed, and from the exit hook. Code that adds a new slow step or a
/// new wait should call it first. The one rule is that no call may sit
/// inside a command's time limit.
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
        drops.write(Instant::now());
        return;
    }
    let Some(sink) = sink::discover() else {
        // Nothing is queued without a sink, so there is nothing to count.
        return;
    };

    let deadline = Instant::now() + call_budget();
    // The counts earlier flushes left behind go out with the first call.
    // They are taken now, under their locks, and held in `taken` until that
    // call is known to have been delivered.
    let mut taken = Some(Taken::take(&items, Instant::now()));
    let mut carry = taken.as_ref().map_or(0, Taken::total);
    let mut first_call = true;
    // Zero outside tests.
    let mut charged = Duration::ZERO;
    let mut rest = items.as_slice();
    while !rest.is_empty() {
        let (chunk, after) = rest.split_at(rest.len().min(CHUNK));
        if deadline.saturating_duration_since(Instant::now() + charged) < MIN_CALL {
            // Too late to give a call its full time: this chunk and the
            // ones after it are dropped with no call, and the breaker is
            // left alone.
            drops.add_all(rest);
            break;
        }
        let lines: Vec<&str> = chunk.iter().map(|q| q.line.as_str()).collect();
        // Only the first call carries, and whether the carry was delivered
        // is decided by that call alone: a later chunk that is posted says
        // nothing about a carry it didn't have.
        let carrying = std::mem::take(&mut first_call);
        let outcome = sink::send(&sink, &lines, carry, deadline);
        charged += call_cost();
        carry = 0;
        match outcome {
            Outcome::Posted | Outcome::PostedSomeInvalid => {
                if carrying {
                    taken = None;
                }
            }
            Outcome::Usage => drops.add_all(chunk),
            Outcome::Off => {
                // Sending is off: stop, and count nothing, not even what
                // this flush had already set aside. What was taken is put
                // back as it was: it was counted before, by a sink that
                // was on.
                set_breaker(Breaker::StoppedSilent);
                if let Some(taken) = taken {
                    taken.give_back(Instant::now());
                }
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
    // The give-back and the new drops share one lock wait.
    let started = Instant::now();
    if let Some(taken) = taken {
        taken.give_back(started);
    }
    drops.write(started);
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
    TEST_CALL_COST.with(|c| c.set(Duration::ZERO));
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
            set_call_budget_for_test(Duration::from_secs(120));
            f
        }

        /// A fixture whose first call can take several seconds and still
        /// leave time for a second, for the one test that needs a call to
        /// be killed at the deadline.
        fn with_budget(budget: Duration) -> Self {
            let f = Fixture::new();
            TEST_CALL_BUDGET.with(|b| b.set(budget));
            f
        }

        /// A fixture where each call is charged `cost` of a long budget,
        /// whatever time it really took.
        fn charging(budget: Duration, cost: Duration) -> Self {
            let f = Fixture::with_budget(budget);
            TEST_CALL_COST.with(|c| c.set(cost));
            f
        }

        /// A fixture with the real call budget, for a test that makes one
        /// call or none.
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
        // About the chunking, not the deadline: the budget is long, so a
        // slow host can't make a later chunk late.
        let f = Fixture::unhurried();
        f.queue(&f.a, 0, 450);
        flush();
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
        // The deadline is real here, since the test is that a call is
        // killed at it. The budget is 8 s and not the real 2.85 s so that
        // the first call can take up to 6 s on a slow host and still leave
        // the second its 2.0 s.
        let budget = Duration::from_secs(8);
        let f = Fixture::with_budget(budget);
        // The second call sleeps far past the deadline.
        f.stub.sleeps(&["0", "300"]);
        f.queue(&f.a, 0, 450);
        let started = Instant::now();
        flush();
        let took = started.elapsed();
        assert!(took >= budget, "the call was ended early: {took:?}");
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
        // Two healthy calls that together leave under 2.0 s for a third.
        // The times are charged, not slept: of a 600 s budget each call is
        // charged 299.5 s, so one call leaves 300.5 s and two leave 1 s,
        // and the host would have to stall for minutes to change either.
        let f = Fixture::charging(Duration::from_secs(600), Duration::from_millis(299_500));
        f.queue(&f.a, 0, 450);
        flush();
        let calls = f.stub.calls();
        assert_eq!(f.sizes(), vec![200, 200]);
        // Neither call was killed.
        assert!(calls.iter().all(|c| c.done));
        // Only the third chunk is counted, and the breaker is open.
        assert_eq!(carry::peek(&f.a), 50);
        assert_eq!(breaker(), Breaker::Open);
        // The next flush sends, with a budget of its own, and carries the
        // 50 out with it.
        f.queue(&f.a, 450, 5);
        flush();
        assert_eq!(f.sizes(), vec![200, 200, 5]);
        assert_eq!(f.stub.calls()[2].carry(), Some(50));
        assert_eq!(carry::peek(&f.a), 0);
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
    fn a_count_goes_out_as_carry_on_the_first_call_and_is_zeroed() {
        let f = Fixture::unhurried();
        assert!(carry::add(&f.a, 7));
        assert!(carry::add(&f.b, 3));
        // Records for both sessions, in three chunks.
        f.queue(&f.a, 0, 250);
        f.queue(&f.b, 250, 200);
        flush();
        let calls = f.stub.calls();
        assert_eq!(f.sizes(), vec![200, 200, 50]);
        // The first call carries both sessions' counts; the others carry
        // nothing.
        assert_eq!(calls[0].carry(), Some(10));
        assert_eq!(
            calls[0].argv,
            vec!["send", "--scope", "koto-journal", "--carry", "10"]
        );
        assert_eq!(calls[1].carry(), None);
        assert_eq!(calls[2].carry(), None);
        assert_eq!(carry::peek(&f.a), 0);
        assert_eq!(carry::peek(&f.b), 0);
        // Nothing is left to carry on the next flush.
        f.queue(&f.a, 450, 1);
        flush();
        assert_eq!(f.stub.calls()[3].carry(), None);
    }

    #[test]
    fn a_session_with_no_count_sends_no_carry_and_gains_no_files() {
        let f = Fixture::unhurried();
        f.queue(&f.a, 0, 3);
        flush();
        assert_eq!(f.stub.calls()[0].carry(), None);
        assert_eq!(std::fs::read_dir(&f.a).unwrap().count(), 0);
    }

    #[test]
    fn exit_3_adds_back_taken_carry() {
        let f = Fixture::unhurried();
        f.stub.exit_codes(&[3]);
        assert!(carry::add(&f.a, 7));
        assert!(carry::add(&f.b, 3));
        f.queue(&f.a, 0, 250);
        f.queue(&f.b, 250, 10);
        flush();
        // The one call carried what was taken...
        let calls = f.stub.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].carry(), Some(10));
        // ...and since the sink refused it, the taken counts are back,
        // each in its own session, with the failed chunk's records and the
        // unsent chunk's on top.
        assert_eq!(carry::peek(&f.a), 7 + 250);
        assert_eq!(carry::peek(&f.b), 3 + 10);
        assert_eq!(breaker(), Breaker::StoppedCounting);
    }

    #[test]
    fn a_killed_call_adds_back_taken_carry() {
        let f = Fixture::new();
        f.stub.sleeps(&["120"]);
        assert!(carry::add(&f.a, 7));
        f.queue(&f.a, 0, 5);
        flush();
        let calls = f.stub.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].carry(), Some(7));
        assert!(!calls[0].done);
        assert_eq!(carry::peek(&f.a), 7 + 5);
    }

    #[test]
    fn a_failed_or_switched_off_first_call_gives_the_carry_back() {
        // Exit 1: the chunk is lost and counted, the carry returns, and the
        // next flush carries both.
        let f = Fixture::unhurried();
        f.stub.exit_codes(&[1, 0]);
        assert!(carry::add(&f.a, 7));
        f.queue(&f.a, 0, 2);
        flush();
        assert_eq!(f.stub.calls()[0].carry(), Some(7));
        assert_eq!(carry::peek(&f.a), 7 + 2);
        f.queue(&f.a, 2, 1);
        flush();
        assert_eq!(f.stub.calls()[1].carry(), Some(9));
        assert_eq!(carry::peek(&f.a), 0);
        drop(f);

        // Exit 2: nothing new is counted, and what was taken is put back
        // as it was.
        let f = Fixture::unhurried();
        f.stub.exit_codes(&[2]);
        assert!(carry::add(&f.a, 7));
        f.queue(&f.a, 0, 2);
        flush();
        assert_eq!(f.stub.calls()[0].carry(), Some(7));
        assert_eq!(carry::peek(&f.a), 7);
        assert_eq!(breaker(), Breaker::StoppedSilent);
    }

    #[test]
    fn a_carry_whose_call_failed_comes_back_even_when_a_later_chunk_is_posted() {
        let f = Fixture::unhurried();
        // The first call, the one that carries, fails with exit 1; the
        // flush goes on and the second chunk is posted.
        f.stub.exit_codes(&[1, 0]);
        assert!(carry::add(&f.a, 7));
        f.queue(&f.a, 0, 250);
        flush();
        let calls = f.stub.calls();
        assert_eq!(f.sizes(), vec![200, 50]);
        assert_eq!(calls[0].carry(), Some(7));
        assert_eq!(calls[1].carry(), None);
        // The 7 were not delivered, so they are back, with the failed
        // chunk's 200 records; the posted chunk adds nothing.
        assert_eq!(carry::peek(&f.a), 7 + 200);
        assert_eq!(breaker(), Breaker::Open);
        // The next flush carries them.
        f.queue(&f.a, 250, 1);
        flush();
        assert_eq!(f.stub.calls()[2].carry(), Some(207));
        assert_eq!(carry::peek(&f.a), 0);
    }

    #[test]
    fn a_carry_delivered_by_the_first_call_stays_out_when_a_later_chunk_fails() {
        let f = Fixture::unhurried();
        f.stub.exit_codes(&[0, 3]);
        assert!(carry::add(&f.a, 7));
        f.queue(&f.a, 0, 250);
        flush();
        assert_eq!(f.sizes(), vec![200, 50]);
        assert_eq!(f.stub.calls()[0].carry(), Some(7));
        // The 7 went out with the first call. Only the refused second
        // chunk's records are counted; the carry is not given back.
        assert_eq!(carry::peek(&f.a), 50);
        assert_eq!(breaker(), Breaker::StoppedCounting);
    }

    #[test]
    fn a_stores_orphan_count_is_carried_and_given_back_on_failure() {
        let f = Fixture::unhurried();
        let base = f.a.parent().unwrap().to_path_buf();
        // A session that held a count is removed: its count becomes an
        // orphan of the store.
        let gone = base.join("session-gone");
        std::fs::create_dir(&gone).unwrap();
        assert!(carry::add(&gone, 6));
        carry::orphan(&gone);
        std::fs::remove_dir_all(&gone).unwrap();
        assert_eq!(carry::peek_orphans(&base), 6);

        // A refused call gives the orphans back to the store.
        f.stub.exit_codes(&[3, 0]);
        assert!(carry::add(&f.a, 1));
        f.queue(&f.a, 0, 2);
        flush();
        assert_eq!(f.stub.calls()[0].carry(), Some(7));
        assert_eq!(carry::peek_orphans(&base), 6);
        assert_eq!(carry::peek(&f.a), 1 + 2);

        // A later flush from any session of the store, in a process whose
        // breaker is open, carries them out.
        set_breaker(Breaker::Open);
        f.queue(&f.b, 2, 1);
        flush();
        assert_eq!(f.stub.calls()[1].carry(), Some(6));
        assert_eq!(carry::peek_orphans(&base), 0);
        // Session a's own count waits for a flush that has its records.
        assert_eq!(carry::peek(&f.a), 3);
    }

    #[test]
    fn a_session_removed_before_the_give_back_keeps_its_count_as_an_orphan() {
        let f = Fixture::unhurried();
        let base = f.a.parent().unwrap().to_path_buf();
        assert!(carry::add(&f.a, 4));
        f.queue(&f.a, 0, 2);
        // The stub removes nothing itself; the session directory is removed
        // here, as the terminal tick would, after its records were queued.
        // Its count moves to the orphans first, as the removal path does.
        carry::orphan(&f.a);
        std::fs::remove_dir_all(&f.a).unwrap();
        f.stub.exit_codes(&[3]);
        flush();
        // The orphans went out as the carry and came back; the dropped
        // records of a session with no directory can't be written there.
        assert_eq!(f.stub.calls()[0].carry(), Some(4));
        assert_eq!(carry::peek_orphans(&base), 4);
        assert!(!f.a.exists());
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
