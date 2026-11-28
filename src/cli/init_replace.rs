//! `koto init NEW --template T --replaces OLD [--wait SECS | --abandon-tick]`:
//! open NEW as a fresh run that restarts OLD's work, and close OLD so
//! nothing drives it again.
//!
//! The command runs in this order:
//!
//! 1. **Check.** Every check that can refuse runs before anything is
//!    written: the names, a call from inside OLD's own tick or background
//!    action (`nested_invocation`), OLD existing and being a run, OLD not
//!    being NEW (`self`), OLD's fence (`superseded_by`, `session_replaced`),
//!    a whole-binding move of OLD that fences this process off
//!    (`session_rebound`), NEW not being some other session
//!    (`session_exists`), and for a NEW still to be created, the template's
//!    compile and its variables.
//! 2. **Lock.** OLD's `tick.lock` is taken exclusive, waited for at most
//!    `--wait` seconds (10 by default). Still held then refuses
//!    `session_busy`, exit 1, nothing written. With `--abandon-tick` the
//!    lock is asked for once: when it is held, the close goes on without it
//!    and the output reports `tick_in_flight: true`.
//! 3. **Stop.** OLD's background runner is stopped and its pool queues
//!    left, as `koto cancel` does; the events that record it go into the
//!    close.
//! 4. **Create.** NEW is created, with no continues relation and nothing
//!    carried, and `koto/replaces` is written in it, naming OLD by name and
//!    session id.
//! 5. **Close.** OLD's log takes, in one write: the stop's events, a
//!    `workflow_cancelled` naming NEW (unless OLD already stands cancelled,
//!    or is finished), and a `session_superseded` fence with `replaced`
//!    set. Without the lock, OLD's runner is then stopped once more and
//!    what that records goes in past the fence.
//! 6. **Leg.** A self-attached request leg of OLD is answered as failed,
//!    best-effort, as `koto cancel` does.
//!
//! Running the same command again finishes an interrupted run: a NEW whose
//! `koto/replaces` names OLD is the earlier run's, and the close writes
//! only what OLD's log still lacks. A NEW without the key isn't taken for
//! an earlier run.
//!
//! `koto session replace OLD --by NEW [--wait SECS | --abandon-tick]` runs
//! the same close for a NEW that already exists: step 4 creates nothing,
//! and writes `koto/replaces` in NEW only when NEW lacks it, so it also
//! repairs a NEW an interrupted `koto init --replaces` left without its
//! key. A NEW whose key names another run is refused `session_exists`. Its
//! refusals print the nested envelope `koto session rebind` prints.
//!
//! Under the cloud backend OLD's checks and its close are made against the
//! bucket's copy of it, pulled from its home prefix. Every line the close
//! owes that copy goes up in one PUT conditional on the copy pulled, with no
//! unconditional fallback, and this host's copy then takes the same close,
//! worked out against its own bytes when it holds events the bucket's lacks.
//! A PUT the bucket refuses is `fence_unavailable`, exit 3, with nothing of
//! the close written in either copy; running the command again finishes.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::cli::init_child::{self, TemplateCompileCache};
use crate::engine::persistence::{
    derive_machine_state, derive_state_from_log, is_terminal_state, superseded_by, Close,
    CloseOutcome, SupersedeFence, FENCE_LOCK_WAIT, REPLACE_FAULT_ENV,
};
use crate::engine::types::{generate_session_id, now_iso8601, Event, SessionKind, StateFileHeader};
use crate::session::cloud::{CloudBackend, CloudClose, PulledState};
use crate::session::context::ContextStore;
use crate::session::tick_lock::{take_exclusive, TakeError};
use crate::session::{Backend, SessionBackend};

/// The key NEW holds naming the run it replaces.
pub const REPLACES_KEY: &str = "koto/replaces";

/// How long the command waits for a tick in flight when `--wait` isn't given.
const DEFAULT_WAIT: Duration = Duration::from_secs(10);

/// The arguments of one `koto init --replaces`.
pub(crate) struct ReplaceArgs<'a> {
    pub name: &'a str,
    pub template: &'a str,
    pub vars: &'a [String],
    pub intent: Option<&'a str>,
    pub dispatched_by: Option<&'a str>,
    pub old: &'a str,
    pub execution_dir: Option<&'a Path>,
    pub legacy_environment: bool,
    pub wait: Option<u64>,
    pub abandon_tick: bool,
}

/// The value of [`REPLACES_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Replaces {
    v: u32,
    session: String,
    session_id: String,
}

/// The command a replace runs under. It decides the refusal envelope and
/// how a message names the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Front {
    /// `koto init NEW --replaces OLD`: the flat init envelope.
    Init,
    /// `koto session replace OLD --by NEW`: the nested envelope.
    Session,
}

impl Front {
    /// What a message starts with: the flag, or the command.
    fn prefix(self) -> &'static str {
        match self {
            Front::Init => "--replaces",
            Front::Session => "session replace",
        }
    }

    /// The command as a caller types it.
    fn invocation(self) -> &'static str {
        match self {
            Front::Init => "koto init --replaces",
            Front::Session => "koto session replace",
        }
    }

    /// The command as the nested-tick refusal names it, after `koto`.
    fn tick_command(self) -> &'static str {
        match self {
            Front::Init => "init --replaces",
            Front::Session => "session replace",
        }
    }
}

/// One replace, as either command asks for it.
struct Replace<'a> {
    front: Front,
    /// NEW.
    name: &'a str,
    old: &'a str,
    wait: Option<u64>,
    abandon_tick: bool,
    /// How to create NEW when it doesn't exist yet. `None` when NEW must
    /// already exist.
    create: Option<&'a ReplaceArgs<'a>>,
}

/// A debug-build seam naming a file the replace creates at its hold point
/// and then waits on until the file is gone, so a test can act between two
/// steps. The point is `before-lock`, once every check has passed and just
/// before OLD's tick.lock is asked for, unless [`HOLD_AT_ENV`] names one of
/// [`crash_at`]'s points. A release build never reads it.
const HOLD_ENV: &str = "KOTO_TEST_REPLACE_HOLD";

/// Moves [`HOLD_ENV`]'s wait to one of [`crash_at`]'s points.
const HOLD_AT_ENV: &str = "KOTO_TEST_REPLACE_HOLD_AT";

/// The longest the replace waits at [`HOLD_ENV`]'s file.
const HOLD_CAP: Duration = Duration::from_secs(60);

/// Wait at [`HOLD_ENV`]'s file when the hold point is `point`, in a debug
/// build with the variable set.
fn hold_at(point: &str) {
    if !cfg!(debug_assertions) {
        return;
    }
    let at = std::env::var(HOLD_AT_ENV).unwrap_or_else(|_| "before-lock".to_string());
    if at != point {
        return;
    }
    let Some(path) = std::env::var_os(HOLD_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    if std::fs::write(&path, b"").is_err() {
        return;
    }
    let started = std::time::Instant::now();
    while path.exists() && started.elapsed() < HOLD_CAP {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Stop at `point`, as a killed process would, when the fault seam names
/// it, after waiting there when [`HOLD_AT_ENV`] names it. A release build
/// never reads either seam.
fn crash_at(point: &str) {
    hold_at(point);
    if cfg!(debug_assertions) && std::env::var(REPLACE_FAULT_ENV).as_deref() == Ok(point) {
        eprintln!("{REPLACE_FAULT_ENV}: stopping at {point}");
        crate::exit::crash(86);
    }
}

/// Print a refusal and exit with `exit`. Under [`Front::Init`] it is
/// `{"error", "command": "init", "code", ...extra}`; under
/// [`Front::Session`] it is `{"error": {"code", "message", ...extra},
/// "command": "session replace"}`.
fn refuse(front: Front, code: &str, message: String, exit: i32, extra: serde_json::Value) -> ! {
    let body = match front {
        Front::Init => {
            let mut body = serde_json::json!({
                "error": message,
                "command": "init",
                "code": code,
            });
            if let (Some(obj), Some(more)) = (body.as_object_mut(), extra.as_object()) {
                obj.extend(more.clone());
            }
            body
        }
        Front::Session => {
            let mut error = serde_json::json!({
                "code": code,
                "message": message,
            });
            if let (Some(obj), Some(more)) = (error.as_object_mut(), extra.as_object()) {
                obj.extend(more.clone());
            }
            serde_json::json!({
                "error": error,
                "command": "session replace",
            })
        }
    };
    super::exit_with_error_code(body, exit)
}

/// Refuse with exit 3 `persistence_error`.
fn failed(front: Front, message: String) -> ! {
    refuse(
        front,
        "persistence_error",
        message,
        super::EXIT_INFRASTRUCTURE,
        serde_json::Value::Null,
    )
}

/// The refusal for an OLD whose fence isn't this command's: another
/// session superseded or replaced it. `after` is what was already written.
fn refuse_fenced(
    backend: &Backend,
    front: Front,
    old: &str,
    name: &str,
    fence: &SupersedeFence,
    after: &str,
) -> ! {
    let p = front.prefix();
    let newest = crate::session::links::newest_member(backend, old)
        .map(|s| s.newest)
        .unwrap_or_else(|| fence.by.clone());
    let plain_to_new = fence.by == name && !fence.replaced;
    let how = if fence.replaced {
        "replaced"
    } else {
        "superseded"
    };
    // A plain fence naming NEW isn't this replace's close, so it can't
    // finish it either.
    let code = if fence.replaced || plain_to_new {
        "session_replaced"
    } else {
        "superseded_by"
    };
    let message = if plain_to_new {
        format!(
            "{p}: session '{old}' is already {how} by '{name}' through a plain \
             supersede, not replaced; {after}. Continue in '{newest}'"
        )
    } else {
        format!(
            "{p}: session '{old}' is already {how} by '{}'; {after}. Continue in \
             '{newest}'",
            fence.by
        )
    };
    let mut extra = serde_json::json!({
        "newest": newest,
        "actor": fence.actor,
    });
    let field = if fence.replaced {
        "replaced_by"
    } else {
        "superseded_by"
    };
    extra[field] = serde_json::json!(fence.by);
    refuse(front, code, message, super::EXIT_CALLER_ERROR, extra)
}

/// Refuse `session_rebound`, with nothing written, when a whole-binding
/// move recorded in OLD's `header` fences this process off from OLD: it
/// runs from a directory the move retired, or it read OLD before the move.
/// It's the check `koto next` makes, run before the lock so such a process
/// takes no lock, stops no runner and creates no NEW; the close repeats it
/// for a move made after this.
///
/// Under the cloud backend `header` is the bucket's copy. This host's copy
/// is read first, for the epoch the read records: a check with no earlier
/// read would record the bucket's epoch as this process's first read of
/// OLD, and a host whose copy predates the move would then pass the epoch
/// rule here and at the close.
fn refuse_rebound_before_the_lock(
    backend: &Backend,
    front: Front,
    old: &str,
    header: &StateFileHeader,
) {
    if header.origin.as_ref().is_none_or(|o| o.whole_epoch == 0) {
        return;
    }
    if backend.as_cloud().is_some() {
        // Read for the epoch it records alone; a host with no copy of OLD
        // has none to record, and the retired-directory rule still applies.
        let _ = backend.read_events_local(old);
    }
    let path = backend
        .session_dir(old)
        .join(crate::session::state_file_name(old));
    // A debug build can fail this serialization at the fault seam's
    // `rebind-check-serialize` point, since a real header always
    // serializes. The point never stops the process by itself, and a
    // release build never reads it.
    let line = if cfg!(debug_assertions)
        && std::env::var(REPLACE_FAULT_ENV).as_deref() == Ok("rebind-check-serialize")
    {
        Err(<serde_json::Error as serde::ser::Error>::custom(
            "rebind-check-serialize",
        ))
    } else {
        serde_json::to_string(header)
    }
    .unwrap_or_default();
    if let Err(rebound) = crate::session::fence_state::check(old, &path, &line) {
        let p = front.prefix();
        refuse(
            front,
            "session_rebound",
            format!("{p}: {rebound}"),
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
}

/// The value of NEW's [`REPLACES_KEY`], `None` when it holds none.
fn read_replaces(backend: &Backend, name: &str) -> std::result::Result<Option<Replaces>, String> {
    let store: &dyn ContextStore = backend;
    if !store.ctx_exists(name, REPLACES_KEY) {
        return Ok(None);
    }
    let bytes = store
        .get(name, REPLACES_KEY)
        .map_err(|e| format!("{e:#}"))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("it doesn't parse: {e}"))
}

/// Whether OLD, as `header` and `events` read, is owed a cancel: false only
/// for a run standing in a terminal state of its template.
fn owes_cancel(backend: &Backend, old: &str, header: &StateFileHeader, events: &[Event]) -> bool {
    match derive_machine_state(header, events, &backend.session_dir(old)) {
        Some(machine) => !is_terminal_state(&machine.template_path, &machine.current_state),
        None => true,
    }
}

/// Run `koto init --replaces`. Prints one JSON object on success; every
/// refusal prints its error and exits.
pub(crate) fn run(backend: &Backend, args: &ReplaceArgs<'_>) -> Result<()> {
    close_and_replace(
        backend,
        &Replace {
            front: Front::Init,
            name: args.name,
            old: args.old,
            wait: args.wait,
            abandon_tick: args.abandon_tick,
            create: Some(args),
        },
    )
}

/// Run `koto session replace OLD --by NEW`: the close `koto init
/// --replaces` makes, for a NEW that already exists. Prints one JSON object
/// on success; every refusal prints its error and exits.
pub(crate) fn run_session(
    backend: &Backend,
    old: &str,
    new: &str,
    wait: Option<u64>,
    abandon_tick: bool,
) -> Result<()> {
    close_and_replace(
        backend,
        &Replace {
            front: Front::Session,
            name: new,
            old,
            wait,
            abandon_tick,
            create: None,
        },
    )
}

/// The replace both commands run, in the order the module doc gives.
fn close_and_replace(backend: &Backend, job: &Replace<'_>) -> Result<()> {
    let front = job.front;
    let name = job.name;
    let old = job.old;
    let p = front.prefix();

    // 1. Checks, nothing written.
    let labels = match front {
        Front::Init => [("", name), ("--replaces ", old)],
        Front::Session => [("--by ", name), ("", old)],
    };
    for (flag, value) in labels {
        if let Err(msg) = crate::discover::validate_workflow_name(value) {
            refuse(
                front,
                "invalid_usage",
                format!("{flag}{msg}"),
                super::EXIT_CALLER_ERROR,
                serde_json::Value::Null,
            );
        }
    }
    if name == old {
        refuse(
            front,
            "self",
            format!("{p}: session '{name}' can't replace itself; nothing was written"),
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
    let own_tick = crate::engine::reentrancy::refuse_if_own_tick(old, front.tick_command());
    if let Err(message) = own_tick {
        refuse(
            front,
            "nested_invocation",
            message,
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
    let background_of = crate::engine::background::BACKGROUND_OF_ENV;
    if std::env::var(background_of).as_deref() == Ok(old) {
        refuse(
            front,
            "nested_invocation",
            format!(
                "{} cannot run on session '{old}' from inside its own \
                 background action: this process inherited {background_of}={old}, and closing \
                 '{old}' stops the runner this command runs under. Run it outside the action; \
                 nothing was written",
                front.invocation()
            ),
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
    if !backend.exists(old) {
        refuse(
            front,
            "session_not_found",
            format!("{p}: no session named '{old}' in this store; nothing was written"),
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
    let (old_header, old_events) = read_old(backend, front, old);
    if old_header.session_kind() != SessionKind::Ordinary {
        refuse(
            front,
            "invalid_usage",
            format!(
                "{p}: '{old}' is koto's record of an agent session, not a run, so \
                 nothing replaces it; nothing was written"
            ),
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
    let old_id = old_header.session_id.clone();

    let new_exists = backend.exists(name);
    if job.create.is_none() && !new_exists {
        refuse(
            front,
            "session_not_found",
            format!(
                "{p}: no session named '{name}' in this store, and this command closes '{old}' \
                 only for a session that already exists. Create '{name}' first, or create it \
                 and close '{old}' in one step with `koto init {name} --template <path> \
                 --replaces {old}`; nothing was written"
            ),
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
    let new_id = new_exists.then(|| {
        let header = backend.read_header(name).unwrap_or_else(|e| {
            failed(
                front,
                format!("{p}: could not read '{name}''s header: {e:#}; nothing was written"),
            )
        });
        if front == Front::Session && header.session_kind() != SessionKind::Ordinary {
            refuse(
                front,
                "invalid_usage",
                format!(
                    "{p}: '{name}' is koto's record of an agent session, not a run, so it \
                     replaces nothing; nothing was written"
                ),
                super::EXIT_CALLER_ERROR,
                serde_json::Value::Null,
            );
        }
        header.session_id
    });
    let nothing = "nothing was written";
    let fence_ours = match superseded_by(&old_events) {
        Some(f)
            if f.replaced
                && f.by == name
                && new_id.as_deref() == Some(f.by_session_id.as_str()) =>
        {
            true
        }
        Some(f) => refuse_fenced(backend, front, old, name, &f, nothing),
        None => false,
    };
    refuse_rebound_before_the_lock(backend, front, old, &old_header);
    if new_exists {
        let ours = match read_replaces(backend, name) {
            Ok(Some(r)) => r.session == old && r.session_id == old_id,
            // `koto session replace` writes the key a NEW lacks.
            Ok(None) => fence_ours || front == Front::Session,
            Err(why) => failed(
                front,
                format!("{p}: '{name}''s {REPLACES_KEY} can't be read: {why}; nothing was written"),
            ),
        };
        if !ours {
            let message = match front {
                Front::Init => format!(
                    "--replaces: session '{name}' already exists, and no earlier run of this \
                     command opened it to replace '{old}'; pick a new name, or remove it with \
                     `koto cancel {name} --cleanup` and run the command again. Nothing was \
                     written"
                ),
                Front::Session => format!(
                    "session replace: session '{name}' already replaces another run: its \
                     {REPLACES_KEY} key names a run other than '{old}', so it can't replace \
                     '{old}' too; nothing was written"
                ),
            };
            refuse(
                front,
                "session_exists",
                message,
                super::EXIT_CALLER_ERROR,
                serde_json::Value::Null,
            );
        }
    } else if let Some(args) = job.create {
        precheck_new(args);
    }
    hold_at("before-lock");

    // 2. OLD's tick lock. Under the cloud backend this host may hold no
    // copy of OLD yet, and the lock lives in its local directory.
    let dir = backend.session_dir(old);
    if backend.as_cloud().is_some() {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            failed(
                front,
                format!(
                    "{p}: '{old}''s directory for its tick.lock couldn't be made: {e}; nothing \
                     was written"
                ),
            );
        }
    }
    let (tick_lock, waited) = take_tick_lock(old, &dir, job);
    let tick_in_flight = tick_lock.is_none();
    crash_at("after-lock");

    // 3. Stop OLD's runner, from its log as it stands under the lock.
    if let Some(cloud) = backend.as_cloud() {
        // Writes a stopped tick left on this host go up first, so the
        // bucket's copy holds them before the close is put on it.
        if let Err(e) = cloud.flush_interrupted_batch(old) {
            failed(
                front,
                format!(
                    "{p}: the writes this host holds for '{old}' couldn't be pushed to the \
                     bucket: {e:#}; '{old}' wasn't closed"
                ),
            );
        }
    }
    let (old_header, old_events) = read_old(backend, front, old);
    // The runner to stop is this host's: under the cloud backend this
    // host's copy of OLD records what it started, whatever the bucket's
    // copy holds.
    let here = backend
        .as_cloud()
        .and_then(|_| backend.read_events_local(old).ok())
        .map(|(_, events)| events);
    let stop_from = here.as_deref().unwrap_or(old_events.as_slice());
    let side_events = super::stop_session_pool_work(backend, old, stop_from);
    let cancel = owes_cancel(backend, old, &old_header, &old_events);

    // 4. Create NEW, unless it exists, and record what it replaces.
    let mut wrote = false;
    let mut environment_report = None;
    let new_id = match (new_id, job.create) {
        (Some(id), _) => id,
        (None, None) => unreachable!("a NEW that doesn't exist is refused above"),
        (None, Some(args)) => {
            let id = generate_session_id();
            let (_, report) = super::create_session(
                backend,
                name,
                args.template,
                args.vars,
                None,
                args.intent,
                args.execution_dir,
                args.legacy_environment,
                args.dispatched_by,
                None,
                Some(&id),
            )?;
            environment_report = report;
            wrote = true;
            id
        }
    };
    crash_at("after-create");
    let key_held = matches!(read_replaces(backend, name), Ok(Some(_)));
    if !key_held {
        let value = serde_json::to_vec(&Replaces {
            v: 1,
            session: old.to_string(),
            session_id: old_id.clone(),
        })?;
        if let Err(e) = super::init_supersede::write_koto_key(backend, name, REPLACES_KEY, &value) {
            let message = match front {
                Front::Init => format!(
                    "--replaces: '{name}' was created, but its {REPLACES_KEY} key couldn't be \
                     written: {e:#}. '{old}' is still open; remove '{name}' with `koto cancel \
                     {name} --cleanup` and run the command again"
                ),
                Front::Session => format!(
                    "session replace: '{name}''s {REPLACES_KEY} key couldn't be written: \
                     {e:#}. '{old}' is still open; run the command again"
                ),
            };
            failed(front, message);
        }
        wrote = true;
    }
    crash_at("after-key");

    // 5. Close OLD.
    let at = now_iso8601();
    let fence = SupersedeFence {
        by: name.to_string(),
        by_session_id: new_id,
        actor: super::init_supersede::actor(),
        at: at.clone(),
        replaced: true,
    };
    let close = Close {
        side_events,
        cancel,
        fence,
    };
    wrote |= close_old(backend, front, old, name, &close, &at);
    if tick_in_flight {
        // The tick in flight may have started a runner since the first stop.
        let (_, events) = read_here(backend, old).unwrap_or_else(|e| {
            failed(
                front,
                format!(
                    "{p}: '{old}' is closed, but its log couldn't be read again to stop its \
                     runner once more: {e:#}"
                ),
            )
        });
        let side_events = super::stop_session_pool_work(backend, old, &events);
        if !side_events.is_empty() {
            let again = Close {
                side_events,
                cancel: false,
                fence: close.fence.clone(),
            };
            wrote |= close_old(backend, front, old, name, &again, &now_iso8601());
        }
    }
    drop(tick_lock);
    crash_at("after-close");

    // 6. The leg, best-effort.
    if cancel {
        if let Some(state) = read_here(backend, old)
            .ok()
            .and_then(|(_, events)| derive_state_from_log(&events))
        {
            super::promote_cancel_onto_self_attached_leg(&dir, &state);
        }
    }

    let children = super::query_children(backend, old);
    let deferred_cleanup = !children.is_empty();
    if deferred_cleanup {
        eprintln!(
            "note: '{old}' has children; they are neither cancelled nor refused, and one that \
             finishes later can't record it in '{old}''s closed log, so its cleanup waits until \
             it is cleaned up directly"
        );
    }
    let (_, events) = backend
        .read_events(name)
        .map_err(|e| anyhow::anyhow!("failed to read '{name}': {e}"))?;
    let mut out = serde_json::json!({
        "name": name,
        "state": derive_state_from_log(&events).unwrap_or_default(),
        "replaces": old,
        "outcome": if wrote { "replaced" } else { "unchanged" },
        "waited": waited,
        "tick_in_flight": tick_in_flight,
        "children": children,
        "deferred_cleanup": deferred_cleanup,
    });
    super::add_environment_report(&mut out, environment_report.as_ref());
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}

/// The checks creating NEW can refuse on, run before any lock: its
/// template's compile and its variables.
fn precheck_new(args: &ReplaceArgs<'_>) {
    let mut cache = TemplateCompileCache::new();
    let compiled = match init_child::compile_for_init(Path::new(args.template), &mut cache) {
        Ok((compiled, _)) => compiled,
        Err(info) => {
            let code = serde_json::to_value(&info.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "template_compile_failed".to_string());
            refuse(Front::Init, &code, info.message, 1, serde_json::Value::Null);
        }
    };
    if let Err(e) = super::resolve_variables(args.vars, &compiled.variables) {
        let mut body = serde_json::json!({
            "error": e.to_string(),
            "command": "init",
        });
        if let Some(obj) = body.as_object_mut() {
            obj.extend(e.fields());
        }
        super::exit_with_error_code(body, super::EXIT_CALLER_ERROR);
    }
}

/// OLD's `tick.lock`, taken exclusive within `--wait`, and whether the take
/// had to wait. With `--abandon-tick` it is asked for once, and `None` when
/// it is held elsewhere.
fn take_tick_lock(old: &str, dir: &Path, job: &Replace<'_>) -> (Option<std::fs::File>, bool) {
    let front = job.front;
    let p = front.prefix();
    let wait = if job.abandon_tick {
        Duration::ZERO
    } else {
        job.wait.map_or(DEFAULT_WAIT, Duration::from_secs)
    };
    match take_exclusive(dir, wait) {
        Ok((file, waited)) => (Some(file), waited),
        Err(TakeError::Busy(_)) if job.abandon_tick => (None, false),
        Err(TakeError::Busy(busy)) => {
            let waited_for_ms = u64::try_from(busy.waited_for.as_millis()).unwrap_or(u64::MAX);
            refuse(
                front,
                "session_busy",
                format!(
                    "session '{old}' is busy: a tick of it is in flight, and it still held {} \
                     after {} s; nothing was written. Stopping that tick's `koto next` frees \
                     the lock; then run the command again, or give --wait more seconds, or \
                     pass --abandon-tick to close '{old}' without it",
                    dir.join(crate::session::TICK_LOCK_FILE).display(),
                    wait.as_secs()
                ),
                1,
                serde_json::json!({"details": {"waited_for_ms": waited_for_ms}}),
            )
        }
        Err(TakeError::Missing) => failed(
            front,
            format!(
                "{p}: '{old}''s directory went away before its tick.lock could be taken; \
                 nothing was written"
            ),
        ),
        Err(TakeError::Failed(e)) => failed(
            front,
            format!("{p}: '{old}''s tick.lock couldn't be taken: {e:#}; nothing was written"),
        ),
    }
}

/// OLD's header and events, as the checks and the close read them: its
/// log, or under the cloud backend the bucket's copy, pulled from its home
/// prefix. Refuses, with nothing written, when they can't be read.
fn read_old(backend: &Backend, front: Front, old: &str) -> (StateFileHeader, Vec<Event>) {
    let p = front.prefix();
    let Some(cloud) = backend.as_cloud() else {
        return backend.read_events(old).unwrap_or_else(|e| {
            failed(
                front,
                format!("{p}: could not read '{old}''s log: {e:#}; nothing was written"),
            )
        });
    };
    let pulled = pull_old(cloud, front, old, "nothing was written");
    (pulled.header, pulled.events)
}

/// OLD's events as this host's copy holds them, for what this host does
/// about its own runner and leg. Under the cloud backend that is the local
/// copy, read without a pull, since a pull refuses a copy holding events
/// the bucket's lacks; the bucket's copy when this host holds none.
fn read_here(backend: &Backend, old: &str) -> Result<(StateFileHeader, Vec<Event>)> {
    match backend.as_cloud() {
        Some(_) => backend
            .read_events_local(old)
            .or_else(|_| backend.read_events(old)),
        None => backend.read_events(old),
    }
}

/// The bucket's copy of OLD, pulled from the prefix this host's copy
/// records as its home, else from this workspace's own. A prefix holding
/// none refuses `fence_unavailable`, a copy that can't be read
/// `persistence_error`, and a fence in it naming a session koto refuses
/// `invalid_fence`, each before anything is built from it; `after` says
/// what was written by then.
fn pull_old(cloud: &CloudBackend, front: Front, old: &str, after: &str) -> PulledState {
    let p = front.prefix();
    let prefix = cloud
        .home_prefix(old)
        .map(|home| home.prefix)
        .unwrap_or_else(|_| cloud.workspace_prefix().to_string());
    let pulled = match cloud.pull_for_fence(&prefix, old) {
        Ok(Some(pulled)) => pulled,
        Ok(None) => refuse(
            front,
            "fence_unavailable",
            format!(
                "{p}: the bucket holds no state of '{old}' under its workspace prefix, so it \
                 can't be closed there; {after}"
            ),
            super::EXIT_INFRASTRUCTURE,
            serde_json::Value::Null,
        ),
        Err(e) => failed(
            front,
            format!("{p}: could not pull '{old}''s state from the bucket: {e:#}; {after}"),
        ),
    };
    if let Err(e) = pulled.fence() {
        refuse(
            front,
            "invalid_fence",
            format!("{p}: '{old}''s log in the bucket can't be trusted: {e:#}; {after}"),
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        );
    }
    pulled
}

/// What a refusal of a close says was left behind, once NEW exists.
fn left_as_is(front: Front, old: &str, name: &str) -> String {
    match front {
        Front::Init => format!(
            "'{name}' was created and is left as it is, and '{old}' wasn't closed. Remove \
             '{name}' with `koto cancel {name} --cleanup` if it isn't wanted"
        ),
        Front::Session => format!("'{name}' is left as it is, and '{old}' wasn't closed"),
    }
}

/// Write `close` to OLD's copy in the bucket and then to this host's, as
/// [`CloudBackend::push_close`] does, on a copy pulled just now, refusing
/// as the outcome says. Returns whether anything was written.
fn close_in_bucket(
    backend: &Backend,
    cloud: &CloudBackend,
    front: Front,
    old: &str,
    name: &str,
    close: &Close,
    at: &str,
) -> bool {
    let p = front.prefix();
    let after = format!("'{old}' wasn't closed, and '{name}' exists");
    let pulled = pull_old(cloud, front, old, &after);
    match cloud.push_close(&pulled, close, at) {
        CloudClose::Closed { wrote } => wrote,
        CloudClose::FencedByOther(other) => refuse_fenced(
            backend,
            front,
            old,
            name,
            &other,
            &left_as_is(front, old, name),
        ),
        CloudClose::Rebound(rebound) => refuse(
            front,
            "session_rebound",
            format!("{p}: {rebound}; '{old}' wasn't closed, and '{name}' exists"),
            super::EXIT_CALLER_ERROR,
            serde_json::Value::Null,
        ),
        CloudClose::Unavailable(reason) => refuse(
            front,
            "fence_unavailable",
            format!(
                "{p}: '{old}' couldn't be closed in the bucket: {reason}. Nothing of the close \
                 was written, so '{old}' is still open, and '{name}' exists. Run the command \
                 again"
            ),
            super::EXIT_INFRASTRUCTURE,
            serde_json::Value::Null,
        ),
    }
}

/// Write `close` to OLD's log, refusing as the outcome says. Returns
/// whether anything was written. Under the cloud backend the close goes to
/// the bucket's copy first ([`close_in_bucket`]).
fn close_old(
    backend: &Backend,
    front: Front,
    old: &str,
    name: &str,
    close: &Close,
    at: &str,
) -> bool {
    let p = front.prefix();
    let local = match backend {
        Backend::Local(local) => local,
        Backend::Cloud(cloud) => {
            return close_in_bucket(backend, cloud, front, old, name, close, at)
        }
    };
    match local.append_close(old, close, at, FENCE_LOCK_WAIT) {
        Ok(CloseOutcome::Closed {
            commits,
            torn_backup,
        }) => !commits.is_empty() || torn_backup.is_some(),
        Ok(CloseOutcome::FencedByOther(other)) => {
            let after = left_as_is(front, old, name);
            refuse_fenced(backend, front, old, name, &other, &after)
        }
        Ok(CloseOutcome::Unavailable) => refuse(
            front,
            "fence_unavailable",
            format!(
                "{p}: '{old}''s log stayed locked by another writer for {} seconds, so \
                 '{old}' wasn't closed; '{name}' exists. Run the command again",
                FENCE_LOCK_WAIT.as_secs()
            ),
            super::EXIT_INFRASTRUCTURE,
            serde_json::Value::Null,
        ),
        Err(e) => {
            if let Some(rebound) = e
                .chain()
                .find_map(|c| c.downcast_ref::<crate::session::rebind_fence::SessionRebound>())
            {
                refuse(
                    front,
                    "session_rebound",
                    format!("{p}: {rebound}; '{old}' wasn't closed, and '{name}' exists"),
                    super::EXIT_CALLER_ERROR,
                    serde_json::Value::Null,
                );
            }
            failed(
                front,
                format!(
                    "{p}: '{old}' couldn't be closed: {e:#}; '{name}' exists. Run the \
                     command again to finish"
                ),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_replaces_key_round_trips_its_value() {
        let r = Replaces {
            v: 1,
            session: "old".to_string(),
            session_id: "abc".to_string(),
        };
        let bytes = serde_json::to_vec(&r).unwrap();
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            r#"{"v":1,"session":"old","session_id":"abc"}"#
        );
        assert_eq!(serde_json::from_slice::<Replaces>(&bytes).unwrap(), r);
    }
}
