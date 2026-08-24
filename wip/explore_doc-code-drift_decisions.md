# Exploration Decisions: doc-code-drift

Execution mode is `auto` (background dispatch, no interactive author). Each entry
follows the research-first protocol: evidence, then the call, then the reason.

## Round 1

- **The checkable claim is "a name that should resolve, resolves"**: two claim
  types make the cut — a `koto <verb> [<subverb>]` token, and a repo-relative
  path. Both are decidable by stat-or-lookup against a set the compiler already
  knows. Rejected: checking bare code identifiers (70% of backticked spans,
  17–23% naive false-positive rate against `src/`, an order of magnitude worse
  than verbs); checking prose accuracy (not decidable); template-snippet
  compilation as the *first* check (reaches 10 of 56 documented template blocks,
  and the fragments are where authors go wrong).

- **`src/**/*.rs` is in scope, and is the primary surface**: the defect that
  prompted this lives in three Rust string literals, and all six documentation
  mentions of it were already truthful. A markdown-only check finds nothing at
  the site of the incident. Requiring a backtick takes `src/` from 42 raw
  unresolved to 10, all genuine.

- **Load-bearing surfaces in, record surfaces out**: `src/`, `plugins/`,
  `docs/guides/`, `docs/reference/`, `docs/testing/`, `docs/STABILITY.md`,
  `docs/workspace-layout.md`, `README.md`, `CLAUDE.md` are checked.
  `docs/designs/`, `docs/prds/`, `docs/briefs/`, and `CHANGELOG.md` are not.
  Measured: 1.24% miss on the former, 8.3% on the latter, 31% in
  `docs/designs/archive/`. A record surface preserving a rejected verb name is
  doing its job; flagging it is the behaviour that gets a check disabled.

- **Ground truth comes from `koto::cli::App::command()`, never from a hardcoded
  list**: the walk is free, typed, recursive, and self-retiring — when
  `koto session rebind` lands under koto#215 the finding disappears on its own.
  Rejected: scraping `koto --help` (needs a release build; help text is
  presentation, not contract) and parsing the `Subcommand` enums with awk
  (re-implements clap's derive semantics; `ValueEnum` variants are a trap).

- **It ships as a Rust integration test, not a `scripts/check-*.sh`**: this
  overrides the house's shell-checker style, and the reason is ground truth — a
  script can only reach the verb set by building the binary or hardcoding it,
  and hardcoding reintroduces the drift being checked. koto's own
  `tests/lib_reexports.rs` is the precedent. The house *conventions* are kept:
  incident-naming header, accumulate-then-report, a fix line per finding.

- **It must run on `src/**` changes**: `validate.yml` (which runs `cargo test`)
  has no `paths:` filter, while every existing content check is gated on
  `plugins/**` or `docs/**`. That topology is the specific reason v0.12.0 shipped
  green, and riding `cargo test` fixes it for free.

- **The escape hatch is a tab-separated allowlist with a mandatory
  `owner/repo#N`, failing in both directions**: adopted from shirabe's
  `check-template-directives.allow`, whose header states the reason a lint needs
  one at all — "a lint that cannot land until the defects it finds are fixed does
  not land." Bidirectional staleness (an entry matching nothing is an error) is
  what stops it rotting into a permanent suppression list. Rejected: negation-aware
  prose parsing ("not implemented yet" is written five different ways in five
  files, and parsing English is the thing we are trying to avoid).

- **koto only, not koto and shirabe**: the clap ground truth lives in koto, four
  of the five instances are reachable there, and shirabe has no koto crate to
  walk. Instance 4 (shirabe's `CLAUDE.md`) and instance 3
  (koto's `CLAUDE.local.md`, generated from `dot-niwa` and untracked here) are
  explicitly out of reach and are recorded as such rather than papered over.

- **Existing gates get audited, not just supplemented**: `validate-plugins.yml`
  globs `*/koto-templates/*.md`, matching two files and compiling one, while the
  four shipped example templates that caused instance 2 sit under
  `references/examples/`. Widening that `find` is the cheapest win in the whole
  investigation and belongs in the same work.

- **Not in this scope**: flag checking (`get_arguments()` — same mechanism,
  unmeasured noise, and verbs alone catch both known incidents); execution smoke
  tests for example templates (instance 2's runtime half is not statically
  decidable); doctests for the 105 ```rust fences (a real adjacent slice, filed
  separately if wanted).
