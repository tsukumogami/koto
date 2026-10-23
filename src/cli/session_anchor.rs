//! `koto session anchor open|observe|close`, and the held-by links and
//! activity events `koto init` and `koto next` record from `KOTO_ANCHOR`
//! (DESIGN-session-retention.md, Decision 2). The logic lives in
//! [`crate::session::anchor`]; this module parses, prints and exits.
//!
//! The three verbs print `key=value` lines on stdout, one per line, so a
//! shell hook can read them without a JSON parser. A refusal prints the
//! usual `{"error", "command"}` object and exits 2; any other failure exits
//! 3. Neither prints an `anchor=` line.

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use crate::session::anchor::{self, AnchorRefusal, Observe, OpenRequest, Source};
use crate::session::{Backend, SessionBackend};

/// The `koto session anchor` verbs.
#[derive(Subcommand)]
pub enum AnchorCommand {
    /// Open or continue the anchor for an agent session.
    ///
    /// The anchor for harness session ID is `anchor-<ID>`. A first open
    /// creates it (`opened=new`); a repeat continues it (`continued`); one
    /// that was removed is created again under the same name (`reopened`).
    /// An anchor that reads closed by an observed close, or that was
    /// migrated, gets a successor `anchor-<ID>-s2`, `-s3` and so on holding
    /// a dispatched-by link to the one before it (`successor`). A `clear`
    /// start finds the anchor its process ran before and links to it; a
    /// `resume` with no anchor of its own does the same through the
    /// transcript path. Every call appends an `anchor_started` event.
    ///
    /// Runs under the anchor's lock and never reads `KOTO_ANCHOR`. Prints
    /// `anchor=<name>`, `opened=new|continued|reopened|successor` and
    /// `observed=yes|no`, and after a `clear` that found its predecessor
    /// `previous=<name>` and `inherit_until=<rfc3339>`.
    Open {
        /// The harness's session id: letters, digits, `_` and `-`, at most
        /// 128, not ending in `-closed` or `-s<n>`
        #[arg(long = "harness-session", value_name = "ID")]
        harness_session: String,

        /// What started the agent session
        #[arg(long, value_name = "SOURCE", value_parser = ["startup", "resume", "compact", "clear", "other"])]
        source: String,

        /// The agent process's token, `<host>:<pid>:<epoch-start>`; only a
        /// digest of it is stored
        #[arg(long, value_name = "TOKEN")]
        process: Option<String>,

        /// The session that dispatched this agent session, recorded as a
        /// dispatched-by link when the anchor is created
        #[arg(long = "dispatched-by", value_name = "NAME")]
        dispatched_by: Option<String>,

        /// The reap key whose close records this anchor, written as its
        /// observed marker when it has none
        #[arg(long = "observed-key", value_name = "KEY")]
        observed_key: Option<String>,

        /// The agent session's workspace directory
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,

        /// The harness's transcript path; only a digest of it is stored
        #[arg(long = "transcript-path", value_name = "PATH")]
        transcript_path: Option<String>,
    },

    /// Write an observed marker into one anchor.
    ///
    /// `--key` writes the reap key the caller read; `--inherit` copies the
    /// marker of the predecessor the anchor's own `anchor_started`
    /// recorded. Writes only into ANCHOR, which must be an anchor, and
    /// leaves an anchor that already holds a marker as it is. Prints
    /// `anchor=<name>` and `observed=yes|no`.
    Observe {
        /// The anchor to mark
        #[arg(long, value_name = "NAME")]
        anchor: String,

        /// The reap key
        #[arg(
            long,
            value_name = "KEY",
            conflicts_with = "inherit",
            required_unless_present = "inherit"
        )]
        key: Option<String>,

        /// Copy the predecessor's marker instead
        #[arg(long)]
        inherit: bool,
    },

    /// Record an agent session's anchors as closed.
    ///
    /// Records `anchor-<ID>`, and the anchors that succeeded it after a
    /// `/clear` or a resume under a new id, each only when its observed
    /// marker is KEY, in a close session of its own, `<anchor>-closed`.
    /// Never writes into an anchor. Prints `closed=<n>`, the number of
    /// anchors this call recorded; a repeat, or a key that matches no
    /// anchor, prints `closed=0` and exits 0.
    Close {
        /// The reap key the anchors were observed with
        #[arg(long, value_name = "KEY")]
        key: String,

        /// The harness session id whose anchors to close
        #[arg(long = "harness-session", value_name = "ID")]
        harness_session: String,
    },
}

/// Run one `koto session anchor` verb.
pub fn handle(backend: &Backend, command: AnchorCommand) -> Result<()> {
    match command {
        AnchorCommand::Open {
            harness_session,
            source,
            process,
            dispatched_by,
            observed_key,
            workspace,
            transcript_path,
        } => {
            let source = Source::parse(&source).unwrap_or(Source::Other);
            let workspace = workspace
                .or_else(|| std::env::current_dir().ok())
                .map(|p| std::fs::canonicalize(&p).unwrap_or(p));
            let req = OpenRequest {
                harness_session,
                source,
                process: process.filter(|p| !p.is_empty()),
                dispatched_by: dispatched_by.filter(|d| !d.is_empty()),
                observed_key: observed_key.filter(|k| !k.is_empty()),
                workspace,
                transcript_path: transcript_path.filter(|t| !t.is_empty()),
            };
            let outcome = anchor::open(backend, &req).unwrap_or_else(|e| fail("open", e));
            println!("anchor={}", outcome.anchor);
            println!("opened={}", outcome.opened.as_str());
            println!("observed={}", yes_no(outcome.observed));
            if let Some(previous) = &outcome.previous {
                println!("previous={previous}");
            }
            if let Some(until) = &outcome.inherit_until {
                println!("inherit_until={until}");
            }
            Ok(())
        }
        AnchorCommand::Observe {
            anchor: name,
            key,
            inherit,
        } => {
            let what = match key {
                Some(key) if !inherit => Observe::Key(key),
                _ => Observe::Inherit,
            };
            let outcome =
                anchor::observe(backend, &name, &what).unwrap_or_else(|e| fail("observe", e));
            println!("anchor={name}");
            println!("observed={}", yes_no(outcome.observed));
            Ok(())
        }
        AnchorCommand::Close {
            key,
            harness_session,
        } => {
            let outcome =
                anchor::close(backend, &key, &harness_session).unwrap_or_else(|e| fail("close", e));
            println!("closed={}", outcome.recorded.len());
            Ok(())
        }
    }
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// Print a failed verb's error and exit: 2 for a refusal, 3 otherwise.
fn fail(verb: &str, e: anyhow::Error) -> ! {
    let code = if e.downcast_ref::<AnchorRefusal>().is_some() {
        super::EXIT_CALLER_ERROR
    } else {
        super::EXIT_INFRASTRUCTURE
    };
    super::exit_with_error_code(
        serde_json::json!({
            "error": format!("{e:#}"),
            "command": format!("session anchor {verb}"),
        }),
        code,
    )
}

/// Record that the running agent session opened or ticked `session`:
/// read `KOTO_ANCHOR` (never `KOTO_DISPATCHER_ANCHOR`) and, when it names
/// an anchor, write the held-by links and the throttled activity event
/// [`anchor::hold`] writes. A variable that is unset or names no anchor
/// does nothing and prints nothing; a write that fails warns on stderr and
/// never fails the command.
pub(crate) fn hold_from_env(backend: &Backend, session: &str, command: &str) {
    let Some(name) = anchor::anchor_from_env() else {
        return;
    };
    if !backend.exists(session) {
        return;
    }
    if let Err(e) = anchor::hold(backend, session, &name, command) {
        eprintln!("warning: could not record that '{session}' is held by anchor '{name}': {e:#}");
    }
}
