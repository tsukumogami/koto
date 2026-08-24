---
schema: brief/v1
status: Done
problem: |
  koto prints and documents names that resolve to nothing: a subcommand in an
  error message, a verb in a shipped skill, a path in a guide. Every gate the
  repo runs passes, because none of them asks whether a name koto states is a
  name koto has.
outcome: |
  A contributor who writes an unresolvable name learns it before the change
  merges, from the verification they already do, with the token and the file
  named. A name that is deliberately promised rather than built is recorded
  against an issue instead of blocking the change.
motivating_context: |
  v0.12.0 shipped an error message instructing users to run
  `koto session rebind`, which does not exist. Filed as koto#216 after the
  missing verb itself was filed as koto#215. Five instances of the same class
  surfaced in one working session, and exploration found three more.
---

# BRIEF: Names koto states must be names koto has

## Status

Done

Framing only. The mechanism — where the check runs, how its scope is expressed,
and what shape its escape hatch takes — is the downstream DESIGN's to settle.
Three alternatives were deliberately left open by exploration — whether name
resolution is one check or two, whether the checked surfaces are a written list
or derived from the lifecycle metadata documents already carry, and whether a
recorded promise is keyed on a token or on a token in a file. Each is a real
fork, and each resolves into a recorded decision in the downstream PRD rather
than being pre-answered here.

## Problem Statement

koto states names that resolve to nothing, and nothing notices.

The clearest case shipped in v0.12.0. When a session's execution anchor is not
satisfied, `koto next` refuses and tells the user to run
`koto session rebind <session> --to <dir>`. That subcommand does not exist and
never has. The same binary that prints the instruction answers it with
`error: unrecognized subcommand 'rebind'`. A user whose checkout genuinely moved
is refused and then sent nowhere.

The work that shipped it was verified with `cargo fmt --check`,
`cargo clippy --all-targets`, the full `cargo test` suite, and CI on every issue
of a fifteen-issue plan. All of it passed, and it was right to pass: none of
those gates asserts that a name appearing in an error message is reachable from
the CLI. The check does not exist, so its absence is invisible.

It is a class rather than an incident, and the class is wider than the CLI.
`plugins/koto-skills/skills/koto-user/references/batch-workflows.md` puts
`koto query` inside a bash fence with the comment "full event log"; `koto query`
is not a verb, and that file ships as a Claude Code skill an agent executes.
`src/engine/respawn.rs` defines `RESUME_CONTEXT_PROMPT`, the text handed to
every respawning agent, and it says to read prior state via
`koto session info <id>`; there is no `session info`. Five documents state what
`koto session` offers, three distinct sets appear among them, and four of the
five are wrong — in the same sentences written to explain the first drift bug.
`docs/reference/error-codes.md` is the sharpest of them: a paragraph whose whole
purpose is to explain that `koto session rebind` does not exist gets the
surrounding verb list wrong in the same breath, omitting two of the seven. Seven
of the twenty repo-relative paths cited in user-facing guides do not exist,
including `plugins/koto-skills/skills/hello-koto/`, which the skill-authoring
guide calls "the reference implementation" in its first paragraph and then builds
the whole guide on. The repo's own `CLAUDE.md` draws a directory tree containing
`cmd/koto/` and `src/gate/`, neither of which is there.

Two things make this worse than ordinary staleness. The first is that a name is
load-bearing in a way prose is not: a reader who meets a stale paragraph is
misled, but a reader who runs a stale command gets an error and no route
forward, and an agent that runs one fails a workflow. The second is that the
repo's existing defense is manual and demonstrably insufficient. `CLAUDE.md`
already instructs contributors to assess the packaged skills after any source
change and to look specifically for "removed subcommands". That rule was in
force, and `koto session rebind` shipped anyway — along with a passing test at
`tests/execution_anchor_test.rs` that asserts the error message contains
`"rebind"`, so the suite did not merely miss the phantom verb, it required one.

What makes the problem tractable is that most of it is decidable. Whether prose
describes behavior accurately is not a question a machine can answer. Whether
a token that is written as a command is a command koto has, and whether a token
that is written as a path is a path that exists, are questions with answers the
compiler already holds.

## User Outcome

A contributor who writes a name koto cannot honor finds out before the change
merges, without having to know that this failure mode exists. The signal arrives
from the verification they already do rather than from a new step they must
remember, it names the token and where it is rather than reporting that
something somewhere moved, and it's specific enough to fix without
investigation.

A contributor who deliberately writes a name that is not built yet is not
blocked and is not asked to delete honest documentation. Promising a command
that is coming is a legitimate thing to do — koto's docs already do it in five
places, correctly — and the outcome preserves that, at the cost of recording the
promise somewhere a reader can find it and an issue can retire it.

A reader of koto's guides, references, and shipped skills can act on what they
read. A command shown in code font runs. A path cited in prose can be opened.
Where a name is not yet real, the document says so and a filed issue says when
it will be.

And the failure retires itself. When a promised verb ships, the promise stops
being an exception, and nobody has to remember to go back and remove it. A
hand-maintained list of a machine-knowable set is the defect one level up, and
four of koto's five such lists are already wrong.

## User Journeys

### A contributor renames a subcommand

A maintainer renames `koto session recover`. The rename compiles, clippy is
clean, and the existing tests pass, because they exercise behavior through the
new name. Before the change merges, the contributor learns that four documents
and one error string still name the old verb, each one named with where it is.
They fix the five sites in the same change. Nobody had to know in advance which
documents mentioned the verb.

### A contributor writes an error message pointing at a repair that does not exist

An engineer implementing an anchor check writes a refusal that tells the user how
to recover, naming the repair command the design specifies. The repair command is
a later issue in the same plan and has not been built. Before the change merges,
the check names the string literal and the verb it contains. The engineer's
options are honest ones: build the verb now, point the message at a repair that
exists today, or record the promise against the issue that will deliver it. What
is no longer available is shipping the instruction and finding out from a user.

### An agent follows a shipped skill

An agent running a koto-backed workflow reads
`plugins/koto-skills/skills/koto-user/`, which tells it to run a command for the
full event log. The command exists, because a change that introduced a verb into
a skill file without introducing it into the CLI would not have merged. The agent
does not burn a turn on `error: unrecognized subcommand` and does not have to
work out whether the skill or the binary is wrong.

### A maintainer deliberately documents something not yet built

A maintainer writing the stability policy commits koto to publishing a migration
tool, naming it in the commitment. The tool does not exist yet; naming it is the
point of the sentence. The check flags it once, and the maintainer records it as
a known promise tied to the issue that will deliver it, in a form the next reader
can see. When the tool ships, the promise is retired by the change that delivered
it rather than surviving as permanent suppression.

### A maintainer moves a file the guides cite

Someone reorganizing the packaged skills deletes the example skill the authoring
guide was built around. Nothing in the move touches the guide, and nothing about
the move suggests it should — the connection runs through a citation in prose,
which is exactly the kind of link a refactor does not surface. Before the move
merges, they learn that four citations in that guide now point at nothing.
They update them, or they take the guide's example with them. What does not
happen is a guide shipping a pointer to a directory the same person deleted an
hour earlier.

## Scope Boundary

### In scope

- **Names that are stated as commands.** A `koto <verb> [<subverb>]` token
  written in code font, wherever koto writes it — Rust string literals and doc
  comments that reach a user, the packaged skills under `plugins/`, guides and
  reference documents, the README, `CLAUDE.md`.
- **Names that are stated as paths.** A repo-relative path written in code font,
  resolved against the tree.
- **Both directions of drift.** The code stating a name the docs correctly say
  is missing is the case that shipped; the docs stating a name the code dropped
  is the case people expect. Neither is privileged.
- **An escape hatch for deliberate promises**, carrying enough to identify the
  promise, and constructed so it cannot quietly become permanent.
- **Fixing what the check finds**, or recording it, so the check lands green
  rather than as a known-failing gate.
- **Names stated from source, not only from documents.** The instance that
  shipped was a string literal in `src/`, which koto's existing content checks do
  not look at.

### Out of scope

- **Whether prose is accurate.** Not decidable, and not attempted. Every instance
  in the corpus is a naming failure; that is the boundary being drawn, and a
  document that names only real things can still describe them wrongly.
- **Code identifiers, types, and field names.** Seventy percent of backticked
  spans and the worst false-positive rate measured — example state names,
  illustrative environment variables, and proposed types are legitimately absent
  from the source. Checking them is how a check earns a reputation for being
  wrong and gets disabled.
- **Design documents, PRDs, briefs, and the changelog.** They record what was
  true or proposed when written, and preserving a rejected or superseded verb
  name is what they are for. This document is one of them.
- **Flags.** Decidable by the same means and deliberately deferred: the noise is
  unmeasured, and no known instance turns on a flag alone.
- **Whether a documented template compiles, and whether it behaves.** A separate
  mechanism from name resolution, with its own existing runner — which today
  reaches one of the eleven template-shaped files in the repo. Widening it is
  worth doing and is separate work; the second half, whether a template that
  compiles then behaves, is not statically decidable at all.
- **Files koto's CI cannot see.** `CLAUDE.local.md` is generated into the working
  copy from another repository and is not tracked here. The same rot in the
  tracked `CLAUDE.md` is in scope; the generated file is out of reach and is
  recorded as such rather than papered over.
- **Other repositories.** shirabe carries an instance of this class, and fixing
  it there is not this feature. The command surface being resolved against lives
  in koto.
- **`koto session rebind` itself.** The missing verb is koto#215 and a separate
  change owns it. This feature must neither depend on that landing nor break when
  it does.

## References

- `docs/designs/current/DESIGN-koto-runs-commands.md` — where
  `koto session rebind` was specified.
- `plugins/koto-skills/skills/koto-user/references/command-reference.md` — the
  one document that records the phantom verb by hand, under a heading reading
  `## koto session rebind — not implemented`, and the only one of the five
  session-verb enumerations that is currently right.
- `CLAUDE.md`, "koto-skills Plugin Maintenance" — the manual rule this feature
  exists because it was insufficient.
