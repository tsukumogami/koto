//! The one serializer and the one text renderer every caller prints a
//! judgment through, so the query and the prunes give the same reason and
//! the same close command. Both strip control characters from every name,
//! hint and reason they print.

use serde_json::{json, Value};

use super::graph::Edge;
use super::judge::{Judgment, Keeper, Member, Retention, Unreadable, Verdict};
use super::settings::RetentionSettings;
use crate::config::session_env::Origin;
use crate::session::anchor::valid_token;
use crate::session::pin::{Pin, PinFact};

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
/// `set_at`, and `source`, where the pin was read (`local` in this build).
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
        for (k, v) in pin_fields(&j.pin, "local") {
            out[k] = v;
        }
        out["pin_keeps"] = json!([sanitize(&j.session)]);
    }
    out
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

/// The text `koto session hierarchy` prints.
pub fn render_text(j: &Judgment, settings: &RetentionSettings) -> String {
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
