# Lead: L3 decidable-claims — what does koto expose that makes a documentation claim mechanically decidable?

All findings below are from reading source in the worktree at
`/home/dgazineu/dev/niwaw/tsuku/tsuku+koto_doc_code_drift-a5071d6a/public/koto/.claude/worktrees/doc-code-drift`,
plus four probes I actually built and ran (a compiled Rust integration test and three
Python extractors). Numbers are measured, not estimated.

## Findings

### 1. The crate has a library target, and the clap parser is reachable from it

`Cargo.toml` is a hybrid workspace-root-plus-package layout:

```toml
[workspace]
members = ["koto-stability-tests"]

[package]
name = "koto"
version = "0.12.1-dev"
edition = "2021"
default-run = "koto"

[[bin]]
name = "koto"
path = "src/main.rs"

[dependencies]
clap = { version = "4", features = ["derive"] }
...
[dev-dependencies]
assert_cmd = "2"
assert_fs = "1"
criterion = { version = "0.5", default-features = false, features = ["cargo_bench_support"] }
filetime = "0.2"
predicates = "3.1.4"
tempfile = "3"
```

There is no `[lib]` stanza, so Cargo infers the library target from `src/lib.rs`, which
exists (53 lines) and is a pure module-declaration-plus-re-export file:

```rust
#[cfg(unix)]
pub mod action;
pub mod buildinfo;
pub mod cache;
pub mod cli;
pub mod config;
pub mod discover;
pub mod engine;
pub mod export;
#[cfg(unix)]
pub mod gate;
pub mod session;
pub mod template;
pub mod workflows_surface;
```

`src/main.rs` is 10 lines and holds no CLI definition at all — it is a shim over the
library:

```rust
use clap::Parser;
use koto::cli::{run, App};

fn main() {
    let app = App::parse();
    if let Err(e) = run(app) {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
```

So `koto::cli::App` is public library surface. Since it derives `clap::Parser`, it also
gets `clap::CommandFactory`, meaning `koto::cli::App::command()` yields the fully-built
`clap::Command` tree from anywhere that can `use koto;` and `use clap;`.

**Verified empirically.** I wrote `tests/zz_probe_clap_walk.rs`, ran
`cargo test --test zz_probe_clap_walk -- --nocapture`, and it compiled and passed. (I have
since deleted the probe file; the worktree is back to its original state.) The probe body:

```rust
use clap::CommandFactory;

fn walk(cmd: &clap::Command, prefix: &str, out: &mut Vec<String>) {
    for sub in cmd.get_subcommands() {
        let path = if prefix.is_empty() { sub.get_name().to_string() }
                   else { format!("{prefix} {}", sub.get_name()) };
        out.push(path.clone());
        for alias in sub.get_all_aliases() { out.push(format!("{path} (alias:{alias})")); }
        walk(sub, &path, out);
    }
}

#[test]
fn probe_full_verb_tree() {
    let cmd = koto::cli::App::command();
    let mut out = Vec::new();
    walk(&cmd, "", &mut out);
    ...
}
```

Two things this settles. First, `clap` is a normal `[dependencies]` entry, not a dev-dep,
and integration tests link against both normal and dev deps — so `use clap::CommandFactory;`
inside `tests/` compiles without adding anything to `Cargo.toml`. Second, the walk is
in-process: no binary build, no subprocess, no `--help` parsing.

### 2. The complete verb tree, as walked from clap

52 verb paths. Zero aliases anywhere (the `(alias:...)` lines never fired):

```
koto cancel
koto config {get, list, set, unset}
koto context {add, exists, get, list, remove}
koto dashboard
koto decisions {list, record}
koto init
koto next
koto overrides {list, record}
koto request {abandon, abandon-request, bind, close, create, get, list, progress, resolve, wait}
koto rewind
koto session {cleanup, dir, list, recover, resolve, start, update}
koto status
koto template {compile, export, validate, validate-feed}
koto version
koto workflows {publish}
koto workspace {prune}
```

Where the tree is defined:

| Type | File:line |
|---|---|
| `App` (`#[derive(Parser)]`) | `src/cli/mod.rs:77-85` |
| `Command` (top-level, 17 variants) | `src/cli/mod.rs:87-288` |
| `WorkspaceCommand` | `src/cli/mod.rs:289-322` |
| `DashboardArgs` (`clap::Args`, tuple variant) | `src/cli/mod.rs:326-352` |
| `ConfigCommand` | `src/cli/mod.rs:353-385` |
| `SessionCommand` | `src/cli/mod.rs:386-500` |
| `ChildrenPolicy` (`clap::ValueEnum`) | `src/cli/mod.rs:502-516` |
| `WorkflowsAction` | `src/cli/mod.rs:517-533` |
| `ContextCommand` | `src/cli/mod.rs:534-580` |
| `ExportFormat` / `ExportArgs` | `src/cli/mod.rs:581-600` |
| `TemplateSubcommand` | `src/cli/mod.rs:645-675` |
| `DecisionsSubcommand` | `src/cli/mod.rs:676-700` |
| `OverridesSubcommand` | `src/cli/overrides.rs:23-49` |
| `RequestGroupArgs` / `RequestCommand` / `StateFilter` | `src/cli/request.rs:328-531` |

Note `koto dashboard` is a tuple variant `Dashboard(DashboardArgs)` and `koto workflows`
carries both filter flags *and* an `Option<WorkflowsAction>` subcommand — clap's walk
handles both without the checker needing to know.

**Cheapest and most robust route: (a), the in-crate clap walk.** Ranking the three:

- **(a) walk `App::command()` from a test** — one `cargo test`, no binary, no shelling out,
  no output parsing, and it sees value-enum variants and aliases too. Verified working.
- **(b) shell out to `koto --help` recursively** — needs a release build (~minutes), needs
  a recursive `--help` crawl with output parsing, and clap's help text is presentation, not
  contract: a `#[command(hide = true)]` or a help-template change silently alters what the
  crawl sees. Strictly worse than (a) with no compensating benefit, since (a) is available.
- **(c) parse source** — requires re-implementing clap's derive semantics (kebab-casing
  `ValidateFeed` → `validate-feed`, `#[clap(name = "accept-remote")]` overrides, flattened
  arg groups). Fragile and pointless here.

### 3. Measured: verb drift in the current tree

I built the extractor and ran it. Method: pull candidate invocations from markdown, match
against the 52-path set from clap.

**Naive pass** (any `koto <word>` inside any fence or any inline-code span, all 117 markdown
files): 46 distinct failures, of which roughly 18 were garbage — `koto could`, `koto is`,
`koto didn`, `koto cargo`, `koto binary`, `koto description`, `koto koto`. Prose that
happened to sit inside backticks.

**Tightened pass** (fence language in `{bash, sh, shell, console, zsh, text, ""}`, `koto`
must start a line after an optional `$ ` prompt; inline spans count only when the span
itself starts with `koto `): **2202 invocations extracted, 30 distinct failures, zero
obvious false positives.** Every one of the 30 is a real name that does not exist. The
tightening removed the entire garbage class.

**Scoped pass** (tightened, restricted to `README.md`, `CHANGELOG.md`, `docs/guides/`,
`docs/reference/`, `docs/STABILITY.md`, `docs/workspace-layout.md`, `plugins/**` — i.e.
user-facing surfaces, excluding design docs and PRDs): **31 files, 705 invocations, 3
failures.**

```
UNKNOWN-SUB  koto session rebind
    docs/guides/default-action-authoring.md
    docs/reference/error-codes.md
    plugins/koto-skills/skills/koto-user/SKILL.md
    plugins/koto-skills/skills/koto-user/references/command-reference.md
    plugins/koto-skills/skills/koto-user/references/error-handling.md
UNKNOWN-TOP  koto query
    plugins/koto-skills/skills/koto-user/references/batch-workflows.md
    plugins/koto-skills/skills/koto-user/references/command-reference.md
UNKNOWN-TOP  koto migrate
    docs/STABILITY.md
```

Two of the three are hard positives. `koto session rebind` is the v0.12.0 incident — the
scoped check catches it. `koto query` is a **second, still-live ghost** nobody has noticed:
`plugins/koto-skills/skills/koto-user/references/batch-workflows.md:330` puts it in a
shipped user-skill code fence with no disclaimer —

```bash
koto status parent~1.task-a     # state, outcome, evidence
koto query parent~1.task-a      # full event log
koto workflows --children parent~1  # all children of that attempt
```

The third, `koto migrate` in `docs/STABILITY.md:93`, is a soft positive: "Tool is published
as `koto migrate` or under a similar discoverable subcommand" — a deliberate future-tense
mention. Any real check needs an escape hatch (allowlist file or inline suppression).

The 27 failures the scoping drops are almost all in `docs/designs/` and `docs/prds/`, which
legitimately name proposed-but-unbuilt verbs (`koto daemon`, `koto gc`, `koto enter-child`,
`koto compose`, `koto delegate`, `koto template lint`, `koto session rehome`, and the 12-file
`koto transition` fossil from the pre-`next` era). **Scope is the whole precision story:**
same extractor, 30 findings unscoped versus 3 scoped, and the 3 are the ones that matter.

Set difference the other way: **zero verbs exist that no markdown names.** Every one of the
52 paths appears somewhere.

### 4. Drift in the disclaimers themselves

Three live docs each enumerate the `koto session` subcommands while disclaiming `rebind`,
and they disagree with each other and with the code (which has seven):

- `docs/reference/error-codes.md:107` — "start, dir, list, cleanup, and resolve" — missing
  `recover` **and** `update`.
- `plugins/koto-skills/skills/koto-user/SKILL.md:201` — "start, dir, list, cleanup, resolve,
  and update" — missing `recover`.
- `plugins/koto-skills/skills/koto-user/references/command-reference.md:698` — "start, dir,
  list, cleanup, recover, resolve, and update" — correct.

The prose written to explain the first drift instance has itself drifted. An inline
enumeration is decidable the same way a fence is, and this is a good argument for the check
covering enumerations, not just command lines.

### 5. Template compilation

`src/template/compile.rs:160`:

```rust
pub fn compile(source_path: &Path, strict: bool) -> anyhow::Result<CompiledTemplate>
```

Public, in a `pub mod compile` under `pub mod template` — a library entry point, callable
from a test with no binary. `strict` maps inversely to the CLI's `--allow-legacy-gates`.

What it takes: a path to a markdown file with YAML front-matter. What it validates:
front-matter parses; `name`, `version`, `initial_state` non-empty; `states` non-empty; every
declared state has a directive section in the markdown body; gates compile per type; then
`CompiledTemplate::validate(strict)` for structural rules (cycles, dead ends, unresolvable
transitions, gate routing). `SourceState` carries `#[serde(deny_unknown_fields)]`, so a
typo'd key is a hard error at compile time — deliberately, per the comment at
`src/template/compile.rs:41-46`.

CLI exit behavior (`src/cli/mod.rs:1454-1471`): success prints the cache path to stdout and
exits 0; failure goes through `exit_with_error` with
`{"error": ..., "command": "template compile"}`.

**Could a check feed documented snippets through it? Only narrowly.** I classified every
yaml/markdown fence in the corpus: **10 blocks are fully compilable** (have `---` frontmatter
plus `initial_state:` plus `states:`), against **46 template fragments** that are not
standalone-compilable — a bare `gates:` map, a lone `transitions:` list, an `accepts:` block.
`compile()` is all-or-nothing; there is no fragment or partial-validate mode. So this
mechanism reaches maybe 18% of the documented template syntax, and the fragments — where
authors are most likely to write syntax the compiler rejects — are exactly what it misses.

Worth noting the 10 live in `plugins/koto-skills/skills/koto-author/references/template-format.md` (3),
`plugins/koto-skills/skills/koto-adhoc/SKILL.md` (2), `docs/guides/default-action-authoring.md` (1),
`docs/guides/custom-skill-authoring.md` (1), and design/archive docs (3).

### 6. Paths

Docs cite repo-relative paths heavily and path existence is trivially checkable. I ran it:
**142 distinct cited paths, 23 missing — a 16% dead rate.**

```
docs/decisions/                                  docs/designs/DESIGN-koto-request-store.md
docs/designs/DESIGN-unified-koto-next.md         docs/schema/session-feed-v1.json
docs/template-format.md                          plugins/hello-koto/
plugins/koto-skills/AGENTS.md                    plugins/koto-skills/evals/
plugins/koto-skills/eval.sh                      plugins/koto-skills/evals/hello-koto/
plugins/koto-skills/koto-templates/koto-author.md
plugins/koto-skills/skills/hello-koto/           plugins/koto-skills/skills/hello-koto/hello-koto.md
plugins/koto-skills/skills/hello-koto/SKILL.md   scripts/audit-unknown-fields.sh
scripts/lib/koto-gates.sh                        src/cli/config.rs
src/dashboard/                                   src/engine/gate.rs
src/engine/override_defaults.rs                  src/gate/
src/gate/mod.rs                                  src/template/gate_defaults.rs
```

Two land in live docs, not design docs: `docs/STABILITY.md` and `docs/workspace-layout.md`
both point at `docs/designs/DESIGN-koto-request-store.md` (the file is under `current/`), and
`docs/guides/custom-skill-authoring.md` points at both
`plugins/koto-skills/skills/hello-koto/SKILL.md` and `plugins/koto-skills/eval.sh`, neither of
which exists. The rest are design docs citing module paths that moved — `src/gate/mod.rs`
became `src/gate.rs`, `src/dashboard/` became `src/cli/dashboard*.rs`.

Ambiguity cases, measured rather than speculated:

- **Directories.** Common and unremarkable. `os.path.exists` handles both; a trailing slash
  is a fine signal but not required. `docs/decisions/` is missing, `docs/guides/` is present.
- **Globs.** Nearly a non-issue. Only 2 of the glob-shaped citations are actually paths
  (`src/cli/dashboard*.rs`, `plugins/*/skills/*/`). The other ~95 `*`-containing backtick
  spans are namespace notation, not paths: `gates.*` (69 uses), `request_store.*`, `vars.*`,
  `derive_*`, `koto-*.state.jsonl`. Requiring the first path segment to be a known top-level
  directory of the repo excludes all of them for free.
- **Cross-repo paths.** Two total, both `shirabe/...` (`shirabe/CLAUDE.md`,
  `shirabe/koto-templates/work-on.md`). Same first-segment allowlist handles it.

So: anchor on a first-segment allowlist (`src|tests|docs|plugins|benches|scripts|test|koto-stability-tests`),
which is what my probe did, and the ambiguity classes evaporate.

### 7. Other decidable claim types

**Exit codes.** `NextErrorCode` at `src/cli/next_types.rs:713` — 13 variants,
`#[serde(rename_all = "snake_case")]`, with `NextErrorCode::exit_code()` at
`src/cli/next_types.rs:756` as the authoritative mapping. `RequestErrorCode` at
`src/cli/request.rs:124` — 23 variants, `RequestErrorCode::exit_code()` at
`src/cli/request.rs:191`. Plus `EXIT_INFRASTRUCTURE: i32 = 3` at `src/cli/mod.rs:75` and
`exit_code_for_engine_error()`. Both enums are `pub` in `pub mod cli`, hence enumerable from
an integration test — but only by exhaustive `match`, since neither derives `strum` nor has a
`const ALL` slice. In practice the check would `match` every variant into a table and diff it
against the doc; the compiler's non-exhaustive-match error is itself the drift alarm when a
variant is added.

`docs/reference/error-codes.md` publishes both as clean markdown tables (`| \`code\` | exit |
meaning |`), so the parse side is trivial. **I diffed both and they are currently in sync** —
13/13 names and exits for `next`, and 23/23 for request (exact `diff` match on the sorted
snake_case name sets). The error-code surface is *not* where koto's drift lives.

**Event kinds.** `EventPayload` at `src/engine/types.rs:480` is `#[serde(untagged)]` — the
discriminant is not serde-derived. The authoritative list is the hand-written
`EventPayload::type_name()` at `src/engine/types.rs:1080`, 28 arms mapping variants to wire
strings, and the mapping is not mechanical: `RequestStoreResult => "request_store.result"`,
`RequestCreated => "request.created"`, `RequestLegBound => "request.leg_bound"`. So the wire
names must come from calling `type_name()`, never from name-mangling the variant. `Event` and
`EventPayload` are re-exported through the frozen `koto::engine::types` surface, so a test can
reach them; the enum has an `Unknown` catch-all arm, so a `match` over constructed values
needs care.

**Reserved kinds.** `src/engine/audit.rs:88` — `pub const RESERVED_KINDS: &[&str]`, a real
const slice, plus `REQUEST_STORE_PREFIX: &str = "request_store."` and the
`is_reserved_kind()` predicate. Directly enumerable, no match needed. Same shape at
`src/cli/vars.rs:12` (`RESERVED_VARIABLE_NAMES`) and `src/engine/batch_validation.rs:76`
(`RESERVED_NAMES`, though that one is private `const`).

**Gate types.** Four `pub const` string literals in `src/template/types.rs:252-258`:
`GATE_TYPE_COMMAND = "command"`, `GATE_TYPE_CONTEXT_EXISTS = "context-exists"`,
`GATE_TYPE_CONTEXT_MATCHES = "context-matches"`, `GATE_TYPE_CHILDREN_COMPLETE =
"children-complete"`. Public consts, trivially enumerable, and each has a matching arm in
`built_in_default()` and the field-schema table at `src/template/types.rs:391-435`.

**Config keys.** Two authoritative lists, both mechanically readable:
`src/config/mod.rs:173` — `get_value(config: &KotoConfig, key: &str)`, an 18-arm `match` over
dotted key literals; and `src/config/validate.rs:4` — `PROJECT_ALLOWLIST: &[&str]`, four
entries, the subset legal in project config. `get_value`'s arms are the full key namespace
(`session.backend`, `session.cloud.*`, `request_store.*` ×10, `workflows.native`). The struct
serde names in `KotoConfig`/`WorkflowsConfig`/`RequestStoreConfig`/`SessionConfig`/`CloudConfig`
are the same strings but the `match` is what `koto config get` actually consults, so the
`match` is the contract. Not enumerable as a slice — a checker would call `get_value` with each
documented key and flag any that returns `None`, which is cleaner than reflecting the struct
and needs no test-only code at all.

**State-machine field names.** `StateFileHeader` is part of the frozen Stage 1 surface
(`src/lib.rs` comment block, `docs/STABILITY.md`), and `tests/lib_reexports.rs` already
constructs it field-by-field precisely so a rename breaks the build. That test is the existing
template for "enumerate a type's surface from a test."

### 8. The test suite already builds and invokes the binary — extensively

`assert_cmd` is a dev-dependency and roughly 20 integration tests shell out to the built
binary. The canonical shape, from `tests/dashboard_test.rs:1-8`:

```rust
use assert_cmd::Command;
...
let mut cmd = Command::cargo_bin("koto").unwrap();
```

Also `assert_cmd::cargo::cargo_bin("koto")` fed to `std::process::Command` for cases needing
raw process control (`tests/status_phase_retrieval_test.rs:726,774`; `tests/request_cli.rs:2091,2142`;
`tests/nested_next_test.rs:35`). No `trycmd`, no `escargot`, no `env!("CARGO_BIN_EXE_koto")`.

And `tests/lib_reexports.rs` is the precedent for the *other* direction — an integration test
that imports `koto` as a library to assert a surface has not moved. Its header:

```rust
//! In-tree compile-check for the Stage 1 frozen public surface
//! (Issue 19 / Decision 5).
//! ... A regression here — a re-export that got dropped, a type that moved
//! without an alias, or the `Error` re-export breaking — is caught at
//! `cargo test` time BEFORE the external `koto-stability-tests` crate runs.
```

A doc-drift check as `tests/doc_verbs.rs` would sit naturally beside it and needs neither a
binary build nor a new dependency.

### 9. CI already runs koto against repo content — but the gate topology has a hole

`.github/workflows/validate-plugins.yml` job `template-compilation` builds the release binary
and compiles every template under `plugins/koto-skills/skills/*/koto-templates/*.md`, failing
CI on any rejection. The "build koto, run it against the repo's own content, fail the PR"
pattern is already established and accepted here.

But that job fires on `paths: ['plugins/**', '.claude-plugin/**']` only. A PR touching only
`src/cli/mod.rs` — the PR that removes a verb, or the one that adds an error message naming a
verb that was never built — does not trigger it. `validate-docs.yml` fires on `docs/**` only,
and delegates to shirabe's format validator, which checks frontmatter and section structure,
not claims about code. **Nothing in this repo runs on `src/**` and inspects docs.** That is
the topology reason the rebind message shipped green: the message was added in
`src/cli/next_types.rs:179` and `src/cli/mod.rs:3473,3489`, the docs were written to match,
and no gate had both in scope. A drift check that only runs on doc changes reproduces the
hole exactly.

One more gap worth recording: `.github/workflows/check-template-freshness.yml` is
`workflow_call`-only and **nothing in this repo calls it** (grep for `uses:.*check-template`
returns nothing). It is a reusable workflow published for downstream consumers. So koto's own
templates get compile-checked by `validate-plugins.yml` but never export-freshness-checked.

## Implications

The cheap, robust check is a `cargo test` that walks `koto::cli::App::command()` and diffs it
against verbs extracted from a scoped markdown set. No binary build, no new dependency, no
`--help` parsing, runs in the existing `Unit Tests` job in seconds. I built and ran every
piece of it; nothing here is speculative.

Precision is a scoping decision, not an extraction-cleverness decision. The same extractor
yields 30 findings across all 117 markdown files and 3 across the 31 user-facing ones, and
the 3 are the true ones. Design docs and PRDs must be out of scope — they name proposed verbs
as their job. Extraction rigor still matters (line-anchoring and a fence-language filter
removed an 18-item garbage class), but scope is what turns the check from noisy to shippable.

The check would have caught the v0.12.0 incident, and it catches a second live one nobody
has noticed: `koto query` in the shipped `koto-user` skill's batch-workflows reference.

Path existence is the highest-yield-per-line addition — one `os.path.exists` per citation, a
first-segment allowlist to kill the glob and cross-repo noise, and a measured 16% dead rate to
fix. Whether design docs are in scope for paths is a separate call from verbs: a design doc
citing a module that moved is more defensibly stale than one proposing an unbuilt verb, but
it is also 21 of the 23 hits, so including them means a large one-time cleanup.

Template snippet compilation is real but narrow — 10 compilable blocks against 46 fragments —
and the fragments are where authors are most likely to write rejected syntax. Not the place to
start.

Error codes need no check right now. Both tables are exactly in sync today, verified by diff.
Worth a cheap assertion to keep them that way, but it is maintenance, not repair.

Whatever gets built has to run on `src/**` changes, not only `docs/**`. That is the specific
gate-topology property that let the incident through, and it is easy to reproduce by accident.

## Surprises

`koto query` is live in a shipped user-facing skill reference, in a bash fence, with no
disclaimer — the same failure class as the rebind incident, still open, in the same skill.

The `rebind` disclaimers have themselves drifted. Three live docs enumerate `koto session`'s
subcommands while explaining that `rebind` is missing, and all three disagree: one omits
`recover` and `update`, one omits `recover`, one is right. Prose written to document drift
drifted.

`main.rs` is 10 lines. I expected the parser to be main-only and the check to need a binary;
the entire CLI already lives in the library, and `App` is public. The check is far cheaper
than the framing suggested.

Both error-code tables are perfectly in sync — 13/13 and 23/23 on an exact diff. Given the
verb and path drift, I expected at least one stale row.

CI already does the hard version of this. `validate-plugins.yml` builds the release binary
and compiles every plugin template. The precedent for "run koto against the repo and fail the
PR" is set; only the trigger paths and the claim type are new.

`check-template-freshness.yml` exists, is well-built, and is dead code in this repo — nothing
calls it. It is published for downstream consumers, so koto's own diagrams are never
freshness-checked.

## Open Questions

Are design docs and PRDs in scope for *path* claims even though they are out of scope for
verb claims? They hold 21 of the 23 dead paths, so including them means a one-time cleanup of
real size, and a design doc citing a module that moved is a different kind of wrong from one
proposing an unbuilt verb.

What is the escape hatch for deliberate forward references? `docs/STABILITY.md`'s `koto
migrate` is correct as written — it describes a tool that will exist. An allowlist file, an
HTML-comment suppression marker, and a "not implemented" section-heading convention (which
`command-reference.md:696` already uses informally) are all plausible; they have different
review properties.

Does the check cover flags, or only verbs? `koto next --dispatch-epoch` and `koto request
wait --resolved-count` are documented and equally decidable from the same clap walk
(`get_arguments()` per subcommand), but flags are far more numerous and the false-positive
surface from prose is larger. Verbs alone would have caught both known incidents.

Should the reverse direction — verbs that exist but are undocumented — be a gate? It is
currently zero, so it would be free to add and would stay green, but it also constrains every
future PR that adds a verb to document it in the same PR. That may be desirable or may be
friction.

Is the `plugins/koto-skills/skills/*/references/` corpus the highest-value scope, given both
live ghosts are there and it is what agents actually read at runtime? A narrower first
version scoped to plugins alone would be nearly zero-noise.

## Summary

koto's CLI lives entirely in a library target (`src/lib.rs`, `pub mod cli`) with a 10-line
`main.rs`, so the complete 52-path verb tree is walkable in-process via
`koto::cli::App::command()` from an ordinary `cargo test` — I wrote and ran that probe, and
it needs no binary build, no `--help` parsing, and no new dependency. A tightened extractor
scoped to user-facing docs (31 files, 705 invocations) produces exactly 3 findings with zero
garbage: the known `koto session rebind` ghost, a second live one nobody has noticed (`koto
query`, in a shipped `koto-user` skill fence), and one deliberate forward reference; the same
extractor unscoped yields 30, so scope — not extraction cleverness — is what makes this
shippable. Path existence adds high yield for almost no code (142 cited paths, 23 dead, 16%),
error codes are already perfectly in sync and need no repair, and template-snippet compilation
reaches only 10 of 56 documented template blocks; the binding constraint is gate topology, since
the check must run on `src/**` changes and not only `docs/**` — the exact hole that let v0.12.0
ship green.
