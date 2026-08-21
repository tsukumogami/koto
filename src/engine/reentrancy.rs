//! Re-entrancy marking for `koto next`.
//!
//! A tick runs template commands as child processes, and those children
//! inherit its environment. Before it runs anything, the tick stamps
//! [`TICK_SESSION_ENV`] with the session it is advancing. A `koto next` that
//! finds the stamp already set was started from inside a command an outer
//! tick is running, and refuses.
//!
//! The refusal is not about redundant work. A nested tick appends to the
//! same event log the outer tick is halfway through processing. The outer
//! tick then finishes against the snapshot it started with and reports a
//! state the session has already left -- a wrong answer rather than a
//! missing one, so nothing surfaces an error. See koto#208.

/// Environment variable naming the session whose tick is running in this
/// process tree.
///
/// Set by `koto next` before it evaluates a gate or runs a default action;
/// read by a nested `koto next` to detect that it is nested. It is a marker,
/// not an input -- nothing reads it to decide which session to act on.
pub const TICK_SESSION_ENV: &str = "KOTO_TICK_SESSION";

/// The session named by an enclosing tick, or `None` when this process is
/// not running inside one.
///
/// A blank value counts as absent, so `KOTO_TICK_SESSION=` clears the marker
/// instead of blocking every tick under a nameless one.
pub fn enclosing_tick() -> Option<String> {
    match std::env::var(TICK_SESSION_ENV) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

/// Stamp this process as advancing `session`, so every command it runs
/// carries the marker.
///
/// The stamp lives for the rest of the process. Nothing clears it: a tick
/// runs one session and then exits.
pub fn mark_tick(session: &str) {
    std::env::set_var(TICK_SESSION_ENV, session);
}

/// The message a nested `koto next` refuses with.
///
/// `enclosing` is the session the outer tick is advancing, `requested` the
/// session this invocation asked for. Both are named because they are often
/// the same session and the reader needs to see that they are.
pub fn nested_invocation_message(enclosing: &str, requested: &str) -> String {
    format!(
        "koto next cannot run inside a command koto is running: the tick on session '{enclosing}' \
         spawned this process and has not finished. A nested tick advances the workflow while the \
         outer tick keeps reporting the state it started with, so the caller is told the session \
         is somewhere it has already left. Remove the `koto next {requested}` call from the \
         template's command -- the enclosing tick is what advances the session."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_names_both_sessions() {
        let msg = nested_invocation_message("outer", "inner");
        assert!(
            msg.contains("'outer'"),
            "should name the enclosing tick: {msg}"
        );
        assert!(
            msg.contains("koto next inner"),
            "should name the call to remove: {msg}"
        );
    }

    #[test]
    fn the_marker_name_is_the_documented_one() {
        // The name is a contract: the koto-user skill and the error-code
        // reference both spell it out for agents debugging a refusal.
        assert_eq!(TICK_SESSION_ENV, "KOTO_TICK_SESSION");
    }
}
