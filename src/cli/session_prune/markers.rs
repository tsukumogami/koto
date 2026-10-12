//! `koto session prune --migrated-from <workspace>`: remove the migration
//! markers `koto session import` left under a source workspace's bucket
//! prefix, each with the old copy beside it.
//!
//! A marker is what makes the source host refuse a session it no longer
//! owns, so it goes only when the session it points at is safe elsewhere:
//! its target must be `present` (the target's state file header carries
//! the session id the marker names, and its whole log reads with no error
//! and begins with every event of the source's log, unchanged, so a
//! truncated or corrupt copy, or one behind or apart from the source,
//! never stands in for it), and then the marker must be at least
//! `session.cloud.marker_retain_days` old, or the caller must say the source
//! host is gone with `--source-retired`. Any other target status keeps the
//! marker whatever its age, and so does a marker that can't be read.
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
    /// `unreadable` for a marker that can't be read, else `age` or
    /// `source_retired`.
    pub reason: String,
    pub detail: String,
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
        }
    }
}

/// A stored marker plan. It lives at `markers-<source prefix>-<token>.json`,
/// a name no session plan can have, and records what the preview ran with
/// so `--apply --plan` runs with the same.
#[derive(Debug, Serialize, Deserialize)]
struct StoredMarkerPlan {
    mode: String,
    plan: String,
    created_at: String,
    migrated_from: String,
    source_prefix: String,
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

/// The rule: whether a marker of `age` whose target stands at `status` is
/// removable, what decided it, and a sentence saying so.
fn decide(
    status: &TargetStatus,
    expected_id: &str,
    age: Duration,
    retain_days: u64,
    source_retired: bool,
) -> (bool, &'static str, String) {
    let days = age.as_secs() / DAY;
    match status {
        TargetStatus::Present => {
            let bound = Duration::from_secs(retain_days.saturating_mul(DAY));
            if age >= bound {
                (
                    true,
                    "age",
                    format!(
                        "the target is present and the marker is {days} days old, past the \
                         {retain_days}-day bound"
                    ),
                )
            } else if source_retired {
                (
                    true,
                    "source_retired",
                    format!(
                        "the target is present and --source-retired was given; the marker is \
                         {days} days old, inside the {retain_days}-day bound"
                    ),
                )
            } else {
                (
                    false,
                    "age",
                    format!(
                        "the target is present but the marker is {days} days old, inside the \
                         {retain_days}-day bound (session.cloud.marker_retain_days); pass \
                         --source-retired once the source host is gone for good"
                    ),
                )
            }
        }
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
    retain_days: u64,
    source_retired: bool,
    now: SystemTime,
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
    // A marker dated in the future is as young as it gets.
    let age = now.duration_since(body.at).unwrap_or(Duration::ZERO);
    let (removable, reason, detail) = decide(
        &status,
        &body.target.session_id,
        age,
        retain_days,
        source_retired,
    );
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
    })
}

struct Verdict {
    remove: Vec<MarkerEntry>,
    kept: Vec<MarkerEntry>,
    token: String,
}

fn judge(
    cloud: &CloudBackend,
    source_prefix: &str,
    retain_days: u64,
    source_retired: bool,
) -> Result<Verdict> {
    let now = SystemTime::now();
    let mut remove = Vec::new();
    let mut kept = Vec::new();
    for name in cloud.migration_marker_names(source_prefix)? {
        let Some(entry) = evaluate(
            cloud,
            source_prefix,
            &name,
            retain_days,
            source_retired,
            now,
        ) else {
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
        if args.source_retired {
            usage(
                "--plan applies with the --source-retired choice its preview made; pass \
                 --source-retired only to the preview",
            );
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
    let source_retired = match &stored {
        Some(plan) => plan.source_retired,
        None => args.source_retired,
    };

    let verdict = judge(
        cloud,
        &source_prefix,
        args.marker_retain_days,
        source_retired,
    )?;

    if !args.apply {
        let plan = StoredMarkerPlan {
            mode: MODE.to_string(),
            plan: verdict.token.clone(),
            created_at: crate::engine::types::now_iso8601(),
            migrated_from: migrated_from.to_string(),
            source_prefix: source_prefix.clone(),
            source_retired,
            remove: verdict.remove.clone(),
        };
        write_plan_file(
            &base,
            &plan_file_name(&source_prefix, &verdict.token),
            &serde_json::to_vec(&plan)?,
        )?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "mode": "preview",
                "migrated_from": migrated_from,
                "prefix": source_prefix,
                "marker_retain_days": args.marker_retain_days,
                "source_retired": source_retired,
                "plan": verdict.token,
                "remove": verdict.remove,
                "kept": verdict.kept,
            }))?
        );
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

    let outcome = apply(
        cloud,
        &source_prefix,
        &verdict.remove,
        args.marker_retain_days,
        source_retired,
    );
    if let Some(token) = args.plan {
        if outcome.failed.is_empty() {
            remove_plan_file(&base, &plan_file_name(&source_prefix, token));
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "mode": "apply",
            "migrated_from": migrated_from,
            "prefix": source_prefix,
            "plan": verdict.token,
            "removed": outcome.removed,
            "skipped": outcome.skipped,
            "kept": verdict.kept,
            "failed": outcome.failed,
        }))?
    );
    if !outcome.failed.is_empty() {
        std::process::exit(EXIT_REMOVAL_FAILED);
    }
    Ok(())
}

#[derive(Default)]
struct Outcome {
    removed: Vec<MarkerEntry>,
    skipped: Vec<serde_json::Value>,
    failed: Vec<serde_json::Value>,
}

/// Remove each entry's source copy, marker last, after reading the marker
/// again and judging it again: a marker rewritten since (another
/// `migrated_at`), gone, or no longer removable is skipped with the reason.
fn apply(
    cloud: &CloudBackend,
    source_prefix: &str,
    entries: &[MarkerEntry],
    retain_days: u64,
    source_retired: bool,
) -> Outcome {
    let mut outcome = Outcome::default();
    for entry in entries {
        let skip = |reason: String| json!({"name": entry.name, "reason": reason});
        let now = SystemTime::now();
        let fresh = match evaluate(
            cloud,
            source_prefix,
            &entry.name,
            retain_days,
            source_retired,
            now,
        ) {
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
    fn only_a_present_target_is_removable_and_then_by_age_or_retirement() {
        let day = Duration::from_secs(DAY);
        let young = day * 2;
        let old = day * 31;

        // Present: by age past the bound, or inside it with --source-retired.
        let present = TargetStatus::Present;
        let by_age = decide(&present, "id", old, 30, false);
        assert_eq!((by_age.0, by_age.1), (true, "age"));
        assert!(decide(&present, "id", day * 30, 30, false).0);
        let kept = decide(&present, "id", young, 30, false);
        assert_eq!((kept.0, kept.1), (false, "age"));
        assert!(kept.2.contains("inside the 30-day bound"), "{}", kept.2);
        let retired = decide(&present, "id", young, 30, true);
        assert_eq!((retired.0, retired.1), (true, "source_retired"));
        // A bound of 0 days makes every present target's marker old enough.
        assert_eq!(decide(&present, "id", Duration::ZERO, 0, false).1, "age");
        assert!(decide(&present, "id", Duration::ZERO, 0, false).0);
        // A huge bound doesn't overflow.
        assert!(!decide(&present, "id", old, u64::MAX, false).0);

        // Anything else is kept at any age, with or without the flag, and the
        // status is the reason.
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
            for age in [young, old] {
                for retired in [false, true] {
                    let (removable, reason, _) = decide(&status, "id", age, 30, retired);
                    assert!(!removable, "{word} {age:?} {retired}");
                    assert_eq!(reason, word);
                }
            }
        }
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
            reason: "age".to_string(),
            detail: String::new(),
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
