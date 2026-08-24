---
schema: prd/v1
status: Done
problem: |
  koto states names that resolve to nothing -- a subcommand in an error message,
  a verb in a shipped skill, a path in a guide -- and every gate the repository
  runs passes anyway, because none of them asks whether a name koto states is a
  name koto has. Contributors and agents are the affected parties: a contributor
  cannot discover the failure without knowing to look for it, and an agent
  following a shipped skill runs a command that does not exist.
goals: |
  A change that introduces an unresolvable command or path name is refused
  before it merges, by a check that resolves both against ground truth taken
  from the code rather than from a maintained list. Its false-positive rate is
  low enough that nobody disables it, and a name that is deliberately written
  without being built is recorded in the open instead of blocking the change or
  being suppressed forever.
upstream: docs/briefs/BRIEF-doc-code-drift.md
source_issue: 216
motivating_context: |
  v0.12.0 shipped an error message telling users to run `koto session rebind`,
  which does not exist. The verb is koto#215; this is koto#216, the question of
  why nothing caught it. Exploration found the same class in eight further live
  places across CLI verbs, shipped skills, guides, and the repository's own
  CLAUDE.md.
---

# PRD: Name resolution for koto's documentation and error strings

## Status

Done

## Problem Statement

koto states names that resolve to nothing, and nothing notices.

When a session's execution anchor is not satisfied, `koto next` refuses and
tells the user to run `koto session rebind <session> --to <dir>`. That
subcommand does not exist. The same binary that prints the instruction answers
it with `error: unrecognized subcommand 'rebind'`, so a user whose checkout
moved is refused and then sent nowhere. It shipped in v0.12.0 behind
`cargo fmt --check`, `cargo clippy --all-targets`, the full `cargo test` suite,
and CI on every issue of a fifteen-issue plan — all of which passed, correctly,
because none of them asserts that a name in an error message is reachable from
the CLI.

Two populations are affected. A contributor renaming or removing a verb has no
way to learn which documents and error strings still name the old one short of
grepping for it, which requires already suspecting the problem. An agent is
affected more directly: the packaged skills under `plugins/koto-skills/` are
executed rather than read, so
`plugins/koto-skills/skills/koto-user/references/batch-workflows.md` telling an
agent to run `koto query` costs a turn and a failed workflow.

It is a class rather than an incident, and every instance is live on the current
tree. Three Rust string literals name `koto session rebind`.
`src/engine/respawn.rs` hands every respawning agent a prompt naming
`koto session info`, which does not exist, and the prompt is pinned by a
byte-equality snapshot test. Five documents state what `koto session` offers,
three distinct sets appear among them, and four of the five are wrong — including
`docs/reference/error-codes.md`, whose paragraph explaining that
`koto session rebind` does not exist gets the surrounding verb list wrong in the
same breath. Seven of the twenty repo-relative paths cited in user-facing guides
do not resolve, including the example skill directory that
`docs/guides/custom-skill-authoring.md` builds its entire walkthrough on. The
repository's own `CLAUDE.md` draws a tree containing `cmd/koto/` and `gate/`,
and names `src/gate/` in a table a few dozen lines later; none of the three
exists.

Not all of those are reachable by the same rule, and the requirements below say
which are and which are not. Two of the instances above motivate the work without
being covered by it. A name *omitted* is not checkable: the four wrong
`koto session` verb lists are wrong by leaving `recover` out, and every verb they
do name resolves. And a path whose leading directory was renamed away is not
checkable either, which is why `cmd/koto/` is repaired by hand rather than caught
— the rule that would reach it is the rule that reports every child of every
directory diagram in the repository. Known Limitations carries both.

Why now: the manual defense already exists and has been shown insufficient.
`CLAUDE.md` instructs contributors to assess the packaged skills after any
source change and to look specifically for "removed subcommands". That rule was
in force when `koto session rebind` shipped. Worse, a passing test at
`tests/execution_anchor_test.rs` asserts the refusal message contains
`"rebind"` — the suite did not merely miss the phantom verb, it required one. A
second manual pass over the same surfaces is not a proportionate answer to a
failure that a manual pass already had.

## Goals

- A change introducing an unresolvable command or path name is refused before it
  merges, without the author having to know this failure mode exists.
- The refusal names the token and where it is, precisely enough to act on
  without investigation.
- Ground truth for what commands exist is taken from the code, so the check
  retires its own findings when a promised verb ships and cannot itself drift.
- The check runs when source changes, not only when documents change.
- Writing down a name that is deliberately not built stays possible, at the cost
  of recording it where a reader can see it.
- No recorded exception can survive the condition that justified it.
- The check lands green: what it finds today is either fixed or recorded.

## User Stories

**As a maintainer renaming a subcommand**, I want to be told which documents and
error strings still name the old verb, so that I fix them in the same change
rather than shipping a repository that contradicts itself.

**As an engineer implementing a refusal path**, I want to be stopped when the
repair command I point users at has not been built, so that the choice between
building it, pointing elsewhere, and recording the promise is one I make
deliberately rather than one a user discovers.

**As an agent executing a packaged koto skill**, I need every command the skill
tells me to run to exist, so that I do not spend a turn on
`error: unrecognized subcommand` and then have to work out whether the skill or
the binary is wrong.

**As a maintainer writing policy that commits koto to a tool it has not built**,
I want to record the commitment rather than delete it or suppress the warning
forever, so that the promise stays visible and shipping the tool retires it.

**As a maintainer reorganizing files**, I want to learn that guides cite paths I
just moved, so that a refactor that never touched a guide does not silently
break it.

**As a contributor who has just been failed by this check**, I want its output to
tell me what to do, so that my first instinct is to fix the name rather than to
disable the check.

## Requirements

### Functional

**R1 — Command-name resolution.** The check SHALL extract every candidate
`koto <verb> [<subverb>]` invocation from the checked surfaces and resolve it
against koto's command surface. An invocation that does not resolve SHALL be
reported as a finding.

**R2 — Ground truth from the code.** The set of valid commands SHALL be derived
from koto's own command definition. No enumeration of koto's command surface
SHALL be maintained by hand, in the check or in anything the check reads other
than R7 exception records. Adding or removing a subcommand in source SHALL
change what the check accepts, with no accompanying edit to the check.

**R3 — Path resolution.** The check SHALL extract every candidate
repo-relative path from the checked surfaces and resolve it against the tree
being checked. A path that does not resolve SHALL be reported as a finding. A
trailing `:<line>` or `:<start>-<end>` suffix SHALL be stripped before
resolution.

**R3a — Path candidacy is anchored on an existing top-level entry.** A token
SHALL be treated as a repo-relative path only when its leading segment names an
entry that exists at the root of the tree being checked. This is the check's
discrimination rule for paths, and it is load-bearing: it is what separates
`plugins/koto-skills/skills/hello-koto/SKILL.md`, cited in
`docs/guides/custom-skill-authoring.md` and absent from the tree, from
`feature/anchor`, `CI/CD`, `path/to/your-template.md`, `~/.koto/sessions`, and
the children of rendered directory trees. Measured against this corpus, the
anchor takes several hundred path candidates down to 33 findings — 21 genuine
against 12 correct as written. Four of the 21 are dead design-doc citations
inside `src/` itself that nobody had noticed, at
`src/engine/batch_validation.rs:8`, `src/workflows_surface/contract.rs:8`,
`src/cli/batch_view.rs:10`, and `src/cli/task_spawn_error.rs:43`, three of them
in `//!` comments: each writes a design-document path rooted at `docs/designs/`
where the live document is under `docs/designs/current/`.

**R3a-i — The anchor's own false positives.** koto's root carries the directory
names every project carries, so a guide instructing the reader about *their*
repository anchors against koto's tree and fires. `.claude/` and `target/` SHALL
be excluded as leading segments, and a token carrying a placeholder
metavariable — `<name>`, `{{VAR}}`, `*` — SHALL NOT be a path candidate. Twelve
verified instances motivate this, the clearest being
`docs/guides/template-freshness-ci.md`, which names
`.github/workflows/check-templates.yml` in a sentence that says "in your repo".

`.github/` is deliberately *not* excluded, despite being the clearest instance.
Measured, excluding it silences one false positive and blinds two live internal
citations of koto's own workflow files — a net loss. The one false positive is
carried as an intentional record instead, which is what that category is for.

Applying R3a-i takes the 33 findings above to 24: all 21 genuine survive, and
nine of the twelve correct-as-written are removed. The three that remain,
`.github/` among them, are recorded under R7.

**R3b — The false negative R3a accepts.** A path whose leading segment no
longer exists — a wholesale directory rename or deletion — SHALL NOT be reported.
`cmd/koto/` at `CLAUDE.md:11` and the bare `gate/` at `CLAUDE.md:15` are live
instances the check therefore does not reach. The second is a child of a rendered
directory tree, where indentation carries a parent a flat scan cannot see; the
anchor happens to exclude every such child on this tree because koto's diagrams
use names that are not root entries, and that is a coincidence rather than a
rule. No structural test for tree diagrams is required — the anchor is the whole
path rule. (The
table row at `CLAUDE.md:72` citing `src/gate/` is *not* one of them: its leading
segment exists, so R3a admits it and the check reports it as an ordinary
finding.) The false negative is accepted deliberately: reaching it means treating
every slash-bearing token as a path, which measured against this corpus gives 93
findings, of which 9 are cross-repo tokens excluded on other grounds, leaving 84
adjudicable — 56 correct as written against 28 genuine. The anchor with R3a-i
gives 21 genuine against 3. The landing change SHALL
repair `cmd/koto/` and `gate/` by hand, since no check will.

**R4 — Code-font restriction.** A candidate SHALL be extracted only from a
backticked span or from inside a fenced code block. Prose outside code font
SHALL NOT produce candidates. Measured, this rule alone takes `src/`'s raw
unresolved command count from 42 to 10, and all 10 that survive are genuine.

**R4a — Fences are in scope whether or not they carry a language tag.** The
guide instance of the motivating defect —
`docs/guides/default-action-authoring.md`, which shows
`koto session rebind <session> --to <dir>` — is in an untagged fence. A rule
keyed on the tag would miss it, and would make detection depend on whether an
author happened to write one.

**R5 — Checked surfaces.** The check SHALL cover exactly these, and this list is
the definition of what is in scope rather than a sample of it: `src/**/*.rs`,
`plugins/koto-skills/**`, `docs/guides/**`, `docs/reference/**`,
`docs/testing/**`, `docs/STABILITY.md`, `docs/workspace-layout.md`, `README.md`,
and `CLAUDE.md`. Anything not named here is out of scope, including
`docs/designs/`, `docs/prds/`, `docs/briefs/`, `CHANGELOG.md`, `tests/`,
`test/`, `benches/`, `scripts/`, and `.github/`. The record surfaces are
excluded because preserving a rejected or superseded name is their purpose; they
carry 120 of the corpus's 129 unresolved command names.

**R6 — Rust string literals and comments are in scope.** Within `src/`, string
literals, `///` item doc comments, and `//!` module doc comments SHALL be
checked. Plain `//` comments SHALL NOT be. The instance that shipped is a string
literal; doc comments reach users through generated help text; plain comments
reach nobody outside the file.

**R6a — Where a finding is reported SHALL NOT depend on where a formatter
wrapped a literal.** A backticked command split across Rust line continuations
SHALL be found. In the motivating instance the phrase survives on one physical
line only because of where the author broke the string.

**R7 — Recorded exceptions.** The check SHALL support recording a candidate as a
known exception, in one of exactly two categories:

- **Promised.** A name that is intended to exist and does not yet. The record
  SHALL carry an issue reference in `owner/repo#N` form. A promised record
  without one SHALL be an error.
- **Intentional.** A name that is correct as written and is not going to be
  built to match — a forward-looking policy commitment, or a name written
  precisely to document its own absence. No issue reference is required.

Every record in either category SHALL carry a human-readable reason. A record
with no reason SHALL be an error.

**R7a — Category precedence.** The categories overlap: a name can be both
intended to exist and written to document its own absence, which is true of the
four `koto session rebind` sites whose sentences explain that the verb is not
implemented. A name that any filed issue intends to
create SHALL be recorded as promised, whatever its sentence says about it.
Without this rule the same finding can be recorded two ways with different
rigor, at the author's discretion.

**R8 — A promised exception expires when the name becomes real.** A promised
record that no longer matches any finding SHALL itself be reported as a finding.
The change that makes the name real is the change that removes the record.

**R8a — An intentional exception expires when the text it protects changes.** An
intentional record SHALL be bound to the passage that justifies it, such that
editing that passage retires the record and requires it to be re-made
deliberately. R8's condition — the name became real — is by construction the one
that will never occur for an intentional record, so it cannot be that category's
expiry. Without a second condition, a genuine defect miscategorised as
intentional, with a plausible reason nothing verifies, is suppressed forever and
invisibly. The binding SHOULD err toward over-firing: a spurious retirement costs
a re-affirmation, while a missed one costs a permanent suppression.

**R9 — Findings name their location and their fix.** Each finding SHALL name
the offending token and the file and line it appears at, and SHALL state both
how to fix it and how to record it as an exception under R7. A finding reporting
a stale exception SHALL surface that record's reason text, and SHALL distinguish
the two ways a record goes stale, because the correct response differs:

- **R8, a promised record whose name became real.** The record is obsolete. The
  message SHALL say so and SHALL carry the reason text, so a contributor who has
  never read this document learns what else the change needs to touch.
- **R8a, an intentional record whose passage was edited.** The record may still
  be right; the binding is what lapsed. The message SHALL say that re-affirming
  the record is the expected action, and SHALL NOT imply prose needs correcting.

The second case will be common — R8a tells the DESIGN to err toward over-firing —
and it surfaces as CI failing on a documentation edit that had nothing to do with
the recorded name. That is the cost of the over-firing preference and is
deliberate; a message that misdescribes it would make the cost look like a bug.

**R10 — Accumulate, then report.** The check SHALL report every finding in one
run rather than stopping at the first, so that a contributor sees the whole
repair in one pass.

**R11 — Exit status.** The check SHALL exit zero when there are no findings and
non-zero when there are, so that CI routes on it without parsing output.

**R12 — The check lands green.** Every finding the check produces against the
tree at the time it lands SHALL be either fixed or recorded under R7. The check
SHALL NOT land as a known-failing gate.

**R13 — Independence from koto#215.** The check SHALL NOT require
`koto session rebind` to exist, and SHALL NOT require it to be absent. When the
verb lands, the findings naming it stop being findings and their promised
records go stale under R8. The promised records' reason text SHALL name the four
prose passages that state the verb does not exist, so that the stale-record
finding koto#215 sees under R9 also names the prose it must correct.

Why the requirement takes that shape is recorded in Decisions and Trade-offs:
this PRD has no mechanism to bind koto#215's change, so the only lever is the
finding that change will see.

**R14 — Arbitrary root.** The check SHALL accept an optional path naming the
tree to check, defaulting to the repository it is run from. Ground truth under R2
SHALL be derived from the tree named by that argument, not from the tree the
check was built from — otherwise the add-a-verb and remove-a-verb criteria cannot
be built, and the v0.12.0 run would resolve v0.12.0's documents against today's
command surface. Without this requirement the fixtures under R20 cannot be built
and the v0.12.0 demonstration cannot be run.

### Non-functional

**R15 — Triggering.** The check SHALL run on changes to `src/` as well as to
documentation. A trigger scoped to documentation paths reproduces the gap that
let v0.12.0 ship.

**R16 — The pre-exception finding list is an artifact, not a claim.** The
landing change SHALL commit the complete list of findings the check produces
against its own tree with every exception record removed, each classified as
either a genuine defect or a token correct as written. **A token is correct as
written when its text should not change whatever else is repaired.** Genuine
defects SHALL outnumber tokens correct as written in that list.

The list SHALL be recorded as a sorted set of `(file, token, classification)`
without line numbers, so that a later commit editing a checked file does not
invalidate it. A reviewer who disputes a classification changes that entry and
re-checks the majority; the bar is a property of the committed list, so
disagreement is resolved by editing the artifact rather than by argument about a
number nobody can see.

The list is required rather than a ratio alone because the classification is a
human judgment per finding, and a bare number in a pull-request description
records a judgment nobody else can re-execute. Committed, it is checkable:
delete the records, re-run, and the file-and-token set must match.

**R17 — Exception budget.** The check SHALL land with no more than fifteen
records in total and no more than five intentional records. The intentional cap
is separate because that category is the one whose expiry condition is weakest,
so its growth is what needs to stay visible.

**R18 — Local reproduction.** A contributor SHALL be able to run the check
locally with one documented command and no arguments, and get the same verdict
CI gets.

**R19 — Runtime.** The check SHALL add no more than ten seconds to the
verification it runs within, measured on a warm build.

**R20 — Testability.** The check SHALL be exercised by its own tests. Those
tests SHALL include a fixture reproducing the v0.12.0 `koto session rebind`
string literals verbatim, so that a future change cannot silently stop detecting
the instance that motivated the work.

## Acceptance Criteria

Detection of the motivating instance:

- [ ] Run against a checkout of the `v0.12.0` tag via R14's root argument, the
      check reports the three `koto session rebind` string literals at
      `src/cli/mod.rs` and `src/cli/next_types.rs`. Executed and its output
      recorded, not inferred from the files being byte-identical to HEAD.
- [ ] The same run reports `koto session rebind` in
      `docs/guides/default-action-authoring.md`, where it appears in an untagged
      fence (R4a).
- [ ] Run against the current tree, the check reports `koto session info` in
      `src/engine/respawn.rs` and `koto query` in the packaged `koto-user`
      skill.
- [ ] The check's own test suite contains the v0.12.0 literals as a fixture, and
      deleting the detection makes that test fail (R20).

Ground truth:

- [ ] No enumeration of koto's command surface appears in the check or in
      anything the check reads, other than R7 exception records (R2).
- [ ] Adding a subcommand to koto's command definition and naming it in a guide
      produces no finding, with no edit to the check.
- [ ] Removing a subcommand that a guide names produces a finding for that guide.
- [ ] Adding `session rebind` to the command definition removes the rebind
      findings and reports their promised records as stale, and nothing else
      (R13).

Extraction rules:

- [ ] A candidate written in prose without code font produces no finding; the
      same token in backticks produces one (R4).
- [ ] A candidate in a fenced block produces a finding whether the fence carries
      a `bash` tag or no tag at all (R4a).
- [ ] A backticked command split across a Rust line continuation produces a
      finding (R6a).
- [ ] An unresolvable verb in a `///` doc comment and in a `//!` module doc
      comment each produce a finding; the same verb in a plain `//` comment does
      not (R6).

Paths:

- [ ] A repo-relative path that does not exist produces a finding; the same path
      with a `:120` or `:120-140` suffix produces the same finding (R3).
- [ ] `plugins/koto-skills/skills/hello-koto/SKILL.md`, cited in
      `docs/guides/custom-skill-authoring.md` — leading segment exists, full path
      does not — produces a finding (R3a).
- [ ] `src/gate/` at `CLAUDE.md:72` produces a finding, since its leading segment
      exists (R3a). It is not among R3b's accepted false negatives.
- [ ] None of `feature/anchor`, `CI/CD`, `path/to/your-template.md`,
      `~/.koto/sessions`, `gates.*`, a URL, or a bare `engine/` child of a
      rendered directory tree produces a finding (R3a).
- [ ] A token whose leading segment is `.claude/` or `target/` produces no
      finding, and neither does one carrying `<name>`, `{{VAR}}`, or `*`
      (R3a-i).
- [ ] `plugins/koto-skills/skills/<name>/SKILL.md` produces no finding while
      `plugins/koto-skills/skills/hello-koto/SKILL.md` produces one, so the
      metavariable rule is not satisfied by dropping every token containing a
      punctuation character (R3a-i).
- [ ] `.github/workflows/check-templates.yml` in
      `docs/guides/template-freshness-ci.md` produces a finding and is carried as
      an intentional record, since `.github/` is not excluded (R3a-i, R7).
- [ ] The four dead design-doc citations in `src/` each produce a finding
      (R3, R6): `src/engine/batch_validation.rs`, `src/cli/batch_view.rs`, and
      `src/cli/task_spawn_error.rs` name the batch-child-spawning design, and
      `src/workflows_surface/contract.rs` names the native-workflows-render
      design. Each writes the path with `docs/designs/` where the live document
      is under `docs/designs/current/`; the broken paths are quoted in the
      check's fixtures rather than here, since writing one in this document
      would create a fifth instance of the defect.
- [ ] `cmd/koto/` at `CLAUDE.md:11` and the bare `gate/` at `CLAUDE.md:15`
      produce no finding, and the check's tests assert that absence so the
      accepted false negative is recorded in code rather than only in prose
      (R3b).

Surfaces:

- [ ] A planted unresolvable token produces a finding in each of `src/`,
      `plugins/koto-skills/`, `docs/guides/`, `docs/reference/`,
      `docs/testing/`, `docs/STABILITY.md`, `docs/workspace-layout.md`,
      `README.md`, and `CLAUDE.md` (R5).
- [ ] The same planted token produces no finding in `docs/designs/`,
      `docs/prds/`, `docs/briefs/`, `CHANGELOG.md`, `tests/`, `test/`,
      `benches/`, `scripts/`, or `.github/` (R5).

Exceptions:

- [ ] A candidate carrying a well-formed promised record produces no finding;
      deleting the record makes the finding reappear (R7).
- [ ] A candidate carrying a well-formed intentional record produces no finding,
      and the record is accepted with no issue reference (R7).
- [ ] A promised record with no issue reference is an error naming the offending
      record (R7).
- [ ] A record of either category with no reason is an error naming the record
      (R7).
- [ ] A promised record that matches no finding is reported as a finding naming
      the stale record (R8).
- [ ] Editing the passage an intentional record is bound to retires that record,
      and the check reports it as stale until it is re-made (R8a).
- [ ] `koto migrate` in `docs/STABILITY.md` is carried as an intentional record,
      and shipping a `migrate` subcommand would report that record as stale.
- [ ] The four `koto session rebind` sites whose sentences document the verb's
      absence are recorded promised, not intentional, because koto#215 intends
      to create the name (R7a).

Output and behavior:

- [ ] Every finding's output names the token, the file, the line, how to fix it,
      and how to record it as an exception (R9).
- [ ] A stale-record finding surfaces that record's reason text (R9).
- [ ] A stale promised record and a stale intentional record produce different
      messages: the first says the record is obsolete, the second says
      re-affirming it is the expected action and does not imply prose needs
      correcting (R9).
- [ ] Every promised record for a `koto session rebind` site carries reason text
      naming the four prose passages that state the verb does not exist (R13).
- [ ] A run with several findings reports all of them, not the first (R10).
- [ ] The check exits zero on a clean tree and non-zero when it has findings
      (R11).
- [ ] The check accepts a root path argument and checks that tree instead of the
      one it was invoked from (R14).
- [ ] Run against a fixture tree whose command definition carries a verb the
      invoking repository lacks, with a document in that tree naming it, the
      check produces no finding; inverted — the invoking repository has the verb
      and the fixture tree does not — it produces one. An implementation
      resolving against the invoking tree fails both halves (R14, R2).

Landing:

- [ ] On the branch that lands the check, the check exits zero.
- [ ] The branch commits the pre-exception finding list with each finding
      classified, and deleting the records and re-running reproduces the same
      `(file, token)` set (R16).
- [ ] In that committed list, genuine defects outnumber tokens correct as
      written (R16).
- [ ] The branch carries no more than fifteen exception records and no more than
      five intentional ones, every promised one naming a filed issue and every
      record carrying a reason (R17, R7).
- [ ] `cmd/koto/` and the bare `gate/` in `CLAUDE.md` are corrected by hand in
      the landing change, since R3b means the check does not reach them.
      `src/gate/` at `CLAUDE.md:72` is corrected too, as an ordinary finding the
      check does report and R12 therefore requires be fixed or recorded.
- [ ] The check runs in CI on a pull request touching only a file under `src/`
      (R15).
- [ ] A contributor running the single documented command on the landing branch
      gets the same verdict CI reports (R18).
- [ ] The check's contribution to the verification it runs within is measured
      and is at most ten seconds (R19).
- [ ] `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test` are
      clean on the landing branch, and the `koto-stability-tests` crate's tests
      pass. This discharges no requirement above; it is the repository's standing
      bar, restated because the check runs inside that verification and must not
      break it.

## Decisions and Trade-offs

Seven entries. Three close the alternatives the upstream BRIEF deferred; four
originate here.

**One check or two, for commands and paths.** Settled: one check with two rules.
Alternatives were two independent checks with separate outputs, or one check
covering commands only with paths filed as follow-up. They share the corpus
walk, the code-font extraction rule of R4, the surface list of R5, and the
exception mechanism of R7 — everything except the resolution step. Two checks
would duplicate all of that and give a contributor two things to run and two ways
to be wrong about scope. The cost accepted is that a path finding and a command
finding cannot be triaged by exit status alone; R9 and R10 make the distinction
visible in the output instead. Whether "one check" is one file or two behind one
entry point is structural and belongs to the DESIGN.

**How the checked surfaces are expressed.** Settled: an explicit list, R5. The
alternative was deriving the boundary from lifecycle metadata documents already
carry — skip `status: Superseded`, skip `docs/designs/archive/`, skip anything
with `schema: design/v1` or `prd/v1` — which is self-maintaining and needs no
edit when a design document is added. Rejected on two grounds. The guides and
reference documents carry no frontmatter at all, so the derivation needs a
hardcoded default for exactly the surfaces that matter most, and the rule
producing that default would be invisible in a diff. And the boundary is a
judgment about which surfaces are load-bearing, which is a different judgment
from which documents are historical: `CLAUDE.md` carries no lifecycle metadata
and is the most load-bearing file in the repository. The cost accepted is that
adding a documentation directory requires an edit to R5, and a directory nobody
adds is silently unchecked. R5 is a closed definition rather than a sample so
that the gap is visible in one place.

**What an exception record is keyed on.** Deliberately open; the DESIGN owns it.
The options are the token alone — one record retires every site naming
`koto session rebind`, reads coarsely, and cannot express "this one file may say
it and no other" — and the token plus a file, which is precise, needs a record per
site, and goes stale when a file is renamed for unrelated reasons. It stays open because it depends on the record's storage shape and on
how R8 and R8a compute staleness, both of which are the DESIGN's decisions.
R7, R8, R8a, and R17 constrain the answer from the requirements side: any key
must carry a category and a reason, must be detectably stale by the condition its
category names, and must not need more than fifteen records to cover the current
tree.

**Two exception categories rather than one, with a precedence rule and separate
expiries.** Originates here. `docs/STABILITY.md` commits koto to publishing a
migration tool "as `koto migrate` or under a similar discoverable subcommand".
That sentence is correct as written, no issue will retire it, and filing one to
satisfy a lint would be filing a fake issue. A single issue-bearing category
would make R12, R16, and the goal that exceptions cannot become permanent
suppression jointly unsatisfiable on that one line. Excluding
`docs/STABILITY.md` from R5 was considered and rejected: it is a published
contract and exactly the kind of document that should not name things that do
not exist.

Adding the category opened two holes that R7a and R8a close. The categories
overlap on the four `koto session rebind` sites that both await koto#215 and
document the verb's absence, so R7a gives promised precedence — otherwise the
weaker obligation is always available. And R8's expiry condition, the name
becoming real, is by definition the condition that never occurs for an
intentional record, so R8a binds those records to the passage instead. The
asymmetry is the whole argument and needs no instance to demonstrate it: R8 asks
"did the name become real", and an intentional record is by definition one where
that will not happen. `koto migrate` is the live case — the acceptance criterion
carrying it concedes that its R8 expiry is a condition nobody expects. Without
R8a, a genuine defect recorded intentional with a plausible reason ships
forever, and nothing in the document could ever say so.

**Path candidacy is anchored on an existing top-level entry.** Originates here,
and it is a reversal. An earlier draft removed the anchor so the check could
reach `cmd/koto/`, a directory renamed away and still drawn in `CLAUDE.md`'s
tree. Implementing both readings and counting settled it: with the anchor the corpus
yields 33 path findings, 21 genuine against 12 correct as written, and R3a-i
takes the second number to 3. Without the anchor, a charitable
implementation yields 93; 9 of those are cross-repo tokens excluded on other
grounds, and of the 84 that remain, 56 are correct as written against 28 genuine
— git branch names in worked examples, `path/to/your-template.md`, `CI/CD`,
`18/18`, a regex character class, the children of every rendered directory tree
in the repository. The token the removal was meant to reach lives inside the construct
that generates most of the noise, because a tree's indentation carries the
parent that a flat scan cannot see. So the anchor stays, R3b states the false
negative, and the two live instances it misses are fixed by hand in the same
change.

**How this work couples to koto#215, given it cannot bind it.** Originates here.
The two changes interact: when `koto session rebind` ships, the findings naming
it disappear and their promised records go stale, so koto#215's branch goes red
unless it removes them. An earlier draft wrote that as a requirement on
koto#215's change, which this PRD has no standing to impose and no mechanism to
enforce. What it can do is make the failure self-explanatory to someone who has
never read this document, which is why R9 requires a stale-record finding to
surface the record's reason and R13 requires that reason to name the four prose
passages. Note the sequencing: koto#215 may have branched before the check
merged, in which case nothing fires until it rebases onto a main that carries
the check. That is the correct outcome and not a guarantee — the check is green
on both sides of koto#215 once it rebases, not unconditionally.

The limit is worth stating rather than leaving implied: the cheapest way to make
that CI failure go away is to delete the stale records, which requires reading
nothing. The reason text is displayed, not consumed, and no check verifies that
the prose was corrected. So this mechanism raises the odds that the four passages
are noticed; it does not ensure it. The alternative — a check that knows a
sentence claims a verb does not exist and re-reads that claim when the verb
lands — is prose comprehension, which is the boundary this whole document
declines to cross.

**Whether the existing template-compilation gate gets widened.** Settled: not in
this work. `.github/workflows/validate-plugins.yml` compiles templates matching
`plugins/koto-skills/skills/*/koto-templates/*.md`, which matches two files, one
of which its own `*.mermaid.md` guard skips — so a job named for compiling
templates compiles one file, while eleven files outside the test fixtures are
template-shaped and four shipped example templates sit outside the glob. Widening
it is a small change and worth doing. It is excluded because it is a different
mechanism answering a different question — whether a template compiles, not
whether a name resolves — and folding it in would make this work's scope depend
on a gate it does not otherwise touch. File it separately.

## Known Limitations

- **Naming only.** A document that names only real things can still describe them
  wrongly. Every instance in the corpus is a naming failure, and that boundary is
  drawn deliberately.
- **Drift by omission is invisible.** The four wrong `koto session` verb lists
  are wrong because they leave `recover` out. Every name they contain resolves,
  so nothing in this check reaches them. They are cited in the Problem Statement
  as evidence of the class, not as instances this work fixes.
- **A renamed or deleted top-level directory is not reached, and neither is a
  tree diagram's children.** R3a's anchor is what keeps the path rule usable, and
  R3b names the price: `cmd/koto/` at `CLAUDE.md:11` and the bare `gate/` at
  `CLAUDE.md:15` are real dead citations the check cannot see. Both are corrected
  by hand in the landing change, and the check's tests assert the absence so a
  future reader finds the limitation in code rather than only here. The table row
  at `CLAUDE.md:72` citing `src/gate/` is reachable and is an ordinary finding.
- **A shipped skill routes agents to unchecked documentation.**
  `plugins/koto-skills/skills/koto-user/references/error-handling.md` and
  `docs/guides/cli-usage.md` both send readers to
  `docs/designs/current/DESIGN-batch-child-spawning.md` for the full rule
  definitions, and that document names `koto query`, which does not exist.
  Bringing `docs/designs/current/` into R5 would add roughly 65 findings that are
  legitimately there, so the exclusion stands and the consequence is recorded: an
  agent can be routed from a checked surface into an unchecked one.
- **`CLAUDE.local.md` is out of reach.** It is generated into the working copy
  from another repository and is not tracked here, so no check in koto's CI can
  see it. It carries the largest single instance of this class, describing koto
  as a Go project with `go build`, `pkg/`, `internal/`, and two subcommands that
  do not exist. Fixing it requires a change where its source lives.
- **Flags are not checked.** `koto session list --parent`, in the same respawn
  prompt as `koto session info`, is decidable by the same means and is not
  covered. Deferred because the noise is unmeasured and no known instance turns
  on a flag alone.
- **When `koto session rebind` ships, four passages become false in the other
  direction.** `docs/reference/error-codes.md`, the `koto-user` SKILL and its
  `error-handling.md` and `command-reference.md` references all state that the
  subcommand does not exist. The moment it does, those sentences are wrong, and
  this check will not say so — every name in them resolves. The stale promised
  record is the only lever, which is why R9 requires its reason text to be
  surfaced and R13 requires those reasons to name the passages.
- **An intentional record's reason is never checked.** Nothing verifies that a
  reason is true, and R8a bounds a miscategorisation only for passages somebody
  edits — text that earns an intentional record is settled by nature, and a
  stability contract can go years untouched. R17's cap of five intentional
  records is the hard bound; R8a is a partial second one.
- **A stale exception is detected only when the check runs.** R8 and R8a catch a
  record that has become obsolete on the next run, which is every pull request,
  so the window is small and the record is visible in the meantime.

## Out of Scope

- **Prose accuracy.** Not decidable.
- **Code identifiers, types, and field names.** They are 70% of backticked spans
  and their measured miss rate against source is an order of magnitude worse than
  command names, for legitimate reasons. This is the rule that would get the
  check disabled. See the BRIEF's Scope Boundary for the full argument.
- **Everything R5 does not name**, including `docs/designs/`, `docs/prds/`,
  `docs/briefs/`, `CHANGELOG.md`, `tests/`, `test/`, `benches/`, `scripts/`, and
  `.github/`.
- **Renamed top-level directories**, **drift by omission**, **flags**, and
  **files koto's CI cannot see.** See Known Limitations.
- **Whether a documented template compiles or behaves.** See Decisions and
  Trade-offs.
- **Other repositories.** shirabe carries an instance of this class. The command
  surface being resolved against lives in koto, and fixing shirabe is separate
  work.
- **Implementing `koto session rebind`.** koto#215 owns the verb.
- **Changing what any error message says.** Where a finding is in `src/`, this
  work either records it under R7 or fixes the name; deciding that the anchor
  refusal should point somewhere else is koto#215's call.
