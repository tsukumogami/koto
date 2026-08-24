# Lead: How does the wider ecosystem check that documentation agrees with code, and what of it fits a Rust CLI like koto?

Method note: the ecosystem half is web research with URLs. The fit half is not
argument — I built three throwaway prototypes against this worktree and ran them,
so every precision/recall number below is measured on koto's real tree, not
estimated. The prototypes lived in `$CLAUDE_JOB_DIR/tmp` and one temporary test
file (`tests/zz_prototype_doc_verbs.rs`) that I have since deleted; the tree is
clean.

## Findings

### 0. What koto already has (measured, not recalled)

`Cargo.toml` dev-dependencies, verbatim:

```toml
[dev-dependencies]
assert_cmd = "2"
assert_fs = "1"
criterion = { version = "0.5", default-features = false, features = ["cargo_bench_support"] }
filetime = "0.2"
predicates = "3.1.4"
tempfile = "3"
```

`tempfile` is also a *runtime* dependency (`[dependencies]`), and `clap = { version = "4", features = ["derive"] }` is a runtime dependency.

Present in `Cargo.lock`: `clap`, `assert_cmd`, `assert_fs`, `predicates`, `tempfile`, `criterion`.
**Absent**: `insta`, `trycmd`, `snapbox`, `clap_mangen`, `clap_complete`, `clap-markdown`.

So the four crates a "just add a snapshot/literate-testing crate" answer would
reach for are all new dependencies. That matters less than it sounds — see the
verdicts — because the measurement below shows koto needs none of them.

### 1. `trycmd` / `snapbox` — wrong tool for this bug

[trycmd](https://docs.rs/trycmd/) enumerates test-case files (`*.toml`, or
markdown) and runs the commands inside them, comparing stdout/stderr/exit
status against the expected output written in the file. It is the closest thing
in the Rust ecosystem to "literate CLI testing".

What it requires of the docs, from the docs.rs reference:

- It only reads fenced blocks whose info string is **`trycmd` or `console`**. Every other language, and any block tagged `ignore`, is skipped entirely.
- Inside a recognized block the syntax is a shell transcript: `$` starts a command, `>` continues the prior one, `? <status>` asserts an exit code, and every following line is expected stdout+stderr.
- The first argument is resolved against `bin.name` from `Cargo.toml`, so it runs the real built binary.
- Output matching supports only crude elision: `...`, `[..]`, `[EXE]`, `[ROOT]`, `[CWD]`. `TRYCMD=overwrite` re-records snapshots "best-effort" and does not guarantee elision patterns survive.

Measured against koto's docs: koto has **85 ```bash fences and zero ```console
or ```trycmd fences** across `docs/` and `README.md`. Adopting trycmd means
rewriting all 85 blocks into shell-transcript form *and authoring expected
output for each one*, then keeping that output current forever. Most of those
blocks are illustrative fragments (`koto next <workflow>`, config snippets),
not runnable transcripts with stable output.

**Does it verify a command exists?** Only incidentally: an unknown subcommand
makes clap exit 2 with a usage error, which mismatches the recorded output, so
the test fails. But that only happens for blocks you converted *and* recorded
output for. It cannot see `koto session rebind` in a prose sentence, in a table
cell, or — decisively — in a Rust string literal.

**Verdict: wrong tool.** It answers "does this documented command still produce
this output", which koto does not currently ask. Cost is 85 block rewrites plus
permanent output maintenance, plus a new dev-dependency, in exchange for a
check that structurally cannot reach the place the v0.12.0 bug lived.
[snapbox](https://github.com/assert-rs/snapbox) is the lower-level library
behind it and has the same blind spot; its own docs position it as "trycmd for
one-off cases".

### 2. clap introspection — the right primitive, and it is nearly free

I proved this rather than asserting it. A temporary integration test:

```rust
use clap::CommandFactory;
use koto::cli::App;
fn walk(cmd: &clap::Command, prefix: &str, out: &mut BTreeSet<String>) {
    for sub in cmd.get_subcommands() { /* recurse, push "session start" etc. */ }
}
```

Result: **52 verb paths enumerated, test body ran in 0.00 s**, whole
`cargo test --test …` invocation 6.3 s wall on a warm target dir (that is
almost entirely the incremental link, not the check). The full tree:

```
cancel  config{get,list,set,unset}  context{add,exists,get,list,remove}
dashboard  decisions{list,record}  init  next  overrides{list,record}
request{abandon,abandon-request,bind,close,create,get,list,progress,resolve,wait}
rewind  session{cleanup,dir,list,recover,resolve,start,update}  status
template{compile,export,validate,validate-feed}  version  workflows{publish}
workspace{prune}
```

No `session rebind`. No `session info`. No `query`, `transition`, or `state`.

This works because `koto::cli::App` is public (`src/lib.rs:5` re-exports
`pub mod cli`) and `clap` is a normal dependency, which Cargo makes available to
integration tests. **Zero new dependencies.** `Command::get_subcommands()`,
`get_name()`, `get_all_aliases()`, and `get_arguments()` give you the whole
surface including flags and aliases.

`clap_mangen` and `clap_complete` are *output* generators (roff, shell
completions) built on the same introspection. They are irrelevant to a checker —
you would only add them if you wanted to ship man pages or completions.

**Verdict: good fit.** This is the ground truth any check should compare
against, and it costs one `use` statement.

### 3. `#[doc = include_str!("../README.md")]` + doctests — narrow

The standard trick: pull the README into the crate docs so its fenced blocks
become doctests. koto does not do this today (`grep include_str src/lib.rs` is
empty). The limitation is structural: rustdoc only compiles blocks it believes
are **Rust**. A ```bash block is never compiled, never run, never checked. koto's
README and guides document a *CLI*, so the blocks that matter are shell, not
Rust.

There is a real sub-case: `docs/guides/library-usage.md` and 105 ```rust fences
across `docs/`. Those *are* compile-checkable, and doctests would catch API
drift in them. That is a separate (also real) slice of the class, worth
recording, but it is not the slice that shipped the v0.12.0 bug.

**Verdict: partial fit** — right tool for the Rust-snippet slice, irrelevant to
CLI verbs, template syntax, build commands, and paths.

### 4. Link/path checkers — `lychee` is the only one that reaches plain markdown

- [`cargo-deadlinks`](https://github.com/deadlinks/cargo-deadlinks) checks links in **generated rustdoc HTML**. It never looks at `docs/*.md`. Wrong tool here.
- [`mdbook-linkcheck`](https://github.com/Michael-F-Bryan/mdbook-linkcheck) is an mdbook *backend*. It requires a `book.toml` and an mdbook-shaped source tree. koto's `docs/` is a flat set of markdown directories, not a book. Adopting it means adopting mdbook. Its config does cover the useful knobs (`follow-web-links`, `traverse-parent-directories`, regex excludes) if you were already an mdbook shop.
- [`lychee`](https://github.com/lycheeverse/lychee) checks links in plain markdown and **does resolve relative links to adjacent local files by default**; `--root-dir` extends that to root-relative (`/foo`) links, though it [requires a fully-qualified path](https://lychee.cli.rs/recipes/local-folder/). There is a maintained [lychee-action](https://github.com/lycheeverse/lychee-action).

The catch, and it is the whole catch: lychee checks **markdown link targets**
(`[text](path)`), not **backticked paths in prose**. koto's docs name source
files as `` `src/cli/mod.rs` ``, not as links. I measured the difference.

**Backticked repo-relative path existence, measured on this tree:**

| Scope | Backticked repo paths | Do not exist |
|---|---|---|
| guides + reference + README | 20 | **7 (35%)** |
| designs | 493 | 24 (5%) |

The seven dead paths in user-facing guides are all genuine, currently-shipping
doc bugs. The worst:

- `docs/guides/custom-skill-authoring.md:5` — "The hello-koto skill in `plugins/koto-skills/skills/hello-koto/` is the reference implementation. This guide explains every piece of it." That directory does not exist. `plugins/koto-skills/skills/` contains `koto-adhoc`, `koto-author`, `koto-user`. The entire guide walks through a nonexistent example, and names it four more times (`eval.sh`, `evals/`, `hello-koto.md`, `SKILL.md`).
- `docs/guides/template-freshness-ci.md:9` names `.github/workflows/check-templates.yml`; the real file is `check-template-freshness.yml`.

**Verdict: lychee is a partial fit** (it would catch broken *markdown links*, a
real but different slice, and it is a binary install in CI, not a dependency).
For the backticked-path slice, a 20-line existence check beats it — measured
above, and it found seven real bugs on first run.

### 5. General approaches, with named projects and failure modes

**Help-text snapshot tests.** Record `--help` output to a golden file, diff on
every run. [insta](https://insta.rs/) is the Rust standard;
[ratatui documents the pattern](https://ratatui.rs/recipes/testing/snapshots/);
`cargo insta review` is the accept/reject loop.

The failure mode is not theoretical here — **koto already has a snapshot test of
exactly this shape, and it is currently locking in a wrong command.**
`src/engine/respawn.rs:98` defines the prompt every resuming agent receives:

```rust
pub const RESUME_CONTEXT_PROMPT: &str = "You are resuming session <id>. Read your prior state via `koto session info <id>` and prior children via `koto session list --parent <id>`; advance from where you left off.";
```

`koto session info` **does not exist**. Neither does `--parent` on
`session list` (the clap dump shows `List,` — a bare unit variant with no
arguments; `--parent` belongs to `session start`). And `tests/respawn.rs:597`
asserts that string byte-for-byte, with the comment "any edit to this template
requires changing the test deliberately". The snapshot is doing its job
perfectly: it guarantees the string never changes. It has no opinion on whether
the string is true. **Snapshot tests detect change, not correctness** — and
here one is actively cementing an error into a prompt shipped to agents.

**Generated CLI reference docs.** Generate the reference *from* the parser so it
cannot drift. [clap-markdown](https://github.com/ConnorGray/clap-markdown)
renders `clap_markdown::help_markdown::<Cli>()` to a `CommandLineHelp.md` you
commit — its README's rationale is that "committing `CommandLineHelp.md` to
version control makes it easy to track user-visible changes to the
command-line interface". Real users: `wolfram-app-discovery`, `wolfram-cli`, and
[jj (Jujutsu) uses it for its CLI reference](https://github.com/martinvonz/jj/pull/3891).
The rust-cli book [recommends clap_mangen in `build.rs`](https://rust-cli.github.io/book/in-depth/docs.html)
and regenerating in CI, failing the build when the output differs from what is
checked in. Failure mode: it guarantees only that *the generated file* matches
the parser. Every hand-written sentence elsewhere is out of scope.

**`--help` diffing in CI.** Same idea without the crate: run the binary, diff
against a committed file. Failure mode: needs a built binary, and the diff is a
wall of text that tells you *something moved* rather than *this name is wrong*.

**Grep-based allowlist linters.** A script extracts identifiers from docs and
checks them against a list derived from the code. Failure mode is the one the
core question names: fire on prose and the check gets disabled. Measured below —
it is avoidable, but only with a deliberate extraction rule.

### 6. The generation alternative, assessed honestly

Would "generate the CLI reference from clap and assert the committed copy
matches" have caught instance 1? **No. Not even close, and for three
independent reasons.**

1. koto has no CLI reference page to generate. `docs/reference/` contains `error-codes.md` and `session-feed.md`. The nearest thing is `docs/guides/cli-usage.md` (1031 lines of hand-written prose) and `plugins/koto-skills/skills/koto-user/references/command-reference.md`. Generation would produce a *new* page; the drifting prose would still be there beside it.
2. Instance 1's primary site is a **Rust string literal**: `src/cli/next_types.rs:179` builds the directive text `"Later ticks must run there or below it -- \`koto session rebind {}\` moves it."`. No amount of generating markdown from clap inspects a string literal in `src/`.
3. The secondary sites are prose *about* the missing command. `docs/guides/default-action-authoring.md:574` and `docs/reference/error-codes.md:107` both name `koto session rebind` — and both explicitly say **"This subcommand has not landed yet"**, listing what `koto session` actually offers. So does `plugins/koto-skills/skills/koto-user/SKILL.md:201`. The docs were *right*. A generated reference would agree with them and change nothing.

Generation is a good idea for a different reason (it removes a whole future
category of hand-maintained drift, and gives the checker a clean artifact to
compare against). It is not a fix for this bug class. **Verdict: partial fit,
complementary, not sufficient.**

### 7. The measurement that decides this: does a docs-vs-clap check drown in prose?

This is the question the core framing says must be answered — "must not fire so
often on prose or placeholders that it gets disabled" — so I measured it in
three passes.

**Pass 1, naive `koto <word>` over all text.** Catastrophic, exactly as feared:
`koto is` (x18), `koto has` (x23), `koto already` (x21), `koto executes` (x13).
Hundreds of hits, essentially all prose. This is the version that gets disabled
in a week.

**Pass 2, restrict extraction to inline code spans and fenced blocks.** The
signal appears immediately.

**Pass 3, add two-level resolution** (a top-level verb that owns subcommands
must be followed by a real subcommand; a verb with no subcommands takes anything
after it as an argument) **and fix two extraction bugs I hit** — backtick
pairing must split on backticks and take odd indices rather than regex-match
`` `…` `` pairs (a naive regex pairs the *closing* tick of one span with the
*opening* tick of the next), and the token before `koto` must be
whitespace/start/`$` rather than a word boundary (a hyphen is a word boundary,
so `\bkoto\b` fires inside `hello-koto`).

Final measured result:

| Scope | Invocations scanned | Unresolved | Noise |
|---|---|---|---|
| **guides + reference + README** | 248 | **3** | **1** (`koto session and …` prose) |
| src (`.rs`) | 208 | **8** | **0** |
| tests | 142 | **2** | **0** |
| plugins (shipped skill) | 418 | **10** | **0** |
| docs/designs | 1006 | 102 | ~100 (all legitimate) |

Every one of the 20 hits outside `docs/designs` is a true positive:

- `src/cli/next_types.rs:179`, `src/cli/mod.rs:3473`, `src/cli/mod.rs:3489` — `koto session rebind` in user-facing messages. **This is instance 1, all three sites, found by a 60-line script.**
- `src/engine/respawn.rs:98` + `:807`, `tests/respawn.rs:597` + `:603` — `koto session info` in the agent resume prompt and its snapshot guard. **A second live instance, previously unreported.**
- `src/engine/types.rs:883`, `:1039`, `src/cli/mod.rs:4857` — `koto query --events` in doc comments; `query` was renamed to `state` per `DESIGN-event-log-format.md:188` and then `state` never shipped either.
- `plugins/koto-skills/.../SKILL.md` ×3 and `command-reference.md` ×4 — `koto session rebind` in the *shipped* Claude Code skill; ×3 more for `koto query`.

`docs/designs` is the one place the check must not run: 102 unresolved hits
across 33 distinct verbs (`koto transition` ×27, `koto query` ×18,
`koto state` ×7, `koto generate` ×6…), and every one is legitimate — a design
doc proposing or recording a verb is exactly what a design doc is for.

### 8. Bash script vs Rust test

| | `scripts/check-doc-verbs.sh` | `tests/doc_verbs.rs` |
|---|---|---|
| Ground truth | Must shell out to a built binary and parse `--help` recursively, or hardcode the list | `App::command()`, structured, recursive, **no binary needed** |
| Build cost in CI | Needs `cargo build` (koto pulls ratatui, rust-s3, crossterm — not cheap) or a separate job | Rides the existing `cargo test` job, +0.00 s |
| Parsing | Scraping `--help` text is brittle; indentation and clap's rendering are not a contract | `get_subcommands()` is a typed API |
| Local run | `bash scripts/check-doc-verbs.sh` — works with no toolchain thought | `cargo test --test doc_verbs` — one command contributors already run |
| Failure message | Whatever you `echo`. Can be excellent — koto's `check-evals-exist.sh` proves the team writes good ones | Same, plus `assert!` gives file:line and it fails inside the suite the author is already running |
| Precedent | `scripts/check-evals-exist.sh`, wired into `.github/workflows/eval-plugins.yml:19` | 42 files in `tests/`, `cargo test -- --test-threads=1` in `validate.yml` |
| Scanning docs | Trivial (`grep`) | Also trivial (`std::fs` walk); the prototype is ~60 lines |

**Recommendation: Rust test.** The deciding factor is the ground truth. The
script's only routes to the verb list are (a) build the binary and scrape help
text, which is both slow and brittle, or (b) hardcode the list, which reintroduces
the exact drift being checked. The test gets the parser tree directly, for free,
in a job that already runs. The one thing the script does better — running
without a toolchain — does not apply, since every koto contributor has cargo.

### 9. The suppression problem, which is not optional

Note the tension in the data: `docs/reference/error-codes.md:107` and
`docs/guides/default-action-authoring.md:574` name `koto session rebind`
*deliberately*, in sentences whose entire purpose is to say the command does
not exist yet. A checker with no escape hatch fires on both, the author's only
move is to delete honest documentation, and the check earns a reputation for
being wrong. It needs a waiver — an HTML comment marker, or (better, since it
self-documents) treating a fenced/backticked mention on a line that also
contains a negation marker like "not implemented" / "has not landed" as
acknowledged. The cheaper and more honest version: an explicit allowlist file
with a required reason column, which doubles as the TODO list of verbs koto has
promised and not delivered.

## Implications

1. **The ground truth is free.** `App::command()` in an integration test with no new dependencies enumerates all 52 verb paths in 0.00 s. Anything that adds a crate to get at koto's CLI shape is solving a problem koto does not have.
2. **The check must scan `src/`, not just `docs/`.** Instance 1's primary site is a string literal, and so is the newly-found `koto session info`. Every docs-only approach in the ecosystem survey — trycmd, generated references, lychee, doctests — is structurally blind to it. This is the single most important design constraint, and it is the one the ecosystem has the least to offer on.
3. **It must scan `plugins/` too.** The shipped Claude Code skill carries 10 unresolved invocations. That surface reaches users at least as directly as the guides.
4. **`docs/designs/**` must be excluded, by name.** 102 legitimate hits vs 3 elsewhere. Include it and the signal-to-noise inverts from 20:1 to 1:5, and the check gets disabled — precisely the failure the core question warns about.
5. **Backticked-only extraction is the whole trick.** Same corpus, same resolver: unscoped prose gives hundreds of false positives, backtick-scoped gives one. The rule is cheap to state ("if you wrote it in code font, it must exist") and easy for a contributor to internalize.
6. **Snapshot tests are not a defense and can be an accomplice.** `tests/respawn.rs` byte-asserts a prompt containing a nonexistent command. Any proposal in the design phase that leans on golden files should be measured against this example.
7. **Two more checks fall out nearly free**, and both find real bugs today: backticked repo-relative path existence (7 dead paths in guides, 35%), and `#[doc = include_str!]` doctests for the 105 ```rust fences.

## Surprises

- **The docs were right and the code was wrong.** I expected instance 1 to be a stale guide. Instead `error-codes.md`, `default-action-authoring.md`, and the shipped `SKILL.md` all explicitly flag `koto session rebind` as unimplemented, in bold. The error message in `src/cli/next_types.rs` is the liar. This inverts the framing: the primary direction of drift here is *code → docs*, and it means a docs-generation strategy is aimed at the wrong end of the pipe.
- **A second live instance, unreported.** `koto session info` in `RESUME_CONTEXT_PROMPT` — shipped to every resuming agent, guarded by a byte-equality snapshot test that makes it *harder* to fix. Plus `koto session list --parent`, a flag that lives on `session start`, in the same sentence.
- **Precision is far better than anyone would guess.** I expected the "does it drown in prose" question to be the hard part. Measured: 248 scanned in the user-facing docs, 3 unresolved, 1 of them noise. On `src/` + `tests/` + `plugins/`: 768 scanned, 20 unresolved, **zero** noise.
- **The guide for authoring skills walks through an example that does not exist.** `plugins/koto-skills/skills/hello-koto/` is named as "the reference implementation" in the first paragraph and five more times. It is not in the tree.
- **Two subtle extraction bugs cost most of the apparent false-positive rate.** Naive backtick regex pairing and `\b` matching inside `hello-koto` produced 2 of the 5 initial "false positives". Both vanished with a two-line fix. Worth writing into the design so the implementer does not rediscover them.

## Open Questions

1. Should the checker also resolve **flags** (`--parent` on `session list`)? clap exposes `get_arguments()`, so it is the same mechanism. It would have caught the second half of the respawn prompt bug. Cost: more surface, likely more noise from illustrative `--flag`-style prose. Unmeasured.
2. What is the waiver mechanism — allowlist file with reasons, inline comment marker, or negation-aware parsing? The allowlist doubles as a promised-verbs TODO, which argues for it, but needs an owner.
3. Does `plugins/koto-skills/**` belong in the same check or a separate one? It ships on a different cadence than the binary, so a verb that lands in koto `main` but not yet in a released plugin is a legitimate mismatch in one direction only.
4. The class also covers **template syntax the compiler rejects** and **build commands**. Template syntax is plausibly checkable by feeding fenced template blocks to `koto template validate` (which exists); build commands are not obviously decidable. Out of scope for this lead — flagging for whoever owns the surface inventory.
5. Should the three stale `koto query --events` doc comments be fixed to `koto state` (which does not exist either) or deleted? The rename is recorded in `DESIGN-event-log-format.md:188` and neither name ever shipped.

## Summary

koto needs no new dependency: a `#[test]` calling `App::command()` enumerates all 52 verb paths in 0.00 s, and a ~60-line docs+source scanner built on it found instance 1 at all three sites plus a previously-unreported second instance (`koto session info` in the agent resume prompt, cemented by a byte-equality snapshot test in `tests/respawn.rs`) and three stale `koto query` references — 20 hits with zero false positives across `src/`, `tests/`, and `plugins/`, and 3 hits with 1 noise across the user-facing docs, provided extraction is limited to backticked/fenced spans and `docs/designs/**` is excluded (102 legitimate hits live there). The ecosystem options are all partial or wrong: trycmd needs 85 ```bash fences rewritten as `console` transcripts with recorded output and still cannot see a string literal, generated CLI references would not have caught instance 1 for three independent reasons (no reference page exists, the primary site is Rust source, and the prose sites already correctly say the command is unimplemented), doctests reach only the 105 ```rust fences, and lychee checks markdown links rather than the backticked paths where 7 of 20 references in koto's guides are already dead. Build it as a Rust test rather than a shell script — the script's only routes to ground truth are scraping `--help` from a slow build or hardcoding the list, which reintroduces the drift being checked.
