//! `koto session index audit|declare|revoke` (koto#610), the operator
//! commands that keep the cloud bucket's name index complete. The walk and
//! the plans live in [`crate::session::index_audit`]; this module resolves
//! the bucket, prints the plan, asks, writes and exits.
//!
//! Each command prints what it will write or delete before any write, then
//! asks `[y/N]` unless given `--yes`; anything but `y` or `yes`, end of
//! input included, exits 2 with nothing written. A refusal exits 2, and a
//! listing, read or write the bucket didn't answer exits 3. Writes go to the
//! bucket once each, with no retry, as a push writes an index entry. Every
//! piece of bucket text printed is escaped and capped
//! ([`index_audit::shown`]).

use std::io::{self, Write};
use std::time::SystemTime;

use anyhow::Result;
use clap::Subcommand;
use s3::Bucket;

use crate::session::index_audit::{self, shown, AuditRefusal, DeclareRefusal, Walk};
use crate::session::name_index::{AUDIT_KEY, DECLARATION_KEY};

/// The `koto session index` verbs.
#[derive(Subcommand)]
pub enum IndexCommand {
    /// Walk the bucket, backfill the index entries older sessions lack, and
    /// record the audit.
    ///
    /// Lists every workspace prefix, every session under it and every
    /// entry under `koto-names/`, then PUTs the empty entry
    /// `koto-names/<name>/<prefix>` for each state object that has none,
    /// and ends by writing the audit record `koto-names/_audit` that
    /// `koto session index declare` rests on. On a declared bucket, a state
    /// object with no entry that is newer than the declaration's audit
    /// means an older release is still writing, so the audit deletes
    /// `koto-names/_complete` first and names that object. Entries whose
    /// prefix holds no state for their name are counted; `--prune` deletes
    /// them.
    ///
    /// Cloud backend only. Exits 0 when the walk answered and every write
    /// went up, 2 on a refusal or a declined prompt, 3 when a listing still
    /// failed after its retries (no audit record is written then) or a write
    /// failed.
    Audit {
        /// Delete the entries whose prefix holds no state object for their
        /// name. On a declared bucket it needs `--revoke`.
        #[arg(long)]
        prune: bool,

        /// With `--prune` on a declared bucket, delete the declaration
        /// first, so lookups go back to the full sweep until the bucket is
        /// declared again.
        #[arg(long, requires = "prune")]
        revoke: bool,

        /// Write without asking.
        #[arg(long)]
        yes: bool,
    },

    /// Declare the index complete, so a miss in it is an answer.
    ///
    /// Walks the bucket again and PUTs `koto-names/_complete` only when
    /// every listing answered, a valid `koto-names/_audit` exists, no state
    /// object lacks its entry, and the probation has passed: the newest
    /// state object is at least 7 days younger than the audit record, both
    /// as the bucket timestamps them. On a bucket where no state object is
    /// written in the 7 days after the audit, the probation can't pass;
    /// `--no-probation` is the way out.
    ///
    /// Cloud backend only. Exits 0 when declared, 2 on a refusal or a
    /// declined prompt, 3 when a listing or read got no answer or the write
    /// failed.
    Declare {
        /// Declare without waiting out the probation. Every other refusal
        /// still holds.
        #[arg(long = "no-probation")]
        no_probation: bool,

        /// Write without asking.
        #[arg(long)]
        yes: bool,
    },

    /// Delete the declaration, `koto-names/_complete`, and nothing else.
    ///
    /// Lookups go back to reading a miss in the index as unknown and sweep
    /// every prefix, until the bucket is declared again. Cloud backend only.
    /// Exits 0 when deleted, 2 on a declined prompt, 3 when the delete
    /// failed.
    Revoke {
        /// Delete without asking.
        #[arg(long)]
        yes: bool,
    },
}

impl IndexCommand {
    fn name(&self) -> &'static str {
        match self {
            IndexCommand::Audit { .. } => "session index audit",
            IndexCommand::Declare { .. } => "session index declare",
            IndexCommand::Revoke { .. } => "session index revoke",
        }
    }
}

/// Handle `koto session index <verb>`.
pub fn handle(action: IndexCommand) -> Result<()> {
    let command = action.name();
    let bucket = configured_bucket(command);
    match action {
        IndexCommand::Audit { prune, revoke, yes } => audit(command, &bucket, prune, revoke, yes),
        IndexCommand::Declare { no_probation, yes } => declare(command, &bucket, no_probation, yes),
        IndexCommand::Revoke { yes } => revoke_declaration(command, &bucket, yes),
    }
}

fn fail(command: &str, error: String, code: i32) -> ! {
    super::exit_with_error_code(
        serde_json::json!({ "error": error, "command": command }),
        code,
    )
}

/// The configured bucket, resolved as `koto session needed` resolves the
/// store. The local backend refuses, exit 2, before any request; building
/// the bucket makes none.
fn configured_bucket(command: &str) -> Box<Bucket> {
    use crate::session::cloud::CloudBackend;

    let (base, cloud) = super::session::needed_store()
        .unwrap_or_else(|e| fail(command, format!("{e:#}"), super::EXIT_CALLER_ERROR));
    let Some(cloud) = cloud else {
        fail(
            command,
            "koto session index works only under the cloud backend (session.backend = \"cloud\"); \
             the local backend has no bucket index"
                .to_string(),
            super::EXIT_CALLER_ERROR,
        );
    };
    match CloudBackend::bucket_reader(base, &cloud) {
        Ok(reader) => reader.bucket_handle(),
        Err(e) => fail(
            command,
            format!("the bucket can't be reached with this configuration: {e:#}"),
            super::EXIT_INFRASTRUCTURE,
        ),
    }
}

/// Walk the bucket, printing what the walk skipped and which listings got
/// no answer.
fn walk_bucket(command: &str, bucket: &Bucket) -> Walk {
    let walk = index_audit::walk(bucket).unwrap_or_else(|e| {
        fail(
            command,
            format!("the bucket can't be listed: {e:#}"),
            super::EXIT_INFRASTRUCTURE,
        )
    });
    println!(
        "walked {} workspace prefixes, {} sessions, {} index entries",
        walk.prefixes.len(),
        walk.states.len(),
        walk.entries.len()
    );
    if !walk.skipped.is_empty() {
        println!(
            "skipped {} keys that fail the name or prefix rules:",
            walk.skipped.len()
        );
        for key in &walk.skipped {
            println!("  {}", shown(key));
        }
    }
    for (prefix, why) in &walk.unanswered {
        eprintln!(
            "listing {} got no answer after its retries: {}",
            shown(&index_audit::listing_name(prefix)),
            shown(why)
        );
    }
    walk
}

/// Ask `[y/N]` on stdout and read one line, as `koto workspace prune` does.
/// `gives_up` is printed first, one line, when the run gives something up.
/// Anything but `y` or `yes`, end of input included, exits 2.
fn confirm(command: &str, yes: bool, gives_up: Option<&str>) {
    if yes {
        return;
    }
    if let Some(line) = gives_up {
        println!("{line}");
    }
    print!("Proceed? [y/N] ");
    let answered = io::stdout().flush().and_then(|_| {
        let mut input = String::new();
        io::stdin().read_line(&mut input).map(|n| (n, input))
    });
    let proceed = match answered {
        Ok((0, _)) | Err(_) => false,
        Ok((_, input)) => matches!(input.trim().to_lowercase().as_str(), "y" | "yes"),
    };
    if !proceed {
        println!();
        fail(
            command,
            "aborted by operator; nothing was written".to_string(),
            super::EXIT_CALLER_ERROR,
        );
    }
}

/// One write, once. A failure exits 3 after `done` says what went up.
fn write_once(
    command: &str,
    what: &str,
    key: &str,
    result: Result<s3::request::ResponseData, s3::error::S3Error>,
    done: &str,
) {
    if let Err(e) = result {
        fail(
            command,
            format!(
                "{what} {} failed: {}; {done}",
                shown(key),
                shown(&crate::session::cloud::without_url_userinfo(&e.to_string()))
            ),
            super::EXIT_INFRASTRUCTURE,
        );
    }
}

const GIVES_UP_DECLARATION: &str = "This gives up the declaration: every lookup goes back to \
     the full sweep of the bucket until someone declares it again.";

const GIVES_UP_PROBATION: &str = "This gives up the probation wait that would catch an older \
     koto release still writing to the bucket.";

fn audit(command: &str, bucket: &Bucket, prune: bool, revoke: bool, yes: bool) -> Result<()> {
    let started_at = SystemTime::now();
    let walk = walk_bucket(command, bucket);
    let plan = match index_audit::plan_audit(&walk, prune, revoke) {
        Ok(plan) => plan,
        Err(AuditRefusal::PruneOnDeclared) => fail(
            command,
            format!(
                "the bucket is declared ({DECLARATION_KEY}), and lookups trust its index: \
                 --prune deletes entries only with --revoke, which deletes the declaration \
                 first; nothing was written"
            ),
            super::EXIT_CALLER_ERROR,
        ),
        Err(AuditRefusal::DeclarationUnread(why)) => fail(
            command,
            format!(
                "whether the bucket is declared got no answer ({}); nothing was written",
                shown(&why)
            ),
            super::EXIT_INFRASTRUCTURE,
        ),
    };

    if let Some(revocation) = &plan.revoke {
        println!("will DELETE {DECLARATION_KEY}, before any other write:");
        if revocation.requested {
            println!("  --prune --revoke asks for it");
        }
        for key in &revocation.older_writers {
            println!(
                "  {} has no index entry and is newer than the declaration's audit",
                shown(key)
            );
        }
    }
    println!("will PUT {} index entries:", plan.puts.len());
    for key in &plan.puts {
        println!("  {}", shown(key));
    }
    if prune {
        println!("will DELETE {} stale index entries:", plan.deletes.len());
    } else {
        println!(
            "found {} stale index entries (kept; --prune deletes them):",
            plan.stale.len()
        );
    }
    for key in &plan.stale {
        println!("  {}", shown(key));
    }
    if plan.write_record {
        println!("will PUT {AUDIT_KEY}");
    } else {
        println!("will not PUT {AUDIT_KEY}: a listing got no answer, so the walk is partial");
    }

    let writes = plan.revoke.is_some()
        || !plan.puts.is_empty()
        || !plan.deletes.is_empty()
        || plan.write_record;
    if writes {
        let gives_up = plan
            .revoke
            .as_ref()
            .is_some_and(|r| r.requested)
            .then_some(GIVES_UP_DECLARATION);
        confirm(command, yes, gives_up);
    }

    let mut written = 0u64;
    let mut pruned = 0u64;
    let done = |written: u64, pruned: u64| {
        format!("{written} entries were written and {pruned} pruned before it")
    };
    if plan.revoke.is_some() {
        write_once(
            command,
            "DELETE",
            DECLARATION_KEY,
            bucket.delete_object(DECLARATION_KEY),
            "nothing else was written",
        );
        println!("revoked the declaration ({DECLARATION_KEY})");
    }
    for key in &plan.puts {
        write_once(
            command,
            "PUT",
            key,
            bucket.put_object(key, b""),
            &done(written, pruned),
        );
        written += 1;
    }
    for key in &plan.deletes {
        write_once(
            command,
            "DELETE",
            key,
            bucket.delete_object(key),
            &done(written, pruned),
        );
        pruned += 1;
    }
    if plan.write_record {
        let record =
            index_audit::audit_record(&walk, started_at, SystemTime::now(), written, pruned);
        let body = serde_json::to_vec(&record)?;
        write_once(
            command,
            "PUT",
            AUDIT_KEY,
            bucket.put_object(AUDIT_KEY, &body),
            &done(written, pruned),
        );
    }
    println!(
        "prefixes {}, sessions {}, entries written {}, entries pruned {}, stale entries {}, keys skipped {}",
        walk.prefixes.len(),
        walk.states.len(),
        written,
        pruned,
        plan.stale.len() - pruned as usize,
        walk.skipped.len()
    );
    if !walk.unanswered.is_empty() {
        fail(
            command,
            format!(
                "{} listings got no answer after their retries, so the audit is partial and \
                 {AUDIT_KEY} was not written",
                walk.unanswered.len()
            ),
            super::EXIT_INFRASTRUCTURE,
        );
    }
    Ok(())
}

/// The most keys one refusal names before it counts the rest.
const NAMED_KEYS: usize = 20;

fn refusal_text(refusal: &DeclareRefusal) -> String {
    match refusal {
        DeclareRefusal::Unanswered(listings) => {
            let named: Vec<String> = listings
                .iter()
                .map(|(prefix, why)| {
                    format!(
                        "{} ({})",
                        shown(&index_audit::listing_name(prefix)),
                        shown(why)
                    )
                })
                .collect();
            format!(
                "listings got no answer after their retries, so the walk is partial: {}",
                named.join(", ")
            )
        }
        DeclareRefusal::AuditUnread(why) => {
            format!(
                "the audit record {AUDIT_KEY} couldn't be read: {}",
                shown(why)
            )
        }
        DeclareRefusal::NoAudit => format!(
            "there is no audit record {AUDIT_KEY} that parses; run `koto session index audit` first"
        ),
        DeclareRefusal::EntryLess(keys) => {
            let mut named: Vec<String> = keys
                .iter()
                .take(NAMED_KEYS)
                .map(|k| shown(k).into_string())
                .collect();
            if keys.len() > NAMED_KEYS {
                named.push(format!("and {} more", keys.len() - NAMED_KEYS));
            }
            format!(
                "state objects have no index entry, so an older release may still be writing: \
                 {}; run `koto session index audit` again, then wait out the probation",
                named.join(", ")
            )
        }
        DeclareRefusal::Probation {
            audit_at,
            bucket_now,
        } => {
            let waited = match (audit_at, bucket_now) {
                (None, _) => "the bucket gave the audit record no time koto can read".to_string(),
                (_, None) => {
                    "the bucket holds no state object with a time koto can read".to_string()
                }
                (Some(audit_at), Some(now)) => match now.duration_since(*audit_at) {
                    Ok(span) => format!(
                        "the newest state object is {} after the audit record",
                        index_audit::span_text(span)
                    ),
                    Err(_) => "no state object is newer than the audit record".to_string(),
                },
            };
            format!(
                "the probation hasn't passed: {waited}, and declaring needs {} in the bucket's \
                 own time; --no-probation declares without it",
                index_audit::span_text(index_audit::PROBATION)
            )
        }
    }
}

fn declare(command: &str, bucket: &Bucket, no_probation: bool, yes: bool) -> Result<()> {
    let walk = walk_bucket(command, bucket);
    let record = match index_audit::plan_declare(&walk, no_probation) {
        Ok(record) => record,
        Err(refusals) => {
            let code = if refusals.iter().any(DeclareRefusal::is_infrastructure) {
                super::EXIT_INFRASTRUCTURE
            } else {
                super::EXIT_CALLER_ERROR
            };
            let reasons: Vec<String> = refusals.iter().map(refusal_text).collect();
            fail(
                command,
                format!("{DECLARATION_KEY} was not written: {}", reasons.join("; ")),
                code,
            );
        }
    };
    println!(
        "will PUT {DECLARATION_KEY}, resting on the audit of {} prefixes and {} sessions \
         ({} entries written, {} pruned)",
        record.prefixes, record.sessions, record.entries_written, record.entries_pruned
    );
    confirm(command, yes, no_probation.then_some(GIVES_UP_PROBATION));
    let declaration = index_audit::declaration(
        record,
        SystemTime::now(),
        crate::session::version::machine_name(),
    );
    let body = serde_json::to_vec(&declaration)?;
    write_once(
        command,
        "PUT",
        DECLARATION_KEY,
        bucket.put_object(DECLARATION_KEY, &body),
        "the bucket is not declared",
    );
    println!(
        "declared the index complete ({DECLARATION_KEY}) as {}",
        shown(&declaration.machine)
    );
    Ok(())
}

fn revoke_declaration(command: &str, bucket: &Bucket, yes: bool) -> Result<()> {
    println!("will DELETE {DECLARATION_KEY}");
    confirm(command, yes, None);
    write_once(
        command,
        "DELETE",
        DECLARATION_KEY,
        bucket.delete_object(DECLARATION_KEY),
        "the declaration may still be in place",
    );
    println!("revoked the declaration ({DECLARATION_KEY})");
    Ok(())
}
