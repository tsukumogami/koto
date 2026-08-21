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
/// `enclosing` is the session named by the marker, `requested` the session
/// this invocation asked for. Both are named because they are often the same
/// session and the reader needs to see that they are.
///
/// The message does not claim the enclosing tick is still running, because
/// nothing here checks. The marker has no liveness: a command that detaches
/// itself -- `setsid`, a backgrounded subshell -- escapes the process-group
/// kill at timeout and carries the marker for as long as it lives, so a
/// `koto next` it runs minutes later is refused by a tick that exited long
/// ago. No shipped template does this, which is why the marker stays a plain
/// inherited flag rather than growing a pid and a liveness probe. The person
/// who hits it cannot read this comment, so the escape hatch is named in the
/// message itself.
pub fn nested_invocation_message(enclosing: &str, requested: &str) -> String {
    format!(
        "koto next cannot run inside a command koto is running: this process inherited \
         {TICK_SESSION_ENV} from a tick on session '{enclosing}'. A nested tick advances the \
         workflow while that tick goes on reporting the state it started with, so the caller is \
         told the session is somewhere it has already left. If this is a template's command, \
         remove the `koto next {requested}` call -- the enclosing tick is what advances the \
         session. If that tick has already exited and this process outlived it (a command that \
         detaches with setsid or backgrounds itself does), clear the marker: \
         `{TICK_SESSION_ENV}= koto next {requested}`."
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

    /// The marker has no liveness, so a process that outlived its tick can be
    /// refused by one that is long gone. The way out has to be in the message
    /// -- whoever hits it is by definition not reading this file.
    #[test]
    fn the_message_carries_the_escape_hatch() {
        let msg = nested_invocation_message("outer", "inner");
        assert!(
            msg.contains("KOTO_TICK_SESSION= koto next inner"),
            "should spell out the command that clears the marker: {msg}"
        );
    }

    /// The message must not assert that the enclosing tick is still running.
    /// Nothing checks, and a detached command outliving its tick makes the
    /// claim false at exactly the moment someone is trying to debug it.
    #[test]
    fn the_message_does_not_claim_the_tick_is_still_running() {
        let msg = nested_invocation_message("outer", "inner");
        assert!(
            !msg.contains("has not finished"),
            "the message states liveness it never checked: {msg}"
        );
    }

    #[test]
    fn the_marker_name_is_the_documented_one() {
        // The name is a contract: the koto-user skill and the error-code
        // reference both spell it out for agents debugging a refusal.
        assert_eq!(TICK_SESSION_ENV, "KOTO_TICK_SESSION");
    }
}
