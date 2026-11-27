//! `koto session rebind <name> --whole`: a takeover of a session by its new
//! holder, the agent session whose anchor `KOTO_ANCHOR` names.
//!
//! One call moves the whole binding a session carries from the agent
//! session that ran it before: the execution anchor (with the fence a plain
//! rebind raises, plus `origin.whole_epoch`, which makes every process that
//! read the session before the move refuse afterwards), the held-by links
//! (the caller's anchor becomes the only one), the `/workflows` publish
//! location (dropped, so the next commit publishes where the caller's
//! Claude Code session reads) and the recorded command environment (the
//! caller's, so later commands run with the new holder's values).
//!
//! The move holds the session's `tick.lock` exclusive from its first read
//! to its last write, so it waits for a tick in flight (`--wait`) and a
//! tick that arrives during it waits for it and then meets the fence. The
//! parts are written in a fixed order, each logged by an event of its own
//! before it is made, so a move stopped part way finishes when run again
//! and no part happens twice: every event carries the epoch the move
//! committed, and a re-run reads only the events after its move's own.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};

use crate::engine::types::{
    now_iso8601, EnvironmentRerecorded, Event, EventPayload, HeldByRebound, PublishLocationDropped,
    StateFileHeader, ValidatedSessionId, WholeRebound,
};
use crate::session::context::ContextStore;
use crate::session::context_log::{append_to_session_best_effort, WRITER_KOTO};
use crate::session::links::{self, LinkHints, LinkKind};
use crate::session::{Backend, SessionBackend};
use crate::workflows_surface::discover::{
    is_explicit_location, EXPLICIT_LOCATION_KEY, PUBLISH_LOCATION_KEY,
};

const COMMAND: &str = "session rebind";

/// How long the move waits for a tick in flight when `--wait` isn't given.
const DEFAULT_WAIT: Duration = Duration::from_secs(10);

/// How often a lock the move waits for is asked for again.
const POLL: Duration = Duration::from_millis(50);

/// The bound on the waits for the session's other locks (its manifest, its
/// log's append lock and `environment.lock`), which are held only for a
/// few writes at a time.
const LOCK_WAIT: Duration = crate::engine::persistence::LOG_REPLACE_LOCK_WAIT;

/// The fault-injection seam for the binary's tests, gated as the import's
/// is: a debug build reads it, a release build never does, so it is not a
/// setting. Unset, nothing changes.
const FAULT_ENV: &str = "KOTO_TEST_REBIND_FAULT";

/// A debug-build seam naming a file the move creates once it holds
/// `tick.lock` exclusive, just before its commit, and then waits on until
/// the file is gone, so a test can start a tick against a move in
/// progress. A release build never reads it.
const HOLD_ENV: &str = "KOTO_TEST_REBIND_HOLD";

/// A debug-build seam moving [`HOLD_ENV`]'s wait from just before the commit
/// to one of the points [`FAULT_ENV`] names, so a test can act while the
/// move's later parts are still to run.
const HOLD_AT_ENV: &str = "KOTO_TEST_REBIND_HOLD_AT";

/// The longest the move waits at [`HOLD_ENV`]'s file.
const HOLD_CAP: Duration = Duration::from_secs(60);

/// Stop at `point`, as a killed process would, when [`FAULT_ENV`] names it,
/// after waiting there when [`HOLD_AT_ENV`] names it.
fn crash_at(point: &str) {
    if !cfg!(debug_assertions) {
        return;
    }
    if std::env::var(HOLD_AT_ENV).as_deref() == Ok(point) {
        hold();
    }
    if std::env::var(FAULT_ENV).as_deref() == Ok(point) {
        eprintln!("{}: stopping at {}", FAULT_ENV, point);
        std::process::exit(86);
    }
}

/// [`hold`] just before the commit, unless [`HOLD_AT_ENV`] moved it.
fn hold_point() {
    if cfg!(debug_assertions) && std::env::var_os(HOLD_AT_ENV).is_none() {
        hold();
    }
}

/// Create [`HOLD_ENV`]'s file and wait until it is gone, in a debug build
/// with the variable set.
fn hold() {
    if !cfg!(debug_assertions) {
        return;
    }
    let Some(path) = std::env::var_os(HOLD_ENV).map(PathBuf::from) else {
        return;
    };
    if std::fs::write(&path, b"").is_err() {
        return;
    }
    let started = Instant::now();
    while path.exists() && started.elapsed() < HOLD_CAP {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Refuse with `{"error": {"code", "message"}, "command"}` on stdout.
fn refuse(code: &str, message: String, exit: i32) -> ! {
    super::exit_with_error_code(
        serde_json::json!({
            "error": {"code": code, "message": message},
            "command": COMMAND,
        }),
        exit,
    )
}

/// Run `koto session rebind <name> --whole [--to <dir>] [--wait <secs>]`.
///
/// The checks come before any lock or write, in the order the refusals are
/// documented: the name, a call from inside the session's own tick
/// (`nested_invocation`), the caller's anchor (`no_anchor`), the target
/// directory, the plain rebind's guard, a migrated source, and that the
/// session exists.
pub(super) fn handle(
    backend: &Backend,
    name: &str,
    to: Option<&str>,
    wait: Option<u64>,
) -> Result<()> {
    let name = ValidatedSessionId::new(name)?.into_inner();
    let name = name.as_str();

    if let Err(message) = crate::engine::reentrancy::refuse_if_own_tick(name, COMMAND) {
        refuse("nested_invocation", message, super::EXIT_CALLER_ERROR);
    }

    let holder = holder_anchor(backend, name);

    let target = canonical_target(name, to)?;

    if let Err(e) = super::session::rebind_guard(backend, name, to) {
        super::refuse_if_lookup_failed(&e, COMMAND);
        super::exit_with_error_code(
            serde_json::json!({"error": e.to_string(), "command": COMMAND}),
            super::EXIT_CALLER_ERROR,
        );
    }
    if let Err(e) = backend.check_not_migrated_to_write(name) {
        if e.is::<crate::session::SessionMigrated>() {
            super::exit_with_error_code(
                serde_json::json!({"error": e.to_string(), "command": COMMAND}),
                super::EXIT_CALLER_ERROR,
            );
        }
    }
    if !backend.exists(name) {
        anyhow::bail!("session '{}' does not exist", name);
    }

    let wait = wait.map_or(DEFAULT_WAIT, Duration::from_secs);
    let report = match backend {
        Backend::Local(local) => move_local(backend, local, name, &holder, &target, wait)?,
        Backend::Cloud(cloud) => move_cloud(backend, cloud, name, &holder, &target, wait)?,
    };
    println!("{}", serde_json::to_string(&report.to_json(name, &target))?);
    Ok(())
}

/// The anchor `KOTO_ANCHOR` names, refused with `no_anchor` unless it is an
/// anchor in this store other than `name` itself: the move hands the session
/// to that anchor's agent session, so with no such anchor it has no holder
/// to hand it to.
fn holder_anchor(backend: &Backend, name: &str) -> String {
    let Some(anchor) = crate::session::anchor::anchor_from_env() else {
        no_anchor(name, "KOTO_ANCHOR is unset or isn't a session name");
    };
    if anchor == name {
        no_anchor(name, &format!("KOTO_ANCHOR names '{name}' itself"));
    }
    if !crate::session::anchor::is_anchor(backend, &anchor) {
        no_anchor(
            name,
            &format!("KOTO_ANCHOR names '{anchor}', which isn't an anchor on this machine"),
        );
    }
    anchor
}

/// Refuse the move of `name` with `no_anchor`, saying `why`.
fn no_anchor(name: &str, why: &str) -> ! {
    refuse(
        "no_anchor",
        format!(
            "koto session rebind --whole hands '{name}' to its new holder, the anchor \
             KOTO_ANCHOR names, and {why}; nothing was written. Run it from the agent session \
             taking the session over, whose shells carry its KOTO_ANCHOR"
        ),
        super::EXIT_CALLER_ERROR,
    )
}

/// `to`, or the current directory, canonical, refused with the plain
/// rebind's messages when it isn't a directory on this machine.
fn canonical_target(name: &str, to: Option<&str>) -> Result<PathBuf> {
    let requested = match to {
        Some(dir) => PathBuf::from(dir),
        None => std::env::current_dir()
            .context("--to was omitted and the current directory could not be read")?,
    };
    let target = std::fs::canonicalize(&requested).with_context(|| {
        format!(
            "cannot rebind '{}' to {}: no such directory on this machine",
            name,
            requested.display()
        )
    })?;
    if !target.is_dir() {
        anyhow::bail!(
            "cannot rebind '{}' to {}: not a directory",
            name,
            target.display()
        );
    }
    Ok(target)
}

/// The move on the cloud backend, under `tick.lock` held exclusive for all
/// of it, as the local move is, with three differences.
///
/// The read is a pull, and the machine that pushed the session last must be
/// this one (`rebind_cross_host`): a move made over another host's writes
/// would fence that host off without its knowing, and moving a session
/// between machines is what `koto session import` is for.
///
/// The commit goes to the bucket first: the log with the move's event and
/// header, PUT on the ETag the pull left, and adopted here only once the
/// bucket took it. A PUT the bucket refuses because its copy moved since
/// the pull writes nothing anywhere ([`settle_lost_commit`]).
///
/// After the parts, `version.json` goes up one above the version the move
/// read, as this machine's, so a host that pushed before the move refuses
/// its next flush as `session_rebound` before any of its writes go up. The
/// context is then carried to the session's home when the move changed it,
/// as a plain rebind carries it.
fn move_cloud(
    backend: &Backend,
    cloud: &crate::session::cloud::CloudBackend,
    name: &str,
    holder: &str,
    target: &Path,
    wait: Duration,
) -> Result<Report> {
    let local = cloud.local();
    let dir = backend.session_dir(name);
    // A session this host holds no copy of has no directory for its lock
    // yet; the pull below brings the log into it.
    if !dir.is_dir() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let (_tick_lock, waited) = take_tick_lock(name, &dir, wait)?;
    let mut report = Report {
        waited,
        ..Report::default()
    };
    let mut home_before = cloud.home_prefix(name).ok().map(|home| home.prefix);

    let (header, events) = backend.read_events(name)?;
    let version_before = refuse_cross_host(cloud, name)?;
    refuse_run_in_flight(name, &dir);

    let from = header.execution_dir.clone();
    let before_epoch = header.origin.as_ref().map(|o| o.anchor_epoch);
    let mut commit = commit_state(&header, &events, target);
    let epoch = match (&commit, before_epoch) {
        (Commit::Done, e) => e,
        (_, e) => e.map(|e| e + 1),
    };
    // Run again past the commit, the header is moved: the logged `from` gives the home before.
    let logged_from = events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::ExecutionAnchorReboundWhole(w) if w.epoch == epoch => Some(w.from.clone()),
        _ => None,
    });
    // One that logged no `from` names no home before, which the carry treats as moved.
    if let (Commit::Done, Some(None)) = (&commit, &logged_from) {
        home_before = None;
    }
    if let (Commit::Done, Some(Some(from))) = (&commit, logged_from) {
        home_before = cloud.home_prefix_from(name, &from).ok().map(|h| h.prefix);
    }
    let state_path = local.state_path(name);

    crate::workflows_surface::materialize::without_materializing(|| -> Result<()> {
        if !matches!(commit, Commit::Done) {
            hold_point();
            // As on the local path: the lift ends with this block, and a
            // landed commit is recorded by `note_mover` below.
            let _lift = header
                .origin
                .as_ref()
                .filter(|o| o.whole_epoch > 0)
                .map(|o| crate::session::fence_state::note_taking_over(&state_path, o.whole_epoch));
            cloud.check_fence(name)?;
            if !commit_remote(cloud, name, &state_path, &header, &commit, target, epoch)? {
                settle_lost_commit(backend, name, &header, target)?;
                commit = Commit::Done;
            }
            report.rebound = true;
        }
        if let Some(epoch) = epoch {
            cloud.note_mover(name, epoch);
        }
        crash_at("after-commit");

        let (_, events) = backend.read_events(name)?;
        let since = since_move(&events, epoch);
        let logged_epoch = epoch.unwrap_or(0);

        move_held_by(
            backend,
            local,
            name,
            holder,
            since,
            logged_epoch,
            &mut report,
        )?;
        drop_publish_location(backend, name, since, logged_epoch, &mut report)?;
        rerecord_environment(backend, name, since, logged_epoch, &mut report)?;
        Ok(())
    })?;

    cloud.push_moved_version(name, version_before)?;
    crash_at("after-version");
    cloud.carry_context_after_rebind(name, home_before.as_deref());

    report.from = if report.rebound {
        from
    } else {
        Some(target.to_path_buf())
    };
    Ok(report)
}

/// Refuse the move of `name` with `rebind_cross_host`, exit 2, when the
/// bucket's `version.json` was last written by another machine; otherwise
/// the version it records (`None` when there is none). Asked under
/// `tick.lock`, after the pull and before the commit.
fn refuse_cross_host(
    cloud: &crate::session::cloud::CloudBackend,
    name: &str,
) -> Result<Option<u64>> {
    let Some(remote) = cloud.remote_version(name)? else {
        return Ok(None);
    };
    let here = crate::session::version::get_or_create_machine_id()?;
    if remote.machine_id != here {
        refuse(
            "rebind_cross_host",
            format!(
                "session '{name}' was last pushed from another machine ({}), not this one ({here}); \
                 nothing was written. koto session rebind --whole moves a session only on the \
                 machine that pushed it last. To take it over on this machine, run `koto session \
                 import {name}` here, then move the copy it makes",
                remote.machine_id
            ),
            super::EXIT_CALLER_ERROR,
        );
    }
    Ok(Some(remote.version))
}

/// The first half and the local half of the cloud commit: the log as the
/// move leaves it, built from this host's copy (the event appended after
/// every line kept as it is, and the header rewritten), PUT to the bucket
/// on the ETag the pull left, then adopted here. `false` when the bucket's
/// copy moved since the pull and nothing was written.
fn commit_remote(
    cloud: &crate::session::cloud::CloudBackend,
    name: &str,
    state_path: &Path,
    header: &StateFileHeader,
    commit: &Commit,
    target: &Path,
    epoch: Option<u64>,
) -> Result<bool> {
    use crate::session::cloud::MovedPut;

    let base = std::fs::read(state_path)
        .with_context(|| format!("failed to read {}", state_path.display()))?;
    let payload = matches!(commit, Commit::Needed).then(|| {
        EventPayload::ExecutionAnchorReboundWhole(WholeRebound {
            from: header
                .execution_dir
                .clone()
                .or_else(|| header.origin.as_ref().map(|o| o.anchor.clone())),
            to: target.to_path_buf(),
            whole: true,
            epoch,
        })
    });
    let (moved, seq) = moved_log(name, &base, header, payload.as_ref(), target)?;
    let put = cloud.put_moved_state(name, &moved)?;
    if matches!(put, MovedPut::Lost) {
        return Ok(false);
    }
    crash_at("after-remote-commit");
    cloud.adopt_moved_state(name, &moved, &put)?;
    if let (Some(payload), Some(seq)) = (payload, seq) {
        let info = crate::engine::persistence::CommitInfo {
            seq,
            prior_log: String::from_utf8_lossy(&base).into_owned(),
        };
        cloud.local().after_commit(name, &payload, &info);
    }
    Ok(true)
}

/// After the bucket refused the move's PUT because its copy moved since the
/// pull: pull again and say what moved it. The move's own commit, which
/// landed with its reply lost, is the move done. A rebind elsewhere past
/// the epoch the move read is `session_rebound`. Anything else is a write
/// the move didn't see, and it refuses with nothing written; run again, it
/// starts from the copy that holds that write.
fn settle_lost_commit(
    backend: &Backend,
    name: &str,
    read: &StateFileHeader,
    target: &Path,
) -> Result<()> {
    let (now, events) = backend.read_events(name)?;
    if matches!(commit_state(&now, &events, target), Commit::Done) {
        return Ok(());
    }
    let epoch = |h: &StateFileHeader| h.origin.as_ref().map(|o| o.anchor_epoch);
    if epoch(&now) > epoch(read) {
        return Err(crate::session::rebind_fence::SessionRebound {
            name: name.to_string(),
            anchor: now.execution_dir.as_ref().map(|d| d.display().to_string()),
            from_retired: false,
        }
        .into());
    }
    anyhow::bail!(
        "the bucket's copy of session '{name}' changed after the move read it, so the move \
         wrote nothing; run `koto session rebind {name} --whole` again"
    )
}

/// The log `base` as the move leaves it: its header rewritten
/// ([`move_header`]) and, with `payload`, the move's event appended at the
/// next `seq`, which is returned. Every other line is kept byte for byte.
/// Refused when `base`'s header isn't the one the move read (`read`): the
/// log changed under it.
fn moved_log(
    name: &str,
    base: &[u8],
    read: &StateFileHeader,
    payload: Option<&EventPayload>,
    target: &Path,
) -> Result<(Vec<u8>, Option<u64>)> {
    let text =
        std::str::from_utf8(base).with_context(|| format!("the log of '{name}' isn't UTF-8"))?;
    let mut lines = text.lines();
    let first = lines
        .next()
        .with_context(|| format!("the log of '{name}' is empty"))?;
    let now: StateFileHeader = serde_json::from_str(first)
        .with_context(|| format!("failed to parse the header of '{name}'"))?;
    anyhow::ensure!(
        now.execution_dir == read.execution_dir
            && now.origin.as_ref().map(|o| o.anchor_epoch)
                == read.origin.as_ref().map(|o| o.anchor_epoch),
        "session '{name}' was rebound while the move read it; nothing was written. Run the move \
         again"
    );
    let mut out = crate::engine::persistence::serialize_header_line(&move_header(now, target))?;
    out.push('\n');
    let mut last_seq = 0;
    for line in lines {
        if let Some(seq) = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("seq").and_then(serde_json::Value::as_u64))
        {
            last_seq = last_seq.max(seq);
        }
        out.push_str(line);
        out.push('\n');
    }
    let seq = match payload {
        Some(payload) => {
            let event = Event {
                seq: last_seq + 1,
                timestamp: now_iso8601(),
                event_type: payload.type_name().to_string(),
                payload: payload.clone(),
                idempotency_hash: None,
            };
            out.push_str(&serde_json::to_string(&event).context("failed to serialize the event")?);
            out.push('\n');
            Some(last_seq + 1)
        }
        None => None,
    };
    Ok((out.into_bytes(), seq))
}

/// `h` as a whole-binding move to `target` leaves it: bound to `target`,
/// whose anchor it records, the epoch one up with the anchor left retired,
/// and that epoch as `whole_epoch`. A header with no origin gets the new
/// `execution_dir` alone.
fn move_header(mut h: StateFileHeader, target: &Path) -> StateFileHeader {
    let from = h.execution_dir.clone();
    if let Some(origin) = h.origin.as_mut() {
        let left = from.unwrap_or_else(|| origin.anchor.clone());
        origin.anchor = target.to_path_buf();
        crate::session::rebind_fence::advance(origin, Some(&left), target);
        origin.whole_epoch = origin.anchor_epoch;
    }
    h.execution_dir = Some(target.to_path_buf());
    h
}

/// Refuse with `background_run_in_flight`, exit 2, while a run of `name`
/// is in flight: it runs its command in the old anchor and keeps its locks
/// in the session's directory, so the move waits until it has ended, as a
/// plain rebind does.
fn refuse_run_in_flight(name: &str, dir: &Path) {
    #[cfg(unix)]
    if let Some(run_id) = crate::engine::background::run_in_flight(dir) {
        super::exit_with_error_code(
            serde_json::json!({
                "error": {
                    "code": "background_run_in_flight",
                    "message": crate::engine::background::in_flight_refusal(name, &run_id),
                    "run_id": run_id,
                },
                "command": COMMAND,
            }),
            super::EXIT_CALLER_ERROR,
        );
    }
    #[cfg(not(unix))]
    let _ = (name, dir);
}

/// What a move did, for its output.
#[derive(Default)]
struct Report {
    rebound: bool,
    from: Option<PathBuf>,
    waited: bool,
    added: Option<String>,
    removed: Vec<String>,
    dropped: bool,
    rerecorded: bool,
    changed: Vec<String>,
}

impl Report {
    fn to_json(&self, name: &str, target: &Path) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "rebound": self.rebound,
            "from": self.from,
            "to": target,
            "whole": true,
            "waited": self.waited,
            "held_by": {"added": self.added, "removed": self.removed},
            "publish_location": {"dropped": self.dropped},
            "environment": {"rerecorded": self.rerecorded, "changed": self.changed},
        })
    }
}

/// Where a session's log stands against a move to `target`.
enum Commit {
    /// The header records the move: nothing to commit.
    Done,
    /// The move's event is in the log and the header wasn't rewritten (a
    /// move stopped between the two): finish the header only.
    Torn,
    /// Neither: append the event, then rewrite the header.
    Needed,
}

/// The move on the local backend, under `tick.lock` held exclusive for all
/// of it.
fn move_local(
    backend: &Backend,
    local: &crate::session::local::LocalBackend,
    name: &str,
    holder: &str,
    target: &Path,
    wait: Duration,
) -> Result<Report> {
    let dir = backend.session_dir(name);
    let (_tick_lock, waited) = take_tick_lock(name, &dir, wait)?;
    let mut report = Report {
        waited,
        ..Report::default()
    };

    let (header, events) = backend.read_events(name)?;
    refuse_run_in_flight(name, &dir);

    let from = header.execution_dir.clone();
    let before_epoch = header.origin.as_ref().map(|o| o.anchor_epoch);
    let commit = commit_state(&header, &events, target);
    // The epoch the move commits, `None` for a session with no origin,
    // which has no fence to raise.
    let epoch = match (&commit, before_epoch) {
        (Commit::Done, e) => e,
        (_, e) => e.map(|e| e + 1),
    };
    let state_path = local.state_path(name);

    crate::workflows_surface::materialize::without_materializing(|| -> Result<()> {
        if !matches!(commit, Commit::Done) {
            hold_point();
            // A move back into a directory an earlier move retired runs
            // from there; its own commit must pass the path rule it is
            // about to lift. The lift ends with this block, refused commit
            // or not; a landed one is recorded by `note_mover` below.
            let _lift = header
                .origin
                .as_ref()
                .filter(|o| o.whole_epoch > 0)
                .map(|o| crate::session::fence_state::note_taking_over(&state_path, o.whole_epoch));
            commit_move(local, name, &state_path, &header, &commit, target, epoch)?;
            report.rebound = true;
        }
        if let Some(epoch) = epoch {
            crate::session::fence_state::note_mover(&state_path, epoch);
        }
        crash_at("after-commit");

        let (_, events) = backend.read_events(name)?;
        let since = since_move(&events, epoch);
        let logged_epoch = epoch.unwrap_or(0);

        move_held_by(
            backend,
            local,
            name,
            holder,
            since,
            logged_epoch,
            &mut report,
        )?;
        drop_publish_location(backend, name, since, logged_epoch, &mut report)?;
        rerecord_environment(backend, name, since, logged_epoch, &mut report)?;
        Ok(())
    })?;

    report.from = if report.rebound {
        from
    } else {
        Some(target.to_path_buf())
    };
    Ok(report)
}

/// Whether the log already records the move to `target`.
///
/// Done when the header binds the session to `target` and its last
/// whole-binding move wrote the current epoch (a plain rebind since then
/// bumps `anchor_epoch` alone). Torn when the log's last rebound event is a
/// whole move to `target` at the epoch above the header's: the event went
/// in and the header rewrite didn't. A session with no origin, which has no
/// epochs, is done when it is bound to `target` and its last rebound event
/// is a whole move there.
fn commit_state(header: &StateFileHeader, events: &[Event], target: &Path) -> Commit {
    let last_rebound = events.iter().rev().find_map(|e| match &e.payload {
        EventPayload::ExecutionAnchorRebound { .. } => Some(None),
        EventPayload::ExecutionAnchorReboundWhole(w) => Some(Some(w)),
        _ => None,
    });
    let last_whole = last_rebound.flatten().filter(|w| w.to == target);
    let bound_here = header.execution_dir.as_deref() == Some(target);
    match header.origin.as_ref() {
        Some(o) => {
            if bound_here
                && o.anchor == target
                && o.whole_epoch > 0
                && o.whole_epoch == o.anchor_epoch
            {
                Commit::Done
            } else if last_whole.is_some_and(|w| w.epoch == Some(o.anchor_epoch + 1)) {
                Commit::Torn
            } else {
                Commit::Needed
            }
        }
        None => match last_whole {
            Some(_) if bound_here => Commit::Done,
            Some(_) => Commit::Torn,
            None => Commit::Needed,
        },
    }
}

/// The events after the move's own `execution_anchor_rebound` (the last
/// whole-binding one at `epoch`), where the parts it owes are logged; all
/// of them when the log holds no such event.
fn since_move(events: &[Event], epoch: Option<u64>) -> &[Event] {
    let at = events.iter().rposition(
        |e| matches!(&e.payload, EventPayload::ExecutionAnchorReboundWhole(w) if w.epoch == epoch),
    );
    match at {
        Some(i) => &events[i + 1..],
        None => events,
    }
}

/// The commit: the move's event, then the header, under the session's
/// manifest lock and its log's append lock held across both, so no context
/// write checks the fence between them. The header records the target as
/// the anchor, the epoch one up with the anchor left retired, and that
/// epoch as `whole_epoch`. A torn commit gets the header alone.
fn commit_move(
    local: &crate::session::local::LocalBackend,
    name: &str,
    state_path: &Path,
    header: &StateFileHeader,
    commit: &Commit,
    target: &Path,
    epoch: Option<u64>,
) -> Result<()> {
    use crate::engine::persistence;

    let from = header
        .execution_dir
        .clone()
        .or_else(|| header.origin.as_ref().map(|o| o.anchor.clone()));
    let payload = EventPayload::ExecutionAnchorReboundWhole(WholeRebound {
        from: from.clone(),
        to: target.to_path_buf(),
        whole: true,
        epoch,
    });
    let started = Instant::now();
    let committed = local.with_manifest_lock_within(name, LOCK_WAIT, || {
        let held =
            persistence::hold_log_lock(state_path, LOCK_WAIT.saturating_sub(started.elapsed()))?;
        // Read again under the lock: a plain rebind takes no tick.lock, and
        // one that landed since the move's read would leave the event
        // naming the wrong epoch.
        let now = persistence::read_header(state_path)?;
        anyhow::ensure!(
            now.execution_dir == header.execution_dir
                && now.origin.as_ref().map(|o| o.anchor_epoch)
                    == header.origin.as_ref().map(|o| o.anchor_epoch),
            "session '{name}' was rebound while the move read it; nothing was written. Run the \
             move again"
        );
        let info = match commit {
            Commit::Needed => {
                let info = persistence::append_event_tracked_held(
                    &held,
                    state_path,
                    &payload,
                    &now_iso8601(),
                )?;
                crash_at("after-move-event");
                Some(info)
            }
            _ => None,
        };
        let target = target.to_path_buf();
        crate::engine::claim::rewrite_header_atomically_held(&held, state_path, |h| {
            move_header(h, &target)
        })?;
        drop(held);
        Ok(info)
    })?;
    let Some(info) = committed else {
        return Err(
            persistence::StateLogBusy::new(state_path.to_path_buf(), started.elapsed()).into(),
        );
    };
    // Every lock of the session is released: the after-commit steps take
    // them again.
    if let Some(info) = info {
        local.after_commit(name, &payload, &info);
    }
    Ok(())
}

/// Make the caller's anchor `holder` the session's only held-by link.
///
/// The change is logged first, as `held_by_rebound`, unless the log
/// already holds one for this move: a move stopped after logging it
/// completes the removals it owes without logging them twice.
fn move_held_by(
    backend: &Backend,
    local: &crate::session::local::LocalBackend,
    name: &str,
    holder: &str,
    since: &[Event],
    epoch: u64,
    report: &mut Report,
) -> Result<()> {
    let current = held_by_peers(backend, local, name)?;
    let added = (!current.iter().any(|p| p == holder)).then(|| holder.to_string());
    let removed: Vec<String> = current.into_iter().filter(|p| p != holder).collect();
    if added.is_none() && removed.is_empty() {
        return Ok(());
    }
    let logged = since
        .iter()
        .any(|e| matches!(&e.payload, EventPayload::HeldByRebound(h) if h.epoch == epoch));
    if !logged {
        backend.append_event(
            name,
            &EventPayload::HeldByRebound(HeldByRebound {
                epoch,
                added: added.clone(),
                removed: removed.clone(),
            }),
            &now_iso8601(),
        )?;
        crash_at("after-held-by-event");
    }
    if added.is_some() {
        let hints = LinkHints::for_peer(backend, holder);
        links::write_link(backend, name, LinkKind::HeldBy, holder, &hints)?;
    }
    for peer in &removed {
        remove_key(backend, name, &links::link_key(LinkKind::HeldBy, peer))?;
    }
    crash_at("after-held-by");
    report.added = added;
    report.removed = removed;
    Ok(())
}

/// The anchors `name` holds held-by links to, sorted: the keys the store
/// lists, and any link file under `ctx/` it doesn't (lock files, temp files
/// and names that aren't anchor names aside).
fn held_by_peers(
    backend: &Backend,
    local: &crate::session::local::LocalBackend,
    name: &str,
) -> Result<Vec<String>> {
    let mut peers: Vec<String> = links::read_links(backend, name)?
        .into_iter()
        .filter(|l| l.kind == LinkKind::HeldBy)
        .map(|l| l.peer)
        .collect();
    let prefix = links::link_key(LinkKind::HeldBy, "");
    let dir = local
        .session_dir(name)
        .join("ctx")
        .join(prefix.trim_end_matches('/'));
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if file.starts_with('.')
                || file.ends_with(".lock")
                || !entry.file_type().is_ok_and(|t| t.is_file())
                || links::validate_peer(LinkKind::HeldBy, &file).is_err()
            {
                continue;
            }
            peers.push(file);
        }
    }
    peers.sort();
    peers.dedup();
    Ok(peers)
}

/// Remove koto's own key `key` of `name` and log the removal, best-effort,
/// as `koto context remove` logs an agent's.
fn remove_key(backend: &Backend, name: &str, key: &str) -> Result<()> {
    backend.remove_with_writer(name, key, WRITER_KOTO)?;
    append_to_session_best_effort(
        backend,
        name,
        &EventPayload::ContextRemoved {
            key: key.to_string(),
            writer: Some(WRITER_KOTO.to_string()),
        },
    );
    Ok(())
}

/// Drop the session's stored publish location, and its explicit mark, so
/// the next commit resolves the location afresh: `KOTO_WORKFLOWS_DIR`, or
/// the folder of the Claude Code session driving it.
///
/// Logged first as `publish_location_dropped`. A re-run that finds the
/// drop logged removes the location only while it still holds the value
/// logged, so a location published after the move stays.
fn drop_publish_location(
    backend: &Backend,
    name: &str,
    since: &[Event],
    epoch: u64,
    report: &mut Report,
) -> Result<()> {
    if !backend.ctx_exists(name, PUBLISH_LOCATION_KEY) {
        return Ok(());
    }
    let was = String::from_utf8_lossy(&backend.get(name, PUBLISH_LOCATION_KEY)?).into_owned();
    let logged = since.iter().find_map(|e| match &e.payload {
        EventPayload::PublishLocationDropped(p) if p.epoch == epoch => Some(p.was.clone()),
        _ => None,
    });
    match logged {
        Some(logged) if logged != was => return Ok(()),
        Some(_) => {}
        None => {
            let explicit = is_explicit_location(backend, name, Path::new(&was));
            backend.append_event(
                name,
                &EventPayload::PublishLocationDropped(PublishLocationDropped {
                    epoch,
                    was: was.clone(),
                    explicit,
                }),
                &now_iso8601(),
            )?;
            crash_at("after-publish-event");
        }
    }
    // The mark first: a location left without its mark is still dropped
    // by a re-run, and a mark left without its location has no effect.
    if backend.ctx_exists(name, EXPLICIT_LOCATION_KEY) {
        remove_key(backend, name, EXPLICIT_LOCATION_KEY)?;
    }
    remove_key(backend, name, PUBLISH_LOCATION_KEY)?;
    crash_at("after-publish");
    report.dropped = true;
    Ok(())
}

/// Record the caller's command environment in place of the session's, so
/// the commands of every later tick run with the new holder's `PATH`,
/// `HOME` and `XDG_CONFIG_HOME`. The legacy flag is kept as recorded. A
/// session with no record yet is left to adopt one on its next tick.
///
/// Logged first as `environment_rerecorded`, with the new record, the
/// `PATH` entries recording dropped and the variables that changed, under
/// the session's `environment.lock`, which a tick adopting a record takes.
fn rerecord_environment(
    backend: &Backend,
    name: &str,
    since: &[Event],
    epoch: u64,
    report: &mut Report,
) -> Result<()> {
    let Some(old) = backend.read_header(name)?.command_environment else {
        return Ok(());
    };
    let (record, recording) = crate::engine::command_env::record_from_process(old.legacy);
    if record == old {
        return Ok(());
    }
    let lock_path = backend.session_dir(name).join("environment.lock");
    let _lock = lock_file_within(&lock_path, LOCK_WAIT)?.with_context(|| {
        format!(
            "the command environment of '{name}' was still locked by another koto command after \
             {} s; the move's other parts are done. Run the move again",
            LOCK_WAIT.as_secs()
        )
    })?;
    let changed: Vec<String> = crate::engine::command_env::drift(&old, |n| std::env::var(n).ok())
        .into_iter()
        .map(str::to_string)
        .collect();
    let logged = since.iter().any(|e| {
        matches!(&e.payload, EventPayload::EnvironmentRerecorded(r)
            if r.epoch == epoch && r.environment == record)
    });
    if !logged {
        backend.append_event(
            name,
            &EventPayload::EnvironmentRerecorded(EnvironmentRerecorded {
                epoch,
                environment: record.clone(),
                dropped: recording.dropped_path_entries.clone(),
                changed: changed.clone(),
            }),
            &now_iso8601(),
        )?;
        crash_at("after-environment-event");
    }
    backend.rewrite_header(name, &|mut h| {
        h.command_environment = Some(record.clone());
        h
    })?;
    crash_at("after-environment");
    report.rerecorded = true;
    report.changed = changed;
    Ok(())
}

/// Take `name`'s `tick.lock` exclusive, asking every [`POLL`] until `wait`
/// has passed, and say whether it had to wait. Past `wait` the move refuses
/// with `session_busy`, exit 1, having written nothing. A lock file a
/// same-host import replaced while the move waited is taken again at its
/// path, in what is left of the same wait.
#[cfg(unix)]
fn take_tick_lock(name: &str, dir: &Path, wait: Duration) -> Result<(std::fs::File, bool)> {
    let path = dir.join(crate::session::TICK_LOCK_FILE);
    let started = Instant::now();
    let mut waited = false;
    loop {
        let Some(file) = crate::session::tick_lock::open(dir)
            .with_context(|| format!("failed to open {}", path.display()))?
        else {
            anyhow::bail!("session '{}' does not exist", name);
        };
        match flock_within(&file, wait, started)? {
            Some(w) => waited |= w,
            None => refuse(
                "session_busy",
                format!(
                    "session '{name}' is busy: a tick of it still held {} after {} s; nothing \
                     was written. Run `koto session rebind {name} --whole` again once the tick \
                     returns, or give --wait more seconds",
                    path.display(),
                    wait.as_secs()
                ),
                1,
            ),
        }
        if crate::session::tick_lock::still_current(&file, &path) {
            return Ok((file, waited));
        }
    }
}

#[cfg(not(unix))]
fn take_tick_lock(_name: &str, _dir: &Path, _wait: Duration) -> Result<(std::fs::File, bool)> {
    anyhow::bail!("koto session rebind --whole needs flock, which this platform lacks")
}

/// Open `path` (created when missing) and lock it exclusive within `wait`:
/// `None` when it was still held elsewhere then.
#[cfg(unix)]
fn lock_file_within(path: &Path, wait: Duration) -> Result<Option<std::fs::File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    Ok(flock_within(&file, wait, Instant::now())?.map(|_| file))
}

#[cfg(not(unix))]
fn lock_file_within(path: &Path, _wait: Duration) -> Result<Option<std::fs::File>> {
    Ok(Some(
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .with_context(|| format!("failed to open {}", path.display()))?,
    ))
}

/// Ask for `file`'s exclusive `flock` every [`POLL`] until `wait` has
/// passed since `started`: `Some(waited)` once held, where `waited` says
/// whether the first ask failed, and `None` when it was still held
/// elsewhere then. A `wait` of zero asks once.
#[cfg(unix)]
fn flock_within(file: &std::fs::File, wait: Duration, started: Instant) -> Result<Option<bool>> {
    use std::os::unix::io::AsRawFd;
    let mut waited = false;
    loop {
        // SAFETY: flock on a descriptor `file` owns, for the duration of the
        // call; it neither closes nor keeps the descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(waited));
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(anyhow::anyhow!("failed to lock: {err}"));
        }
        waited = true;
        let elapsed = started.elapsed();
        if elapsed >= wait {
            return Ok(None);
        }
        std::thread::sleep(POLL.min(wait - elapsed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{SessionOrigin, SessionStoreIdentity};

    fn header(dir: &str, origin: Option<(u64, u64)>) -> StateFileHeader {
        let mut h: StateFileHeader =
            serde_json::from_str(r#"{"workflow":"wf","template_hash":"h","created_at":"t"}"#)
                .unwrap();
        h.execution_dir = Some(PathBuf::from(dir));
        h.origin = origin.map(|(anchor_epoch, whole_epoch)| SessionOrigin {
            anchor: PathBuf::from(dir),
            store: SessionStoreIdentity {
                kind: "local".into(),
                base: PathBuf::from("/s"),
                bucket: None,
                prefix: None,
            },
            anchor_epoch,
            retired_anchors: vec![],
            whole_epoch,
        });
        h
    }

    fn whole(seq: u64, to: &str, epoch: Option<u64>) -> Event {
        Event {
            seq,
            timestamp: "t".into(),
            event_type: "execution_anchor_rebound".into(),
            payload: EventPayload::ExecutionAnchorReboundWhole(WholeRebound {
                from: Some(PathBuf::from("/old")),
                to: PathBuf::from(to),
                whole: true,
                epoch,
            }),
            idempotency_hash: None,
        }
    }

    #[test]
    fn a_header_bound_by_a_whole_move_at_its_epoch_is_done() {
        let h = header("/new", Some((1, 1)));
        assert!(matches!(
            commit_state(&h, &[], Path::new("/new")),
            Commit::Done
        ));
        // A plain rebind since bumped the epoch alone: a new move is owed.
        let h = header("/new", Some((2, 1)));
        assert!(matches!(
            commit_state(&h, &[], Path::new("/new")),
            Commit::Needed
        ));
    }

    #[test]
    fn a_logged_move_the_header_lacks_is_torn() {
        let h = header("/old", Some((0, 0)));
        let events = [whole(3, "/new", Some(1))];
        assert!(matches!(
            commit_state(&h, &events, Path::new("/new")),
            Commit::Torn
        ));
        // Logged to somewhere else: a move of its own is owed.
        assert!(matches!(
            commit_state(&h, &events, Path::new("/other")),
            Commit::Needed
        ));
    }

    #[test]
    fn the_parts_are_read_after_the_move_s_own_event() {
        let events = [
            whole(1, "/a", Some(1)),
            whole(2, "/b", Some(2)),
            whole(3, "/c", Some(3)),
        ];
        assert_eq!(since_move(&events, Some(2)).len(), 1);
        assert_eq!(since_move(&events, Some(9)).len(), 3);
    }
}
