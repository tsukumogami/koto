//! Finding the event sink and making one call to it.
//!
//! The event sink is an optional executable at a fixed path under the
//! user's home, `$HOME/.local/libexec/fleet/event-sink`. koto hands it a
//! batch of session records on stdin and reads only its exit status. This
//! module answers two questions and nothing else: is there a sink koto may
//! run ([`discover`]), and what did one call do ([`send`]). What to send
//! and what to do about the answer belong to `queue`.
//!
//! ## Trust
//!
//! koto runs the sink on local ownership alone. The directory is resolved
//! to its canonical path, and every directory from there up to `/` must be
//! owned by the user or by root and writable by nobody else. The file must
//! be a regular file, not a symlink, owned by the user, executable, and
//! writable by nobody else. The sink is run by that canonical path, so only
//! the user or root could swap a component between the check and the run.
//! The file is checked again just before each run.
//!
//! No variable can point koto at another program. `HOME` locates the fixed
//! path, and `FLEET_EVENT_SINK=off` turns sending off; no other value of it
//! is read, and nothing else is.
//!
//! ## The call
//!
//! `event-sink send --scope koto-journal [--carry N]`, one record per line
//! on stdin. The sink runs in its own process group with `/` as its working
//! directory, no stdout or stderr, and an environment cleared to a short
//! allowlist, so a credential in the user's shell never reaches it. A call
//! that outlives its deadline is ended by killing that group and no other.

use std::io::Write as _;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

/// The sink's path under the home directory.
const SINK_DIR: &str = ".local/libexec/fleet";
const SINK_FILE: &str = "event-sink";

/// The one variable that switches sending, and the one value that does.
#[cfg(not(test))]
const SWITCH_ENV: &str = "FLEET_EVENT_SINK";
const SWITCH_OFF: &str = "off";

/// The scope koto's records are sent under.
const SCOPE: &str = "koto-journal";

/// The variables the sink is started with, each only when it is set. The
/// first six are what a program needs to find its files and print text;
/// the last three are the identity the sink stamps each record with.
const ENV_ALLOWLIST: &[&str] = &[
    "HOME",
    "PATH",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "CLAUDE_CODE_SESSION_ID",
    "NIWA_DISPATCH_ID",
    "NIWA_WORKSPACE",
];

/// How long a killed call's process group is given to empty.
const GROUP_EXIT_WAIT: Duration = Duration::from_millis(100);

/// How long a finished call waits for its stdin writer before leaving it.
const WRITER_WAIT: Duration = Duration::from_millis(50);

/// The canonical path of a sink that passed discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SinkPath(PathBuf);

impl SinkPath {
    /// A sink at `path`, for a test that installs its own.
    #[cfg(test)]
    pub(crate) fn for_test(path: PathBuf) -> Self {
        SinkPath(path)
    }
}

/// What one call to the sink did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Exit 0: the records were posted.
    Posted,
    /// Exit 2: sending is off at the sink.
    Off,
    /// Exit 3: the sink dropped the records.
    Dropped,
    /// Exit 4: posted, with some records invalid.
    PostedSomeInvalid,
    /// Exit 1, any other code, or a death by a signal koto didn't send: the
    /// call failed in a way the contract gives no meaning to.
    Usage,
    /// The call outlived its deadline and its group was killed, or koto
    /// couldn't wait on it.
    Killed,
    /// The sink couldn't be started, or no longer passed its check.
    SpawnFailed,
}

/// Whether the file whose metadata is `meta` is a sink `uid` may run: a
/// regular file (never a symlink: `meta` comes from `lstat`), owned by
/// `uid`, with an execute bit, and with no write bit for group or others.
/// The owner must be `uid` itself, so a koto running as root refuses a sink
/// another user owns.
pub(crate) fn trusted(meta: &std::fs::Metadata, uid: u32) -> bool {
    let mode = meta.permissions().mode();
    meta.file_type().is_file() && meta.uid() == uid && mode & 0o111 != 0 && mode & 0o022 == 0
}

/// Whether the directory whose metadata is `meta` is one a sink may sit
/// under: a directory owned by `uid` or by root, with no write bit for
/// group or others. A sticky world-writable directory such as `/tmp` fails:
/// its write bit is what matters, not who may delete from it.
pub(crate) fn dir_trusted(meta: &std::fs::Metadata, uid: u32) -> bool {
    let mode = meta.permissions().mode();
    meta.file_type().is_dir() && (meta.uid() == uid || meta.uid() == 0) && mode & 0o022 == 0
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// The sink under `home`, when there is one koto may run.
///
/// `switch` is the value of the switch variable. Nothing here reads the
/// environment, so the checks can be tested against any directory.
fn discover_in(
    home: Option<&std::ffi::OsStr>,
    switch: Option<&std::ffi::OsStr>,
) -> Option<SinkPath> {
    if switch.is_some_and(|v| v == SWITCH_OFF) {
        return None;
    }
    let home = Path::new(home?);
    if !home.is_absolute() {
        return None;
    }
    let uid = euid();
    // Resolve the directory first, so every check below is of the path the
    // sink will be run by, with no symlink left in it to swap.
    let dir = std::fs::canonicalize(home.join(SINK_DIR)).ok()?;
    let mut ancestor = Some(dir.as_path());
    while let Some(d) = ancestor {
        let meta = std::fs::symlink_metadata(d).ok()?;
        if !dir_trusted(&meta, uid) {
            return None;
        }
        ancestor = d.parent();
    }
    let path = dir.join(SINK_FILE);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    trusted(&meta, uid).then_some(SinkPath(path))
}

/// The sink koto may run, found once per process on the first lookup.
/// `None` when there is none, when it fails a check, or when the switch
/// variable says `off`.
#[cfg(not(test))]
pub(crate) fn discover() -> Option<SinkPath> {
    static FOUND: std::sync::OnceLock<Option<SinkPath>> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            discover_in(
                std::env::var_os("HOME").as_deref(),
                std::env::var_os(SWITCH_ENV).as_deref(),
            )
        })
        .clone()
}

// A unit test never looks at the real home: its thread has no sink until it
// installs one.
#[cfg(test)]
thread_local! {
    static TEST_SINK: std::cell::RefCell<Option<SinkPath>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn discover() -> Option<SinkPath> {
    TEST_SINK.with(|s| s.borrow().clone())
}

/// Give this thread a sink, or take it away.
#[cfg(test)]
pub(crate) fn set_sink_for_test(sink: Option<SinkPath>) {
    TEST_SINK.with(|s| *s.borrow_mut() = sink);
}

/// Call the sink once with `lines`, one record each, and `carry` as its
/// `--carry` when it is above zero. The call is ended at `deadline`.
///
/// The sink's own check is repeated first, so a file replaced since
/// discovery is not run. koto reads nothing but the exit status.
pub(crate) fn send(path: &SinkPath, lines: &[&str], carry: u64, deadline: Instant) -> Outcome {
    match std::fs::symlink_metadata(&path.0) {
        Ok(meta) if trusted(&meta, euid()) => {}
        _ => return Outcome::SpawnFailed,
    }
    let mut cmd = Command::new(&path.0);
    cmd.args(["send", "--scope", SCOPE]);
    if carry > 0 {
        cmd.arg("--carry").arg(carry.to_string());
    }
    cmd.env_clear();
    for name in ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(name) {
            cmd.env(name, value);
        }
    }
    cmd.current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // A group of its own, so a timeout can end the sink and whatever
        // it started without touching koto's own group.
        .process_group(0);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => return Outcome::SpawnFailed,
    };
    // The group id is the child's pid, since it leads its own group.
    let pgid = i32::try_from(child.id()).unwrap_or(0);

    // A chunk can be larger than a pipe buffer, so the thread that watches
    // the deadline can't also be the one writing. The writer owns the
    // bytes and the pipe; a sink that stops reading ends it with EPIPE.
    let mut payload = Vec::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
    for line in lines {
        payload.extend_from_slice(line.as_bytes());
        payload.push(b'\n');
    }
    let writer = child.stdin.take().map(|mut stdin| {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&payload);
            // Dropping `stdin` closes the pipe, which is the sink's EOF.
        })
    });

    let wait = deadline.saturating_duration_since(Instant::now());
    let outcome = match child.wait_timeout(wait) {
        Ok(Some(status)) => match status.code() {
            Some(0) => Outcome::Posted,
            Some(2) => Outcome::Off,
            Some(3) => Outcome::Dropped,
            Some(4) => Outcome::PostedSomeInvalid,
            _ => Outcome::Usage,
        },
        // Out of time, or the wait itself failed: end the call.
        Ok(None) | Err(_) => {
            kill_group(pgid);
            let _ = child.wait();
            crate::action::wait_for_group_exit(pgid, GROUP_EXIT_WAIT);
            Outcome::Killed
        }
    };

    // The writer has finished, or will as soon as the pipe's last reader is
    // gone. A sink that exited on time but left a child holding the pipe
    // could block it for good, so it is given a moment and then left: a
    // parked thread costs nothing and ends with the process.
    if let Some(writer) = writer {
        let until = Instant::now() + WRITER_WAIT;
        while !writer.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(1));
        }
        if writer.is_finished() {
            let _ = writer.join();
        }
    }
    outcome
}

/// SIGKILL the group `pgid`, the one the sink was started in. Sent only to
/// a group id above 1: 0 would be koto's own group and 1 is init's.
fn kill_group(pgid: i32) {
    if pgid > 1 {
        // SAFETY: SIGKILL to the process group this module created for the
        // sink it spawned; `pgid` is that child's pid.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn meta(path: &Path) -> std::fs::Metadata {
        std::fs::symlink_metadata(path).unwrap()
    }

    #[test]
    fn trusted_requires_the_owner_a_regular_file_an_execute_bit_and_no_shared_write() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("sink");
        std::fs::write(&file, "x").unwrap();
        let me = euid();
        let with = |mode: u32| {
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            meta(&file)
        };
        assert!(trusted(&with(0o700), me));
        assert!(trusted(&with(0o755), me));
        assert!(trusted(&with(0o500), me));
        // No execute bit.
        assert!(!trusted(&with(0o600), me));
        // Writable by group, or by others.
        assert!(!trusted(&with(0o770), me));
        assert!(!trusted(&with(0o702), me));
        // Owned by someone else: checked through the uid argument, so no
        // chown is needed. A root koto refuses a user's sink too.
        assert!(!trusted(&with(0o700), me.wrapping_add(1)));
        if me != 0 {
            assert!(!trusted(&with(0o700), 0));
        }
        // A directory and a symlink are not regular files.
        assert!(!trusted(&meta(tmp.path()), me));
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!trusted(&meta(&link), me));
    }

    #[test]
    fn dir_trusted_allows_the_user_or_root_and_no_shared_write() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("d");
        std::fs::create_dir(&dir).unwrap();
        let me = euid();
        let with = |mode: u32| {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
            meta(&dir)
        };
        assert!(dir_trusted(&with(0o700), me));
        assert!(dir_trusted(&with(0o755), me));
        assert!(!dir_trusted(&with(0o775), me));
        assert!(!dir_trusted(&with(0o757), me));
        // Sticky and world-writable, as `/tmp` is.
        assert!(!dir_trusted(&with(0o1777), me));
        // Another user's directory, unless that user is root.
        let other = if me == 0 { 1 } else { me.wrapping_add(1) };
        assert!(!dir_trusted(&with(0o755), other));
        // The root directory is root's and passes for anyone.
        if meta(Path::new("/")).permissions().mode() & 0o022 == 0 {
            assert!(dir_trusted(&meta(Path::new("/")), me));
        }
    }

    // The checks on a real layout (a passing sink, a symlink, a writable
    // file or parent, a home under a sticky directory, the switch's other
    // values) are driven through the koto binary in
    // `tests/event_sink_discovery_test.rs`, where the build directory gives
    // a home whose ancestors pass. Here: the answers that need no layout.
    #[test]
    fn discovery_finds_nothing_without_an_absolute_home_or_a_sink() {
        let tmp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        assert_eq!(discover_in(None, None), None);
        assert_eq!(discover_in(Some(OsStr::new("relative/home")), None), None);
        // A home with no sink directory, then with an empty one.
        assert_eq!(discover_in(Some(home.as_os_str()), None), None);
        std::fs::create_dir_all(home.join(SINK_DIR)).unwrap();
        assert_eq!(discover_in(Some(home.as_os_str()), None), None);
        assert_eq!(
            discover_in(Some(home.as_os_str()), Some(OsStr::new("off"))),
            None
        );
    }

    #[test]
    fn send_maps_exit_codes_and_refuses_a_file_that_no_longer_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("sink");
        let sink = SinkPath(file.clone());
        let later = || Instant::now() + Duration::from_secs(20);
        for (code, outcome) in [
            (0, Outcome::Posted),
            (1, Outcome::Usage),
            (2, Outcome::Off),
            (3, Outcome::Dropped),
            (4, Outcome::PostedSomeInvalid),
            (9, Outcome::Usage),
        ] {
            std::fs::write(&file, format!("#!/bin/sh\ncat >/dev/null\nexit {code}\n")).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(send(&sink, &["{}"], 0, later()), outcome, "exit {code}");
        }
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o770)).unwrap();
        assert_eq!(send(&sink, &["{}"], 0, later()), Outcome::SpawnFailed);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(send(&sink, &["{}"], 0, later()), Outcome::SpawnFailed);
    }

    #[test]
    fn send_kills_a_call_that_outlives_its_deadline() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("sink");
        std::fs::write(&file, "#!/bin/sh\ncat >/dev/null\nsleep 30\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        let started = Instant::now();
        let outcome = send(
            &SinkPath(file),
            &["{}"],
            0,
            Instant::now() + Duration::from_millis(300),
        );
        assert_eq!(outcome, Outcome::Killed);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
}
