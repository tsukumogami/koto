//! [`SessionFacts`]: what the graph scan reads about one session, and only
//! that: the header's first line, the `koto/` entries of the manifest plus
//! the link files it doesn't list yet, each link body, and `chain/parent`.
//!
//! Every path is checked with `lstat` before it is read, so a symlinked
//! session directory, `ctx/`, `ctx/koto/` or anything under it reads as
//! unreadable rather than being followed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::engine::types::{SessionKind, StateFileHeader};
use crate::session::anchor::OBSERVED_KEY;
use crate::session::context::Manifest;
use crate::session::links::{LinkKind, LINKS_PREFIX, LINK_BODY_VERSION, MAX_LINK_BODY_BYTES};
use crate::session::pin::{self, PinFact, PIN_KEY};
use crate::session::state_file_name;

/// The key `koto session import` writes into the copy it creates, naming
/// the session it came from (Decision 6).
pub const IMPORTED_FROM_KEY: &str = "koto/imported-from";

/// The largest header line read; a header is far shorter.
const MAX_HEADER_BYTES: u64 = 256 * 1024;

/// The largest `chain/parent` value read; a session name is far shorter.
const MAX_CHAIN_PARENT_BYTES: u64 = 1024;

/// How deep the walk under `ctx/koto/` goes. The reserved area is
/// `koto/links/<kind>/<peer>` at its deepest.
const MAX_KOTO_DEPTH: usize = 4;

/// One link a session holds, as the graph uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFact {
    /// The kind's key component: `dispatched-by`, `held-by`, `closes`, or
    /// one this build doesn't know, which still counts as an edge.
    pub kind: String,
    /// The session the link names.
    pub peer: String,
    /// The peer's session id the link recorded, when it did.
    pub session_id: Option<String>,
}

/// What `koto/imported-from` says the session was copied from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ImportedFrom {
    pub from_session: String,
    pub from_session_id: String,
}

/// What the graph scan read about one session.
#[derive(Debug, Clone, Default)]
pub struct SessionFacts {
    pub name: String,
    /// The header's `kind`; an unknown one, or an unreadable header, reads
    /// as an ordinary session.
    pub kind: SessionKind,
    /// The header's `session_id`, when it isn't empty.
    pub session_id: Option<String>,
    pub parent_workflow: Option<String>,
    pub created_at: Option<String>,
    /// Every link the session holds, in key order.
    pub links: Vec<LinkFact>,
    /// A well-formed `chain/parent`.
    pub chain_parent: Option<String>,
    /// A readable `koto/imported-from`.
    pub imported_from: Option<ImportedFrom>,
    /// Whether the manifest lists an observed marker.
    pub observed: bool,
    /// The session's pin (`koto/links/pin`). Never a link and never one of
    /// its `problems`: a pin, readable or not, keeps this session only.
    pub pin: PinFact,
    /// What makes this session's own component unreadable: a malformed
    /// link or `chain/parent`, a symlink or special file under `ctx/koto/`.
    pub problems: Vec<String>,
    /// What makes every component unreadable: this session's header,
    /// manifest or links directory can't be read, so it may name any
    /// session.
    pub store_problems: Vec<String>,
}

/// What a session's `chain/parent` context key holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChainParent {
    Absent,
    Names(String),
    Malformed(String),
}

/// The session's header, from the first line of its state file.
pub(crate) fn read_header_line(session_dir: &Path, name: &str) -> Result<StateFileHeader, String> {
    let path = session_dir.join(state_file_name(name));
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => return Err("its state file is a symlink or not a regular file".to_string()),
        Err(e) => return Err(format!("its state file can't be read: {e}")),
    }
    let file = fs::File::open(&path).map_err(|e| format!("its state file can't be read: {e}"))?;
    let mut line = String::new();
    BufReader::new(file.take(MAX_HEADER_BYTES))
        .read_line(&mut line)
        .map_err(|e| format!("its header can't be read: {e}"))?;
    serde_json::from_str(line.trim()).map_err(|_| "its header can't be parsed".to_string())
}

/// Read `chain/parent` from a session directory's local context. It is
/// well formed only as a regular file holding exactly one session name,
/// with nothing around it.
pub(crate) fn read_chain_parent(session_dir: &Path) -> ChainParent {
    let path = session_dir.join("ctx").join("chain").join("parent");
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ChainParent::Absent,
        Err(e) => return ChainParent::Malformed(format!("it can't be read: {e}")),
    };
    if !meta.file_type().is_file() {
        return ChainParent::Malformed("it isn't a regular file".to_string());
    }
    if meta.len() > MAX_CHAIN_PARENT_BYTES {
        return ChainParent::Malformed("it is too long to be a session name".to_string());
    }
    let Ok(bytes) = fs::read(&path) else {
        return ChainParent::Malformed("it can't be read".to_string());
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return ChainParent::Malformed("it isn't UTF-8".to_string());
    };
    if text.trim() != text || crate::session::validate::validate_session_id(&text).is_err() {
        return ChainParent::Malformed("it isn't exactly one session name".to_string());
    }
    ChainParent::Names(text)
}

/// Whether a malformed `chain/parent` is `name` with whitespace around it.
pub(crate) fn chain_parent_mentions(session_dir: &Path, name: &str) -> bool {
    let path = session_dir.join("ctx").join("chain").join("parent");
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_file() && meta.len() <= MAX_CHAIN_PARENT_BYTES => {}
        _ => return false,
    }
    fs::read(&path)
        .map(|b| String::from_utf8_lossy(&b).trim() == name)
        .unwrap_or(false)
}

/// The kind of what sits at `path`, without following a symlink.
enum Entry {
    Missing,
    Dir,
    File,
    Other,
}

fn entry_at(path: &Path) -> Entry {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_dir() => Entry::Dir,
        Ok(m) if m.file_type().is_file() => Entry::File,
        Ok(_) => Entry::Other,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Entry::Missing,
        Err(_) => Entry::Other,
    }
}

/// Read at most one byte more than [`MAX_LINK_BODY_BYTES`] of the file at
/// `path`, so an oversized body is seen as such without reading it all.
fn read_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    fs::File::open(path)?
        .take(MAX_LINK_BODY_BYTES as u64 + 1)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

/// A link body as the engine reads it: any version, keeping only what it
/// needs.
#[derive(Deserialize)]
struct AnyLinkBody {
    v: Option<serde_json::Value>,
    session: Option<String>,
    session_id: Option<String>,
}

/// What the link file at `path` names, for [`super::edge_of`]: the session
/// its body names and the session id it recorded. `Ok(None)` for a file
/// that names nothing the graph could join (empty, oversized or malformed,
/// which the graph reads as a problem of its holder, never as an edge);
/// `Err` for a file that can't be read.
pub(crate) fn link_file_names(
    path: &Path,
) -> std::io::Result<Option<(Option<String>, Option<String>)>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => return Ok(None),
        Err(e) => return Err(e),
    }
    let body = read_bounded(path)?;
    if body.is_empty() || body.len() > MAX_LINK_BODY_BYTES {
        return Ok(None);
    }
    Ok(serde_json::from_slice::<AnyLinkBody>(&body)
        .ok()
        .map(|b| (b.session, b.session_id)))
}

impl SessionFacts {
    /// Read session `name` under `base`.
    pub fn read(base: &Path, name: &str) -> SessionFacts {
        let mut facts = SessionFacts {
            name: name.to_string(),
            ..Default::default()
        };
        let dir = base.join(name);
        match entry_at(&dir) {
            Entry::Dir => {}
            Entry::Missing => {
                facts
                    .store_problems
                    .push("its session directory is gone".to_string());
                return facts;
            }
            _ => {
                facts.store_problems.push(
                    "its session directory is a symlink, not a directory, or can't be read"
                        .to_string(),
                );
                return facts;
            }
        }
        match read_header_line(&dir, name) {
            Ok(header) => {
                facts.kind = header.session_kind();
                facts.session_id = Some(header.session_id).filter(|id| !id.is_empty());
                facts.parent_workflow = header.parent_workflow;
                facts.created_at = Some(header.created_at);
            }
            Err(why) => facts.store_problems.push(why),
        }
        facts.read_context(&dir);
        match read_chain_parent(&dir) {
            ChainParent::Absent => {}
            ChainParent::Names(p) => facts.chain_parent = Some(p),
            ChainParent::Malformed(why) => facts
                .problems
                .push(format!("its chain/parent context key is malformed: {why}")),
        }
        facts
    }

    /// Read the `koto/` area: the manifest's entries and the files under
    /// `ctx/koto/`.
    fn read_context(&mut self, dir: &Path) {
        let ctx = dir.join("ctx");
        match entry_at(&ctx) {
            Entry::Missing => return,
            Entry::Dir => {}
            _ => {
                self.store_problems.push(
                    "its ctx directory is a symlink, not a directory, or can't be read".to_string(),
                );
                return;
            }
        }
        let manifest = match entry_at(&ctx.join("manifest.json")) {
            Entry::Missing => Manifest::default(),
            Entry::File => match fs::read(ctx.join("manifest.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok())
            {
                Some(m) => m,
                None => {
                    self.store_problems
                        .push("its context manifest can't be read".to_string());
                    return;
                }
            },
            _ => {
                self.store_problems.push(
                    "its context manifest is a symlink, not a regular file, or can't be read"
                        .to_string(),
                );
                return;
            }
        };
        let listed: BTreeSet<&str> = manifest
            .keys
            .keys()
            .map(String::as_str)
            .filter(|k| k.starts_with(crate::session::links::RESERVED_PREFIX))
            .collect();
        self.observed = listed.contains(OBSERVED_KEY);

        // The pin, before the link walk and kept out of it: whatever sits at
        // its path is a fact of this session alone.
        self.pin = pin::read_at(dir, listed.iter().any(|k| pin::is_pin_key(k)));

        // Every regular file under ctx/koto/, by its key.
        let mut files: BTreeMap<String, PathBuf> = BTreeMap::new();
        let koto = ctx.join("koto");
        match entry_at(&koto) {
            Entry::Missing => {}
            Entry::Dir => {
                if let Err(why) = self.walk(&koto, "koto", 1, &mut files) {
                    self.store_problems.push(why);
                    return;
                }
            }
            _ => {
                self.store_problems.push(
                    "its ctx/koto directory is a symlink, not a directory, or can't be read"
                        .to_string(),
                );
                return;
            }
        }

        if listed.contains(IMPORTED_FROM_KEY) {
            self.imported_from = files
                .get(IMPORTED_FROM_KEY)
                .and_then(|path| read_bounded(path).ok())
                .filter(|b| b.len() <= MAX_LINK_BODY_BYTES)
                .and_then(|b| serde_json::from_slice(&b).ok());
        }

        // The links: every key the manifest lists in the links area, plus
        // the non-empty dispatched-by and held-by files it doesn't list yet
        // (a write that landed its file but not its manifest entry). An
        // unlisted `closes` file never counts: only a whole close session
        // may close an anchor. Empty unlisted files are the store's own
        // per-key lock files.
        let mut keys: BTreeSet<String> = listed
            .iter()
            .filter(|k| k.starts_with(LINKS_PREFIX) && !pin::is_pin_key(k))
            .map(|k| k.to_string())
            .collect();
        let mut bodies: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for (key, path) in &files {
            let Some(rest) = key.strip_prefix(LINKS_PREFIX) else {
                continue;
            };
            let kind = rest.split('/').next().unwrap_or("");
            let unlisted_ok =
                kind == LinkKind::DispatchedBy.as_str() || kind == LinkKind::HeldBy.as_str();
            if !keys.contains(key) && !unlisted_ok {
                continue;
            }
            match read_bounded(path) {
                Ok(body) => {
                    if !body.is_empty() {
                        keys.insert(key.clone());
                    }
                    bodies.insert(key.clone(), body);
                }
                Err(e) => self.problems.push(format!("{key} can't be read: {e}")),
            }
        }
        for key in keys {
            self.read_link(&key, bodies.get(&key));
        }
    }

    /// Collect the regular files under `dir` (whose key is `prefix`) into
    /// `files`. A symlink or special file is a problem of this session; a
    /// directory that can't be listed is unreadable for the whole store.
    fn walk(
        &mut self,
        dir: &Path,
        prefix: &str,
        depth: usize,
        files: &mut BTreeMap<String, PathBuf>,
    ) -> Result<(), String> {
        let entries = fs::read_dir(dir).map_err(|e| format!("{prefix}/ can't be listed: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{prefix}/ can't be listed: {e}"))?;
            let Some(file_name) = entry.file_name().to_str().map(str::to_string) else {
                self.problems
                    .push(format!("{prefix}/ holds a name that isn't UTF-8"));
                continue;
            };
            let key = format!("{prefix}/{file_name}");
            // The pin is read on its own (`pin::read_at`), whatever its type.
            if key == PIN_KEY {
                continue;
            }
            let path = entry.path();
            // The entry's own type, which never follows a symlink.
            match entry.file_type() {
                Ok(t) if t.is_file() => {
                    files.insert(key, path);
                }
                Ok(t) if t.is_dir() && depth < MAX_KOTO_DEPTH => {
                    self.walk(&path, &key, depth + 1, files)?
                }
                Ok(t) if t.is_dir() => self.problems.push(format!("{key} is nested too deep")),
                Ok(_) => self
                    .problems
                    .push(format!("{key} is a symlink or not a regular file")),
                Err(e) => self.problems.push(format!("{key} can't be read: {e}")),
            }
        }
        Ok(())
    }

    /// Read one link key and its file.
    fn read_link(&mut self, key: &str, body: Option<&Vec<u8>>) {
        let rest = &key[LINKS_PREFIX.len()..];
        let Some((kind, peer)) = rest.split_once('/') else {
            self.problems
                .push(format!("{key} isn't a link key (koto/links/<kind>/<peer>)"));
            return;
        };
        if kind.is_empty() || peer.is_empty() || peer.contains('/') {
            self.problems
                .push(format!("{key} isn't a link key (koto/links/<kind>/<peer>)"));
            return;
        }
        let valid_peer = crate::discover::validate_workflow_name(peer).is_ok();
        if !valid_peer {
            self.problems
                .push(format!("{key} names a peer that isn't a session name"));
        }
        let mut link = LinkFact {
            kind: kind.to_string(),
            peer: peer.to_string(),
            session_id: None,
        };
        match body {
            None => self
                .problems
                .push(format!("{key} is listed but its file is missing")),
            Some(b) if b.len() > MAX_LINK_BODY_BYTES => self
                .problems
                .push(format!("{key} is larger than {MAX_LINK_BODY_BYTES} bytes")),
            Some(b) => match serde_json::from_slice::<AnyLinkBody>(b).ok() {
                None => self.problems.push(format!("{key} is malformed")),
                Some(body) => {
                    let current = body.v.as_ref().and_then(|v| v.as_u64())
                        == Some(u64::from(LINK_BODY_VERSION));
                    match (current, body.session) {
                        // Today's version: its body names the key's peer.
                        (true, Some(session)) if session == peer => {
                            link.session_id = body.session_id;
                        }
                        (true, _) => self.problems.push(format!(
                            "{key} is malformed: its body names another session"
                        )),
                        // A version this build doesn't know is an edge by the
                        // `session` its body names, when that parses.
                        (false, Some(session))
                            if crate::discover::validate_workflow_name(&session).is_ok() =>
                        {
                            link.peer = session;
                            link.session_id = body.session_id;
                        }
                        (false, _) => self.problems.push(format!(
                            "{key} has a body version this koto doesn't know and names no session"
                        )),
                    }
                }
            },
        }
        if crate::discover::validate_workflow_name(&link.peer).is_ok() {
            self.links.push(link);
        }
    }
}
