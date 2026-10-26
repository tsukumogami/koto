//! The one serializer and the one text renderer every caller prints a
//! judgment through, so the query and the prunes give the same reason and
//! the same close command. Both strip control characters from every name,
//! hint and reason they print.

use serde_json::{json, Value};

use super::graph::Edge;
use super::judge::{Judgment, Keeper, Member, MemberKind, Retention, Unreadable, Verdict};
use super::relation::{RelationKeeper, RelationVerdict};
use super::settings::RetentionSettings;
use crate::config::session_env::Origin;
use crate::session::anchor::{valid_token, AuthorityRecord};
use crate::session::pin::{Pin, PinFact, ReleaseRecord};

/// `s` with every control character removed.
pub fn sanitize(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Whether `c` is escaped by [`display_text`]: a control character, a
/// bidirectional override or isolate, a zero-width or invisible formatting
/// character, or a line or paragraph separator.
fn needs_escape(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

/// Stored text from a pin, a release record or an authority record, as
/// every surface prints it: each character [`needs_escape`] names is
/// written as `\u{XXXX}` and a backslash as `\\`, so a stored string can't
/// carry a terminal escape or reorder the line it lands in. JSON output
/// carries the escaped form too.
pub fn display_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\\' {
            out.push_str("\\\\");
        } else if needs_escape(c) {
            out.push_str(&format!("\\u{{{:04X}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// A readable pin as every kept entry carries it: `reason`, `set_by`,
/// `set_at`, and `source`, where the pin was read: `local`, this host's
/// copy, or `bucket`, the bucket's pin object under the cloud backend.
pub fn pin_json(pin: &Pin, source: &str) -> Value {
    json!({
        "reason": display_text(&pin.reason),
        "set_by": pin.set_by.as_deref().map(display_text),
        "set_at": display_text(&pin.set_at),
        "source": source,
    })
}

/// The fields a kept entry for a pinned session carries: `kept_by_pin`
/// and either `pin` or `pin_unreadable`. Empty for a session with no pin,
/// so its entry is the one it was before pins existed.
pub fn pin_fields(pin: &PinFact, source: &str) -> serde_json::Map<String, Value> {
    let mut fields = serde_json::Map::new();
    match pin {
        PinFact::None => {}
        PinFact::Present(p) => {
            fields.insert("kept_by_pin".to_string(), json!(true));
            fields.insert("pin".to_string(), pin_json(p, source));
        }
        PinFact::Unreadable(problem) => {
            fields.insert("kept_by_pin".to_string(), json!(true));
            fields.insert("pin_unreadable".to_string(), json!(problem.as_str()));
        }
    }
    fields
}

/// The text a kept pinned session prints: `pinned: <reason> (set by
/// <anchor or "hand"> at <time>)`, or `pin unreadable: <reason name>`.
/// `None` for a session with no pin.
pub fn pin_text(pin: &PinFact) -> Option<String> {
    match pin {
        PinFact::None => None,
        PinFact::Present(p) => Some(format!(
            "pinned: {} (set by {} at {})",
            display_text(&p.reason),
            p.set_by
                .as_deref()
                .map(display_text)
                .unwrap_or_else(|| "hand".to_string()),
            display_text(&p.set_at)
        )),
        PinFact::Unreadable(problem) => Some(format!("pin unreadable: {}", problem.as_str())),
    }
}

/// A release record as `koto session unpin` and the hierarchy query print
/// it: who released the pin, with what authority, why and when, and the
/// pin it released, each `null` where nothing was recorded (a release by
/// hand, or a pin that couldn't be read).
pub fn release_json(r: &ReleaseRecord) -> Value {
    let text = |s: &Option<String>| s.as_deref().map(display_text);
    json!({
        "released_by": text(&r.released_by),
        "authority": text(&r.authority),
        "authority_at": text(&r.authority_at),
        "authority_source": text(&r.authority_source),
        "reason": text(&r.reason),
        "released_at": display_text(&r.released_at),
        "pin": {
            "reason": text(&r.pin.reason),
            "set_by": text(&r.pin.set_by),
            "set_at": text(&r.pin.set_at),
        },
    })
}

/// The line the hierarchy query prints for a release record.
fn release_text(r: &ReleaseRecord) -> String {
    let by = match (&r.released_by, &r.authority) {
        (None, _) => "by hand".to_string(),
        (Some(anchor), Some(authority)) => format!(
            "by {} with the {} authority",
            display_text(anchor),
            display_text(authority)
        ),
        (Some(anchor), None) => format!("by {}", display_text(anchor)),
    };
    let mut text = format!("pin released {by} at {}", display_text(&r.released_at));
    if let Some(reason) = &r.reason {
        text.push_str(&format!(" (reason: {})", display_text(reason)));
    }
    match (&r.pin.reason, &r.pin.set_at) {
        (Some(reason), Some(set_at)) => text.push_str(&format!(
            "; the pin it released: {} (set by {} at {})",
            display_text(reason),
            r.pin
                .set_by
                .as_deref()
                .map(display_text)
                .unwrap_or_else(|| "hand".to_string()),
            display_text(set_at)
        )),
        _ => text.push_str("; the pin it released couldn't be read"),
    }
    text
}

/// One keeper as `relation_keepers` lists it: `keeper`, `standing`,
/// `retention_passes_at` for retention standing, `pin` (or
/// `pin_unreadable`) for pin standing, and `path`, nearest first.
fn relation_keeper_json(k: &RelationKeeper) -> Value {
    let mut v = json!({
        "keeper": sanitize(&k.keeper),
        "standing": k.standing.as_str(),
    });
    if let Some(at) = &k.retention_passes_at {
        v["retention_passes_at"] = json!(at);
    }
    match &k.pin {
        PinFact::None => {}
        PinFact::Present(p) => v["pin"] = pin_json(p, k.pin_source),
        PinFact::Unreadable(problem) => v["pin_unreadable"] = json!(problem.as_str()),
    }
    v["path"] = json!(k.path.iter().map(|n| sanitize(n)).collect::<Vec<_>>());
    v
}

/// The fields a kept entry carries when the continues relation keeps it:
/// `kept_by_relation: true`, `relation_keepers` (one per keeper, ordered by
/// name) and `relation_unreadable` (`holder` and `reason`), each omitted
/// when empty. Empty for a session the relation doesn't keep, so its entry
/// is the one it was before the relation existed (R23). A merged bucket
/// verdict renders through this unchanged.
pub fn relation_json(r: &RelationVerdict) -> serde_json::Map<String, Value> {
    let mut fields = relation_report_json(r);
    if r.keeps() {
        fields.insert("kept_by_relation".to_string(), json!(true));
    }
    fields
}

/// [`relation_json`] less `kept_by_relation`: what an explicit removal
/// (`koto session cleanup`, `koto cancel --cleanup`) reports it passed
/// over.
pub fn relation_report_json(r: &RelationVerdict) -> serde_json::Map<String, Value> {
    let mut fields = serde_json::Map::new();
    if !r.keepers.is_empty() {
        fields.insert(
            "relation_keepers".to_string(),
            json!(r
                .keepers
                .iter()
                .map(relation_keeper_json)
                .collect::<Vec<_>>()),
        );
    }
    if !r.unreadable.is_empty() {
        fields.insert(
            "relation_unreadable".to_string(),
            json!(r
                .unreadable
                .iter()
                .map(|u| json!({"holder": sanitize(&u.holder), "reason": u.reason.as_str()}))
                .collect::<Vec<_>>()),
        );
    }
    if let Some(first) = r.unchecked.first() {
        // The pin guard's `bucket_unchecked` names its `error`; the walk's
        // adds the sessions it couldn't check, each with its own.
        fields.insert(
            "bucket_unchecked".to_string(),
            json!({
                "error": first.error.as_str(),
                "sessions": r
                    .unchecked
                    .iter()
                    .map(|u| json!({"name": sanitize(&u.name), "error": u.error.as_str()}))
                    .collect::<Vec<_>>(),
            }),
        );
    }
    fields
}

/// The text lines for a session the relation keeps: `kept: continued by
/// <keeper> (<standing>)`, with ` via <path joined by ", ">` when the path
/// isn't empty, one per keeper; then `kept: relation unreadable: <holder>
/// (<reason>)` per relation that couldn't be judged. Empty when the
/// relation doesn't keep it.
pub fn relation_text(r: &RelationVerdict) -> Vec<String> {
    let mut lines: Vec<String> = r
        .keepers
        .iter()
        .map(|k| {
            let mut line = format!(
                "kept: continued by {} ({})",
                sanitize(&k.keeper),
                k.standing.as_str()
            );
            if !k.path.is_empty() {
                let path: Vec<String> = k.path.iter().map(|n| sanitize(n)).collect();
                line.push_str(&format!(" via {}", path.join(", ")));
            }
            line
        })
        .collect();
    lines.extend(r.unreadable.iter().map(|u| {
        format!(
            "kept: relation unreadable: {} ({})",
            sanitize(&u.holder),
            u.reason.as_str()
        )
    }));
    lines.extend(r.unchecked.iter().map(|u| {
        format!(
            "kept: bucket not checked for what continues it: {} ({})",
            sanitize(&u.name),
            u.error.as_str()
        )
    }));
    lines
}

/// The pinned members of `j`'s hierarchy other than the judged session,
/// sorted by name.
fn other_pinned(j: &Judgment) -> Vec<String> {
    j.pinned_members
        .keys()
        .filter(|n| **n != j.session)
        .map(|n| sanitize(n))
        .collect()
}

/// The command that closes an observed anchor by hand, with its observed
/// marker `key` and recorded harness session id filled in, each
/// single-quoted. Built only when both pass the close command's own check,
/// `^[A-Za-z0-9_-]{1,128}$`; `None` otherwise.
pub fn close_command(key: &str, harness_session: &str) -> Option<String> {
    (valid_token(key) && valid_token(harness_session)).then(|| {
        format!("koto session anchor close --key='{key}' --harness-session='{harness_session}'")
    })
}

/// A keeper as the JSON of a judgment lists it: `name`, `kind`,
/// `observed`, and for a stale anchor `stale` with its `close_command` or
/// `malformed_marker`. The prunes list stale anchors through this too, so
/// they print the same command the query prints.
pub fn keeper_json(k: &Keeper) -> Value {
    let mut v = json!({
        "name": sanitize(&k.name),
        "kind": k.kind.as_str(),
        "observed": k.observed,
    });
    if k.stale {
        v["stale"] = json!(true);
        match &k.close_command {
            Some(cmd) => v["close_command"] = json!(sanitize(cmd)),
            None => v["malformed_marker"] = json!(true),
        }
    }
    v
}

fn retention_json(r: &Retention) -> Value {
    json!({
        "release": {
            "at": r.release.at,
            "member": sanitize(&r.release.member),
            "close_kind": r.release.close_kind.as_str(),
        },
        "passes_at": r.passes_at,
    })
}

fn unreadable_json(u: &Unreadable) -> Value {
    json!({"name": sanitize(&u.name), "reason": sanitize(&u.reason)})
}

fn edge_json(e: &Edge) -> Value {
    json!({
        "holder": sanitize(&e.holder),
        "via": sanitize(&e.via.describe()),
        "named": sanitize(&e.named),
        "target": sanitize(&e.target),
    })
}

fn member_json(m: &Member) -> Value {
    json!({
        "name": sanitize(&m.name),
        "kind": m.kind.as_str(),
        "alive": m.alive,
        "stopped_at": m.stopped_at,
        "close_kind": m.close_kind.map(|k| k.as_str()),
        "absent": m.absent,
        "migrated": m.migrated,
        "observed": m.observed,
        "imported": m.imported,
        "stale": m.stale,
        "clamped": m.clamped,
        "joined_by": m.joined_by.iter().map(edge_json).collect::<Vec<_>>(),
    })
}

/// The settings object: each setting's value as written and its origin.
fn settings_json(settings: &RetentionSettings) -> Value {
    let mut map = serde_json::Map::new();
    for s in settings.all() {
        let mut entry = json!({
            "value": sanitize(&s.raw),
            "origin": s.origin.source(),
        });
        if let Origin::Env(var) = s.origin {
            entry["variable"] = json!(var);
        }
        map.insert(s.name.to_string(), entry);
    }
    Value::Object(map)
}

/// The JSON of a judgment: what `koto session hierarchy --json` prints and
/// what a prune's kept entry carries as its `hierarchy` object.
pub fn to_json(j: &Judgment, settings: &RetentionSettings) -> Value {
    let (kept, keepers, retention, unreadable) = match &j.verdict {
        Verdict::Keep {
            keepers,
            retention,
            unreadable,
        } => (
            true,
            keepers.as_slice(),
            retention.as_ref(),
            unreadable.as_slice(),
        ),
        Verdict::Release { release } => (false, &[][..], release.as_ref(), &[][..]),
    };
    let mut out = json!({
        "session": sanitize(&j.session),
        "kept": kept,
        "keepers": keepers.iter().map(keeper_json).collect::<Vec<_>>(),
        "retention": retention.map(retention_json),
        "unreadable": unreadable.iter().map(unreadable_json).collect::<Vec<_>>(),
        "members": j.members.iter().map(member_json).collect::<Vec<_>>(),
        "clamped": j.clamped.iter().map(|n| sanitize(n)).collect::<Vec<_>>(),
        "now": j.now,
        "settings": settings_json(settings),
    });
    // A pinned session's own judgment adds its pin, and says the pin keeps
    // this session only: `kept` stays the hierarchy's verdict. Nothing is
    // added for a session with no pin.
    if j.pin.is_pinned() {
        for (k, v) in pin_fields(&j.pin, j.pin_source) {
            out[k] = v;
        }
        out["pin_keeps"] = json!([sanitize(&j.session)]);
    }
    out
}

/// The JSON `koto session hierarchy <name> --json` prints: [`to_json`],
/// plus what only the query reports. Each anchor member carries its
/// `last_activity`; `pinned_members` names every other member a pin keeps,
/// when there is one; `pin_release` is the judged session's latest
/// release record, when it has one; and `authorities` and
/// `unknown_authorities` are what the judged anchor's latest start was
/// handed (`authority`), when it records any. A prune's `hierarchy` object
/// is [`to_json`] alone, so a prune entry for a session with no pin prints
/// what it printed before pins existed.
pub fn query_json(
    j: &Judgment,
    settings: &RetentionSettings,
    release: Option<&ReleaseRecord>,
    authority: Option<&AuthorityRecord>,
) -> Value {
    let mut out = to_json(j, settings);
    if let Some(members) = out["members"].as_array_mut() {
        for (v, m) in members.iter_mut().zip(&j.members) {
            if m.kind == MemberKind::Anchor {
                v["last_activity"] = json!(m.last_activity);
            }
        }
    }
    let others = other_pinned(j);
    if !others.is_empty() {
        out["pinned_members"] = json!(others);
    }
    // The continues relation (R13): the predecessor this session names,
    // the sessions naming it, and what keeps it through them. Each is
    // omitted when there is none.
    if let Some(p) = &j.predecessor {
        out["predecessor"] = json!(sanitize(p));
    }
    if !j.successors.is_empty() {
        out["successors"] = json!(j.successors.iter().map(|n| sanitize(n)).collect::<Vec<_>>());
    }
    for (k, v) in relation_json(j.relation_of(&j.session)) {
        out[k] = v;
    }
    if let Some(r) = release {
        out["pin_release"] = release_json(r);
    }
    if let Some(a) = authority {
        let shown = |names: &[String]| names.iter().map(|n| display_text(n)).collect::<Vec<_>>();
        out["authorities"] = json!(shown(&a.names));
        out["unknown_authorities"] = json!(shown(&a.unknown));
    }
    out
}

/// The line the hierarchy query prints for an anchor's authority record.
fn authority_text(a: &AuthorityRecord) -> String {
    let list = |names: &[String]| {
        if names.is_empty() {
            "none".to_string()
        } else {
            names
                .iter()
                .map(|n| display_text(n))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    format!(
        "authorities handed at its latest start ({}, {}): {}; unknown: {}",
        display_text(&a.source),
        display_text(&a.at),
        list(&a.names),
        list(&a.unknown)
    )
}

fn keeper_text(k: &Keeper) -> String {
    let mut text = format!("{} ({}", sanitize(&k.name), k.kind.as_str());
    if k.observed {
        text.push_str(", observed");
    }
    if k.stale {
        text.push_str(", stale");
    }
    text.push(')');
    text
}

/// One line naming a live member, and for a stale anchor the command that
/// closes it by hand (or why there is none): the detail a prune gives a
/// session that is itself alive.
pub fn keeper_line(k: &Keeper) -> String {
    let mut text = format!("alive: {}", keeper_text(k));
    if k.stale {
        match &k.close_command {
            Some(cmd) => text.push_str(&format!(
                "; stale: past retention.stale_after since its last activity; close it by hand \
                 with: {}",
                sanitize(cmd)
            )),
            None => text.push_str(
                "; stale: past retention.stale_after since its last activity; its observed marker \
                 or harness session id is malformed, so no close command is printed",
            ),
        }
    }
    text
}

fn retention_text(r: &Retention) -> String {
    format!(
        "released at {} when {} stopped ({}); the retention passes at {}",
        r.release.at,
        sanitize(&r.release.member),
        r.release.close_kind.as_str(),
        r.passes_at
    )
}

/// One line saying why a hierarchy is kept or released: a prune's rule
/// `h` detail, and the head of the query's text.
pub fn reason_text(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Keep {
            keepers,
            retention,
            unreadable,
        } => {
            if !keepers.is_empty() {
                let names: Vec<String> = keepers.iter().map(keeper_text).collect();
                let mut text = format!("kept: alive in its hierarchy: {}", names.join(", "));
                if !unreadable.is_empty() {
                    text.push_str(&format!("; and {}", unreadable_text(unreadable)));
                }
                text
            } else if !unreadable.is_empty() {
                format!("kept: {}", unreadable_text(unreadable))
            } else if let Some(r) = retention {
                format!("kept: {}", retention_text(r))
            } else {
                "kept".to_string()
            }
        }
        Verdict::Release { release: Some(r) } => {
            format!(
                "released: nothing in its hierarchy is alive; {}",
                retention_text(r).replace("passes at", "passed at")
            )
        }
        Verdict::Release { release: None } => {
            "released: nothing in its hierarchy is alive or has a stop of its own".to_string()
        }
    }
}

fn unreadable_text(unreadable: &[Unreadable]) -> String {
    let items: Vec<String> = unreadable
        .iter()
        .map(|u| format!("{}: {}", sanitize(&u.name), sanitize(&u.reason)))
        .collect();
    format!(
        "something in its hierarchy can't be read ({})",
        items.join("; ")
    )
}

fn edge_text(e: &Edge, member: &str) -> String {
    let via = sanitize(&e.via.describe());
    if e.holder == member {
        if e.named != e.target {
            format!(
                "its {via} names {} (resolved to {})",
                sanitize(&e.named),
                sanitize(&e.target)
            )
        } else {
            format!("its {via} names {}", sanitize(&e.target))
        }
    } else {
        format!("{}'s {via} names it", sanitize(&e.holder))
    }
}

/// The text `koto session hierarchy` prints for a session with no release
/// record.
pub fn render_text(j: &Judgment, settings: &RetentionSettings) -> String {
    query_text(j, settings, None, None)
}

/// The text `koto session hierarchy` prints: the verdict, the session's own
/// pin, its latest release record (`release`), an anchor's authority
/// record (`authority`) and the hierarchy's other pinned members, then the
/// keepers, members and settings.
pub fn query_text(
    j: &Judgment,
    settings: &RetentionSettings,
    release: Option<&ReleaseRecord>,
    authority: Option<&AuthorityRecord>,
) -> String {
    let mut out = String::new();
    match pin_text(&j.pin) {
        None => out.push_str(&format!(
            "{}: {}\n",
            sanitize(&j.session),
            reason_text(&j.verdict)
        )),
        Some(pin) => out.push_str(&format!(
            "{name}: kept by its pin; its hierarchy: {}\n{pin}\n  the pin keeps {name} only; \
             every other member is kept or released by the hierarchy's rule\n",
            reason_text(&j.verdict),
            name = sanitize(&j.session),
        )),
    }
    if let Some(r) = release {
        out.push_str(&format!("{}\n", release_text(r)));
    }
    if let Some(a) = authority {
        out.push_str(&format!("{}\n", authority_text(a)));
    }
    let others = other_pinned(j);
    if !others.is_empty() {
        out.push_str(&format!(
            "pinned members, each kept by its own pin: {}\n",
            others.join(", ")
        ));
    }
    if let Some(p) = &j.predecessor {
        out.push_str(&format!("continues: {}\n", sanitize(p)));
    }
    if !j.successors.is_empty() {
        let names: Vec<String> = j.successors.iter().map(|n| sanitize(n)).collect();
        out.push_str(&format!("continued by: {}\n", names.join(", ")));
    }
    for line in relation_text(j.relation_of(&j.session)) {
        out.push_str(&format!("{line}\n"));
    }
    if let Verdict::Keep { keepers, .. } = &j.verdict {
        if !keepers.is_empty() {
            out.push_str("keepers:\n");
            for k in keepers {
                out.push_str(&format!("  {}\n", keeper_text(k)));
                if k.stale {
                    match &k.close_command {
                        Some(cmd) => out.push_str(&format!(
                            "    stale: past retention.stale_after since its last activity; \
                             close it by hand with: {}\n",
                            sanitize(cmd)
                        )),
                        None => out.push_str(
                            "    stale: past retention.stale_after since its last activity; its \
                             observed marker or harness session id is malformed, so no close \
                             command is printed\n",
                        ),
                    }
                }
            }
        }
    }
    let retention = match &j.verdict {
        Verdict::Keep { retention, .. } => retention.as_ref(),
        Verdict::Release { release } => release.as_ref(),
    };
    if let Some(r) = retention {
        out.push_str(&format!(
            "release: {} stopped ({}) at {}\nretention passes at: {}\n",
            sanitize(&r.release.member),
            r.release.close_kind.as_str(),
            r.release.at,
            r.passes_at
        ));
    }
    if let Verdict::Keep { unreadable, .. } = &j.verdict {
        if !unreadable.is_empty() {
            out.push_str("unreadable:\n");
            for u in unreadable {
                out.push_str(&format!(
                    "  {}: {}\n",
                    sanitize(&u.name),
                    sanitize(&u.reason)
                ));
            }
        }
    }
    out.push_str("members:\n");
    for m in &j.members {
        let mut flags = vec![m.kind.as_str().to_string()];
        if m.absent {
            flags.push("absent".to_string());
        } else if m.migrated {
            flags.push("migrated".to_string());
        } else if m.alive {
            flags.push("alive".to_string());
        } else {
            flags.push("stopped".to_string());
        }
        if m.observed {
            flags.push("observed".to_string());
        }
        if m.imported {
            flags.push("imported".to_string());
        }
        if m.stale {
            flags.push("stale".to_string());
        }
        if m.clamped {
            flags.push("clamped to now".to_string());
        }
        out.push_str(&format!("  {} ({})", sanitize(&m.name), flags.join(", ")));
        if let (Some(at), Some(kind)) = (&m.stopped_at, m.close_kind) {
            out.push_str(&format!(" stopped {} at {}", kind.as_str(), at));
        }
        out.push('\n');
        for e in &m.joined_by {
            out.push_str(&format!("    joined: {}\n", edge_text(e, &m.name)));
        }
    }
    out.push_str("settings:\n");
    for s in settings.all() {
        out.push_str(&format!(
            "  {} = {} ({})\n",
            s.name,
            sanitize(&s.raw),
            s.origin_label()
        ));
    }
    out
}
