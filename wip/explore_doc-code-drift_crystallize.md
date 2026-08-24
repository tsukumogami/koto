# Crystallize: doc-code-drift

## Stage 1 — What the exploration is

**Result: a chain.**

Competitive analysis is not a candidate: `## Visibility` in the scope file reads
`Public`. `/execute` is not a candidate at stage 2: no `docs/plans/PLAN-*.md`
exists for this topic.

| Category | Score | Reasoning |
|---|---|---|
| **A chain** | **strong** | The exploration converged on something someone will build, a scope boundary emerged (load-bearing vs record surfaces; verbs and paths but not identifiers), and architectural decisions were made during exploration that need a durable home. |
| Spike report | weak | The core question was never "can we do this?" Feasibility was settled inside the first round — two agents built working prototypes — and the open questions are all scoping, not risk. |
| Decision record | weak | There is not one decision here. There are eight, and they come with work attached: an extractor, a resolver, an allowlist format, a CI placement, and an audit of an existing gate's glob. |
| Rejection record | absent | The exploration reached a positive conclusion, not a rejection. The dispatch explicitly licensed "no check is worth it" as an outcome; the measured 1.24% miss rate on load-bearing surfaces defeats it. |
| Competitive analysis | not a candidate | Public repo. |

## Stage 2 — Where the chain starts

**Result: `/scope`.**

| Entry point | Score | Reasoning |
|---|---|---|
| **`/scope`** | **strong** | A single coherent feature — one check with one ground truth and one escape hatch. What to build is now clear; how to draw its boundaries needed evidence and got it. Technical decisions were made that should be on record (test-not-script, clap-not-help-scraping, both-directions allowlist), and the terminal artifact is a PLAN whose issues have a real dependency order. |
| File an issue | moderate | Tempting, and the work is one PR. But the exploration made architectural choices a future contributor will need the reasoning for, and `wip/` is deleted at merge. Filing alone would lose the measurements — the 1.24%-vs-8.3% split is the entire justification for the scope boundary, and without it the first person to see a false positive widens the scope and breaks the check. |
| `/charter` | weak | The project exists; this is one bounded feature within it. No sequencing question across features. |
| `/execute` | not a candidate | No qualifying PLAN. |

## What the chain receives

**The thing to build.** A phantom-reference check that resolves two kinds of
name against ground truth the compiler already holds:

1. **Verbs.** Every `koto <verb> [<subverb>]` token appearing inside a backtick
   span or a ```bash/```sh fence, resolved against the live clap command tree
   walked from `koto::cli::App::command()`.
2. **Paths.** Every backticked repo-relative path whose first segment is a real
   top-level directory, resolved with a filesystem stat, with a trailing
   `:line` or `:start-end` suffix stripped first.

**Where it runs.** A Rust integration test under `tests/`, riding the existing
`cargo test` job in `validate.yml` — the one workflow with no `paths:` filter,
which is what puts a `src/cli/` change in scope.

**Scope of the scan.** In: `src/**/*.rs`, `plugins/koto-skills/**`,
`docs/guides/`, `docs/reference/`, `docs/testing/`, `docs/STABILITY.md`,
`docs/workspace-layout.md`, `README.md`, `CLAUDE.md`. Out: `docs/designs/`,
`docs/prds/`, `docs/briefs/`, `CHANGELOG.md`.

**Escape hatch.** A tab-separated allowlist with a mandatory `owner/repo#N`
issue reference, which fails on stale entries as well as on new findings.

**Known findings the work must land against**, all confirmed on the current
tree:

| Finding | Sites | Disposition |
|---|---|---|
| `koto session rebind` | `src/cli/mod.rs:3473`, `:3489`, `src/cli/next_types.rs:179`, plus 6 doc sites | Allowlist against koto#215 — the concurrent session owns the verb, and the entry self-retires when it lands |
| `koto session info`, `koto session list --parent` | `src/engine/respawn.rs:98`, `:807` | New. Fix in place or allowlist with a filed issue |
| `koto query` | `plugins/.../batch-workflows.md:330`, `command-reference.md:169`, plus `///` comments in `src/engine/types.rs` | New. Shipped in the plugin |
| `koto migrate` | `docs/STABILITY.md:93` | Deliberate future-tense policy; allowlist |
| Dead paths | 7 in load-bearing surfaces, incl. `plugins/koto-skills/skills/hello-koto/` named as "the reference implementation" | Fix the docs |
| `cmd/koto/`, `src/gate/` | `CLAUDE.md:11`, `:15` | Fix the file |

**The acceptance bar, restated so `/scope` cannot lose it.** The check must be
demonstrated to fire on the three `koto session rebind` string literals on the
tree as it stood at `v0.12.0`. `src/cli/mod.rs` is byte-identical between
`v0.12.0` and `origin/main`, so this is provable on HEAD and against the tag with
the same expected result.

**Two things deliberately not in the chain**, recorded so they are refusals
rather than omissions: instance 3's `CLAUDE.local.md` is untracked in koto and
generated from `dot-niwa`, so no check in koto's CI can see it; instance 4 is in
shirabe, which has no koto crate to walk. The check reaches instances 1, 2, 3b
(the tracked `CLAUDE.md`), and 5.

## Auto-mode note

No author was available to confirm. The recommendation follows the evidence and
the dispatch brief, which already names `/scope` then `/execute` as the chain.
