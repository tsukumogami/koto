---
schema: design/v1
status: Accepted
problem: |
  koto has no mechanism that resolves a name it states against the thing that
  name refers to. Building one needs four things decided: where the check runs,
  since the only source of truth about koto's command surface that cannot itself
  drift is koto's own clap tree; what makes a candidate, since code font alone
  admits every English word that follows the word koto; how a deliberate
  exception is stored and keyed, since one phantom verb accounts for sixteen of
  the twenty-three finding sites and the requirements cap total records at
  fifteen; and how an intentional
  exception expires, since the condition that retires a promised one is by
  construction the condition an intentional one will never meet.
decision: |
  A Rust integration test at `tests/doc_names.rs` walks
  `koto::cli::App::command()` for the verb set, scans a fixed surface list for
  backticked and fenced candidates, and resolves commands against that walk and
  paths against the filesystem. Exceptions live in a tab-separated
  `tests/doc_names.allow` keyed on `(kind, token)` rather than on a site, with a
  category column that decides whether an issue reference is required and a
  witness digest that binds an intentional record to the text it protects. The
  scanner is a pure function of a corpus root and a verb set, so fixtures supply
  their own ground truth and the production caller is the only place the clap
  tree is read.
rationale: |
  Ground truth decides the language: a shell script reaches koto's verb set by
  scraping help text, which is presentation rather than contract; by piping it
  from a second build product, which puts the check back outside cargo test and
  into the CI wiring that let the defect ship; or by hardcoding it, which
  reintroduces the drift being checked. Candidacy is decided by measurement: an
  anchored token, one that begins its code span or its line, removes four English
  sentences that would otherwise need permanent exceptions. Record keying is
  decided by arithmetic -- per-site records need twenty entries against a
  budget of fifteen, sixteen of them for `koto session rebind` alone, so
  token-level keying is the only shape that fits. And a witness digest over the normalized spans a
  record suppresses over-fires by design, because a spurious re-affirmation costs
  a line and a missed one costs a permanent suppression.
upstream: docs/prds/PRD-doc-code-drift.md
motivating_context: |
  koto v0.12.0 shipped an error message instructing users to run
  `koto session rebind`, a subcommand that does not exist, past every gate the
  repository runs. Filed as koto#216.
user_visible_surface: true
---

# DESIGN: Name resolution for koto's documentation and error strings

## Status

Accepted

## Context and Problem Statement

koto states names — subcommands in error messages, paths in guides, verbs in the
packaged skills an agent executes — and nothing resolves them against the things
they refer to. `docs/prds/PRD-doc-code-drift.md` establishes what that check must
do; this design settles how it is built.

Three things about the tree shape the answer, and each was measured rather than
assumed.

**The verb set is free from Rust and expensive from anywhere else.** koto's
`src/main.rs` is ten lines. The parser lives in the library: `src/lib.rs`
re-exports `pub mod cli`, and `koto::cli::App` derives `clap::Parser`, so it also
gets `clap::CommandFactory`. An integration test can call `App::command()` and
walk `get_subcommands()` recursively. Running that probe against this tree
yields 52 verb paths, zero aliases, and no `session rebind`, in 0.00s of test
time behind a 6.5s incremental link. Nothing was added to `Cargo.toml` to do it —
`clap` is already a normal dependency, and integration tests link against normal
and dev dependencies alike.

**Precision is a scoping problem, and the scope is already fixed.** The PRD's R5
names nine surfaces and excludes everything else; R3a anchors path candidacy on
an existing root entry; R4 requires code font. Measured against this corpus, the
path half gives 33 findings, 21 genuine, and R3a-i takes the correct-as-written
share from twelve to three. The design does not get to revisit those numbers; it
has to hit them.

The command half was measured by building the rules below and running them.
**914 candidates across the nine surfaces yield four unresolved tokens at
twenty-three sites, with no false positives**: `session rebind` at sixteen,
`query` at four, `session info` at two, and `migrate` at one. Three of the four
are genuine defects and the fourth is the forward commitment in
`docs/STABILITY.md`.

The first version of that measurement was wrong, in the defect class this
project exists to catch, and the correction is worth recording. It reported
thirteen `session rebind` sites because its extractor never joined Rust `\`
string continuations, so it missed `src/cli/mod.rs:3472`, `:3488`, and
`src/cli/next_types.rs:178` — the three v0.12.0 literals, the subject of R20's
fixture, and the whole reason R6a exists. A measurement that misses the
motivating defect is worse than none, and R6a is not a nicety.

**The exception budget decides a design question.** R17 caps records at fifteen
and intentional records at five. Per-site keying needs one record per site that
is recorded rather than repaired: sixteen for `session rebind`, one for
`migrate`, and three for the path tokens that survive R3a-i — twenty against a
cap of fifteen. Token-level keying needs five: one promised and four
intentional, inside both halves of R17's budget. This is the clearest case in the
document of a requirement settling an architecture question by arithmetic, and it
is why the keying decision below has one viable answer rather than a trade-off.
On the command half specifically, token-level keying needs exactly two records —
one promised for `session rebind` against koto#215, one intentional for
`migrate` — and everything else is repaired rather than recorded.

## Decision Drivers

- **D1 — Ground truth must come from koto's own command definition** (R2), and
  from the tree named by the root argument rather than the tree the check was
  built from (R14). Those two pull in opposite directions for anything compiled,
  and the design has to reconcile them.
- **D2 — The check must run when `src/` changes** (R15). koto's `validate.yml`
  runs `cargo test` with no `paths:` filter; every other content check in the
  repository is gated on `plugins/**` or `docs/**`, which is the topology that
  let v0.12.0 ship.
- **D3 — Total exception records must not exceed fifteen, intentional records
  five** (R17).
- **D4 — An intentional record must expire on a condition that can actually
  occur** (R8a), and the expiry should err toward over-firing.
- **D5 — A stale record must produce a message matched to why it went stale**
  (R9), because the correct response differs: obsolete, needing re-affirmation,
  covering something new, covering less than it did, or both at once.
- **D6 — One documented command, no arguments, same verdict as CI** (R18), and
  at most ten seconds added (R19).
- **D7 — The v0.12.0 demonstration must be executable** (R14, R20), which means
  the scanner has to accept a corpus it was not compiled inside.
- **D8 — The house style for checks in this workspace is a `scripts/check-*.sh`
  with a prose header, accumulate-then-report, a fix line per finding, and a
  `_test.sh` sibling.** Thirteen such scripts exist across the two repositories,
  eleven of them in shirabe. Departing from the form needs a reason stronger than
  preference.

## Considered Options

### Option A — A shell script under `scripts/`, in the house style

The shape the existing checkers take: eleven in shirabe, two in koto.
`scripts/check-doc-names.sh`, a long prose header naming the v0.12.0 incident
and what is deliberately not flagged, a path argument so a `_test.sh` sibling
can drive it against fixtures, findings accumulated and reported with `FAIL:`
prefixes, exit 0/1/2, and a tab-separated allowlist. shirabe's
`check-template-interpolation.sh` writes the argument for this shape into its
own header: a check that is "a statement about this repository's own file
layout" belongs in a workflow-invoked script rather than inside a compiled
validator, because folding it in "would put a Rust change and a release in front
of every adjustment to it."

That argument is real, and it is why this option was taken seriously. It fails
on D1. A shell script has three routes to koto's verb set and each has a
disqualifying cost.

It can crawl `koto --help` recursively against a built binary. `cargo test`
already builds `target/debug/koto` — `tests/integration_test.rs` and
`scripts/verify-native-workflows.sh` both consume it — so the build is not the
objection it first appears to be. The objection is that clap's help rendering is
presentation rather than contract: a `hide = true`, a help-template change, or a
wrapping change silently alters what the crawl sees, and the check would report
a verb as missing because its help text moved.

It can call a small helper that walks `App::command()` and dumps the verb set —
`cargo run --example dump-verbs` — which is immune to the presentation
objection, because it reads the same structure Option C reads. This is the
strongest version of Option A and the design rejects it on cost rather than on
correctness: it adds a second build product to maintain, it pipes ground truth
across a language boundary in a format that then needs its own contract, and it
puts the check back outside `cargo test`, which means a workflow, a job, an
entry in `validate.yml`'s aggregator, and a `paths:` filter to get right — the
last being the specific thing three existing koto workflows got wrong and D2
exists to avoid.

Or it can hardcode the list, which reintroduces the exact defect the check
exists to catch, one level up. The repository already demonstrates how that
ends: four of the five hand-maintained `koto session` verb lists in its
documentation are wrong, and the wrongest sits inside the paragraph explaining
the drift bug.

Rejected on D1 and D2, with the note that D8 is a real cost being paid.

### Option B — A `koto` subcommand

`koto doctor` or `koto lint docs`, shipped in the binary, reading the clap tree
from inside the process that defines it. D1 falls out for free.

Rejected on two grounds. It puts user-facing CLI surface into the repository's
own development tooling, which every user then carries in their binary and which
becomes a compatibility surface koto must keep — and koto has a stability
contract that would then cover it. And it is the strong form of exactly what
`check-template-interpolation.sh`'s header warns against: an adjustment to which
directories are scanned would need a release.

### Option C — A Rust integration test under `tests/`

`tests/doc_names.rs`. Reads `App::command()` directly, needs no binary build, no
new dependency, and rides the `cargo test` job that already runs with no
`paths:` filter — so D1, D2, D6 and D7 are satisfied by the placement rather
than by mechanism.

koto has the precedent twice over. `tests/lib_reexports.rs` is an in-tree
compile-check asserting that a public surface has not moved, and
`tests/native_workflows_shape.rs` is a `tests/` drift guard whose subject is a
committed file read by repo-relative path — the same shape as this check.
Neither settles how the root resolves: `native_workflows_shape.rs` reads its
fixture CWD-relative. `tests/integration_test.rs` is the precedent for that, and
it resolves from `CARGO_MANIFEST_DIR`.

The workspace-crate alternative is ruled out by evidence in the repository
rather than by argument. `koto-stability-tests` is a workspace member the root
`cargo test` does not reach, which is why `validate.yml` runs it as its own job.
Putting the check there would reintroduce the CI-wiring problem D2 exists to
avoid.

The cost is D8. This is not a script, so it does not inherit the script family's
shape, and a contributor looking under `scripts/` will not find it. The
mitigation is to carry the house *conventions* across the language boundary
rather than the house *form*: the prose header naming the incident,
accumulate-then-report, a fix line on every finding, a tab-separated allowlist
whose records require an issue reference, and staleness that fails in both
directions. All are language-independent and all are adopted below. koto's own
share of the house style is one CI-wired script against two in-tree `tests/`
drift guards, so for this repository the departure is smaller than D8 implies.

**Chosen.**

### Option D — Generate a CLI reference from clap and diff the committed copy

The `clap-markdown` / `clap_mangen` pattern: render the command surface to a
committed document and fail when the regenerated output differs. jj does this
for its CLI reference.

Rejected because it does not address the defect. koto has no CLI reference page
to generate — generation would produce a *new* document, and the drifting prose
would still sit beside it. More decisively, the instance that motivated the work
is a Rust string literal in `src/cli/next_types.rs`, and no amount of generating
markdown from clap inspects a string literal. It is a good idea for a different
problem and is worth filing separately.

### Option E — `trycmd` literate CLI testing

Run the commands found in markdown and compare recorded output. It is the
closest thing in the Rust ecosystem to what this sounds like from a distance.

Rejected on cost and on reach. It reads only fenced blocks tagged `trycmd` or
`console`; koto has 85 ```bash fences and zero of either, so adoption means
rewriting all 85 into shell-transcript form and authoring expected output for
each, then maintaining that output forever. Most are illustrative fragments with
no stable output. And it answers "does this documented command still produce
this output", which is not the question — it cannot see a verb named in a prose
sentence, in a table cell, or in a Rust string literal.

## Decision Outcome

A Rust integration test at `tests/doc_names.rs`, with exceptions in a
tab-separated `tests/doc_names.allow` keyed on `(kind, token)`.

Five sub-decisions follow, and each is settled here.

**A two-word token whose first word owns subcommands must match in full.** This
is the smallest of the five and the one the check would be useless without. A
token not in the verb set falls back to its first word only when that word owns
no children, so `status my-flow` resolves on `status` and `session rebind` does
not resolve on `session`. Without the parent check the motivating defect
resolves against a real verb and the check reports nothing at all. Solution
Architecture gives the two structures this needs.

**Candidates are anchored, not merely inside code font.** R4 requires code font;
code font alone is not enough, because English prose appears inside code font
constantly and every word of it follows the word `koto` somewhere. A command
candidate is therefore extracted only where `koto` *begins* its context: the
first token of an inline code span, the first token of a line inside a fenced
block after an optional `$ ` prompt, the first token of a Rust string literal or
of a backticked span inside one, or the first token of a backticked span inside a
`///` or `//!` doc comment. Rust takes two rules rather than one because a
literal is code font and a doc comment is prose; Extraction carries the
measurement behind the split. The word must be delimited on both
sides: `koto` preceded by `-` is never a candidate, so `hello-koto` does not
fire, and `koto` must be followed by whitespace or the end of its span, so the
eight `koto::` Rust paths in the checked surfaces are not candidates either.

The rule has a cost, and it is the same shape as R3b's: **a `koto` invocation
that is neither span-initial nor line-initial is invisible.** The pipe form is
the live class — `... | koto next` and its siblings appear at five sites in the
corpus, and the pipe is not the only form: command substitution
(`$(koto session dir ...)`) and an environment prefix (`FOO=bar koto next`) are
mid-line for the same reason. Together they account for 35 sites across 16 files,
every one of which resolves today, so nothing is missed now; a future phantom
verb written in any of those forms would be. The exemplar is
`echo "..." | koto context add`, in `docs/guides/cloud-sync-setup.md`,
`docs/guides/custom-skill-authoring.md`, `docs/guides/cli-usage.md`, and the
`koto-user` command reference. That is accepted for the same reason
R3b's false negative is accepted, and it is recorded here rather than escalated
to the PRD because it is a consequence of a mechanism the PRD does not choose.

The rule is what makes the exception budget reachable. Without it, four English
sentences need permanent intentional records: `Active koto workflow detected` in
a manual-test checklist, `# Run these BEFORE downgrading the koto binary` in a
published contract, `koto is running` inside a reproduced error payload that
must match the binary byte for byte, and `using koto workflow guidance` in a
skill description. All four have `koto` mid-line or mid-span and none survives
anchoring. Measured over the whole corpus, anchoring plus the fence-and-span
rule is what produces the measurement above: four unresolved tokens and no false
positives, where an unanchored extractor over the same surfaces reports prose.

**The scanner is a pure function; the clap tree is read in one place.** D1's two
halves conflict for anything compiled: R2 wants ground truth from koto's command
definition, and R14 wants it from the tree named by the root argument, which a
test compiled against *this* crate cannot produce for an arbitrary tree. They
reconcile by parameterizing. The scanner's signature is

```rust
fn scan(root: &Path, verbs: &BTreeSet<String>, allow: &[Record]) -> Vec<Finding>
```

The repository test is the only caller that derives `verbs` from
`App::command()`; fixtures supply their own. So R2 holds where it matters — no
hand-maintained enumeration exists in the production path — with one honest
qualification: R20's fixtures carry hand-written verb sets by construction, and
that is what makes them fixtures.

What the root argument carries needs saying plainly, because R14's own wording
invites a stronger reading than any compiled check can deliver. **The root
carries the corpus and the allowlist; it does not carry the verb set.** A run
against a checkout of `v0.12.0` therefore resolves v0.12.0's prose against
today's command surface. That is sound for the demonstration it exists to
support — `session rebind` has never existed at any commit, so no verb set in
koto's history resolves it — and it is unavoidable for any approach that does
not build the foreign tree. It is stated here rather than elided because a
reader who assumes otherwise would draw a wrong conclusion from a run against
some other tag.

**Records are keyed on `(kind, token)`, not on a site.** D3 decides this and
leaves no alternative. Per-site records need twenty entries against a budget
of fifteen — sixteen for `koto session rebind`, one for `migrate`, three for
the surviving path tokens — so the budget and per-site keying cannot both hold. Token-level keying is also the truer model
of what a record asserts: the promise is about the verb, not about each sentence
that mentions it, and koto#215 retires the verb rather than sixteen sentences.
The cost is that a record cannot say "this file may name it and no other".
Accepted; nothing in the corpus needs the distinction, and R16's committed list
is where per-site visibility lives.

**An intentional record carries a witness digest over normalized candidate
text.** D4's problem is that R8's expiry — the name became real — is by
definition the event an intentional record says will not happen. The record
carries a second column binding it to what it protects. The digest is specified
in full below; the design decision is that it hashes the *normalized candidate
spans* rather than physical source lines, because R6a took care to make a
finding's location independent of where a formatter wrapped a literal, and
hashing raw lines would reintroduce that dependency for record lifetime.

## Solution Architecture

### Components

```
tests/doc_names.rs
├── verbs()          -- walks App::command() into a BTreeSet<String>,
│                       including aliases, which the walk reports and which
│                       koto happens to have none of today
├── surfaces()       -- the R5 file list, expanded against a root
├── extract()        -- anchored code spans and fences -> Vec<Candidate>
│   ├── markdown()   -- inline spans, fenced blocks (tagged or not),
│   │                   and spans nested inside fences
│   └── rust()       -- string literals with continuations joined and
│                       self-anchoring; /// and //! comments anchored only
│                       by a backticked span; plain // skipped
├── resolve()        -- Candidate -> Option<Finding>
│   ├── command      -- longest-prefix match against the verb set
│   └── path         -- R3a-i filters, R3a anchor, suffix strip, stat
├── allow()          -- parses tests/doc_names.allow into Vec<Record>
├── witness()        -- digest over a finding's normalized span set
├── scan()           -- the pure function; the only thing fixtures call
└── #[test] fns      -- one repository scan, plus fixture-driven cases
```

Which extractor a file gets is decided by extension, and the choice belongs to
this design rather than to the implementer:

| Files | Extractor |
|---|---|
| `*.rs` | the Rust rules: literals self-anchor, doc comments need a backticked span, plain `//` skipped, `#[cfg(test)]` modules skipped entirely |
| `*.md`, `*.mdc` | the markdown rules: fences and inline spans |
| `*.json`, `*.sh`, and extensionless files under `plugins/koto-skills/` | code font throughout, so a line-initial `koto` anchors, plus backticked spans |

The third row exists because a skill package is configuration an agent executes
as much as prose it reads: `plugins/koto-skills/hooks.json` carries a shell
command inside a JSON string, and a phantom verb there fails a real workflow.
Without the row, "across the nine surfaces" would be false of five `.json`, one
`.sh`, one `.mdc` and three extensionless files that R5's glob already names.

`#[cfg(test)]` modules are skipped for the same reason `tests/` is outside R5: a
fixture naming a phantom verb is doing its job. Skipping them also removes
`docs/draft.md` — four `#[cfg(test)]` literals in `src/cli/next_types.rs` — from
the finding set, a path that would otherwise take a fifth intentional record and
put R17's intentional cap exactly at its limit.

The default root is `CARGO_MANIFEST_DIR`, following
`tests/integration_test.rs`. An alternate root is supplied by the
`KOTO_DOC_NAMES_ROOT` environment variable, since a `cargo test` target takes no
positional arguments. Under an alternate root the allowlist is read from that
root when present and treated as empty when absent, so a fixture tree with no
allowlist reports everything.

### The candidate model

```rust
struct Candidate {
    kind: Kind,        // Command | Path
    token: String,     // "session rebind" | "docs/template-format.md"
    file: PathBuf,     // root-relative
    line: usize,
    span: String,      // the normalized code span the token came from
}
```

`span` rather than the source line is what the witness digest hashes. It is the
**innermost** enclosing code span — the backticked span when there is one, the
string literal or fence line otherwise — normalized by collapsing interior
whitespace and stripping surrounding backticks and fence indentation, with Rust
string continuations already joined by the extractor.

`line` is the physical line carrying the matched phrase, not the line the
literal opens on. For `src/cli/mod.rs` that is 3473 rather than 3472, which is
where a reader looking for `session rebind` will actually find it. R6a requires
that a finding *exist* regardless of where a formatter wrapped the literal; it
does not require the reported line to be wrap-independent, and reporting the
opening line would send a reader to a line that does not contain the phrase.

### Extraction

Every `koto` occurrence is considered rather than only the first in a file, and
each is tested for an anchor of its own: a fence supplies one anchor per line and
a document supplies one per code span, so a file with four anchored occurrences
yields four candidates. An occurrence with no anchor yields none.

Backtick spans are extracted from inside fenced blocks as well as outside,
because `docs/reference/error-codes.md` carries the motivating verb inside a
backticked span inside a JSON string inside a ```json fence.

Rust needs two rules rather than one, and the split is measured rather than
stylistic.

A **string literal** is code font in itself, so it supplies its own anchor: a
candidate is produced where `koto` begins the literal, and also where `koto`
begins a backticked span inside it. The two are alternatives — an occurrence
qualifies if either holds — so backticks are not required. The three literals in
the motivating defect all carry them, but a future author may write the
instruction without any, and a rule demanding them would miss exactly the case
this check exists for.

A **`///` or `//!` doc comment** is prose, so only a backticked span inside it
anchors; the comment body does not. Without this split the check reports four
sentences whose wrap happens to put `koto` at the start of a continuation line —
`koto now has two log families`, `koto delivers again`, `koto renders a session`,
`koto session id whose context store` — while the two genuine doc-comment
findings, both `` `koto query --events` ``, are backticked. The rule separates
them exactly, and a plain `//` comment is out of scope either way.

`token` is normalized at extraction. A command candidate keeps the words
following `koto` that match `^[a-z][a-z0-9-]*$`, stopping at the first word that
does not and at a maximum of two. An occurrence yielding no words — a bare
`` `koto` ``, of which the checked surfaces hold eleven — is not a candidate at
all, rather than a candidate with an empty token. So `` `koto session rebind {} --to <dir>` ``,
`koto session rebind <session> --to <dir>` in an untagged fence, and
`koto session rebind my-workflow --to <dir>` inside a JSON payload all normalize
to `session rebind`. This is what makes token-level records work across the five
textually distinct forms the same phantom verb takes here.

### Resolution

**Commands.** The verb set is walked into two structures: the set of full paths
(`session`, `session start`, `workflows`, `workflows publish`, ...) and the set
of first words that own children (`session`, `config`, `context`, `decisions`,
`overrides`, `request`, `template`, `workflows`, `workspace`).

A token resolves when it is in the path set. A two-word token that is *not* in
the path set resolves only when its first word is in the path set **and owns no
children** — that is, when the second word is an argument rather than a
subcommand. Otherwise the candidate is a finding.

The parent check is load-bearing and not decoration. Without it, `session
rebind` falls back to the one-word `session`, which is a real verb, and the
check reports nothing for the defect that motivated the entire work. With it,
`session rebind` fails because `session` owns children, while `status my-flow`
resolves because `status` owns none and everything after it is an argument.

`workflows` is the case that makes the rule look wrong and does not break it: it
owns a child and is also a complete invocation, because `action` is an
`Option<WorkflowsAction>`. Bare `workflows` resolves on the path set,
`workflows publish` resolves on the path set, and `workflows garbage` is a
finding — which is correct, since koto rejects it too. The word-shape rule keeps
`koto workflows <action>` and `koto workflows [--roots]` from producing a
spurious second word in the first place.

**Paths.** In order: reject if the token contains `<`, `{{`, or `*`; reject if
the leading segment is `.claude` or `target`; strip a trailing `:<line>`,
`:<start>-<end>`, or `::<symbol>` suffix; require the leading segment to be a
member of `read_dir(root)`; stat the joined path. `.github` is deliberately
absent from the exclusion list — excluding it silences one false positive and
blinds three live internal citations of koto's own workflow files, so the one
false positive is carried as an intentional record instead.

### The allowlist format and grammar

```
# kind  token           category     issue                 witness   reason
command session rebind  promised     tsukumogami/koto#215  -         Specified in DESIGN-koto-runs-commands and implemented under that issue. When it lands, correct the four passages that state the verb does not exist: docs/reference/error-codes.md, koto-user/SKILL.md, and its error-handling.md and command-reference.md references.
command migrate         intentional  -                     a3f19c2e  STABILITY commits koto to publishing a migration tool; the sentence is a forward commitment, not a claim that the verb exists.
```

Six tab-separated fields, parsed with `splitn(6, '\t')` so a reason may itself
contain tabs. Lines whose first non-whitespace character is `#`, and blank
lines, are skipped. A missing file parses as no records; an empty file likewise.
Every one of the following is an error naming the offending line, not a silently
ignored record: fewer than six fields, a `kind` outside `{command, path}`, a
`category` outside `{promised, intentional}`, a promised record whose `issue` is
`-` or is not `owner/repo#N`, an intentional record whose `issue` is not `-`
(the category carries no issue by definition, and one written there would be a
promised record filed under the weaker obligation), an empty `reason`, a
duplicate `(kind, token)`, and a `witness` that is neither `-` on a promised
record nor `?` or exactly eight hex characters on an intentional one.

The block above is space-aligned for readability; real records are
tab-separated.

A `path` record's token is matched **after** suffix stripping, so one record
covers `src/gate/mod.rs`, `src/gate/mod.rs:206`, and `src/gate/mod.rs:206-230`.

R7a — that a name any filed issue intends to create must be recorded promised
rather than intentional — is an authoring rule the check cannot enforce, because
nothing mechanical can know whether an issue intends to create a name. It is
stated in the allowlist's own header comment and is a thing review checks. The
one mechanical half is enforced: an intentional record carrying an issue
reference is an error, which removes the accidental route into the weaker
category.

### The witness digest

Two things are defined here and they are deliberately not the same thing: the
**member key**, which says whether the suppressed set changed, and the **digest
input**, which says whether the text changed.

The member key of a suppressed finding is `(root-relative file, ordinal of
*this token's* occurrences within that file)`. Scoped to the token, not to all
anchored occurrences: an unrelated valid invocation added above a protected
sentence must not renumber it, which is the defect the ordinal replaced the line
number to prevent. Token-scoping is also what gives path records a member key at
all, since they have no `koto` occurrences to count.

The ordinal rather than the line number, because a line number changes whenever
anything above it is edited. And an ordinal rather than the span text alone,
because two byte-identical spans in one file must stay two members: without that,
`session rebind`'s sixteen sites collapse to fewer — `src/cli/mod.rs` carries
the phrase twice with byte-identical text — and deleting one of a duplicated pair
is invisible.

One over-fire this leaves, stated so it is not discovered in CI: reordering two
occurrences of the same token within a file swaps which ordinal carries which
span, so a pure move reports as a text edit. That is consistent with the stated
over-fire preference and costs a re-affirmation.

The digest is `sha256`, hex-encoded, truncated to the first eight characters,
over the `\n`-joined, lexicographically sorted set of
`<file>\t<ordinal>\t<normalized-span>` for every member. `sha2` and `hex` are
already dependencies. The line number appears nowhere in it, which is what
delivers the immunity the Decision Outcome argues for: `cargo fmt` re-breaking a
literal, or a paragraph inserted above a protected sentence, changes neither the
ordinal nor the normalized span.

A brand-new record is written with `?` in the witness column. The check treats
`?` as "not yet affirmed", reports the record with the digest it should carry,
and fails — so adding an intentional record is a deliberate two-pass operation
and the design says so rather than leaving an author to discover it. `-` remains
reserved for promised records.

Staleness is computed on the member set first and the digest second. Shapes 3
through 5 all ask for a witness update, which only an intentional record carries
— so a promised record can reach shape 1 alone, and shapes 2 through 5 belong to
intentional records. Additions and removals are evaluated independently, so a
change that both adds and removes members — which is what moving text between
files does — produces shapes 4 and 5 together rather than falling through both.

Five shapes:

1. The suppressed set is empty and the record is promised — the name became
   real. R8. The record is obsolete.
2. The suppressed set is empty and the record is intentional — the same, and
   equally an error: an intentional record matching nothing is the
   permanent-suppression hole R8a exists to close, appearing from the other
   direction.
3. The member set is unchanged in size but the digest differs — the protected
   text was edited. R8a. Re-affirmation is the expected action. The message asks
   the author to confirm that every site the record still covers is one it
   should, rather than asserting nothing is wrong: a site deleted and a different
   one added in the same file for the same token leaves the count unchanged and
   lands here, so shape 3 is the one shape that cannot promise the change was
   innocent.
4. The suppressed set *grew* — a new site of a recorded token appeared. This is a
   new defect wearing an old record, and it must not be reported as
   re-affirmation. The message names the new sites specifically and asks whether
   they belong under the record.
5. The suppressed set *shrank* but is not empty — some sites were repaired and
   others were not. This is the ordinary state during a partial repair, so the
   message says so: it names what is gone, names what remains, and asks for
   re-affirmation of the smaller set. Reporting it as an error with no path
   forward would punish the half-done repair that is the normal way a large one
   gets made.

### Failure output

Accumulate every finding, report, exit once. Every site is printed; nothing is
elided behind a count, because R9 requires the file and line of each and R10
requires the whole repair in one pass.

```
FAIL: unresolved command `koto session rebind` (16 sites)
  src/cli/mod.rs:3473
  src/cli/mod.rs:3489
  src/cli/next_types.rs:179
  docs/guides/default-action-authoring.md:574
  ... every remaining site, one per line ...
  koto has no `session rebind`. Either use a command that exists, or record the
  promise: add to tests/doc_names.allow
    command<TAB>session rebind<TAB>promised<TAB>owner/repo#N<TAB>-<TAB><reason>

FAIL: stale record `session rebind` -- the command now exists
  The record said: Specified in DESIGN-koto-runs-commands and implemented under
  that issue. When it lands, correct the four passages that state the verb does
  not exist: ...
  Remove the record in the change that added the verb, and correct what that
  reason names.

FAIL: record `migrate` needs re-affirming -- the text it protects changed
  Re-affirming is the expected action; no prose needs correcting.
  Update the witness column to a91b3f04.

FAIL: record `migrate` now suppresses 2 new sites
  docs/guides/upgrading.md:41
  These were not covered when the record was written. Decide whether they belong
  under it; if they do, update the witness column to 5c0d81b2.
```

Because libtest captures stdout, the repository scan writes findings through
`panic!` with the accumulated report as its message rather than through
`println!`, so the report is visible on a failing run without
`--nocapture`.

### CI placement

None. `tests/doc_names.rs` is picked up by `cargo test` in `validate.yml`'s
existing unit-tests job, which has no `paths:` filter — so D2 is satisfied by
adding no workflow, no job, and no entry to the aggregator's `needs:` list. This
is the strongest argument for Option C over Option A: the placement question a
script would have had to answer, in the presence of a `paths:` trap three
existing workflows fell into, does not arise.

## Implementation Approach

Five batches. Each is a commit on one branch and the branch is one pull request.
The order puts the repairs first, so that the batch which turns the check on has
only mechanism left in it.

**Batch 0 — the repairs, ahead of the check.** Every defect the check will
report and that is fixable by editing prose, fixed before the check exists. The
four dead design-doc citations in `src/`; the four citations of a
request-store design document that exists nowhere, in `docs/STABILITY.md` and
`docs/workspace-layout.md`; the `koto query` and `koto session info` names in
`src/engine/respawn.rs` and the packaged skill, the second of which also touches
the byte-equality snapshot in `tests/respawn.rs`; the `hello-koto` walkthrough
in `docs/guides/custom-skill-authoring.md`, six dead path tokens across ten
sites; and the `CLAUDE.md` corrections, including the two tokens R3b's anchor
cannot reach.

The `hello-koto` repair has a fork, and this design settles it rather than
handing it to the PLAN: **rewrite the guide around `koto-adhoc`, which exists,
rather than authoring the missing example skill.** Authoring it would add shipped
plugin surface no requirement asks for, which then needs evals under
`check-evals-exist.sh`, a row in the skills table `CLAUDE.md` maintains by hand,
and maintenance forever — all so that a guide's citations resolve. The rewrite
touches one document and adds nothing to the plugin. Every change in this batch is correct whether or not the check ever lands, which
is what makes it reviewable on its own terms. It is not independent of the
check's design, though: what belongs in it was decided by running the rules, so a
later change to those rules can add to it.

**Batch 1 — the walk and command resolution.** `verbs()`, `surfaces()`, the
`Candidate` model, anchored extraction, command resolution, and the fixture
harness including R20's v0.12.0 fixture. The repository scan is `#[ignore]`d;
fixture tests run. This batch proves the load-bearing mechanism and the
acceptance fixture arrives with the mechanism it tests rather than after it.

**Batch 2 — path resolution.** R3, R3a, R3a-i, suffix stripping including
`::symbol`, the `read_dir` membership check and lexical normalization. Repository
scan still ignored.

**Batch 3 — the allowlist and the five message shapes.** `allow()`, the member
key, the witness digest, staleness in both directions, and the set-grew,
set-shrank, and mixed cases. Still no repository scan.

**Batch 4 — turn it on and record it.** Remove the `#[ignore]`, write the
records that survive Batch 0 — on the command side, measured, exactly two: a
promised record for `session rebind` against koto#215 and an intentional record
for `migrate` — capture the v0.12.0 run — `git worktree add` a
detached checkout of the tag, then
`KOTO_DOC_NAMES_ROOT=<that path> cargo test --test doc_names -- --nocapture`,
with the output committed alongside R16's pre-exception finding list — measure
R19, and note the check in `CLAUDE.md`.

R16's list is captured against the branch tree at Batch 4, as R16 says, and the
bar holds there. An earlier draft moved the capture to the merge-base on the
worry that Batch 0 would repair every genuine defect and leave a list of nothing
but correct-as-written tokens. The measurement refutes it: `session rebind`'s
sixteen sites are genuine and survive Batch 0 by construction, because the verb
is koto#215's to build and this change records it rather than fixing it. Against
R16's list is a set of `(file, token, classification)` entries, so the count is
in that unit rather than in sites: seven genuine entries against four correct as
written. Seven to four is a majority and not a comfortable one, which is worth
saying plainly — the bar is met, and one reclassification of a path token by a
reviewer would leave it at six to five and still met, while two would not.

## Security Considerations

The check reads files and starts no process, so the surface is what it reads and
what it does with path-shaped text it finds.

**Path traversal through a scanned document.** A candidate path is extracted
from document text and then stat'd, so a document containing
`` `../../../etc/passwd` `` is the case to close. Two mechanisms close it. R3a's
anchor is membership in `read_dir(root)`, and `..` is never an entry in a
directory listing, so a leading `..` fails candidacy outright. And resolution
normalizes the token *lexically* — rejecting any token whose segments include
`..`, and rejecting any absolute token — before joining. Lexical rather than
`fs::canonicalize`, deliberately: canonicalization fails on paths that do not
exist, which is every path this check exists to find, so a canonicalize-based
guard would reject exactly the cases it is meant to evaluate. The absolute-path
case needs its own rejection because Rust's `Path::join` discards the receiver
when the argument is absolute, so `root.join("/etc/passwd")` is `/etc/passwd`
rather than an error.

Two tokens in scope today contain `..` — `src/discover.rs` and the `koto-user`
SKILL — so this is a live path rather than a hypothetical one.

**Symlinks and TOCTOU.** The check only ever stats and never opens, so a symlink
pointing outside the tree leaks existence rather than content, and a
stat-then-nothing sequence has no window to exploit. `symlink_metadata` is used
rather than `metadata` so that a dangling symlink reports as present, matching
what a reader following the citation would find.

**The allowlist is trusted input, and should be.** It is a committed file
changed only through review. The two properties that matter are enforced anyway:
a promised record must carry a well-formed `owner/repo#N`, validated by shape
rather than by network lookup, and no record can suppress a finding without
appearing in the diff that added it.

**Denial of service through a pathological corpus.** The scanner is linear in
corpus bytes with no backtracking, and the surface list is fixed rather than
globbed from document content, so a crafted file cannot make it superlinear.

**No secret handling, no network, no subprocess.** The check has no credentials
to leak and starts nothing.

## Consequences

### Positive

- The verb set can never drift, because nothing maintains it. A subcommand added
  or removed changes what the check accepts with no accompanying edit, and
  koto#215 turns its own findings green.
- Placement costs nothing: no workflow, no job, no aggregator edit, and the
  `paths:` trap three existing workflows fell into cannot be fallen into.
- Reviewing this design against the tree found four dead design-doc citations in
  `src/`, four more in two published documents, and a wholly dead walkthrough in
  the skill-authoring guide, before a line of the check was written.
- A contributor runs one command they already run.

### Negative, and what is done about it

- **It departs from the house form.** Thirteen checkers in this workspace are
  shell scripts under `scripts/`, and someone looking there will not find this
  one. Mitigated by carrying every house convention across and by a pointer in
  `CLAUDE.md`; not fully mitigated, because the discovery problem is real.
- **A new top-level directory silently changes what the check flags,
  repository-wide.** R3a's anchor is `read_dir(root)` membership, so adding a
  directory named `templates/` would make every `templates/...` token in every
  guide a candidate at once, and removing one silently shrinks coverage. This is
  the anchor's mechanism working as designed, and it is also the least
  predictable thing about the check.
- **A documentation directory outside R5 is unchecked, and the fix is now a code
  change.** R5's list lives in compiled Rust rather than in a config file, so
  adding `docs/tutorials/` to the checked set requires editing and reviewing a
  test. That is a higher bar than editing a list, which is a cost, and a more
  visible one, which is not.
- **Token-level keying cannot express a per-file exception.** Nothing in the
  corpus needs it, and R16's committed list carries per-site visibility, but a
  future case needing "this one file may say it" will require a format change.
- **The witness digest over-fires.** Rewording a protected sentence retires its
  record, and CI fails on a documentation edit unrelated to the recorded name.
  Deliberate, and the message says so, but it is a recurring tax rather than a
  nuisance. A protected span moving between files reports as a removal and an
  addition together, which is accurate but is two messages for one move.
- **The doc-comment split has a false-negative class of its own.** A `///`
  comment on a clap field becomes help text a user reads, so an unbackticked verb
  there is user-facing and invisible to the check — `koto workflows publish
  --help` renders one today. Requiring backticks in doc comments is what removes
  the four wrapped-prose false positives, and this is what it costs. A doc
  comment that is help text and one that is developer prose are
  indistinguishable without reading the attribute above them, which is a
  different and larger mechanism.
- **Eight entries in the path set are parents that cannot be invoked alone.**
  `config`, `context`, `decisions`, `overrides`, `request`, `session`,
  `template`, and `workspace` all require a subcommand, and the check resolves a
  bare mention of any of them because clap's walk records them. Thirty-eight
  sites depend on that leniency, nearly all of them prose naming a command family
  rather than an invocation, so tightening it would trade a large number of false
  positives for a defect class nobody has seen.
- **`tests/` is outside R5, so the check does not read its own fixtures,
  allowlist, or header.** That is what allows a fixture to contain a phantom verb
  without the repository scan reporting it, and it also means nothing checks the
  names the check's own prose uses.
- **The check adds compile time to a file most contributors never open.**
  Measured at 6.5s incremental link and 0.00s run against a warm target
  directory, inside R19's ten seconds, but not free.
- **`CLAUDE.local.md`, shirabe, and drift by omission stay uncovered.** All three
  are recorded in the PRD's Known Limitations rather than solved, and this
  design does not change that.
