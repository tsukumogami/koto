//! `koto decider report`: the ledger report and the fixture runner.
//!
//! The command only reads. It reads the ledger, and with `--fixtures` it
//! reads the config files, compiles the named template in memory (no
//! compile cache), and reads the fixture file. It writes no ledger line,
//! no session event, and no file of any kind, and it never changes a mode.
//!
//! Whether a fixture run may send anything is decided by
//! [`crate::decider::build_decider`], which asks
//! `DeciderSettings::opted_in()` and nothing else.
//!
//! Exit codes: 0 when the report printed (whether or not any value is
//! eligible); 2 for a caller problem (not opted in, a bad template, state,
//! field, or fixture line, or an endpoint the run can't use); 3 when the
//! ledger exists but can't be read.

use std::path::{Path, PathBuf};

use clap::Args;

use crate::decider::ledger::ledger_path;
use crate::decider::report::{
    analyze, judge, parse_fixtures, read_ledger, render_table, run_fixtures, FixtureReport,
    JudgeInput, LedgerRead, Report, ReportOptions,
};
use crate::decider::request::{declared_fields, DeclaredField};
use crate::decider::{build_decider, EffectiveModes};
use crate::engine::decider::DeciderPolicy;
use crate::template::decider::declaration_hash;

use super::{EXIT_CALLER_ERROR, EXIT_INFRASTRUCTURE};

/// Subverbs under `koto decider`.
#[derive(clap::Subcommand)]
pub enum DeciderCommand {
    /// Report decider agreement from the ledger and, with --fixtures,
    /// judge promotion eligibility against a golden fixture set
    Report(ReportArgs),
}

/// Flags of `koto decider report`.
#[derive(Args, Debug, Clone)]
pub struct ReportArgs {
    /// Ledger to read (default: _decider_ledger.jsonl in the koto home
    /// directory, ~/.koto)
    #[arg(long, value_name = "PATH")]
    pub ledger: Option<PathBuf>,

    /// Report only questions on this state; with --fixtures, the state
    /// whose declaration the fixtures exercise
    #[arg(long, value_name = "STATE")]
    pub state: Option<String>,

    /// Print the report as JSON
    #[arg(long)]
    pub json: bool,

    /// Count consultations and fixture runs against a user or env endpoint
    /// toward promotion eligibility
    #[arg(long)]
    pub include_custom_endpoints: bool,

    /// Run this JSON Lines fixture set against the configured decider and
    /// judge promotion eligibility (needs --template and --state, and an
    /// opted-in decider)
    #[arg(long, value_name = "PATH", requires = "template", requires = "state")]
    pub fixtures: Option<PathBuf>,

    /// Template source whose declaration the fixtures exercise
    #[arg(long, value_name = "PATH", requires = "fixtures")]
    pub template: Option<PathBuf>,

    /// Declared field to exercise when the state declares more than one
    #[arg(long, value_name = "FIELD", requires = "fixtures")]
    pub field: Option<String>,
}

/// Print `msg` on stderr and exit with `code`. Nothing reaches stdout.
fn fail(code: i32, msg: impl std::fmt::Display) -> ! {
    eprintln!("koto decider report: {}", msg);
    std::process::exit(code);
}

pub fn handle(cmd: DeciderCommand) -> anyhow::Result<()> {
    match cmd {
        DeciderCommand::Report(args) => handle_report(args),
    }
}

fn default_ledger() -> PathBuf {
    match dirs::home_dir() {
        Some(home) => ledger_path(&home.join(".koto")),
        None => fail(
            EXIT_CALLER_ERROR,
            "no home directory to find the ledger in; pass --ledger",
        ),
    }
}

fn load_ledger(path: &Path) -> LedgerRead {
    read_ledger(path).unwrap_or_else(|e| {
        fail(
            EXIT_INFRASTRUCTURE,
            format!("failed to read ledger {}: {}", path.display(), e),
        )
    })
}

fn handle_report(args: ReportArgs) -> anyhow::Result<()> {
    let ledger = args.ledger.clone().unwrap_or_else(default_ledger);
    let opts = ReportOptions {
        state: args.state.clone(),
        include_custom_endpoints: args.include_custom_endpoints,
    };

    let fixtures = args
        .fixtures
        .as_ref()
        .map(|path| fixture_run(&args, path, &ledger, &opts));

    let read = load_ledger(&ledger);
    let ledger_report = analyze(&read, &opts);
    let report = Report {
        ledger: ledger.display().to_string(),
        header: ledger_report.header,
        questions: ledger_report.questions,
        fixtures,
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_table(&report));
    }
    Ok(())
}

/// Validate the template, state, field, and fixture file; then, only if the
/// user is opted in, send every case and judge eligibility.
fn fixture_run(
    args: &ReportArgs,
    fixtures_path: &Path,
    ledger: &Path,
    opts: &ReportOptions,
) -> FixtureReport {
    // clap guarantees both with --fixtures.
    let (Some(template_path), Some(state)) = (&args.template, &args.state) else {
        fail(EXIT_CALLER_ERROR, "--fixtures needs --template and --state");
    };

    let compiled = crate::template::compile::compile(template_path, true).unwrap_or_else(|e| {
        fail(
            EXIT_CALLER_ERROR,
            format!(
                "failed to compile template {}: {}",
                template_path.display(),
                e
            ),
        )
    });
    let Some(template_state) = compiled.states.get(state) else {
        fail(
            EXIT_CALLER_ERROR,
            format!(
                "state {:?} is not in template {}",
                state,
                template_path.display()
            ),
        );
    };
    let declared: Vec<DeclaredField<'_>> = template_state
        .accepts
        .as_ref()
        .map(declared_fields)
        .unwrap_or_default();
    let field = select_field(state, &declared, args.field.as_deref());

    let body = std::fs::read_to_string(fixtures_path).unwrap_or_else(|e| {
        fail(
            EXIT_CALLER_ERROR,
            format!("failed to read fixtures {}: {}", fixtures_path.display(), e),
        )
    });
    let cases = parse_fixtures(&body, &field).unwrap_or_else(|e| {
        fail(
            EXIT_CALLER_ERROR,
            format!("{}: {}; nothing was sent", fixtures_path.display(), e),
        )
    });

    // Opt-in: build_decider() returns a client only when opted_in() holds.
    let config = crate::config::resolve::load_config()
        .unwrap_or_else(|e| fail(EXIT_CALLER_ERROR, format!("failed to load config: {:#}", e)));
    let (settings, warnings) = crate::config::resolve::resolve_decider(&config.decider);
    for w in &warnings {
        eprintln!("warning: {}", w);
    }
    let Some(decider) = build_decider(&settings) else {
        fail(
            EXIT_CALLER_ERROR,
            "fixture runs need an opted-in decider: set KOTO_DECIDER (or decider.mode in \
             ~/.koto/config.toml) to shadow or auto, provide an API key, and use an endpoint \
             from the key's own layer or the default; nothing was sent",
        );
    };

    let policy = DeciderPolicy::from_settings(&settings);
    let mut modes = EffectiveModes::new();
    for (value, answer) in &field.decider.answers {
        modes.set(field.name, value.as_str(), policy.mode_for(answer.mode));
    }

    let results = run_fixtures(decider.as_ref(), &field, &modes, &cases).unwrap_or_else(|e| {
        fail(
            EXIT_CALLER_ERROR,
            format!(
                "fixture runs need network access to the configured endpoint, and it failed \
                 ({}); no result was recorded",
                e
            ),
        )
    });

    let hash = declaration_hash(field.decider, field.description);
    let ledger_report = analyze(&load_ledger(ledger), opts);
    judge(
        &JudgeInput {
            template: template_path.display().to_string(),
            state: state.clone(),
            field: &field,
            declaration_hash: hash.clone(),
            endpoint_origin: settings.endpoint_origin(),
            include_custom_endpoints: opts.include_custom_endpoints,
            ledger: ledger_report.question(state, field.name, &hash),
        },
        results,
    )
}

fn select_field<'a>(
    state: &str,
    declared: &[DeclaredField<'a>],
    wanted: Option<&str>,
) -> DeclaredField<'a> {
    let names: Vec<&str> = declared.iter().map(|f| f.name).collect();
    match wanted {
        Some(name) => declared
            .iter()
            .find(|f| f.name == name)
            .copied()
            .unwrap_or_else(|| {
                fail(
                    EXIT_CALLER_ERROR,
                    format!(
                        "state {:?} declares no decider field {:?} (declared: {})",
                        state,
                        name,
                        if names.is_empty() {
                            "none".to_string()
                        } else {
                            names.join(", ")
                        }
                    ),
                )
            }),
        None => match declared {
            [] => fail(
                EXIT_CALLER_ERROR,
                format!("state {:?} declares no decider field", state),
            ),
            [one] => *one,
            _ => fail(
                EXIT_CALLER_ERROR,
                format!(
                    "state {:?} declares {} decider fields ({}); pass --field",
                    state,
                    names.len(),
                    names.join(", ")
                ),
            ),
        },
    }
}
