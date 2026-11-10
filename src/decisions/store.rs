//! The decision store: the id allocator, the create-only writer and the
//! listing of a session's two areas with the integrity checks every reader
//! applies (DESIGN-decision-record.md, Decision 1 and Security
//! Considerations).
//!
//! A decision is written inside the session's `ctx/manifest.lock` critical
//! section: the allocator reads both areas (and, under the cloud backend,
//! the bucket's copy of them), takes the highest id plus one, and the body
//! is published under that id before the lock is released, so two writers
//! in one session get distinct ids in commit order. Locally the body goes
//! to a synced temporary file in the session directory, outside
//! `ctx/koto/`, and is published with a create-only rename that never
//! replaces; the directory is synced before success is reported. Under the
//! cloud backend the bucket object is put first, with `If-None-Match: *`,
//! so a refusal there leaves no local key behind.
//!
//! A gate-passing command appends its event inside the same critical
//! section, with the body embedded in it ([`write_with`]). A writer that
//! stops between the event and the key leaves the event behind; the
//! session's next write, or its next `koto next` ([`repair`]), publishes
//! the embedded body under its id (DESIGN-decision-record.md, Decision 3).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::body::{DecisionBody, MAX_BODY_BYTES};
use super::{
    codes, is_decision_id, DecisionError, AIMED_AREA, DECISIONS_AREA, MAX_AREA_ENTRIES, PACKS_AREA,
};
use crate::session::Backend;

/// Where a decision file sits.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Area {
    Decisions,
    /// `koto/aimed/<target-session-id>/`.
    Aimed(String),
}

impl Area {
    /// The context key of decision `id` in this area.
    pub fn key(&self, id: &str) -> String {
        match self {
            Area::Decisions => format!("{DECISIONS_AREA}/{id}"),
            Area::Aimed(target) => format!("{AIMED_AREA}/{target}/{id}"),
        }
    }

    /// The area a context key lies in, and the file name in it, or `None`
    /// for a key outside both areas or one nested deeper than they nest.
    pub fn of_key(key: &str) -> Option<(Area, String)> {
        if let Some(name) = key.strip_prefix(&format!("{DECISIONS_AREA}/")) {
            return (!name.contains('/')).then(|| (Area::Decisions, name.to_string()));
        }
        let rest = key.strip_prefix(&format!("{AIMED_AREA}/"))?;
        let (target, name) = rest.split_once('/')?;
        (!name.contains('/')).then(|| (Area::Aimed(target.to_string()), name.to_string()))
    }
}

/// One file found in a session's areas, read or not.
#[derive(Debug, Clone)]
pub struct Found {
    pub area: Area,
    /// The file name, which is the decision id when it is well formed.
    pub name: String,
    /// Its bytes, or why they couldn't be read.
    pub bytes: Result<Vec<u8>, String>,
}

/// What a session's areas hold. `problem` is set when the areas as a whole
/// can't be trusted (an area root that is a symlink or not a directory, an
/// area over its entry limit, a non-empty `koto/packs/`); the session then
/// reads as unreadable whatever `found` holds.
#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub found: Vec<Found>,
    pub problem: Option<String>,
}

impl Listing {
    /// The well-formed ids in both areas.
    pub fn ids(&self) -> Vec<String> {
        self.found
            .iter()
            .filter(|f| is_decision_id(&f.name))
            .map(|f| f.name.clone())
            .collect()
    }
}

/// The directory a context key's file sits at in `ctx_dir`.
fn key_path(ctx_dir: &Path, key: &str) -> PathBuf {
    key.split('/').fold(ctx_dir.to_path_buf(), |p, c| p.join(c))
}

/// `lstat` `path`: `Ok(None)` when it doesn't exist, `Ok(Some(()))` for a
/// real directory, `Err` for a symlink, a file or anything else there.
fn real_dir(path: &Path) -> Result<Option<()>, String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{} can't be read: {e}", path.display())),
        Ok(m) if m.file_type().is_symlink() => Err(format!("{} is a symlink", path.display())),
        Ok(m) if !m.is_dir() => Err(format!("{} isn't a directory", path.display())),
        Ok(_) => Ok(Some(())),
    }
}

/// Read a decision file without following a symlink, confirming a regular
/// file on the open descriptor and refusing more than [`MAX_BODY_BYTES`].
pub fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("can't be opened as a regular file: {e}"))?;
    let meta = file.metadata().map_err(|e| format!("can't be read: {e}"))?;
    if !meta.is_file() {
        return Err("isn't a regular file".to_string());
    }
    if meta.len() > MAX_BODY_BYTES as u64 {
        return Err(format!("is larger than {MAX_BODY_BYTES} bytes"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("can't be read: {e}"))?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(format!("is larger than {MAX_BODY_BYTES} bytes"));
    }
    Ok(bytes)
}

/// The entries of the directory at `dir`, skipping nothing; `Err` when it
/// can't be listed or holds more than [`MAX_AREA_ENTRIES`].
fn entries(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut out = Vec::new();
    for entry in
        std::fs::read_dir(dir).map_err(|e| format!("{} can't be listed: {e}", dir.display()))?
    {
        let entry = entry.map_err(|e| format!("{} can't be listed: {e}", dir.display()))?;
        out.push((
            entry.file_name().to_string_lossy().into_owned(),
            entry.path(),
        ));
        if out.len() > MAX_AREA_ENTRIES {
            return Err(format!(
                "{} holds more than {MAX_AREA_ENTRIES} entries",
                dir.display()
            ));
        }
    }
    out.sort();
    Ok(out)
}

/// List the decision areas under the local `ctx_dir`, reading every file,
/// with no lock and no write.
pub fn list_local(ctx_dir: &Path) -> Listing {
    let mut listing = Listing::default();
    if let Err(e) = real_dir(&ctx_dir.join("koto")) {
        listing.problem = Some(e);
        return listing;
    }
    let read_area = |area: Area, dir: &Path, listing: &mut Listing| -> Result<(), String> {
        for (name, path) in entries(dir)? {
            let bytes = match std::fs::symlink_metadata(&path) {
                Ok(m) if m.file_type().is_symlink() => Err("is a symlink".to_string()),
                Ok(m) if !m.is_file() => Err("isn't a regular file".to_string()),
                Ok(_) => read_file(&path),
                Err(e) => Err(format!("can't be read: {e}")),
            };
            listing.found.push(Found {
                area: area.clone(),
                name,
                bytes,
            });
        }
        Ok(())
    };

    let decisions = key_path(ctx_dir, DECISIONS_AREA);
    match real_dir(&decisions) {
        Ok(Some(())) => {
            if let Err(e) = read_area(Area::Decisions, &decisions, &mut listing) {
                listing.problem = Some(e);
                return listing;
            }
        }
        Ok(None) => {}
        Err(e) => {
            listing.problem = Some(e);
            return listing;
        }
    }

    let aimed = key_path(ctx_dir, AIMED_AREA);
    match real_dir(&aimed) {
        Ok(Some(())) => match entries(&aimed) {
            Ok(targets) => {
                for (target, path) in targets {
                    if let Err(e) = real_dir(&path)
                        .and_then(|d| d.ok_or_else(|| format!("{} vanished", path.display())))
                    {
                        listing.problem = Some(e);
                        return listing;
                    }
                    if crate::session::validate::validate_session_id(&target).is_err() {
                        listing.problem =
                            Some(format!("aimed target '{target}' isn't a session id"));
                        return listing;
                    }
                    if let Err(e) = read_area(Area::Aimed(target), &path, &mut listing) {
                        listing.problem = Some(e);
                        return listing;
                    }
                }
            }
            Err(e) => {
                listing.problem = Some(e);
                return listing;
            }
        },
        Ok(None) => {}
        Err(e) => {
            listing.problem = Some(e);
            return listing;
        }
    }

    let packs = key_path(ctx_dir, PACKS_AREA);
    match real_dir(&packs) {
        Ok(None) => {}
        Ok(Some(())) => {
            if entries(&packs).map(|e| !e.is_empty()).unwrap_or(true) {
                listing.problem = Some(format!("{PACKS_AREA}/ is reserved and holds entries"));
            }
        }
        Err(e) => listing.problem = Some(e),
    }
    listing
}

/// A decision object as the bucket holds it: its context key, and its bytes
/// or why they couldn't be read.
pub type RemoteObject = (String, Result<Vec<u8>, String>);

/// Merge the bucket's copy of a session's areas into the local listing:
/// an object the local copy lacks is added; one whose bytes differ from the
/// local file marks that entry unreadable.
pub fn merge_remote(listing: &mut Listing, remote: Vec<RemoteObject>) {
    let mut index: BTreeMap<(Area, String), usize> = BTreeMap::new();
    for (i, f) in listing.found.iter().enumerate() {
        index.insert((f.area.clone(), f.name.clone()), i);
    }
    for (key, bytes) in remote {
        let Some((area, name)) = Area::of_key(&key) else {
            listing.problem = Some(format!(
                "the bucket holds '{key}', which isn't a decision key"
            ));
            continue;
        };
        match index.get(&(area.clone(), name.clone())) {
            Some(&i) => {
                let local = &mut listing.found[i];
                let same = matches!((&local.bytes, &bytes), (Ok(a), Ok(b)) if a == b);
                if !same && local.bytes.is_ok() {
                    local.bytes = Err("the bucket's copy differs from this host's".to_string());
                }
            }
            None => listing.found.push(Found { area, name, bytes }),
        }
    }
}

/// The listing of `session`'s areas: the local copy, and under the cloud
/// backend the bucket's objects too. Writes nothing anywhere.
pub fn list(backend: &Backend, session: &str) -> Listing {
    let local_dir = match backend {
        Backend::Local(l) => l.ctx_path(session),
        Backend::Cloud(c) => c.local().ctx_path(session),
    };
    let mut listing = list_local(&local_dir);
    if let Backend::Cloud(cloud) = backend {
        match cloud.decision_objects(session) {
            Ok(objects) => merge_remote(&mut listing, objects),
            Err(e) => {
                if listing.problem.is_none() {
                    listing.problem =
                        Some(format!("the bucket's decisions can't be listed: {e:#}"));
                }
            }
        }
    }
    listing
}

/// Record a decision in `session`: allocate the next id under the
/// session's manifest lock, build the body for it with `build`, and publish
/// it create-only. Returns the body as stored.
pub fn write(
    backend: &Backend,
    session: &str,
    build: impl FnOnce(&str) -> Result<DecisionBody, DecisionError>,
) -> Result<DecisionBody, DecisionError> {
    write_with(backend, session, build, None)
}

/// A step [`write_with`] runs inside the critical section once the body is
/// built and size-checked, before the body is published: a gate-passing
/// command appends its event here with the body embedded in it, so the
/// event and the key are written in one critical section (DESIGN-decision-
/// record.md, Decision 3). An error leaves nothing published.
pub type Commit<'a> = &'a mut dyn FnMut(&DecisionBody) -> Result<(), DecisionError>;

/// The environment variable a debug build reads for its fault point.
const FAULT_ENV: &str = "KOTO_TEST_DECISION_FAULT";

/// In a debug build with `KOTO_TEST_DECISION_FAULT=<point>`, stop at
/// `point` as a killed process would: no destructor runs and nothing is
/// cleaned up. A release build never stops.
fn crash_at(point: &str) {
    if cfg!(debug_assertions) && std::env::var(FAULT_ENV).as_deref() == Ok(point) {
        eprintln!("{FAULT_ENV}: stopping at {point}");
        std::process::exit(86);
    }
}

/// [`write`], with `commit` run after the body is built and size-checked
/// and before it is published. Before allocating, the critical section
/// repairs the session ([`repair`]), so the next id is allocated past any
/// decision an earlier writer committed in its event but never published.
pub fn write_with(
    backend: &Backend,
    session: &str,
    build: impl FnOnce(&str) -> Result<DecisionBody, DecisionError>,
    mut commit: Option<Commit<'_>>,
) -> Result<DecisionBody, DecisionError> {
    let local = match backend {
        Backend::Local(l) => l,
        Backend::Cloud(c) => c.local(),
    };
    let infra = |e: anyhow::Error| DecisionError::new("decision_write_failed", format!("{e:#}"));
    local
        .with_manifest_lock(session, || {
            repair_locked(backend, session)?;
            let listing = list(backend, session);
            if let Some(problem) = &listing.problem {
                return Ok(Err(DecisionError::new(
                    codes::INVALID_DECISION,
                    format!("'{session}' holds decision files koto can't read, so it records no more: {problem}"),
                )));
            }
            let ids = listing.ids();
            if ids.len() >= MAX_AREA_ENTRIES {
                return Ok(Err(DecisionError::new(
                    codes::INVALID_DECISION,
                    format!("'{session}' holds {MAX_AREA_ENTRIES} decisions, the most one session may"),
                )));
            }
            let next = ids
                .iter()
                .filter_map(|id| id.parse::<u64>().ok())
                .max()
                .unwrap_or(0)
                + 1;
            let id = super::format_id(next);
            let body = match build(&id) {
                Ok(b) => b,
                Err(e) => return Ok(Err(e)),
            };
            let bytes = body.to_bytes()?;
            if bytes.len() > MAX_BODY_BYTES {
                return Ok(Err(DecisionError::invalid(
                    "body",
                    format!("the decision body is larger than {MAX_BODY_BYTES} bytes"),
                )));
            }
            if let Some(commit) = commit.as_mut() {
                if let Err(e) = commit(&body) {
                    return Ok(Err(e));
                }
                crash_at("after-event");
            }
            let key = Area::Decisions.key(&id);
            Ok(publish_both(backend, session, &key, &bytes)?.map(|()| body))
        })
        .map_err(infra)?
}

/// Publish `bytes` at `key`: under the cloud backend the bucket object
/// first, with `If-None-Match: *`, so a refusal there leaves no local key,
/// then the local file.
fn publish_both(
    backend: &Backend,
    session: &str,
    key: &str,
    bytes: &[u8],
) -> anyhow::Result<Result<(), DecisionError>> {
    let local = match backend {
        Backend::Local(l) => l,
        Backend::Cloud(c) => c.local(),
    };
    if let Backend::Cloud(cloud) = backend {
        if let Err(e) = cloud.put_decision_object(session, key, bytes) {
            return Ok(Err(e));
        }
    }
    match publish(
        &crate::session::SessionBackend::session_dir(local, session),
        &local.ctx_path(session),
        key,
        bytes,
    ) {
        Ok(()) => Ok(Ok(())),
        Err(PublishError::Exists) => Ok(Err(DecisionError::new(
            codes::DECISION_EXISTS,
            format!("decision key '{key}' already exists in '{session}'"),
        ))),
        Err(PublishError::Io(e)) => Err(e),
    }
}

/// The decisions embedded in `events`: the `decision` a gate-passing
/// command's event carries.
pub fn embedded(events: &[crate::engine::types::Event]) -> Vec<&serde_json::Value> {
    use crate::engine::types::EventPayload;
    events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::GateOverrideRecorded { decision, .. }
            | EventPayload::DirectedTransition { decision, .. } => decision.as_ref(),
            EventPayload::DeciderCheckAnswered(a) => a.decision.as_ref(),
            _ => None,
        })
        .collect()
}

/// Whether a decision embedded in `events` has no key on this host: the
/// cheap check a tick runs before it takes the lock to [`repair`].
pub fn needs_repair(
    backend: &Backend,
    session: &str,
    events: &[crate::engine::types::Event],
) -> bool {
    let local = match backend {
        Backend::Local(l) => l,
        Backend::Cloud(c) => c.local(),
    };
    let ctx = local.ctx_path(session);
    embedded(events).into_iter().any(|d| {
        d["id"]
            .as_str()
            .filter(|id| is_decision_id(id))
            .is_some_and(|id| {
                std::fs::symlink_metadata(key_path(&ctx, &Area::Decisions.key(id))).is_err()
            })
    })
}

/// Publish every decision embedded in `session`'s events whose key is
/// missing (its writer stopped between the event and the key), under the
/// session's manifest lock. A body that fails the checks a fresh record
/// gets ([`super::body::validate_stored`]) is skipped with a warning.
pub fn repair(backend: &Backend, session: &str) -> Result<(), DecisionError> {
    let local = match backend {
        Backend::Local(l) => l,
        Backend::Cloud(c) => c.local(),
    };
    local
        .with_manifest_lock(session, || repair_locked(backend, session))
        .map_err(|e| DecisionError::new("decision_write_failed", format!("{e:#}")))
}

/// [`repair`]'s work, run with the manifest lock already held.
fn repair_locked(backend: &Backend, session: &str) -> anyhow::Result<()> {
    use crate::session::SessionBackend;
    let Ok((header, events)) = backend.read_events(session) else {
        // A log that can't be read is the command's to report.
        return Ok(());
    };
    let embedded = embedded(&events);
    if embedded.is_empty() {
        return Ok(());
    }
    let listing = list(backend, session);
    if listing.problem.is_some() {
        // The write that follows refuses on the same listing.
        return Ok(());
    }
    let mut ids: std::collections::BTreeSet<String> = listing.ids().into_iter().collect();
    for value in embedded {
        let body: DecisionBody = match serde_json::from_value(value.clone()) {
            Ok(b) => b,
            Err(e) => {
                eprintln!(
                    "warning: a decision embedded in the log of '{session}' doesn't parse, \
                     so it isn't repaired: {e}"
                );
                continue;
            }
        };
        if ids.contains(&body.id) {
            continue;
        }
        if let Err(e) = super::body::validate_stored(&body, session, &header.session_id) {
            eprintln!(
                "warning: decision {} embedded in the log of '{session}' fails its checks, \
                 so it isn't repaired: {}",
                body.id, e.message
            );
            continue;
        }
        let bytes = body.to_bytes()?;
        let key = Area::Decisions.key(&body.id);
        if let Err(e) = publish_both(backend, session, &key, &bytes)? {
            eprintln!(
                "warning: decision {} of '{session}' isn't repaired: {}",
                body.id, e.message
            );
            continue;
        }
        ids.insert(body.id);
    }
    Ok(())
}

enum PublishError {
    Exists,
    Io(anyhow::Error),
}

/// Write `bytes` to a synced temporary file in `session_dir` (outside
/// `ctx/koto/`) and publish it at `key` under `ctx_dir` with a create-only
/// rename, then sync the area directory.
fn publish(
    session_dir: &Path,
    ctx_dir: &Path,
    key: &str,
    bytes: &[u8],
) -> Result<(), PublishError> {
    let io = |e: std::io::Error, what: &str| PublishError::Io(anyhow::anyhow!("{what}: {e}"));
    let dst = key_path(ctx_dir, key);
    let parent = dst
        .parent()
        .expect("a decision key has a parent")
        .to_path_buf();
    // Refuse an area root or any directory above it that is a symlink, so
    // the write can't be steered outside the session.
    let mut dir = ctx_dir.to_path_buf();
    for component in key.split('/').take(key.split('/').count() - 1) {
        dir = dir.join(component);
        match std::fs::symlink_metadata(&dir) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
                return Err(PublishError::Io(anyhow::anyhow!(
                    "{} isn't a real directory",
                    dir.display()
                )))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&dir)
                    .or_else(|e| {
                        if e.kind() == std::io::ErrorKind::AlreadyExists {
                            Ok(())
                        } else {
                            Err(e)
                        }
                    })
                    .map_err(|e| io(e, "creating the decision area"))?;
            }
            Err(e) => return Err(io(e, "reading the decision area")),
        }
    }
    let mut tmp = tempfile::Builder::new()
        .prefix(".koto-decision-")
        .suffix(".tmp")
        .tempfile_in(session_dir)
        .map_err(|e| io(e, "creating a temporary decision file"))?;
    tmp.write_all(bytes)
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|e| io(e, "writing the temporary decision file"))?;
    let (_, tmp_path) = tmp
        .keep()
        .map_err(|e| io(e.error, "keeping the temporary decision file"))?;
    match crate::engine::atomic_fs::atomic_create_rename_strict(&tmp_path, &dst) {
        Ok(()) => {}
        Err(crate::engine::atomic_fs::AtomicCreateError::Collision) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(PublishError::Exists);
        }
        Err(crate::engine::atomic_fs::AtomicCreateError::Io(e)) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(io(e, "publishing the decision"));
        }
    }
    if let Ok(d) = std::fs::File::open(&parent) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_keys_round_trip() {
        assert_eq!(
            Area::of_key("koto/decisions/000000000001"),
            Some((Area::Decisions, "000000000001".to_string()))
        );
        assert_eq!(
            Area::of_key("koto/aimed/abc/000000000002"),
            Some((Area::Aimed("abc".to_string()), "000000000002".to_string()))
        );
        assert_eq!(Area::of_key("koto/decisions/a/b"), None);
        assert_eq!(Area::of_key("koto/links/x"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_area_root_makes_the_listing_unreadable() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = dir.path().join("ctx");
        std::fs::create_dir_all(ctx.join("koto")).unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, ctx.join("koto/decisions")).unwrap();
        assert!(list_local(&ctx).problem.is_some());
    }

    #[test]
    fn publish_never_replaces() {
        let dir = tempfile::TempDir::new().unwrap();
        let ctx = dir.path().join("ctx");
        std::fs::create_dir_all(&ctx).unwrap();
        let key = "koto/decisions/000000000001";
        assert!(publish(dir.path(), &ctx, key, b"one").is_ok());
        assert!(matches!(
            publish(dir.path(), &ctx, key, b"two"),
            Err(PublishError::Exists)
        ));
        assert_eq!(std::fs::read(ctx.join(key)).unwrap(), b"one");
        // No temporary file is left behind in the session directory.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".koto-decision-")
            })
            .collect();
        assert!(leftovers.is_empty());
    }
}
