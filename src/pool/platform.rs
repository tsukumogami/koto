//! Whether this platform runs pools and background actions (PRD R27).
//!
//! The feature rests on `flock` slot files, a detached runner and, on Linux,
//! a child subreaper, and it is built and tested on Linux and macOS only.
//! Elsewhere a pooled action on a defined pool stops with
//! `pool_unsupported_platform` and the compiler refuses `background: true`,
//! rather than run either on primitives nobody has checked there.
//!
//! This module is compiled on every platform, unlike the rest of
//! [`crate::pool`], because the template compiler asks it too. Unit tests
//! can force the answer for the current thread with [`force_for_test`], so
//! the unsupported path is tested on the platforms CI has.

/// True on Linux and macOS.
pub fn supported() -> bool {
    #[cfg(test)]
    if let Some(forced) = FORCED.with(|f| f.get()) {
        return forced;
    }
    cfg!(any(target_os = "linux", target_os = "macos"))
}

#[cfg(test)]
thread_local! {
    static FORCED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Make [`supported`] answer `value` on this thread until the returned
/// guard drops.
#[cfg(test)]
pub(crate) fn force_for_test(value: bool) -> ForcedPlatform {
    let previous = FORCED.with(|f| f.replace(Some(value)));
    ForcedPlatform { previous }
}

/// Restores [`supported`]'s previous answer on drop.
#[cfg(test)]
pub(crate) struct ForcedPlatform {
    previous: Option<bool>,
}

#[cfg(test)]
impl Drop for ForcedPlatform {
    fn drop(&mut self) {
        FORCED.with(|f| f.set(self.previous));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_and_macos_are_supported() {
        assert_eq!(
            supported(),
            cfg!(any(target_os = "linux", target_os = "macos"))
        );
    }

    #[test]
    fn a_forced_answer_lasts_as_long_as_its_guard() {
        let natural = supported();
        {
            let _forced = force_for_test(!natural);
            assert_eq!(supported(), !natural);
        }
        assert_eq!(supported(), natural);
    }
}
