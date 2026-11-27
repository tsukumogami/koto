//! A stub sink and a way to queue records, for unit tests outside this
//! module that need to see when a flush happened.
//!
//! The tests of a flush point ask one thing: had the queued records reached
//! the sink by the time the code under test got to the step the flush must
//! come before? A unit test's queue and sink belong to its thread, so a
//! test builds a [`SinkFixture`], queues some records, runs the code, and
//! reads [`SinkFixture::received`] from inside the step it cares about.

use std::path::PathBuf;
use std::time::Duration;

use super::event_sink_stub::Stub;
use super::{queue, sink};

/// A stub sink installed for this thread, and one session directory to
/// queue records for. Dropping it takes the sink away again.
pub(crate) struct SinkFixture {
    _tmp: tempfile::TempDir,
    stub: Stub,
    session: PathBuf,
}

impl SinkFixture {
    pub(crate) fn new() -> Self {
        queue::reset_for_test();
        // None of these tests is about the deadline, so no chunk is ever
        // late however slow the host is at starting the stub.
        queue::set_call_budget_for_test(Duration::from_secs(120));
        let tmp = tempfile::tempdir().unwrap();
        let stub = Stub::install(&tmp.path().join("event-sink"), &tmp.path().join("ctl"));
        sink::set_sink_for_test(Some(sink::SinkPath::for_test(stub.path.clone())));
        let session = tmp.path().join("session");
        std::fs::create_dir(&session).unwrap();
        SinkFixture {
            _tmp: tmp,
            stub,
            session,
        }
    }

    /// Queue `n` records.
    pub(crate) fn queue(&self, n: usize) {
        let lines = (0..n)
            .map(|i| format!("{{\"kind\":\"test\",\"n\":{i}}}"))
            .collect();
        queue::push(&self.session, lines, 0);
    }

    /// How many records the sink has been sent, over calls that ran to
    /// their end. A flush returns only once its calls have, so a record
    /// counted here was sent by a flush that had already returned.
    pub(crate) fn received(&self) -> usize {
        self.stub
            .calls()
            .iter()
            .filter(|c| c.done)
            .map(|c| c.lines.len())
            .sum()
    }
}

impl Drop for SinkFixture {
    fn drop(&mut self) {
        sink::set_sink_for_test(None);
        queue::reset_for_test();
    }
}
