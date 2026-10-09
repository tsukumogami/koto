//! Variables a hosting Claude Code session hands koto.
//!
//! Every read of a host variable goes through [`host_env`], so the
//! `/workflows` renderer and the run journal see the same value and a unit
//! test controls both through one seam.

/// The id of the Claude Code session driving this process, set by Claude
/// Code in every subprocess it starts.
pub(crate) const CLAUDE_SESSION_ID_ENV: &str = "CLAUDE_CODE_SESSION_ID";

/// A variable the hosting Claude Code session hands koto.
#[cfg(not(test))]
pub(crate) fn host_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// In unit tests the host's variables are never read from the process
/// environment: a test run inside a Claude Code session would otherwise
/// discover that session's real `/workflows` directory and record its id in
/// a run journal. A test that wants a host variable sets it with
/// [`set_host_env_for_test`].
#[cfg(test)]
pub(crate) fn host_env(name: &str) -> Option<String> {
    HOST_ENV.with(|m| m.borrow().get(name).cloned())
}

#[cfg(test)]
thread_local! {
    static HOST_ENV: std::cell::RefCell<std::collections::HashMap<String, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Set (or with `None`, clear) a host variable for this thread's unit tests.
#[cfg(test)]
pub(crate) fn set_host_env_for_test(name: &str, value: Option<&str>) {
    HOST_ENV.with(|m| {
        let mut m = m.borrow_mut();
        match value {
            Some(v) => m.insert(name.to_string(), v.to_string()),
            None => m.remove(name),
        };
    });
}
