---
schema: design/v1
status: Proposed
problem: |
  Claude Code's `/workflows` list builds each row from the entry's top-level
  `status` (the icon), `workflowName` falling back to `summary` (the label,
  cut at 50 columns) and three number fields koto doesn't write. koto's
  blocked reading lives in `summary`, the steps and `koto.status`, none of
  which reach a row while `workflowName` is set, so a gate-blocked session's
  row and a running session's row are identical.
decision: |
  A blocked entry's `workflowName` becomes `blocked · <session label>`, built
  in `WorkflowFile::with_blocked_gate`, the one call that already applies the
  blocked summary, with the session label cut and the marker never cut. `name`
  keeps the unmarked session label for programs. Top-level `status` stays
  `running`. The retention sweep restores `workflowName` from `name` when it
  rewrites an entry as `killed`. Contract version 7. A new "list row"
  convention probes the row label's source on the pinned Claude Code release,
  and the pin moves to the release the guard and the manual check pass on.
  When Claude Code adds a blocked count, koto writes nothing further.
rationale: |
  The label is the only row part that takes text, and only a marker at its
  start survives the 50-column cut on any intent. Two of the three reasons the
  koto#553 design gave against a blocked title no longer hold, since that fix
  already keeps the bucket at running and draws the step Blocked; the third,
  a title that changes with state, is the cost, held down by keeping the
  label in `name` and putting it right after the marker. A reordered summary
  or a gate name in the marker would push the session label out of the row.
upstream: docs/prds/PRD-workflows-blocked-row.md
user_visible_surface: true
decision_provenance: inline-resolved
---

# DESIGN: a gate-blocked koto session's `/workflows` row reads blocked

## Status

Proposed

## Context and Problem Statement

koto renders a session in Claude Code's `/workflows` screen by writing
`koto-<uuid>.json` into the hosting session's workflows directory on every
event append. `try_materialize` in `src/workflows_surface/materialize.rs`
builds the file from the session's log: `WorkflowFile::new` in
`src/workflows_surface/contract.rs` writes the session label (from
`derive_display_name` in `src/workflows_surface/project.rs`: the intent, else
`<template> · <state>`, else a fallback) to both `name` and `workflowName`,
and `summary` as `koto · <workflow> · <state>`. When
`derive_enriched_projection` classifies the session as blocked (a
non-terminal session whose latest gate evaluation in its current state's
current epoch failed, read by `latest_epoch_gate`), it records the gate, and
`try_materialize` calls `WorkflowFile::with_blocked_gate`, which appends
`· blocked on gate <name>` to `summary`. The koto#553 fix
(`docs/designs/current/DESIGN-koto-workflows-blocked-status.md`) also writes
top-level `status: running` for a blocked session, so it isn't counted
completed, and draws its active step Blocked.

The list doesn't see any of that. Reading the code Claude Code 2.1.325 ships
(the same row code is in 2.1.322 and 2.1.323, and matches the koto#553
design's account of 2.1.314), a list row has exactly three parts:

- **Icon**, from the top-level `status` alone: running is a dim dot, paused a
  dim circle, completed a green tick, failed and killed a red cross, anything
  else nothing.
- **Label**, `workflowName`, else `summary`, else "Dynamic workflow". It's
  plain text with control characters stripped and whitespace collapsed, cut
  with an ellipsis at no more than 50 columns.
- **Facts**, dim, from `durationMs`, `agentCount` and `totalTokens`. koto writes
  none of them, so every koto row ends "0s".

The summary line above the rows still has two counts, "N running" and "N
completed", keyed only on `status`; every value but `running` counts
completed. Steps, `blocked`, `error`, and `summary` while `workflowName` is
set, are read only by the detail view. With a single entry the screen skips
the list and opens that entry's detail.

So the technical problem is narrow: put a reading that says blocked into the
one row part that takes text, the label, without touching `status`, and keep
it in step with the classification on every write. The requirements
(`docs/prds/PRD-workflows-blocked-row.md`) fix the shape of the answer:

- A blocked entry's row label starts with the word `blocked`, followed by the
  session label, so the marker sits inside the 50-column cut whatever the
  label's length (R1, R4, R5). A running entry's row label is the session
  label exactly as contract version 6 writes it (R2).
- Top-level `status` stays `running` (R3).
- The marker clears on the next write after a passing evaluation, a
  transition, a rewind, a re-entry with no new evaluation, a terminal state or
  a cancel (R6), and holds exactly when `koto.status` is `blocked` and the
  summary carries the blocked suffix (R7).
- The detail view keeps what koto#553 settled: the blocked summary suffix, the
  Blocked step, `koto.status` (R8).
- A program reading the file still finds the unmarked session label (R9), and
  an entry the retention sweep rewrites as `killed` doesn't read blocked (R10).
- This design says what koto writes once Claude Code adds a blocked count
  (R11), and re-weighs each reason the koto#553 design gave against a blocked
  title (R12).
- The text is sanitized (R13), the write adds no reads and stays best-effort
  (R14), the contract version goes up with the docs (R15), the guard fixture
  pins a blocked and a running golden file (R16), the convention guard probes
  the row label's source (R17), the verification guide has a two-session list
  check (R18), and nothing needs niwa (R19).

## Decision Drivers

- **The row has one text slot.** Only the label carries words; the icon is
  fixed by `status`, the facts column takes numbers.
- **The 50-column cut.** Anything after the first 50 columns of the label is
  lost, so the marker's position decides whether it shows.
- **The running count is fixed.** Any status word but `running` counts the
  session completed on every release checked.
- **One derivation.** The row marker, the summary suffix and the Blocked step
  must come from the same classification on the same write, so they can't
  disagree.
- **The session stays recognizable.** The operator needs to know which session
  a blocked row is, so the session label has to share the 50 columns with the
  marker.
- **Programs keep a stable answer.** A script that reads the label shouldn't
  have to strip a marker to get it.
- **No new reads, best-effort writes.** The materialize path runs on every
  event append.
- **Tool independence.** Nothing here may depend on niwa.

## Considered Options

### Decision 1: how koto marks the row

Resolved inline.

The question is which field carries the blocked marker, in what form, and
where the unmarked session label lives. The label is `workflowName` while it's
a string, else `summary`. koto controls both.

The koto#553 design turned down "say blocked in the title (`workflowName`)"
for three stated reasons, plus a closing remark, and each has to be re-weighed
for the list row:

1. *It changes nothing about the bucket unless the status also changes.* No
   longer holds as an objection. koto#553 already writes `running` for a
   blocked session, so the bucket is settled by the status, and the title
   isn't being asked to move it. The title's job here is only to make the row
   differ, which the bucket never could.
2. *It leaves the step Stopped or Running.* No longer holds. koto#553 draws the
   active step Blocked through its own step shape; a marked title adds to that
   and changes nothing about the step.
3. *It makes the title, the session's stable label from its intent, change
   with state.* Still holds, and it's the cost this design accepts. Three
   things keep it small: the unmarked label stays in `name`, which the screen
   doesn't read and programs can; the label follows the marker unchanged, so
   the operator still reads the intent; and the title already changes with
   state for every session without an intent, whose label is `<template> ·
   <state>`.
4. *"The summary carries the same word one screen deeper without any of
   that."* That's the gap this work exists for. One screen deeper is the
   detail view, and on 2.1.325 `summary` never reaches the row while
   `workflowName` is set.

Key assumptions:

- The row label chain on 2.1.325 (`workflowName`, then `summary`, then
  "Dynamic workflow") holds on the release the manual check runs against; the
  convention guard watches it (Decision 3).
- The detail view also titles the entry from `workflowName`, so it shows the
  marker too. Its summary already says blocked, so the two agree.
- Nothing in this repository reads `workflowName` back: the retention sweep,
  the dashboard and `try_materialize` don't.

#### Chosen: a `blocked · ` prefix on `workflowName`, the label kept in `name`

A blocked entry's `workflowName` is `blocked · <session label>`, the
separator being koto's existing ` · ` (`LABEL_SEP`). The marker doesn't name
the gate; the summary and the step do. `name` keeps the session label with no
marker, as it does today, so `name` and `workflowName` differ exactly on
blocked entries. A running entry's two fields are unchanged and equal.

The prefix is applied in `WorkflowFile::with_blocked_gate`, which
`try_materialize` already calls exactly when the projection recorded a
blocked gate, and which already rebuilds `summary`. One call now makes all of
the blocked reading the file carries, so the row, the summary and the step
can't drift apart. Since the whole file is rebuilt from the log on every
write, the prefix disappears on the first write after the classification
stops holding, with nothing to clean up.

The cap follows the summary's rule: the session label is sanitized and cut to
leave room for the prefix, and the prefix is never cut. On 2.1.325 a row
reads, for example, `⏺ blocked · Fix the login bug   0s`; a running one
reads `⏺ Fix the login bug   0s`.

#### Alternatives Considered

**Drop `workflowName` on a blocked entry, so the label falls back to a
reordered `summary`** (`blocked on gate <name> · koto · <workflow> ·
<state>`). It would name the gate on the row. Rejected because the summary
doesn't carry the intent, so the row would lose the session's own label, a
gate name of up to 128 characters would push the rest out of the 50 columns,
and the line under the detail view's title would change shape.

**A suffix on `workflowName`** (`<label> · blocked`). It keeps the label's
start stable. Rejected because any intent near 50 columns cuts the suffix off,
which is most real intents.

**A prefix that names the gate** (`blocked on <gate> · <label>`). It tells the
operator why from the list. Rejected because gate names run up to 128
characters, which would push the session label out of view; the gate is one
click away in the summary and the step.

**A different status word, such as `paused`.** It changes the icon. Rejected
because every word but `running` counts the session completed.

**A number in the facts column.** Rejected because no number says blocked, and
`agentCount` also drives the detail header's agent count.

**A new top-level field.** Rejected because Claude Code doesn't read it.

### Decision 2: what koto writes once Claude Code adds a blocked count

Resolved inline.

koto#590 tracks the missing blocked count in the list's summary line. It's
Claude Code's to add, and nobody knows yet what it will key on. The question
is what koto writes once a release has one.

Key assumptions:

- A blocked count, if one comes, keys on a top-level status word, the way the
  two existing counts do.

#### Chosen: nothing further; the marker stays

koto keeps writing `running` and the row marker. koto can't tell which Claude
Code release reads a file, and on every release checked a new word counts
completed, so writing one would put blocked sessions in the completed count
for everyone not on the newest release. The marker is right in both worlds:
with a blocked count, the row says blocked and the count agrees, and nothing
contradicts anything.

Adopting a new status word becomes its own change when such a release exists,
and that change needs two things first: a status-words probe in the
convention fixture showing the pinned release counts the word apart from
completed, and a recorded decision about the cost on older releases. This
work builds neither and doesn't close koto#590, which stays open.

#### Alternatives Considered

**Write a `blocked` status word now.** Rejected because it counts completed on
every current release.

**Drop the marker once a blocked count exists.** Rejected because koto can't
detect the release; older releases would lose the row reading.

**Write a second field for a future count now.** Rejected because Claude Code
hasn't named one, and a guessed field is a contract nobody reads.

### Decision 3: how the guard watches the row label's source

Resolved inline.

The convention guard (`scripts/check-claude-code-conventions.sh`, with probes
in `tests/fixtures/native-workflows/claude-code-conventions.json`) checks a
pinned Claude Code release for the names koto depends on. Its file-shape
probes prove the snapshot loader names `workflowName` and `summary`; they
don't prove the list row is labeled from `workflowName`. The pinned release is
2.1.316, while the row-model facts this design rests on were read on 2.1.322
through 2.1.325.

Key assumptions:

- The minified row model keeps a literal close to its label expression that a
  byte-window probe can anchor on, as the other conventions do.

#### Chosen: a "list row" convention, and the pin moves to the checked release

A new convention, slug `list-row`, with a probe whose token is the row
label's source expression (on 2.1.325, `workflowName??` inside the row model
`label:h.workflowName??h.summary??h.description`), anchored on a nearby
literal of the row model, and a same-length `--alter` demonstration that
renames the token so the run fails naming this convention. The implementer
picks the anchor by reading the pinned release, the way the file-shape
anchors were picked.

The pinned release in `tests/fixtures/native-workflows/claude-code-release`
moves to the release the implementation checks, at least 2.1.325, once the
whole guard passes on it. The manual list check (R18) names the same release,
so the guard and the reproduction rest on one version.

#### Alternatives Considered

**Add the probe to the file-shape convention.** Fewer conventions. Rejected
because the file-shape `--alter` demonstration would then no longer isolate a
row-label failure by name.

**Keep the pin at 2.1.316.** Rejected because the row code was read only on
2.1.322 through 2.1.325, and a probe written against those may not match
2.1.316's bytes.

**Rely on the manual check alone.** Rejected because a release that moves the
label source would then go unnoticed until someone looked.

## Decision Outcome

A gate-blocked session's entry gains one change a row can see: its
`workflowName` reads `blocked · <session label>`. Everything else about the
entry is as koto#553 left it. The top-level `status` is `running`, so the
session sits in the running count; `koto.status` is `blocked`; the summary
ends `· blocked on gate <name>`; the active step reads Blocked with
`gate <name>: FAIL`. `name` carries the session label unmarked, for programs.
`with_blocked_gate` writes the marker together with the summary suffix, so
the row, the summary and the step come from one classification on one write,
and the next write after the session stops being blocked has none of them.

On Claude Code 2.1.325 an operator with two sessions open sees `⏺ blocked ·
Fix the login bug   0s` and `⏺ Add retries   0s` under "2 running", and knows
which one to open without opening either. The marker sits at the start of the
label, so the 50-column cut takes the end of a long intent, never the marker.

The retention sweep, which rewrites an entry whose session went quiet as
`killed`, also sets its `workflowName` back to `name`, so a stale entry
counted completed doesn't claim to be waiting on a gate. The contract goes to
version 7, because `name` and `workflowName` no longer always match. When
Claude Code adds a blocked count, koto keeps writing exactly this; switching
to a new status word would be a separate decision with its own probe.

The guard gains two things: a running golden fixture beside the existing
blocked one, so both readings are pinned the same way, and a `list-row`
convention that fails if a Claude Code release stops labeling the row from
`workflowName`. The pinned release moves to the one the implementation checks.

### Rationale

The decisions lean on each other. The prefix works only because the label is
read from `workflowName`, which is why Decision 3 adds a probe for exactly
that and moves the pin to the release where it was read. Decision 2 can keep
the marker forever because the marker never contradicts a count, which is
also why the status word stays `running` in Decision 1. And putting the label
in `name` is what lets the sweep undo the marker without parsing it, and lets
programs ignore the screen's needs. The cost accepted is a title that reads
differently while blocked; the alternatives each lose either the marker or
the session label inside the row's 50 columns.

## Solution Architecture

### Overview

The change stays inside `src/workflows_surface/`, its tests and fixtures, the
convention guard's fixture, the verification script, and the docs. No engine
code changes: the blocked classification and the gate it names are read
exactly as today.

### Components

**`src/workflows_surface/contract.rs`**

- `CONTRACT_VERSION` goes from 6 to 7, with a history paragraph: version 7
  writes a blocked entry's `workflowName` as `blocked · <label>` while `name`
  keeps the label, so the two differ exactly on blocked entries.
- A constant `BLOCKED_TITLE_PREFIX: &str = "blocked \u{b7} "`.
- `WorkflowFile` keeps the unmarked label it was built with available to
  `with_blocked_gate` (`name` already holds it, sanitized and capped).
- `with_blocked_gate(gate)` rebuilds `workflow_name` in addition to
  `summary`: the raw session label is sanitized and cut to
  `FACT_CAP - prefix_len - CUT_MARKER_len` with `sanitize_capped`, then the
  prefix is put in front. The marker is never cut; the label is. A label that
  fits is written whole. To keep the raw label for this, `WorkflowFile::new`
  stores it in a non-serialized field, or `with_blocked_gate` takes the raw
  label as a second argument from `try_materialize`; the implementer picks
  whichever keeps the existing `new` callers unchanged.
- The doc comments on `name` and `workflow_name` say which carries the marker.

**`src/workflows_surface/materialize.rs`**

- No change to the call order: the existing `if let Some(gate) =
  enriched.blocked_gate` block is where the marker gets applied, through
  `with_blocked_gate`. If `with_blocked_gate` takes the raw label as an
  argument, this is where it's passed (`proj.display_name`).

**`src/workflows_surface/retention.rs`**

- `reconciled` sets `workflowName` to the entry's `name` when both are
  strings, so a swept entry has no marker. An entry whose `name` is missing
  keeps whatever `workflowName` it had. Everything else about the sweep
  stays.

**Tests and fixtures**

- `tests/native_workflows_blocked_test.rs`: the blocked assertions gain a
  `workflowName` check (starts with `blocked · `, then the session label) and
  a `name` check (the unmarked label); `assert_not_blocked` gains a check that
  `workflowName` equals `name` and doesn't start with the prefix, which every
  clearing test (passing re-evaluation, advance, rewind, re-entry without a
  new evaluation) already calls. New tests: a session with no intent (prefix
  then `<template> · <state>`), a 2,000-character intent with wide characters
  (the full prefix survives, total within the cap plus the cut marker), and a
  control character and ESC in the intent and the state name (escaped as the
  summary escapes them). A terminal state and a cancel after a failed gate
  each produce an unmarked label; add those two if the existing finished and
  cancel tests don't already cover a prior failed gate.
- `tests/native_workflows_shape.rs`: the canonical blocked entry's golden
  file, `tests/fixtures/native-workflows/enriched-shape.json`, is regenerated
  with the marked `workflowName` and `contractVersion` 7, and the pinning
  test asserts the exact `workflowName` and `name`. A new canonical running
  entry, built the same way without `with_blocked_gate`, gets its own golden
  file, `tests/fixtures/native-workflows/running-shape.json`, a comparison
  test, a pinning test (no marker, `workflowName == name`) and an ignored
  printer for regeneration.
- `tests/native_workflows_conventions.rs`: `SHAPES` lists the running golden
  too, so its keys and status are checked against the probes.
- `tests/fixtures/native-workflows/reconciled-shape.json` and its test pick up
  version 7; a new retention unit test passes a blocked entry through
  `reconciled` and checks the marker is gone, and a running one keeps its
  label.
- `tests/fixtures/native-workflows/claude-code-conventions.json`: the
  `list-row` convention (Decision 3). `scripts/claude_code_conventions.py`
  and the script's usage text list the new slug if they enumerate slugs.
- `tests/fixtures/native-workflows/claude-code-release`: the checked release.

**Scripts**

- `scripts/verify-native-workflows.sh`: AC3 asserts `workflowName` starts with
  `blocked · ` and `name` is the intent; after the gate passes (render #2) it
  asserts `workflowName` equals `name`.

**Docs**

- `docs/reference/session-feed.md` (the native file section): the row label,
  `name` versus `workflowName`, version 7, and the sweep restoring the label.
- `docs/guides/native-workflows-verification.md`: the version, what the verify
  script checks, and step 5 of the manual check rewritten as a two-session
  list check (R18), quoting the expected rows and summary line and naming the
  release checked.
- The koto-user skill's description of the entry, under
  `plugins/koto-skills/skills/koto-user/`.
- `CHANGELOG.md`: an Unreleased entry.
- `docs/designs/current/DESIGN-koto-workflows-blocked-status.md`: one sentence
  in the rejected-title alternative pointing to this design for the list row.

### Key Interfaces

- **The file contract, version 7.** `workflowName`: the row label. Equal to
  the session label, except on a blocked entry, where it is `blocked · ` plus
  the session label. `name`: the session label, always unmarked. `status`:
  unchanged (`running` for blocked). `koto.status`: unchanged. `summary`:
  unchanged.
- **`WorkflowFile::with_blocked_gate`**: the one place a blocked reading is
  applied to the file (summary suffix and title prefix).
- **The `list-row` convention**: one probe and one `--alter` demo in the
  convention fixture.

### Data Flow

An event append reaches `try_materialize`, which reads the log once (as
today), derives the enriched projection, and gets `blocked_gate` when the
latest gate in the current epoch failed. It builds the file, and when
`blocked_gate` is set, `with_blocked_gate` writes the summary suffix and the
title prefix together. The file is written atomically, then the retention
sweep runs, and `reconciled` restores `workflowName` from `name` on any entry
it marks `killed`. Claude Code reads the files when `/workflows` opens and
labels each row from `workflowName`.

## Implementation Approach

One pull request, in this order, each step a commit.

### Phase 1: The marker in the file

`CONTRACT_VERSION` 7, the prefix constant, `with_blocked_gate` writing the
title, the unit tests in `contract.rs` (prefix present, label after it, cap
cuts the label, escapes), and `materialize.rs` if the raw label is passed in.

Deliverables:
- `src/workflows_surface/contract.rs`
- `src/workflows_surface/materialize.rs` (only if the signature changes)

### Phase 2: The sweep

`reconciled` restores `workflowName` from `name`; its unit tests and the
reconciled golden at version 7.

Deliverables:
- `src/workflows_surface/retention.rs`
- `tests/fixtures/native-workflows/reconciled-shape.json`

### Phase 3: The guard fixture

The regenerated blocked golden, the new running golden with its tests and
printer, `SHAPES` updated, and the fail-then-pass assertions in the blocked
integration tests, including the new no-intent, long-intent, escape, terminal
and cancel cases.

Deliverables:
- `tests/native_workflows_shape.rs`, `tests/native_workflows_conventions.rs`
- `tests/fixtures/native-workflows/enriched-shape.json`,
  `tests/fixtures/native-workflows/running-shape.json`
- `tests/native_workflows_blocked_test.rs`

### Phase 4: The convention guard

The `list-row` convention and its `--alter` demo, checked with
`scripts/check-claude-code-conventions.sh` on the release the work checks,
and the pin moved to it once every convention passes there. If another
convention fails on the newer release, the pin stays and the PR says why.

Deliverables:
- `tests/fixtures/native-workflows/claude-code-conventions.json`
- `tests/fixtures/native-workflows/claude-code-release`
- `scripts/claude_code_conventions.py`, `scripts/check-claude-code-conventions.sh`
  (only if they enumerate slugs)

### Phase 5: The verify script and the docs

`scripts/verify-native-workflows.sh` gains the row assertions; the
session-feed reference, the verification guide (with the two-session list
check), the koto-user skill, the CHANGELOG, and the pointer in the koto#553
design.

Deliverables:
- `scripts/verify-native-workflows.sh`
- `docs/reference/session-feed.md`, `docs/guides/native-workflows-verification.md`,
  `plugins/koto-skills/skills/koto-user/` (the file describing the entry),
  `CHANGELOG.md`, `docs/designs/current/DESIGN-koto-workflows-blocked-status.md`

### What the pull request shows

The PR body records one run of `scripts/verify-native-workflows.sh` (a real
koto session whose gate fails, then passes) with the row assertions passing,
and the test commands it ran, one at a time. It says how a reader reproduces
the list check: the verification guide's two-session steps on the named
Claude Code release, with the rows and summary line they should see. Nobody in
the work starts Claude Code; the list check is the reader's. The PR refers to
koto#590 without a closing keyword.

## Security Considerations

**Untrusted text in another program's screen.** This change writes one more
piece of session-derived text, the marker plus the session label, into a file
that Claude Code reads and draws in a terminal. An intent is user or template
content and a state name comes from a template, so both are untrusted. The
label passes through `sanitize_capped`, the sanitizer the summary already
uses, which escapes control characters (ESC and 8-bit CSI included), bidi
overrides, zero-width and tag characters, and folds newlines, so an intent
can't inject terminal escapes or reorder the row. Claude Code strips control
characters from the label itself as well. The cut applies to the session
label and never to the prefix, and the tests cover a 2,000-character intent
with wide characters, control characters and ESC in the intent and the state
name, and the cap boundary.

**The marker is a convenience, not an authority.** Nothing stops an intent
that literally starts with the word "blocked", so a running row could read
like a blocked one. The authoritative reading stays `koto.status`, the summary
suffix and the Blocked step, which come from the same classification; the
session-feed reference says so.

**No new permissions, reads or data.** The write path reads nothing it didn't
read before, adds no network call, and the gate name stays out of the row.
The retention sweep copies `name`, a string koto already wrote and sanitized,
into `workflowName`.

**The convention guard and its pinned release.** The guard downloads a Claude
Code npm package koto doesn't control; the new `list-row` convention adds a
probe without changing how that package is handled. It is fetched with install
scripts disabled into a fresh temporary directory outside the checkout, from a
fixed package name and a `MAJOR.MINOR.PATCH` version; the helper runs under
`python3 -I`, extracts only the one expected member (size-capped, no links),
and searches its bytes without running, importing or decoding them, so a
hostile release can only make a probe pass or fail wrongly. A cached tarball
is reused only when its sha512 matches the registry's integrity value. The
remaining trust is in the registry and in the reviewer who moves the pin,
which is a version string with no recorded hash: the guard detects honest
format drift, not a malicious release. Moving the pin is reviewed as a
fixture change, and the PR names the release.

## Consequences

### Positive

- An operator finds the stuck session from the list, with no click.
- The blocked session stays in the running count, so the summary line stays
  truthful.
- The row, the summary and the step can't disagree: one call writes all
  three.
- Programs get a field (`name`) that never carries screen markup.
- A Claude Code release that moves the row label's source fails the guard by
  name instead of silently dropping the reading.

### Negative

- The title reads differently while blocked, in the list and in the detail
  view's header, which the koto#553 design wanted to avoid.
- `name` and `workflowName` no longer always match; a reader that used
  `workflowName` as the label sees the marker.
- The row doesn't name the failing gate; the operator opens the entry for
  that.
- The marker is English text. koto's other screen text is too, so this adds
  no new kind of localization debt, but it adds one more string.
- Moving the pinned release can surface unrelated convention drift that has
  to be dealt with in the same PR or deferred with a reason.

### Mitigations

- The label follows the marker unchanged, so the operator still reads the
  intent; the title already varies with state for sessions without an intent.
- The contract version goes to 7 and the session-feed reference names `name`
  as the label field, so readers can tell old from new entries and know where
  the stable label is.
- The gate is in the line under the detail view's title and in the step,
  one click away.
- If the newer release fails another convention, the pin stays and the
  `list-row` probe is checked by hand on the newer release, recorded in the PR.
