# Exploration Scope: doc-code-drift

## Visibility

Public

## Scope

Tactical

## Execution Mode

auto (background dispatch, no interactive author available; research-first
decisions recorded in `wip/explore_doc-code-drift_decisions.md`)

## Topic

Nothing asserts that what koto's documentation names actually exists in the
code. A released binary told users to run `koto session rebind`, a subcommand
that was designed, documented, and never built. Every gate the shipping work
ran -- `cargo fmt --check`, `cargo clippy --all-targets`, the full `cargo test`
suite, CI on all fifteen issues of the plan -- stayed green, because none of
them asserts that a name appearing in an error message or a guide resolves to
anything.

## Evidence: five instances of the class

1. `koto session rebind` -- named in a live error message and in
   `docs/guides/default-action-authoring.md`, specified in a Current design,
   absent from the CLI. Filed as koto#215.
2. koto-author's `references/batch-authoring.md` recommended a batch-routing
   pattern the compiler rejects; the shipped example alongside it did not
   compile. koto#207, fixed in #213.
3. koto's `CLAUDE.local.md` describes a Go repository -- `go build`,
   `go test ./...`, `pkg/`, `internal/`, a `koto transition` subcommand -- for a
   repo that is Rust and has none of those.
4. `shirabe/CLAUDE.md` documents a repo-root `koto-templates/` directory; the
   templates live under `skills/*/koto-templates/`.
5. A PRD cited `src/gate/mod.rs` and `docs/template-format.md`; the live paths
   are `src/gate.rs` and a path under `plugins/koto-skills/`.

The spread is the point: CLI verbs, template syntax the compiler rejects, build
commands, source paths, directory layout.

## Questions exploration must settle

- Which documentation surfaces are in scope, and how load-bearing is each?
- Which claims are mechanically decidable, and would the decidable subset have
  caught the real instances?
- What is the false-positive budget, and what escape hatch keeps a check alive
  past its first week?
- Where does it run: CI job, `cargo test`, `scripts/`, or a lint?
- One repo or both (koto and shirabe)?
- Is a cheaper framing -- a better manual gate -- the honest answer?

## Acceptance test

The check must catch instance 1 on the tree as it stood at `v0.12.0`. Verified
explicitly, not assumed. The concurrent session implementing `koto session
rebind` may land the verb first, so the proof runs against the tag.

## Round 1 leads

- **L1 sibling-checkers** -- read koto's and shirabe's existing drift checkers
  and the CI that runs them. Is the right answer a sibling, or is one already
  extensible?
- **L2 surface-inventory** -- enumerate koto's documentation surfaces, measure
  how much each one asserts about code, and estimate the raw hit rate of a naive
  extractor on each.
- **L3 decidable-claims** -- what does koto expose that makes a claim decidable
  (CLI introspection, `koto template compile`, path existence), and what does
  the CLI actually offer for enumerating its own verbs?
- **L4 v0.12.0-forensics** -- reconstruct instance 1 at the tag: exactly what
  strings named `session rebind`, in which files, and what the CLI's verb set
  was. This is the acceptance fixture.
- **L5 prior-art** -- how do comparable Rust projects and the wider ecosystem
  check doc/code agreement (doc tests, `trycmd`, link checkers, `--help`
  snapshotting), and what does koto already have installed that could carry it?
