//! Host resource pools (DESIGN-host-resource-pools.md).
//!
//! A pool caps how many sessions under one koto root run a named kind of
//! action at once. A template names the pool on a state's `default_action`;
//! the operator sizes it in the user's koto config. Each pool is a set of
//! `flock`ed slot files plus a queue file under `<koto root>/pools/<name>/`.
//!
//! - [`config`] reads one `[pools.<name>]` table from the user config.
//! - [`store`] is the on-disk pool: its directory, slots, holder records and
//!   queue.
//! - [`guard`] is what a tick runs before a pooled action: take a slot, wait
//!   for one, or report the session as queued.

pub mod config;
pub mod guard;
pub mod store;
