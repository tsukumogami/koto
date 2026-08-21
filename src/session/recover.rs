//! Bring sessions the old-layout migration had to set aside back into reach.
//!
//! The migration flattens `base/<repo-id>/<name>/` into `base/<name>/`. A name
//! reused across repositories therefore has several sources and one
//! destination, and only the first source can have it. The rest are moved to
//! `base/.migration-conflicts/<repo-id>/<name>/`, where nothing is lost but
//! nothing is reachable either: `list()` does not report them, `koto next`
//! cannot name them, and no amount of re-running the migration changes that.
//!
//! This module is the way out. It walks the quarantine, and moves each session
//! it finds back into the flat namespace under `r<repo-id>-<name>` -- the one
//! piece of information the old layout still carries about where the session
//! came from, in the one position that keeps a family together.
//!
//! **Why a prefix and not a suffix.** A session's parent is the dotted prefix
//! of its own id: `deploy.stage-2` is a child of `deploy`. Suffixing would
//! rewrite `deploy` to `deploy-r<id>` while leaving `deploy.stage-2` reading as
//! a child of whatever unrelated `deploy` won the flat name. Prefixing rewrites
//! the pair to `r<id>-deploy` and `r<id>-deploy.stage-2`, and the child still
//! points at its own parent. The leading `r` is there because a session id must
//! start with a letter and a repo-id may start with a digit.
//!
//! Nothing here deletes. A quarantined directory is moved, and only ever to a
//! name nothing else occupies; a directory this module cannot recover is
//! reported and left exactly where it is.

use std::fs;
use std::path::{Path, PathBuf};

use crate::session::local::{rename_state_file, rewrite_header_identity, MIGRATION_CONFLICT_DIR};
use crate::session::state_file_name;
use crate::session::validate::validate_session_id;

/// Largest disambiguating suffix tried when the recovered name is taken.
///
/// Mirrors the ceiling the migration's own `quarantine_destination` uses.
/// Reaching it means a thousand same-named sessions from one repository,
/// which is a condition to report rather than to keep grinding at.
const MAX_NAME_ATTEMPTS: u32 = 1000;

/// One directory sitting in the migration quarantine.
#[derive(Debug, Clone)]
pub struct Quarantined {
    /// The repo-id whose old-layout directory this session came out of.
    pub repo_id: String,

    /// Where the directory is right now.
    pub path: PathBuf,

    /// The session id the directory's state file is named for, or `None`
    /// when the directory holds no state file and so is not a session.
    ///
    /// Read from the state file rather than from the directory name because
    /// the two can differ: a directory quarantined a second time lands at
    /// `<name>.1` while its state file keeps the original `<name>`.
    pub session_id: Option<String>,
}

impl Quarantined {
    /// The id this session would be restored under, before checking whether
    /// anything already holds that name.
    ///
    /// `None` when the directory is not a session, or when the name it would
    /// produce is not a legal session id.
    pub fn proposed_id(&self) -> Option<String> {
        let session_id = self.session_id.as_deref()?;
        let candidate = recovered_id(&self.repo_id, session_id);
        validate_session_id(&candidate).ok()?;
        Some(candidate)
    }
}

/// What happened to one quarantined directory.
#[derive(Debug)]
pub enum Outcome {
    /// Moved back into the flat namespace under `id`.
    ///
    /// `header_error` is `Some` when the session is where it belongs and its
    /// state file is named correctly, but the `workflow` field inside still
    /// reads the old name. The usual cause is the header being unparseable
    /// already -- koto#193's other finding -- and the move leaves such a
    /// session no less readable than it was in quarantine, so it is reported
    /// rather than undone. It carries the underlying message because "could
    /// not rewrite the header" and "could not parse the header" are not the
    /// same news, and a reader deciding what to do next needs the difference.
    Recovered {
        /// The flat-namespace id the session now answers to.
        id: String,
        /// Why the header's identity fields were left alone, if they were.
        header_error: Option<String>,
    },

    /// Left in place, with the reason. Not an error: a directory with no
    /// state file is not a session, and moving it would only add clutter.
    Skipped(String),

    /// The move was attempted and failed. The directory is still in
    /// quarantine, so the command can be run again.
    Failed(String),
}

/// The flat-namespace id a quarantined session is restored under.
///
/// See the module docs for why the repo-id leads.
pub fn recovered_id(repo_id: &str, session_id: &str) -> String {
    format!("r{repo_id}-{session_id}")
}

/// The quarantine container under `base`.
pub fn quarantine_root(base: &Path) -> PathBuf {
    base.join(MIGRATION_CONFLICT_DIR)
}

/// Every directory currently in the quarantine, sorted by repo-id and then
/// by directory name so two runs report the same order.
///
/// Returns an empty vector when there is no quarantine, which is the normal
/// case for an install that never hit a collision.
pub fn scan(base: &Path) -> Vec<Quarantined> {
    let root = quarantine_root(base);
    let Ok(repos) = fs::read_dir(&root) else {
        return Vec::new();
    };

    let mut found = Vec::new();
    for repo in repos.flatten() {
        if !repo.path().is_dir() {
            continue;
        }
        let Ok(repo_id) = repo.file_name().into_string() else {
            continue;
        };
        let Ok(sessions) = fs::read_dir(repo.path()) else {
            continue;
        };
        for session in sessions.flatten() {
            let path = session.path();
            if !path.is_dir() {
                continue;
            }
            found.push(Quarantined {
                repo_id: repo_id.clone(),
                session_id: session_id_of(&path),
                path,
            });
        }
    }

    found.sort_by(|a, b| (&a.repo_id, &a.path).cmp(&(&b.repo_id, &b.path)));
    found
}

/// The session id a directory's state file is named for.
///
/// Prefers the state file that matches the directory's own name, and falls
/// back to a lone `koto-*.state.jsonl` when the two have drifted apart.
/// Returns `None` for a directory holding no state file, or holding several
/// with no way to tell which one names it.
fn session_id_of(dir: &Path) -> Option<String> {
    if let Some(dir_name) = dir.file_name().and_then(|n| n.to_str()) {
        if dir.join(state_file_name(dir_name)).is_file() {
            return Some(dir_name.to_string());
        }
    }

    let mut ids = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| id_from_state_file_name(&e.file_name().to_string_lossy()));

    let first = ids.next()?;
    match ids.next() {
        Some(_) => None,
        None => Some(first),
    }
}

/// Invert [`state_file_name`]: `koto-<id>.state.jsonl` -> `<id>`.
fn id_from_state_file_name(file_name: &str) -> Option<String> {
    let id = file_name
        .strip_prefix("koto-")?
        .strip_suffix(".state.jsonl")?;
    (!id.is_empty()).then(|| id.to_string())
}

/// Move one quarantined session back into the flat namespace under `base`.
///
/// The directory moves first and its identity is fixed afterwards, so an
/// interrupted recovery leaves the session under its new directory rather
/// than half-renamed in the quarantine. When fixing the identity fails, the
/// directory is put back where it was so the command can simply be re-run.
pub fn recover_one(base: &Path, entry: &Quarantined) -> Outcome {
    let Some(session_id) = entry.session_id.as_deref() else {
        return Outcome::Skipped("no state file: not a session".to_string());
    };

    let candidate = recovered_id(&entry.repo_id, session_id);
    if let Err(e) = validate_session_id(&candidate) {
        return Outcome::Skipped(format!("would not be a valid session id: {e}"));
    }

    let Some(id) = free_id(base, &candidate) else {
        return Outcome::Failed(format!(
            "no free name left near {} after {} attempts",
            candidate, MAX_NAME_ATTEMPTS
        ));
    };

    let dest = base.join(&id);
    if let Err(e) = fs::rename(&entry.path, &dest) {
        return Outcome::Failed(format!(
            "could not move {} to {}: {}",
            entry.path.display(),
            dest.display(),
            e
        ));
    }

    if let Err(e) = rename_state_file(&dest, session_id, &id) {
        // The session would be invisible under its new name with the old
        // state filename, so put it back rather than leave it stranded
        // somewhere new.
        let _ = fs::rename(&dest, &entry.path);
        return Outcome::Failed(format!("{e:#}"));
    }

    // A header that will not parse is the corruption koto#193 also reports.
    // It does not make the session less reachable than it is now, so it is
    // recorded rather than treated as a failure -- undoing the move would
    // put a recoverable session back out of reach to protect a field that
    // was already unreadable. The message rides along so the report can say
    // which failure this was rather than assuming the common one.
    let header_error = rewrite_header_identity(&dest, &id)
        .err()
        .map(|e| format!("{e:#}"));

    // Best-effort tidy: both calls only succeed on an empty directory.
    if let Some(repo_dir) = entry.path.parent() {
        let _ = fs::remove_dir(repo_dir);
    }
    let _ = fs::remove_dir(quarantine_root(base));

    Outcome::Recovered { id, header_error }
}

/// The first unused name at or after `candidate`: `candidate`, then
/// `candidate-2`, `candidate-3`, and so on.
///
/// The suffix uses `-` rather than `.` because `.` is the parent separator,
/// and a numbered `.` suffix would read as a child of the unnumbered name.
fn free_id(base: &Path, candidate: &str) -> Option<String> {
    if !base.join(candidate).exists() {
        return Some(candidate.to_string());
    }
    for n in 2..MAX_NAME_ATTEMPTS {
        let numbered = format!("{candidate}-{n}");
        if !base.join(&numbered).exists() {
            return Some(numbered);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::local::LocalBackend;
    use crate::session::SessionBackend;
    use tempfile::TempDir;

    /// Write a quarantined session: `base/.migration-conflicts/<repo>/<dir>/`
    /// holding a state file named for `session_id`.
    fn quarantine(
        base: &Path,
        repo_id: &str,
        dir: &str,
        session_id: &str,
        header: &str,
    ) -> PathBuf {
        let path = quarantine_root(base).join(repo_id).join(dir);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(state_file_name(session_id)), header).unwrap();
        path
    }

    /// A parseable header line for `workflow`, with an optional parent.
    fn header_line(workflow: &str, parent: Option<&str>) -> String {
        let parent = match parent {
            Some(p) => format!(r#","parent_workflow":"{p}""#),
            None => String::new(),
        };
        format!(
            r#"{{"schema_version":1,"workflow":"{workflow}","template_hash":"abc","created_at":"2026-01-01T00:00:00Z"{parent}}}
"#
        )
    }

    fn read_header(base: &Path, id: &str) -> serde_json::Value {
        let path = base.join(id).join(state_file_name(id));
        let content = fs::read_to_string(path).unwrap();
        serde_json::from_str(content.lines().next().unwrap()).unwrap()
    }

    #[test]
    fn scan_finds_nothing_when_there_is_no_quarantine() {
        let tmp = TempDir::new().unwrap();
        assert!(scan(tmp.path()).is_empty());
    }

    #[test]
    fn scan_reads_the_session_id_from_the_state_file() {
        let tmp = TempDir::new().unwrap();
        quarantine(
            tmp.path(),
            "abcdef1234567890",
            "deploy",
            "deploy",
            &header_line("deploy", None),
        );

        let found = scan(tmp.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].repo_id, "abcdef1234567890");
        assert_eq!(found[0].session_id.as_deref(), Some("deploy"));
        assert_eq!(
            found[0].proposed_id().as_deref(),
            Some("rabcdef1234567890-deploy")
        );
    }

    #[test]
    fn scan_reads_through_a_renumbered_quarantine_directory() {
        // A second quarantine of the same name lands at `<name>.1`, but the
        // state file inside still carries the original name.
        let tmp = TempDir::new().unwrap();
        quarantine(
            tmp.path(),
            "abcdef1234567890",
            "deploy.1",
            "deploy",
            &header_line("deploy", None),
        );

        let found = scan(tmp.path());
        assert_eq!(found[0].session_id.as_deref(), Some("deploy"));
    }

    #[test]
    fn a_directory_with_no_state_file_is_left_alone() {
        let tmp = TempDir::new().unwrap();
        let path = quarantine_root(tmp.path())
            .join("abcdef1234567890")
            .join("not-a-session");
        fs::create_dir_all(&path).unwrap();

        let found = scan(tmp.path());
        assert_eq!(found.len(), 1);
        assert!(found[0].session_id.is_none());

        let outcome = recover_one(tmp.path(), &found[0]);
        assert!(matches!(outcome, Outcome::Skipped(_)), "{outcome:?}");
        assert!(path.exists(), "a skipped directory must not be moved");
    }

    #[test]
    fn a_recovered_session_is_listable_again() {
        let tmp = TempDir::new().unwrap();
        // The flat name is taken, which is why this one was quarantined.
        let taken = tmp.path().join("deploy");
        fs::create_dir_all(&taken).unwrap();
        fs::write(
            taken.join(state_file_name("deploy")),
            header_line("deploy", None),
        )
        .unwrap();
        quarantine(
            tmp.path(),
            "abcdef1234567890",
            "deploy",
            "deploy",
            &header_line("deploy", None),
        );

        let found = scan(tmp.path());
        let outcome = recover_one(tmp.path(), &found[0]);
        let Outcome::Recovered { id, header_error } = outcome else {
            panic!("expected a recovery, got {outcome:?}");
        };
        assert_eq!(id, "rabcdef1234567890-deploy");
        assert_eq!(header_error, None);

        let backend = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        let ids: Vec<String> = backend.list().unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            vec!["deploy".to_string(), "rabcdef1234567890-deploy".to_string()]
        );
        assert!(backend.exists("rabcdef1234567890-deploy"));
        // The session that held the flat name is untouched.
        assert!(backend.exists("deploy"));
    }

    #[test]
    fn recovery_rewrites_the_header_to_the_new_identity() {
        let tmp = TempDir::new().unwrap();
        quarantine(
            tmp.path(),
            "abcdef1234567890",
            "deploy",
            "deploy",
            &header_line("deploy", None),
        );

        let found = scan(tmp.path());
        recover_one(tmp.path(), &found[0]);

        let header = read_header(tmp.path(), "rabcdef1234567890-deploy");
        assert_eq!(header["workflow"], "rabcdef1234567890-deploy");
        assert!(header.get("parent_workflow").is_none());
    }

    #[test]
    fn a_family_recovers_together() {
        // Prefixing is what keeps the child pointing at its own parent.
        let tmp = TempDir::new().unwrap();
        let repo = "abcdef1234567890";
        quarantine(
            tmp.path(),
            repo,
            "deploy",
            "deploy",
            &header_line("deploy", None),
        );
        quarantine(
            tmp.path(),
            repo,
            "deploy.stage-2",
            "deploy.stage-2",
            &header_line("deploy.stage-2", Some("deploy")),
        );

        for entry in scan(tmp.path()) {
            recover_one(tmp.path(), &entry);
        }

        let child = read_header(tmp.path(), "rabcdef1234567890-deploy.stage-2");
        assert_eq!(child["workflow"], "rabcdef1234567890-deploy.stage-2");
        assert_eq!(child["parent_workflow"], "rabcdef1234567890-deploy");
        // And the parent it names is really there.
        let backend = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        assert!(backend.exists("rabcdef1234567890-deploy"));
    }

    #[test]
    fn two_repos_recover_the_same_name_side_by_side() {
        let tmp = TempDir::new().unwrap();
        for repo in ["abcdef1234567890", "0123456789abcdef"] {
            quarantine(
                tmp.path(),
                repo,
                "deploy",
                "deploy",
                &header_line("deploy", None),
            );
        }

        for entry in scan(tmp.path()) {
            recover_one(tmp.path(), &entry);
        }

        let backend = LocalBackend::with_base_dir(tmp.path().to_path_buf());
        assert!(backend.exists("rabcdef1234567890-deploy"));
        assert!(backend.exists("r0123456789abcdef-deploy"));
    }

    #[test]
    fn recovery_never_writes_over_an_existing_session() {
        let tmp = TempDir::new().unwrap();
        let repo = "abcdef1234567890";
        // Something already sits on the name recovery would pick.
        let occupied = tmp.path().join(recovered_id(repo, "deploy"));
        fs::create_dir_all(&occupied).unwrap();
        fs::write(occupied.join("mine.txt"), b"mine").unwrap();
        quarantine(
            tmp.path(),
            repo,
            "deploy",
            "deploy",
            &header_line("deploy", None),
        );

        let found = scan(tmp.path());
        let outcome = recover_one(tmp.path(), &found[0]);
        let Outcome::Recovered { id, .. } = outcome else {
            panic!("expected a recovery, got {outcome:?}");
        };
        assert_eq!(id, "rabcdef1234567890-deploy-2");
        assert_eq!(fs::read(occupied.join("mine.txt")).unwrap(), b"mine");
    }

    #[test]
    fn a_session_with_an_unreadable_header_is_still_moved_back_into_reach() {
        let tmp = TempDir::new().unwrap();
        // koto#193's other finding: state files written headerless by an
        // older koto. The header cannot be rewritten, but the session must
        // not stay stranded because of it.
        quarantine(
            tmp.path(),
            "abcdef1234567890",
            "headerless",
            "headerless",
            "{\"seq\":1,\"type\":\"context_added\"}\n",
        );

        let found = scan(tmp.path());
        let outcome = recover_one(tmp.path(), &found[0]);
        let Outcome::Recovered { id, header_error } = outcome else {
            panic!("expected a recovery, got {outcome:?}");
        };
        assert_eq!(id, "rabcdef1234567890-headerless");
        let header_error = header_error.expect("an unparseable header cannot be rewritten");
        assert!(
            header_error.contains("failed to read header"),
            "the report should say which failure this was, got {header_error:?}"
        );
        // The state file is named for its directory, which is what makes the
        // session addressable at all.
        assert!(tmp.path().join(&id).join(state_file_name(&id)).is_file());
    }

    #[test]
    fn recovering_everything_drains_the_quarantine() {
        let tmp = TempDir::new().unwrap();
        quarantine(
            tmp.path(),
            "abcdef1234567890",
            "deploy",
            "deploy",
            &header_line("deploy", None),
        );

        for entry in scan(tmp.path()) {
            recover_one(tmp.path(), &entry);
        }

        assert!(scan(tmp.path()).is_empty());
        assert!(
            !quarantine_root(tmp.path()).exists(),
            "an emptied quarantine should not linger"
        );
    }

    #[test]
    fn contents_survive_the_move_byte_for_byte() {
        let tmp = TempDir::new().unwrap();
        let dir = quarantine(
            tmp.path(),
            "abcdef1234567890",
            "deploy",
            "deploy",
            &header_line("deploy", None),
        );
        fs::create_dir_all(dir.join("ctx")).unwrap();
        fs::write(dir.join("ctx").join("notes.md"), b"kept").unwrap();

        let found = scan(tmp.path());
        recover_one(tmp.path(), &found[0]);

        let recovered = tmp.path().join("rabcdef1234567890-deploy");
        assert_eq!(
            fs::read(recovered.join("ctx").join("notes.md")).unwrap(),
            b"kept"
        );
    }

    #[test]
    fn id_inversion_round_trips() {
        assert_eq!(
            id_from_state_file_name(&state_file_name("deploy.stage-2")).as_deref(),
            Some("deploy.stage-2")
        );
        assert_eq!(id_from_state_file_name("notes.md"), None);
        assert_eq!(id_from_state_file_name("koto-.state.jsonl"), None);
    }
}
