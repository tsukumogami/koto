//! The one way a koto process ends.
//!
//! Every exit goes through [`exit`], which runs the hooks registered with
//! [`register`] and then ends the process. A command that defers work to
//! its way out (the cloud writes a tick batches, a scratch directory a read
//! keeps until it is done) registers a hook instead of calling its cleanup
//! at each place it might leave: `main` ends in [`exit`] for a normal
//! return and for an error, and every early exit in the CLI calls it too.
//!
//! `clippy.toml` bans the standard library's exit function everywhere else,
//! so a new exit site that skips the hooks fails the lint rather than
//! review.
//!
//! Hooks run in reverse registration order, so the first one registered
//! runs last. Each runs under `catch_unwind`: a hook that panics doesn't
//! stop the ones after it. The list is taken out of its lock before any
//! hook runs, so a hook that calls [`exit`] itself ends the process without
//! running a hook twice, and a hook that registers another finds nobody
//! left to run it.
//!
//! [`crash`] is the exception. It ends the process as a kill would, with no
//! hook, for the fault points tests use to simulate one.
//!
//! A panic still ends the process without running hooks.

use std::panic::{catch_unwind, AssertUnwindSafe};

/// What a hook is: run once, on whichever thread calls [`exit`].
type Hook = Box<dyn FnOnce() + Send>;

#[cfg(not(test))]
static HOOKS: std::sync::Mutex<Vec<Hook>> = std::sync::Mutex::new(Vec::new());

// Each unit test gets a list of its own: the harness runs tests on separate
// threads of one process, and a shared list would hand one test another's
// hooks.
#[cfg(test)]
thread_local! {
    static HOOKS: std::cell::RefCell<Vec<Hook>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Add `hook` to the list [`exit`] runs. Hooks run in reverse registration
/// order.
pub fn register(hook: impl FnOnce() + Send + 'static) {
    push(Box::new(hook));
}

#[cfg(not(test))]
fn push(hook: Hook) {
    // A poisoned lock still holds a usable list: a hook is only ever pushed
    // or the whole list taken.
    HOOKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(hook);
}

#[cfg(test)]
fn push(hook: Hook) {
    HOOKS.with(|h| h.borrow_mut().push(hook));
}

/// Take every registered hook, leaving the list empty.
#[cfg(not(test))]
fn take() -> Vec<Hook> {
    std::mem::take(
        &mut *HOOKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

#[cfg(test)]
fn take() -> Vec<Hook> {
    HOOKS.with(|h| std::mem::take(&mut *h.borrow_mut()))
}

/// Run every registered hook, last registered first, and leave the list
/// empty. A hook that panics is skipped past.
fn run_hooks() {
    for hook in take().into_iter().rev() {
        // The hook is consumed whether it returns or unwinds, and nothing
        // it borrowed is read afterwards.
        let _ = catch_unwind(AssertUnwindSafe(hook));
    }
}

/// Run the registered hooks, then end the process with `code`.
pub fn exit(code: i32) -> ! {
    run_hooks();
    exit_now(code)
}

/// End the process with `code` and run no hook: what a killed process
/// leaves behind. Only the fault points that simulate a kill call this.
pub fn crash(code: i32) -> ! {
    exit_now(code)
}

// The one direct exit in the crate, kept on one line so a search for direct
// exits finds only this function.
#[rustfmt::skip]
#[allow(clippy::disallowed_methods)] fn exit_now(code: i32) -> ! { std::process::exit(code) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn recorder() -> (Arc<Mutex<Vec<&'static str>>>, impl Fn(&'static str) -> Hook) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let hook = move |name: &'static str| -> Hook {
            let log = Arc::clone(&log);
            Box::new(move || log.lock().unwrap().push(name))
        };
        (seen, hook)
    }

    #[test]
    fn hooks_run_in_reverse_registration_order() {
        let (seen, hook) = recorder();
        register(hook("first"));
        register(hook("second"));
        register(hook("third"));
        run_hooks();
        assert_eq!(*seen.lock().unwrap(), vec!["third", "second", "first"]);
    }

    #[test]
    fn a_panicking_hook_does_not_stop_the_rest() {
        let (seen, hook) = recorder();
        register(hook("first"));
        register(|| panic!("a hook that fails"));
        register(hook("third"));
        // Keep the expected panic's message out of the test output.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        run_hooks();
        std::panic::set_hook(previous);
        assert_eq!(*seen.lock().unwrap(), vec!["third", "first"]);
    }

    #[test]
    fn the_list_is_empty_while_hooks_run_and_after() {
        let (seen, hook) = recorder();
        let late = hook("registered by a hook");
        let log = Arc::clone(&seen);
        register(move || {
            // What a hook that called `exit` again would find: nothing
            // left to run.
            let left = take().len();
            log.lock().unwrap().push(if left == 0 {
                "list empty"
            } else {
                "list not empty"
            });
            register(late);
        });
        run_hooks();
        assert_eq!(*seen.lock().unwrap(), vec!["list empty"]);
        // The hook registered during the run was not run by it. A second
        // run finds it, which is what a later `exit` would do.
        run_hooks();
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["list empty", "registered by a hook"]
        );
        assert!(take().is_empty());
    }

    #[test]
    fn running_with_no_hooks_does_nothing() {
        run_hooks();
        assert!(take().is_empty());
    }
}
