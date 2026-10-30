//! `koto session prune --migrated-from <workspace>`: remove the migration
//! markers `koto session import` left under a source workspace's bucket
//! prefix, each with the old copy beside it.
//!
//! A marker is what makes the source host refuse a session it no longer
//! owns, and the source is the copy's read-only predecessor, so both go
//! only when the copy's lineage no longer needs them: the marker's target
//! must be `present` (the target's state file header carries the session id
//! the marker names, and its whole log reads with no error and begins with
//! every event of the source's log, unchanged, so a truncated or corrupt
//! copy, or one behind or apart from the source, never stands in for it),
//! and then the continues relation must no longer keep the source: the
//! bucket walk from the source, with the marker's target passed in as a
//! later member, finds no later session with standing (R15). While one
//! has standing the marker is kept with reason `relation` and the
//! relation's report fields (`kept_by_relation`, `relation_keepers`). Any
//! other target status keeps the marker, and so does a marker that can't be
//! read.
//!
//! `session.cloud.marker_retain_days` and `--source-retired` no longer
//! decide removal. Both are still accepted; a run that sets either lists it
//! under `ignored` in its output and says so on stderr.
//!
//! Removal deletes everything under `<source prefix>/<name>/`, the marker
//! last, so a removal that stops part way leaves the marker refusing. The
//! apply judges each marker again just before its removal, but nothing
//! locks the bucket between that judgement and the deletes.
//! No local session is read or removed; the only local file read or
//! written is the stored plan.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    exit_with_error_code, is_token, read_plan_file, remove_plan_file, token_over, usage,
    write_plan_file, PruneArgs, EXIT_PLAN_REFUSED, EXIT_REMOVAL_FAILED,
};
use crate::session::cloud::{is_workspace_prefix, CloudBackend, TargetStatus};
use crate::session::continues_bucket::{Clock, Pointer};
use crate::session::hierarchy::{self, RelationVerdict};
use crate::session::validate::validate_session_id;
use crate::session::Backend;

/// The `mode` a marker plan records.
pub(super) const MODE: &str = "markers";

const DAY: u64 = 24 * 60 * 60;

/// A marker's target, as the marker names it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Target {
    pub session: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub prefix: String,
}

/// One marker as the preview prints it and a stored plan keeps it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct MarkerEntry {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub migrated_at: Option<String>,
    #[serde(default)]
    pub age_days: Option<u64>,
    #[serde(default)]
    pub target: Option<Target>,
    /// `migrated`, `present`, `replaced`, `absent`, `unverified` or
    /// `unknown`; `None` when the marker itself couldn't be read.
    #[serde(default)]
    pub target_status: Option<String>,
    pub removable: bool,
    /// How many objects the removal deletes under the source copy's prefix,
    /// the marker included; `None` when the marker couldn't be read, or the
    /// copy couldn't be listed or its log couldn't be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects: Option<usize>,
    /// What decided it: the target status when it isn't `present`,
    /// `unreadable` for a marker that can't be read, else `relation` while
    /// a later session keeps the source, or `released` once none does.
    pub reason: String,
    pub detail: String,
    /// For a marker the relation keeps: `kept_by_relation`,
    /// `relation_keepers`, `relation_unreadable` and `bucket_unchecked`
    /// (`hierarchy::relation_json`). Empty, and absent from the JSON,
    /// otherwise.
    #[serde(flatten)]
    pub relation: serde_json::Map<String, serde_json::Value>,
}

impl MarkerEntry {
    /// The line this entry contributes to the plan token.
    fn token_line(&self) -> String {
        format!(
            "marker\t{}\t{}\n",
            self.name,
            self.migrated_at.as_deref().unwrap_or("")
        )
    }

    fn key(&self) -> (String, Option<String>) {
        (self.name.clone(), self.migrated_at.clone())
    }

    fn brief(&self) -> serde_json::Value {
        json!({"kind": self.kind, "name": self.name, "migrated_at": self.migrated_at})
    }

    fn unreadable(name: &str, detail: String) -> Self {
        MarkerEntry {
            kind: "marker".to_string(),
            name: name.to_string(),
            migrated_at: None,
            age_days: None,
            target: None,
            target_status: None,
            removable: false,
            objects: None,
            reason: "unreadable".to_string(),
            detail,
            relation: serde_json::Map::new(),
        }
    }
}

/// A stored marker plan. It lives at `markers-<source prefix>-<token>.json`,
/// a name no session plan can have. `source_retired` records the flag the
/// preview was given; it no longer decides anything, and is kept so a plan
/// file reads the same across releases.
#[derive(Debug, Serialize, Deserialize)]
struct StoredMarkerPlan {
    mode: String,
    plan: String,
    created_at: String,
    migrated_from: String,
    source_prefix: String,
    #[serde(default)]
    source_retired: bool,
    remove: Vec<MarkerEntry>,
}

fn plan_file_name(source_prefix: &str, token: &str) -> String {
    format!("markers-{source_prefix}-{token}.json")
}

/// A marker's body, read from the bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkerBody {
    migrated_at: String,
    at: SystemTime,
    target: Target,
}

/// Parse a marker body. `Err` says why it can't be used: it doesn't parse,
/// has no `migrated_at` that reads as a time, or its target doesn't name a
/// session and a workspace prefix (a marker is untrusted, and its target is
/// what the status listing is built from).
fn parse_marker(bytes: &[u8]) -> std::result::Result<MarkerBody, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("the marker doesn't parse as JSON: {e}"))?;
    let migrated_at = value
        .get("migrated_at")
        .and_then(|v| v.as_str())
        .ok_or("the marker has no migrated_at")?
        .to_string();
    let at = crate::engine::wake::parse_rfc3339_millis(&migrated_at)
        .ok_or_else(|| format!("the marker's migrated_at {migrated_at:?} isn't a time"))?;
    let target = value.get("target").ok_or("the marker has no target")?;
    let field = |name: &str| {
        target
            .get(name)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("the marker's target has no {name}"))
    };
    let target = Target {
        session: field("session")?,
        session_id: field("session_id")?,
        workspace: field("workspace").ok(),
        prefix: field("prefix")?,
    };
    if validate_session_id(&target.session).is_err() {
        return Err(format!(
            "the marker's target session {:?} isn't a session name",
            target.session
        ));
    }
    if !is_workspace_prefix(&target.prefix) {
        return Err(format!(
            "the marker's target prefix {:?} isn't a workspace prefix",
            target.prefix
        ));
    }
    Ok(MarkerBody {
        migrated_at,
        at,
        target,
    })
}

/// The rule: whether a marker whose target stands at `status` is
/// removable, what decided it, and a sentence saying so. `relation` is the
/// walk's verdict for the source, asked only for a present target: while
/// it keeps, so does the marker.
fn decide(
    status: &TargetStatus,
    expected_id: &str,
    relation: Option<&RelationVerdict>,
) -> (bool, &'static str, String) {
    match status {
        TargetStatus::Present => match relation {
            Some(verdict) if verdict.keeps() => (
                false,
                "relation",
                format!(
                    "the target is present, and the source is its read-only predecessor while \
                     a later session has standing ({})",
                    hierarchy::relation_text(verdict).join("; ")
                ),
            ),
            _ => (
                true,
                "released",
                "the target is present, and no later session keeps the source".to_string(),
            ),
        },
        TargetStatus::Migrated => (
            false,
            "migrated",
            "the target was migrated again: it carries a marker of its own".to_string(),
        ),
        TargetStatus::Replaced(id) => (
            false,
            "replaced",
            format!(
                "the target name now holds another session (session id {id:?}, where the marker \
                 names {expected_id:?})"
            ),
        ),
        TargetStatus::Absent => (
            false,
            "absent",
            "the target has no state file in the bucket, so this old copy may be the only one"
                .to_string(),
        ),
        TargetStatus::Unverified(why) => (
            false,
            "unverified",
            format!(
                "the target's copy can't be shown to hold everything the source's does, so \
                 this old copy may be the only complete one: {why}"
            ),
        ),
        TargetStatus::Unknown(why) => (
            false,
            "unknown",
            format!("the target can't be checked: {why}"),
        ),
    }
}

/// Judge one marker now. `None` when it is gone.
fn evaluate(
    cloud: &CloudBackend,
    source_prefix: &str,
    name: &str,
    clock: &Clock,
) -> Option<MarkerEntry> {
    let bytes = match cloud.read_migration_marker(source_prefix, name) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return None,
        Err(e) => {
            return Some(MarkerEntry::unreadable(
                name,
                format!("the marker can't be read: {e:#}"),
            ))
        }
    };
    let body = match parse_marker(&bytes) {
        Ok(body) => body,
        Err(why) => return Some(MarkerEntry::unreadable(name, why)),
    };
    let source = cloud.migrated_source_copy(source_prefix, name);
    let status = cloud.marker_target_status(
        &body.target.prefix,
        &body.target.session,
        &body.target.session_id,
        source.as_ref().map_err(String::as_str),
    );
    // A present target: what keeps the source now is the relation, the
    // marker's target passed in as a later member (R6, R15).
    let relation = matches!(status, TargetStatus::Present).then(|| {
        cloud.relation_walk_with(
            &Pointer {
                prefix: source_prefix.to_string(),
                name: name.to_string(),
            },
            source
                .as_ref()
                .ok()
                .and_then(|copy| copy.session_id.as_deref()),
            &[Pointer {
                prefix: body.target.prefix.clone(),
                name: body.target.session.clone(),
            }],
            clock,
        )
    });
    // A marker dated in the future is as young as it gets.
    let age = clock.now.duration_since(body.at).unwrap_or(Duration::ZERO);
    let (removable, reason, detail) = decide(&status, &body.target.session_id, relation.as_ref());
    Some(MarkerEntry {
        kind: "marker".to_string(),
        name: name.to_string(),
        migrated_at: Some(body.migrated_at),
        age_days: Some(age.as_secs() / DAY),
        target: Some(body.target),
        target_status: Some(status.as_str().to_string()),
        removable,
        objects: source.ok().map(|copy| copy.objects),
        reason: reason.to_string(),
        detail,
        relation: relation
            .filter(|v| v.keeps())
            .map(|v| hierarchy::relation_json(&v))
            .unwrap_or_default(),
    })
}

struct Verdict {
    remove: Vec<MarkerEntry>,
    kept: Vec<MarkerEntry>,
    token: String,
}

fn judge(cloud: &CloudBackend, source_prefix: &str) -> Result<Verdict> {
    cloud.begin_relation_pass();
    let clock = Clock::configured();
    let mut remove = Vec::new();
    let mut kept = Vec::new();
    for name in cloud.migration_marker_names(source_prefix)? {
        let Some(entry) = evaluate(cloud, source_prefix, &name, &clock) else {
            continue;
        };
        if entry.removable {
            remove.push(entry);
        } else {
            kept.push(entry);
        }
    }
    let token = token_over(remove.iter().map(MarkerEntry::token_line).collect());
    Ok(Verdict {
        remove,
        kept,
        token,
    })
}

/// Handle `koto session prune --migrated-from`.
pub(super) fn handle(backend: &Backend, args: &PruneArgs<'_>) -> Result<()> {
    let migrated_from = args.migrated_from.expect("marker mode has --migrated-from");
    if args.execution_dir.is_some() || args.older_than.is_some() || args.all {
        usage(
            "--migrated-from judges migration markers, not local sessions; pass no \
             --execution-dir, --older-than or --all with it",
        );
    }
    if args.plan.is_some() && !args.apply {
        usage("--plan applies a previewed plan; pass it with --apply");
    }
    if let Some(token) = args.plan {
        if !is_token(token) {
            usage(format!(
                "--plan {token:?} isn't a plan token (32 lowercase hex digits)"
            ));
        }
    }
    let Backend::Cloud(cloud) = backend else {
        usage(
            "--migrated-from needs the cloud backend: migration markers live in the bucket, \
             and the local backend has none",
        );
    };
    let source_prefix = crate::session::local::prefix_for_workspace(Path::new(migrated_from))
        .unwrap_or_else(|e| usage(format!("--migrated-from: {e}")));

    let base = backend.local_base_dir().to_path_buf();
    let stored = match args.plan {
        Some(token) => Some(
            read_plan(&base, &source_prefix, token)?
                .unwrap_or_else(|| plan_unknown(token, migrated_from)),
        ),
        None => None,
    };
    // Neither setting decides removal any more (R15): each one the run
    // sets is reported, and the run goes on as it would without it.
    let ignored = ignored_settings(args);
    for name in &ignored {
        eprintln!("ignored: {name} no longer decides removal");
    }

    let verdict = judge(cloud, &source_prefix)?;

    if !args.apply {
        let plan = StoredMarkerPlan {
            mode: MODE.to_string(),
            plan: verdict.token.clone(),
            created_at: crate::engine::types::now_iso8601(),
            migrated_from: migrated_from.to_string(),
            source_prefix: source_prefix.clone(),
            source_retired: args.source_retired,
            remove: verdict.remove.clone(),
        };
        write_plan_file(
            &base,
            &plan_file_name(&source_prefix, &verdict.token),
            &serde_json::to_vec(&plan)?,
        )?;
        let mut out = json!({
            "mode": "preview",
            "migrated_from": migrated_from,
            "prefix": source_prefix,
            "marker_retain_days": args
                .marker_retain_days
                .unwrap_or(crate::config::DEFAULT_MARKER_RETAIN_DAYS),
            "source_retired": args.source_retired,
            "plan": verdict.token,
            "remove": verdict.remove,
            "kept": verdict.kept,
        });
        if !ignored.is_empty() {
            out["ignored"] = json!(ignored);
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    let mut verdict = verdict;
    if let (Some(token), Some(stored)) = (args.plan, &stored) {
        if verdict.token != token {
            let now: BTreeSet<_> = verdict.remove.iter().map(MarkerEntry::key).collect();
            let then: BTreeSet<_> = stored.remove.iter().map(MarkerEntry::key).collect();
            let added: Vec<_> = verdict
                .remove
                .iter()
                .filter(|e| !then.contains(&e.key()))
                .map(MarkerEntry::brief)
                .collect();
            let dropped: Vec<_> = stored
                .remove
                .iter()
                .filter(|e| !now.contains(&e.key()))
                .map(MarkerEntry::brief)
                .collect();
            exit_with_error_code(
                json!({"error": {
                    "code": "prune_plan_stale",
                    "message": format!(
                        "the markers plan {token} would remove have changed since the \
                         preview; nothing was removed"
                    ),
                    "plan": token,
                    "current": verdict.token,
                    "added": added,
                    "dropped": dropped,
                }}),
                EXIT_PLAN_REFUSED,
            );
        }
        // Never remove what the preview didn't name, whatever the hash says.
        let named: BTreeSet<_> = stored.remove.iter().map(MarkerEntry::key).collect();
        verdict.remove.retain(|e| named.contains(&e.key()));
    }

    let outcome = apply(cloud, &source_prefix, &verdict.remove);
    if let Some(token) = args.plan {
        if outcome.failed.is_empty() {
            remove_plan_file(&base, &plan_file_name(&source_prefix, token));
        }
    }
    let mut out = json!({
        "mode": "apply",
        "migrated_from": migrated_from,
        "prefix": source_prefix,
        "plan": verdict.token,
        "removed": outcome.removed,
        "skipped": outcome.skipped,
        "kept": verdict.kept,
        "failed": outcome.failed,
    });
    if !ignored.is_empty() {
        out["ignored"] = json!(ignored);
    }
    println!("{}", serde_json::to_string_pretty(&out)?);
    if !outcome.failed.is_empty() {
        std::process::exit(EXIT_REMOVAL_FAILED);
    }
    Ok(())
}

/// The settings this run sets that no longer decide removal:
/// `marker_retain_days` when a config file sets
/// `session.cloud.marker_retain_days`, and `source_retired` when
/// `--source-retired` was passed.
fn ignored_settings(args: &PruneArgs<'_>) -> Vec<&'static str> {
    let mut ignored = Vec::new();
    if args.marker_retain_days.is_some() {
        ignored.push("marker_retain_days");
    }
    if args.source_retired {
        ignored.push("source_retired");
    }
    ignored
}

#[derive(Default)]
struct Outcome {
    removed: Vec<MarkerEntry>,
    skipped: Vec<serde_json::Value>,
    failed: Vec<serde_json::Value>,
}

/// Remove each entry's source copy, marker last, after reading the marker
/// again and judging it again, the relation included: a marker rewritten
/// since (another `migrated_at`), gone, or no longer removable is skipped
/// with the reason.
fn apply(cloud: &CloudBackend, source_prefix: &str, entries: &[MarkerEntry]) -> Outcome {
    let mut outcome = Outcome::default();
    for entry in entries {
        let skip = |reason: String| json!({"name": entry.name, "reason": reason});
        // Every judgment reads the bucket afresh: a later session that
        // gained standing since the preview keeps the source.
        cloud.begin_relation_pass();
        let fresh = match evaluate(cloud, source_prefix, &entry.name, &Clock::configured()) {
            Some(fresh) => fresh,
            None => {
                outcome
                    .skipped
                    .push(skip("the marker is already gone".to_string()));
                continue;
            }
        };
        if fresh.migrated_at != entry.migrated_at {
            outcome.skipped.push(skip(format!(
                "the marker changed since it was judged (migrated_at now {})",
                fresh.migrated_at.as_deref().unwrap_or("unreadable")
            )));
            continue;
        }
        if !fresh.removable {
            outcome
                .skipped
                .push(skip(format!("now kept: {}", fresh.detail)));
            continue;
        }
        match cloud.delete_migrated_source(source_prefix, &entry.name) {
            Ok(()) => outcome.removed.push(fresh),
            Err(e) => outcome.failed.push(json!({
                "name": entry.name,
                "error": format!(
                    "{e:#}; the marker is still there, so the old copy still refuses; run the \
                     prune again to finish"
                ),
            })),
        }
    }
    outcome
}

fn read_plan(base: &Path, source_prefix: &str, token: &str) -> Result<Option<StoredMarkerPlan>> {
    let Some(bytes) = read_plan_file(base, &plan_file_name(source_prefix, token))? else {
        return Ok(None);
    };
    let plan: StoredMarkerPlan = match serde_json::from_slice(&bytes) {
        Ok(plan) => plan,
        Err(_) => return Ok(None),
    };
    if plan.mode != MODE || plan.plan != token || plan.source_prefix != source_prefix {
        return Ok(None);
    }
    Ok(Some(plan))
}

fn plan_unknown(token: &str, migrated_from: &str) -> ! {
    exit_with_error_code(
        json!({"error": {
            "code": "prune_plan_unknown",
            "message": format!(
                "no stored marker plan {token} for --migrated-from {migrated_from}: it expired, \
                 was applied already, was previewed on another host or for another workspace; \
                 preview again"
            ),
        }}),
        EXIT_PLAN_REFUSED,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "0123456789abcdef";

    fn marker(migrated_at: &str) -> serde_json::Value {
        json!({
            "schema": 1,
            "target": {"session": "foo", "session_id": "id-1", "workspace": "/work/new",
                       "prefix": PREFIX},
            "machine_id": "m",
            "migrated_at": migrated_at,
        })
    }

    #[test]
    fn a_marker_parses_only_with_a_time_and_a_usable_target() {
        let ok = parse_marker(marker("2026-01-01T00:00:00.000Z").to_string().as_bytes()).unwrap();
        assert_eq!(ok.target.session, "foo");
        assert_eq!(ok.target.prefix, PREFIX);
        assert_eq!(ok.target.workspace.as_deref(), Some("/work/new"));
        // The import's own format, and the one without millis.
        assert!(parse_marker(marker("2026-01-01T00:00:00Z").to_string().as_bytes()).is_ok());

        let mut cases: Vec<(Vec<u8>, &str)> = vec![
            (b"not json".to_vec(), "doesn't parse"),
            (br#"{"schema":1}"#.to_vec(), "no migrated_at"),
            (marker("yesterday").to_string().into_bytes(), "isn't a time"),
        ];
        let mut no_target = marker("2026-01-01T00:00:00Z");
        no_target.as_object_mut().unwrap().remove("target");
        cases.push((no_target.to_string().into_bytes(), "no target"));
        let mut no_id = marker("2026-01-01T00:00:00Z");
        no_id["target"]
            .as_object_mut()
            .unwrap()
            .remove("session_id");
        cases.push((no_id.to_string().into_bytes(), "no session_id"));
        let mut bad_session = marker("2026-01-01T00:00:00Z");
        bad_session["target"]["session"] = json!("../x");
        cases.push((bad_session.to_string().into_bytes(), "isn't a session name"));
        let mut bad_prefix = marker("2026-01-01T00:00:00Z");
        bad_prefix["target"]["prefix"] = json!("../../other");
        cases.push((
            bad_prefix.to_string().into_bytes(),
            "isn't a workspace prefix",
        ));
        for (bytes, why) in cases {
            let err = parse_marker(&bytes).unwrap_err();
            assert!(err.contains(why), "{why}: {err}");
        }
    }

    #[test]
    fn a_present_target_is_removable_only_once_the_relation_lets_go() {
        use crate::session::hierarchy::{RelationKeeper, Standing};
        let present = TargetStatus::Present;
        // Nothing later keeps the source: removable, released.
        let (removable, reason, _) = decide(&present, "id", Some(&RelationVerdict::default()));
        assert_eq!((removable, reason), (true, "released"));
        // The copy runs: kept, the relation the reason, the keeper named.
        let mut keeps = RelationVerdict::default();
        keeps.keepers.push(RelationKeeper {
            keeper: "copy".to_string(),
            standing: Standing::Alive,
            retention_passes_at: None,
            pin: crate::session::pin::PinFact::None,
            pin_source: crate::session::pin::SOURCE_BUCKET,
            path: Vec::new(),
        });
        let (removable, reason, detail) = decide(&present, "id", Some(&keeps));
        assert_eq!((removable, reason), (false, "relation"));
        assert!(detail.contains("continued by copy (alive)"), "{detail}");

        // Anything else is kept whatever the relation says, and the status
        // is the reason.
        for (status, word) in [
            (TargetStatus::Migrated, "migrated"),
            (TargetStatus::Replaced("id-2".to_string()), "replaced"),
            (TargetStatus::Absent, "absent"),
            (
                TargetStatus::Unverified("the target's log ends first".to_string()),
                "unverified",
            ),
            (
                TargetStatus::Unknown("listing failed".to_string()),
                "unknown",
            ),
        ] {
            for relation in [None, Some(&keeps)] {
                let (removable, reason, _) = decide(&status, "id", relation);
                assert!(!removable, "{word}");
                assert_eq!(reason, word);
            }
        }
    }

    #[test]
    fn the_settings_that_no_longer_decide_are_reported_ignored() {
        let args = |days: Option<u64>, retired: bool| PruneArgs {
            execution_dir: None,
            older_than: None,
            all: false,
            apply: false,
            plan: None,
            confirm_short_retention: false,
            migrated_from: Some("/w"),
            source_retired: retired,
            marker_retain_days: days,
        };
        assert!(ignored_settings(&args(None, false)).is_empty());
        assert_eq!(
            ignored_settings(&args(Some(0), false)),
            ["marker_retain_days"]
        );
        assert_eq!(ignored_settings(&args(None, true)), ["source_retired"]);
        assert_eq!(
            ignored_settings(&args(Some(30), true)),
            ["marker_retain_days", "source_retired"]
        );
    }

    #[test]
    fn the_marker_token_covers_names_and_migrated_at_in_any_order() {
        let entry = |name: &str, at: &str| MarkerEntry {
            kind: "marker".to_string(),
            name: name.to_string(),
            migrated_at: Some(at.to_string()),
            age_days: Some(1),
            target: None,
            target_status: Some("present".to_string()),
            removable: true,
            objects: Some(3),
            reason: "released".to_string(),
            detail: String::new(),
            relation: serde_json::Map::new(),
        };
        let lines = |es: &[MarkerEntry]| es.iter().map(MarkerEntry::token_line).collect();
        let a = entry("foo", "2026-01-01T00:00:00.000Z");
        let b = entry("foo-2", "2026-01-02T00:00:00.000Z");
        let token = token_over(lines(&[a.clone(), b.clone()]));
        assert_eq!(token.len(), 32);
        assert_eq!(token, token_over(lines(&[b.clone(), a.clone()])));
        assert_eq!(a.token_line(), "marker\tfoo\t2026-01-01T00:00:00.000Z\n");
        // Age and detail don't enter it; a rewritten marker does.
        let mut older = a.clone();
        older.age_days = Some(40);
        older.detail = "x".to_string();
        assert_eq!(token, token_over(lines(&[older, b.clone()])));
        let rewritten = entry("foo", "2026-02-01T00:00:00.000Z");
        assert_ne!(token, token_over(lines(&[rewritten, b])));
    }

    #[test]
    fn a_marker_plan_file_name_cant_be_a_session_plans() {
        let name = plan_file_name(PREFIX, "0123456789abcdef0123456789abcdef");
        assert_eq!(
            name,
            "markers-0123456789abcdef-0123456789abcdef0123456789abcdef.json"
        );
        assert!(!is_token(name.trim_end_matches(".json")));
    }
}
