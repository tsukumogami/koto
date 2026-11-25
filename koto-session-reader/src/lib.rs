//! The read half of koto's session store.
//!
//! The module tree mirrors koto's, so koto re-exports each module at the
//! path it had before the split and `koto::engine::types` stays the same
//! item a dependent names.

#![forbid(unsafe_code)]
// The crate reads the session store and never writes it: clippy.toml bans
// every write, create, remove, link, lock, spawn and socket path, and every
// file open outside fs_read.rs. Denied here so a violation fails the build's
// lint run even without -D warnings.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod cache;
pub mod dashboard;
pub mod decider;
pub mod engine;
pub mod findings;
pub mod fs_read;
pub mod koto_root;
pub mod redact;
pub mod session;
pub mod source;
pub mod template;
