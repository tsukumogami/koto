//! A `LocalBackend` that records every event appended through
//! `SessionBackend::append_event`, and can refuse one event type, for
//! tests that need to see a write go through the store's commit funnel or
//! make that write fail.
//!
//! Include it from a test file with
//!
//! ```ignore
//! #[path = "support/funnel_backend.rs"]
//! mod funnel_backend;
//! ```
//!
//! Every method, the trait's defaulted ones included, delegates to the
//! wrapped `LocalBackend`, so the wrapper behaves as that store does apart
//! from the recording and the refusal. A refused append
//! returns an error without calling it, as an append that failed before
//! its commit would, so nothing is written to the log and no post-commit
//! hook (the run journal among them) runs.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use koto::engine::types::{Event, EventPayload, SessionStoreIdentity, StateFileHeader};
use koto::session::local::LocalBackend;
use koto::session::{SessionBackend, SessionError, SessionInfo, SessionLock};

/// The error message a refused append carries.
pub const INJECTED_FAILURE: &str = "injected append failure";

pub struct FunnelBackend {
    inner: LocalBackend,
    /// `(session, event type)` for each `append_event` call, refused ones
    /// included, in call order.
    appended: Mutex<Vec<(String, String)>>,
    /// The event type (`EventPayload::type_name`) whose append fails.
    refuse: Option<&'static str>,
}

impl FunnelBackend {
    /// A store on `base`, journaling inside it, as
    /// `LocalBackend::with_base_dir` does.
    pub fn new(base: &Path) -> Self {
        FunnelBackend {
            inner: LocalBackend::with_base_dir(base.to_path_buf()),
            appended: Mutex::new(Vec::new()),
            refuse: None,
        }
    }

    /// The same store, refusing every append of `event_type`.
    pub fn refusing(base: &Path, event_type: &'static str) -> Self {
        FunnelBackend {
            refuse: Some(event_type),
            ..Self::new(base)
        }
    }

    /// `(session, event type)` for each append, in call order.
    pub fn appended(&self) -> Vec<(String, String)> {
        self.appended.lock().unwrap().clone()
    }
}

impl SessionBackend for FunnelBackend {
    fn create(&self, id: &str) -> anyhow::Result<PathBuf> {
        self.inner.create(id)
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        self.inner.session_dir(id)
    }

    fn exists(&self, id: &str) -> bool {
        self.inner.exists(id)
    }

    fn cleanup(&self, id: &str) -> anyhow::Result<()> {
        self.inner.cleanup(id)
    }

    fn list(&self) -> anyhow::Result<Vec<SessionInfo>> {
        self.inner.list()
    }

    fn append_header(&self, id: &str, header: &StateFileHeader) -> anyhow::Result<()> {
        self.inner.append_header(id, header)
    }

    fn append_event(
        &self,
        id: &str,
        payload: &EventPayload,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        let event_type = payload.type_name();
        self.appended
            .lock()
            .unwrap()
            .push((id.to_string(), event_type.to_string()));
        if self.refuse == Some(event_type) {
            anyhow::bail!(INJECTED_FAILURE);
        }
        self.inner.append_event(id, payload, timestamp)
    }

    fn read_events(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
        self.inner.read_events(id)
    }

    fn read_events_local(&self, id: &str) -> anyhow::Result<(StateFileHeader, Vec<Event>)> {
        self.inner.read_events_local(id)
    }

    fn rewrite_header(
        &self,
        id: &str,
        f: &dyn Fn(StateFileHeader) -> StateFileHeader,
    ) -> anyhow::Result<()> {
        self.inner.rewrite_header(id, f)
    }

    fn count_unreadable(&self) -> usize {
        self.inner.count_unreadable()
    }

    fn store_identity(&self) -> Option<SessionStoreIdentity> {
        self.inner.store_identity()
    }

    fn read_header(&self, id: &str) -> anyhow::Result<StateFileHeader> {
        self.inner.read_header(id)
    }

    fn init_state_file(
        &self,
        id: &str,
        header: StateFileHeader,
        initial_events: Vec<Event>,
    ) -> Result<(), SessionError> {
        self.inner.init_state_file(id, header, initial_events)
    }

    fn ensure_pushed(&self, id: &str) -> Result<(), SessionError> {
        self.inner.ensure_pushed(id)
    }

    fn relocate(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.inner.relocate(from, to)
    }

    fn lock_state_file(&self, id: &str) -> Result<SessionLock, SessionError> {
        self.inner.lock_state_file(id)
    }
}
