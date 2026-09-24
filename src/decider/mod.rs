//! Opt-in decider support.
//!
//! A decider is a typed decision model that koto can consult for a
//! template-declared decision instead of stopping for agent evidence.
//! Whether a user is opted in is decided in exactly one place:
//! [`crate::config::resolve::DeciderSettings::opted_in`], built by
//! [`crate::config::resolve::resolve_decider`].

pub mod types;

pub use types::{ApiKey, GlobalMode, SettingOrigin};
