# Exploration Findings: doc-code-drift

## Core Question

Nothing asserts that what koto's documentation names actually exists in the
code. What check should catch it, and where should it live?

## Round 1

### Key Insights

**The docs were right and the binary was wrong** (v0120-forensics, prior-art,
surface-inventory — three agents independently). Every one of the six
load-bearing mentions of `koto session rebind` at v0.12.0 explicitly tells the
reader the subcommand does not exist; `command-reference.md:696` even carries a
section heading `## koto session rebind — not implemented`. The three Rust
string literals at `src/cli/mod.rs:3473`, `src/cli/mod.rs:3489`, and
`src/cli/next_types.rs:179` are the liars. This inverts the dispatch's framing:
a check that reads only markdown would have found nothing at the site of the
defect. **`src/**/*.rs` must be in scope on day one.**

**A passing test required the defect.** `tests/execution_anchor_test.rs:400-404`
asserts `message.contains("rebind")` with the rationale "the refusal must point
at the rebind verb". The suite did not miss the phantom verb; it enforced it.
Whatever gets built has to be able to fire while that test is green.

**The ground truth is free.** koto's whole CLI lives in the library target —
`src/main.rs` is 10 lines, `src/lib.rs` re-exports `pub mod cli`, and
`koto::cli::App` derives `clap::Parser`. So `App::command()` yields the complete
52-path verb tree from an ordinary integration test: no binary build, no `--help`
scraping, no new dependency. Two agents wrote the probe and ran it; the walk
takes 0.00s (decidable-claims, prior-art).

**Precision is a scoping decision, not an extraction-cleverness decision.** The
same extractor yields wildly different noise depending only on which
directories it reads (surface-inventory, measured over the whole corpus):

| Surface class | `koto ...` candidates | Unresolved | Rate |
|---|---:|---:|---:|
| Load-bearing (guides, reference, plugins, README, testing, docs root) | 726 | 9 | **1.24%** |
| Record (designs, PRDs, briefs) | 1,438 | 120 | 8.3% |
| `docs/designs/archive/` alone | 87 | 27 | 31.0% |

The archive's 31% is the archive doing its job — it preserves a CLI design
(`koto transition`, `koto template lint`, `koto cache clear`) that deliberately
no longer exists. Reading record surfaces is broken by design.

**The feared failure mode is not the real one.** A 40-span random sample split
17.5% CLI invocation / 7.5% repo path / 70% code identifier / 5% placeholder /
**0% prose-in-backticks**. The danger is not prose; this codebase uses backticks
for code and the discipline holds. The danger is category (d): checking bare
identifiers against `src/` carries a 17–23% naive false-positive rate, ten times
worse than CLI verbs. **Do not check identifiers.**

**Requiring a backtick collapses the `src/` noise entirely.** A raw `koto <word>`
scan of `src/**/*.rs` gives 274 mentions, 42 unresolved — but 32 of those 42 are
`koto writes` / `koto builds` / `koto renders` prose in `//` comments, all
outside backticks. Requiring the token to sit inside a backtick or a bash fence
takes 42 down to 10, and **all 10 are genuine drift**.

**The check finds live bugs nobody had reported.** Beyond instance 1's three
sites: `koto query` in a shipped ```bash fence at
`plugins/koto-skills/skills/koto-user/references/batch-workflows.md:330`, and
`koto session info` in `RESUME_CONTEXT_PROMPT` (`src/engine/respawn.rs:98`) —
a const handed to every respawning agent, naming a verb that does not exist,
cemented by a byte-equality snapshot test at `tests/respawn.rs:597` that makes
it *harder* to fix.

**Path existence is the highest yield per line of code.** 142 distinct cited
repo-relative paths across the corpus, 23 dead (16%). On load-bearing surfaces
alone: 20 cited, 7 dead (35%). Among them
`docs/guides/custom-skill-authoring.md:5`, whose first paragraph calls
`plugins/koto-skills/skills/hello-koto/` "the reference implementation" and
names it five more times — the directory does not exist. A first-segment
allowlist (`src|tests|docs|plugins|scripts|benches|test`) kills the glob and
cross-repo ambiguity for free: of ~97 `*`-containing backtick spans only 2 are
actually paths, the rest being namespace notation like `gates.*` (69 uses).

**The gate topology is what let v0.12.0 ship green** (decidable-claims,
sibling-checkers). Every koto workflow that runs a content check is gated on
`paths: plugins/**` or `paths: docs/**`. The rebind message was a `src/cli/`
change. `validate.yml` — which runs `cargo test` — has no paths filter. A check
that runs only on doc changes reproduces the hole exactly.

**Existing gates are aimed at the wrong files.** `validate-plugins.yml`'s
template-compilation job globs
`plugins/koto-skills/skills/*/koto-templates/*.md`, which matches two files, one
of which the `*.mermaid.md` guard skips — so a job named "compile all templates"
compiles exactly one. The four shipped example templates that caused instance 2
live two directories away under `references/examples/`. Widening that `find` is a
one-line change (v0120-forensics).

### Tensions

**Bash script vs. Rust test.** sibling-checkers argues for a `scripts/check-*.sh`
sibling: it is the house style, koto and shirabe have sixteen such checkers, and
shirabe's `check-template-interpolation.sh:30-33` writes the argument down —
a check that is "a statement about this repository's own file layout" should not
require a Rust change and a release for every adjustment. prior-art and
decidable-claims argue for `tests/`: the script's only routes to ground truth
are shelling out to a built binary (koto pulls ratatui, rust-s3, crossterm — not
a cheap build) or hardcoding the verb list, which reintroduces the exact drift
being checked.

Resolved for the test, and the deciding factor is the ground truth rather than
style. koto already has the precedent — `tests/lib_reexports.rs` is an in-tree
compile-check asserting a public surface has not moved, and its header says so.
The house *conventions* carry over regardless of language: a prose header naming
the incident, accumulate-then-report rather than fail-fast, a suggested fix on
every finding, and a both-directions allowlist.

**Does the check cover documentation at all, given the docs were right?**
v0120-forensics recommends scoping to Rust literals only, on the grounds that the
prose sites were already truthful and would produce noise. surface-inventory and
prior-art measured the doc surfaces and found 1.24% and 3-hits-in-248
respectively, with the two hits outside the rebind cluster being genuine bugs
(`koto query` shipped in a plugin fence, `koto migrate` a deliberate forward
reference). Resolved for including load-bearing docs: the measured rate is low,
the shipped plugin is the surface agents actually execute, and excluding it would
leave `koto query` live.

**Are `docs/designs/current/` in scope?** They are labelled `Current`, which
means "not superseded", not "matches HEAD" —
`DESIGN-koto-runs-commands.md:691`, a `Current` doc, is where `koto session
rebind` was specified. Including them means 67 more findings, essentially all
legitimate. Resolved for excluding: a design is a record of a decision, and the
same doc that specified the verb also records why. The exclusion is drawn on the
existing `status:` frontmatter and directory layout, not a hand-maintained list.

### Gaps

- Flags (`--parent` on `session list`) are equally decidable via
  `get_arguments()` and would have caught the second half of the respawn-prompt
  bug, but the noise is unmeasured. Verbs alone catch both known incidents.
- The runtime half of instance 2 — a template that compiles and then
  cycle-detects on the second poll — is not statically decidable. Compile
  coverage is cheap; execution coverage is a different order of investment.
- Instance 3 (`CLAUDE.local.md` as a Go repo) is untracked in koto and generated
  by niwa from `dot-niwa`. **No check in koto's CI can ever see that file.** The
  same rot in the tracked `CLAUDE.md` (`cmd/koto/`, `src/gate/`) is reachable and
  in scope; the generated file is not.

### Decisions taken

See `wip/explore_doc-code-drift_decisions.md`.

## Decision: Crystallize
