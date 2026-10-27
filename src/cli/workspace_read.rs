//! `koto --workspace <id> <verb>`: read a session in another workspace's
//! part of the cloud bucket, in place, without importing it (koto#455).
//!
//! Only the read verbs accept the option: [`read_verb`] names them, and every
//! other verb, today's and any added later, refuses it with
//! `workspace_read_only` before anything is read or written. The reads go
//! through a [`WorkspaceReader`], a read-only cloud backend on the named
//! workspace's prefix whose local copies live in a scratch directory that is
//! removed however the command exits.
//!
//! A read through `--workspace` logs nothing: `context get` and `exists`
//! record no `context_read`, anchored or not, since that would write to the
//! session read or to this host. It reads no log either, so it answers from
//! the store alone.

use anyhow::Result;

use super::{
    context, exit_with_error_code, handle_status, Command, ContextCommand, EXIT_CALLER_ERROR,
    EXIT_FLUSH,
};
use crate::session::cloud::workspace_read::{WorkspaceId, WorkspaceReadError, WorkspaceReader};
use crate::session::Backend;

/// The `code` a verb that refuses `--workspace` prints.
pub const READ_ONLY_CODE: &str = "workspace_read_only";

/// The verb `command` is and the session it reads, when it is one of the
/// read verbs `--workspace` serves; `None` for every other verb.
///
/// This is the whole guard, and it fails closed: a verb added to koto is
/// refused until it is added here. `tests` walks every verb of the CLI to
/// hold it to that. A listing of the workspace's sessions (koto#343) joins
/// here when it exists, over [`WorkspaceReader::sessions`].
pub(super) fn read_verb(command: &Command) -> Option<(&'static str, &str)> {
    match command {
        Command::Status { name } => Some(("status", name)),
        Command::Context { subcommand } => match subcommand {
            ContextCommand::Get { session, .. } => Some(("context get", session)),
            ContextCommand::Exists { session, .. } => Some(("context exists", session)),
            ContextCommand::List { session, .. } => Some(("context list", session)),
            ContextCommand::Add { .. } | ContextCommand::Remove { .. } => None,
        },
        _ => None,
    }
}

/// Print `e` as `command`'s error envelope and exit with its code.
fn exit_with_read_error(command: &str, e: &WorkspaceReadError) -> ! {
    exit_with_error_code(
        serde_json::json!({
            "error": e.message,
            "code": e.code.as_str(),
            "command": command,
        }),
        e.code.exit_code(),
    );
}

/// Run `command` against the session it names in `workspace`'s part of the
/// bucket. Refuses any verb but the read verbs, a workspace that names
/// nothing, and a workspace whose session backend isn't the cloud one.
pub(super) fn run(workspace: &str, command: Command) -> Result<()> {
    let Some((verb, name)) = read_verb(&command).map(|(verb, name)| (verb, name.to_string()))
    else {
        exit_with_error_code(
            serde_json::json!({
                "error": "--workspace reads a session in another workspace, so only \
                          `koto status`, `koto context get`, `koto context exists` and \
                          `koto context list` accept it; a verb that writes or removes \
                          refuses it",
                "code": READ_ONLY_CODE,
            }),
            EXIT_CALLER_ERROR,
        );
    };
    let id = WorkspaceId::parse(workspace).unwrap_or_else(|e| exit_with_read_error(verb, &e));

    let config = crate::config::resolve::load_config()?;
    crate::config::session_env::check(&config)?;
    if config.session.backend != "cloud" {
        exit_with_error_code(
            serde_json::json!({
                "error": format!(
                    "--workspace reads the cloud bucket, and the session backend here is '{}'",
                    config.session.backend
                ),
                "code": "workspace_needs_cloud",
                "command": verb,
            }),
            EXIT_CALLER_ERROR,
        );
    }
    let reader = WorkspaceReader::open(&config.session.cloud, id)?;
    // The scratch directory goes however the command exits: on return when
    // `_scratch` drops, and on `exit_with_error_code`'s way out through the
    // exit flush. Only `koto next` sets that flush otherwise, and it never
    // gets here; a read verb that set it first would leave scratch behind,
    // so that is refused loudly rather than ignored.
    let scratch_path = reader.scratch_dir().to_path_buf();
    let set = EXIT_FLUSH.set(Box::new(move || {
        let _ = std::fs::remove_dir_all(&scratch_path);
    }));
    assert!(
        set.is_ok(),
        "the exit flush was already set, so a --workspace read couldn't remove its scratch"
    );
    let located = reader.locate(&name).and_then(|()| {
        if verb == "status" {
            reader.fetch_template(&name)
        } else {
            Ok(())
        }
    });
    if let Err(e) = located {
        exit_with_read_error(verb, &e);
    }
    // `scratch` owns the directory `backend` keeps its copies in, so it
    // must outlive `backend`: it is dropped last, or by hand before
    // `process::exit` below.
    let (cloud, scratch) = reader.into_parts();
    let backend = Backend::Cloud(cloud);
    // A session imported into another workspace refuses here, exit 2, as
    // it does without `--workspace`, naming where it went. There is no
    // `session_bound_elsewhere` check, on purpose: that refusal guards this
    // host's shared copy against another directory's prefix, and this
    // backend reads the session under the prefix it was named by into a
    // scratch copy, never the shared one.
    if let Err(e) = backend.check_not_migrated(&name) {
        exit_with_error_code(
            serde_json::json!({ "error": e.to_string(), "command": verb }),
            EXIT_CALLER_ERROR,
        );
    }

    let Command::Context { subcommand } = command else {
        // `read_verb` let through only `status` and the context reads.
        return handle_status(&backend, &name);
    };
    match subcommand {
        ContextCommand::Get {
            session,
            key,
            to_file,
        } => {
            // No events and no reader anchor: the read is logged nowhere
            // (see the module header), unlike the same verb without
            // `--workspace`.
            if let Err(e) = context::handle_get(
                &backend,
                &backend,
                None,
                None,
                &session,
                &key,
                to_file.as_deref(),
            ) {
                context::exit_with_failed_read(verb, &e);
            }
        }
        ContextCommand::Exists { session, key } => {
            // Logged nowhere, as `context get` above.
            let code = match context::handle_exists(&backend, &backend, None, None, &session, &key)
            {
                context::KeyPresence::Present => 0,
                context::KeyPresence::Absent => 1,
                context::KeyPresence::Unusable(reason) => {
                    exit_with_error_code(
                        serde_json::json!({ "error": reason, "command": verb }),
                        EXIT_CALLER_ERROR,
                    );
                }
            };
            // `process::exit` runs no destructor and not the exit flush,
            // which only `exit_with_error_code` runs, so the scratch
            // directory is removed here by hand.
            drop(backend);
            drop(scratch);
            std::process::exit(code);
        }
        ContextCommand::List { session, prefix } => {
            if let Err(e) = context::handle_list(&backend, None, &session, prefix.as_deref()) {
                context::exit_with_failed_read(verb, &e);
            }
        }
        ContextCommand::Add { .. } | ContextCommand::Remove { .. } => {
            unreachable!("read_verb refuses context add and remove")
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::App;
    use clap::{CommandFactory, Parser};

    /// The verbs `--workspace` reads through. Everything else refuses it.
    const READ_VERBS: [&str; 4] = ["status", "context get", "context exists", "context list"];

    /// Every runnable verb of `cmd`, as the words that name it: each leaf,
    /// and each group whose subcommand is optional (`koto workflows`).
    fn verbs(cmd: &clap::Command, path: &[String], out: &mut Vec<Vec<String>>) {
        for sub in cmd.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            let mut words = path.to_vec();
            words.push(sub.get_name().to_string());
            if sub.has_subcommands() {
                if !sub.is_subcommand_required_set() {
                    out.push(words.clone());
                }
                verbs(sub, &words, out);
            } else {
                out.push(words);
            }
        }
    }

    /// The `clap::Command` the words name.
    fn find<'a>(root: &'a clap::Command, words: &[String]) -> &'a clap::Command {
        words.iter().fold(root, |cmd, word| {
            cmd.find_subcommand(word)
                .unwrap_or_else(|| panic!("no subcommand {word:?}"))
        })
    }

    /// A value clap accepts for `arg`: its first possible value, else a
    /// word that parses as a name, a path or a number alike.
    fn value_for(arg: &clap::Arg) -> String {
        arg.get_possible_values()
            .first()
            .map(|v| v.get_name().to_string())
            .unwrap_or_else(|| "1".to_string())
    }

    /// The arguments `cmd` can't run without: each required argument, and
    /// one argument of each required group.
    fn required_args(cmd: &clap::Command) -> Vec<String> {
        let mut required: Vec<&clap::Arg> = cmd
            .get_arguments()
            .filter(|a| a.is_required_set() && !a.is_global_set())
            .collect();
        for group in cmd.get_groups().filter(|g| g.is_required_set()) {
            let first = group
                .get_args()
                .next()
                .expect("a required group has an arg");
            if let Some(arg) = cmd.get_arguments().find(|a| a.get_id() == first) {
                if !required.iter().any(|r| r.get_id() == arg.get_id()) {
                    required.push(arg);
                }
            }
        }
        let mut positional = Vec::new();
        let mut named = Vec::new();
        for arg in required {
            let takes_value = arg.get_action().takes_values();
            if arg.is_positional() {
                positional.push((arg.get_index().unwrap_or(0), value_for(arg)));
            } else if let Some(long) = arg.get_long() {
                named.push(format!("--{long}"));
                if takes_value {
                    named.push(value_for(arg));
                }
            }
        }
        positional.sort();
        positional
            .into_iter()
            .map(|(_, v)| v)
            .chain(named)
            .collect()
    }

    /// Parse `argv`, adding each option clap still says is missing (one
    /// required only in the absence of another, say) until it parses.
    fn parse_filling_in(cmd: &clap::Command, mut argv: Vec<String>) -> App {
        use clap::error::{ContextKind, ContextValue, ErrorKind};
        for _ in 0..8 {
            let err = match App::try_parse_from(&argv) {
                Ok(app) => return app,
                Err(err) => err,
            };
            let missing = match (err.kind(), err.get(ContextKind::InvalidArg)) {
                (ErrorKind::MissingRequiredArgument, Some(ContextValue::Strings(missing))) => {
                    missing.clone()
                }
                _ => panic!("`{}` doesn't parse: {err}", argv.join(" ")),
            };
            for usage in missing {
                let long = usage
                    .split_whitespace()
                    .next()
                    .and_then(|flag| flag.strip_prefix("--"))
                    .unwrap_or_else(|| panic!("`{}` lacks {usage}", argv.join(" ")));
                let arg = cmd
                    .get_arguments()
                    .find(|a| a.get_long() == Some(long))
                    .unwrap_or_else(|| panic!("no --{long} on `{}`", cmd.get_name()));
                argv.push(format!("--{long}"));
                if arg.get_action().takes_values() {
                    argv.push(value_for(arg));
                }
            }
        }
        panic!("`{}` still doesn't parse", argv.join(" "));
    }

    /// Every verb koto has, parsed with `--workspace`: the read verbs are
    /// let through and every other verb refuses. A verb added later is
    /// refused without anyone listing it, since the guard is an allowlist;
    /// this walks the live CLI so a read verb that is renamed or dropped,
    /// or a verb the guard lets through by mistake, fails here.
    #[test]
    fn only_the_read_verbs_accept_workspace() {
        // Built, so each group lists the arguments that joined it.
        let mut root = App::command();
        root.build();
        let mut all = Vec::new();
        verbs(&root, &[], &mut all);
        assert!(all.len() > 40, "walked only {} verbs", all.len());

        let mut let_through = Vec::new();
        for words in &all {
            let cmd = find(&root, words);
            let mut argv = vec![
                "koto".to_string(),
                "--workspace".to_string(),
                "0123456789abcdef".to_string(),
            ];
            argv.extend(words.iter().cloned());
            argv.extend(required_args(cmd));
            let app = parse_filling_in(cmd, argv);
            assert_eq!(app.workspace.as_deref(), Some("0123456789abcdef"));
            if let Some((verb, _)) = read_verb(&app.command) {
                assert_eq!(
                    verb,
                    words.join(" "),
                    "read_verb misnames `{}`",
                    words.join(" ")
                );
                let_through.push(words.join(" "));
            }
        }
        let_through.sort();
        let mut expected = READ_VERBS.to_vec();
        expected.sort();
        assert_eq!(let_through, expected, "the verbs --workspace reads through");
    }

    /// `--workspace` is an option of `koto` itself, before the verb: after a
    /// verb that has no option of that name, clap refuses it.
    #[test]
    fn workspace_goes_before_the_verb() {
        let err = App::try_parse_from(["koto", "next", "s", "--workspace", "0123456789abcdef"])
            .err()
            .expect("--workspace after the verb is refused");
        assert!(err.to_string().contains("--workspace"), "{err}");
    }
}
