# Decision 3: the baseline fixture's embedded prose

## Question

`tests/next_response_baseline.rs` generates a document that is compared as
whole-string equality against
`tests/fixtures/next-response-baseline/instruction-free.json`. The document
embeds the test file's `NOTES` array and each sequence's `label` and
`description`. Some of that embedded prose states the delivery boundary this
change moves. R18 says no committed file may assert the old rule; the fixture's
whole point is that it does not change. What happens to those strings?

## The strings at issue

`capture()` (`tests/next_response_baseline.rs:519-522`) builds the document from
exactly three prose sources: `NOTES`, `seq.label`, and `seq.description`. The
module doc comment (lines 2-29) and the panic message (lines 546-568) are *not*
embedded, so they are ordinary source prose with no fixture coupling. Three
embedded strings are in scope, and only two of them assert anything.

**1. The self-transition sequence description — asserts the old rule.**

```
tests/next_response_baseline.rs:362
        description: "`implement` transitions to itself, ending one occupancy and beginning another.",
```
```
tests/fixtures/next-response-baseline/instruction-free.json:84
      "description": "`implement` transitions to itself, ending one occupancy and beginning another.",
```

This is the load-bearing one. In the vocabulary every shipped document uses,
occupancy *is* the delivery window: `plugins/koto-skills/skills/koto-user/references/response-shapes.md:38`
says instructions are "delivered once per **occupancy** of that phase", and
`docs/guides/cli-usage.md:82` says the same. So "a self-transition ends one
occupancy and begins another" reads, today, as "a self-transition re-delivers" —
which is the exact sentence R18 forbids. It is also lifted almost verbatim from
`docs/prds/PRD-inline-phase-details.md:142` ("A self-transition therefore ends one
occupancy and begins another"), the passage this PRD reverses.

Worse, the new PRD retires the word. Its Definitions
(`docs/prds/PRD-self-loop-suppresses-details.md:105-128`) open by saying "the
shipped documents use one word, 'occupancy', for two things this change
separates", and split it into **delivery window** (moves) and **epoch** (does
not). A description written in the retired vocabulary is stale twice over.

**2. The equality note — its facts survive, its reasoning does not.**

```
tests/next_response_baseline.rs:269
    "Several recorded bodies are identical to each other -- init and rewind arrivals, the conditional and unconditional and self-transition arrivals, the non-advancing repeat and its `--full` counterpart. That is not redundancy to be tidied away. The equality across paths is exactly what a delivery rule applied to one construction site and not the other would break.",
```
```
tests/fixtures/next-response-baseline/instruction-free.json:10
    "Several recorded bodies are identical to each other -- init and rewind arrivals, the conditional and unconditional and self-transition arrivals, the non-advancing repeat and its `--full` counterpart. That is not redundancy to be tidied away. The equality across paths is exactly what a delivery rule applied to one construction site and not the other would break."
```

The factual half stays true: R14 requires these instruction-free bodies to be
byte-identical after the change, so the self-transition arrival body still equals
the conditional and unconditional ones. What breaks is the final sentence's
argument — it says the equality across those paths is what a delivery rule
applied unevenly across construction sites would break. This change *deliberately*
applies the rule unevenly across those paths (the self-entry no longer opens a
window; the conditional and unconditional arrivals still do), and the fixture's
equality survives only because no template here declares instructions. A reader
who takes this note at face value would conclude the self-loop suppression is a
bug the baseline was built to catch. It does not state the old rule outright, but
it argues from it.

**3. The enumeration note and the label — grep hits, not assertions.**

```
tests/next_response_baseline.rs:266  /  instruction-free.json:7
    "Every call sequence the plan enumerates -- conditional-transition arrival, unconditional-transition arrival, directed transition, self-transition, rewind, the `--full` override, `koto init` plus the first tick, and a batch child's first tick -- is expressible in the template grammar and is recorded here. Nothing was omitted.",
```
```
tests/next_response_baseline.rs:361  /  instruction-free.json:85
        label: "self-transition-arrival",
```

Both name a call sequence. Neither says anything about instructions, delivery, or
windows, so neither is an R18 assertion — but both match the acceptance
criterion's grep at `docs/prds/PRD-self-loop-suppresses-details.md:389`, so a
reviewer will land on them and must be able to dismiss them. The label
additionally must *not* change: it is in the required-label list at
`tests/next_response_baseline.rs:587-601`, and the baseline acceptance criterion
(line 343) says "nothing else in the fixture may" change beyond `notes` and
`description`.

Two other descriptions are stale for the *previous* feature rather than this one
and are out of scope: line 342 ("This is the response the delivery rule will
later change" — the change has since shipped) and line 372 (a directed transition
into an occupied `implement`, which R5 now makes suppress). Neither asserts the
self-entry boundary. Leave them; touching them widens the fixture diff for no
R18 benefit.

## Options considered

### Option 1: Lockstep edit — change the `.rs` strings, mirror into the fixture

**What it is / mechanism.** Edit the two strings in
`tests/next_response_baseline.rs`, then make the byte-identical edit in the
fixture. Two sub-mechanisms, and they are not equivalent:

*1a, hand-edit the fixture.* The document is `serde_json::to_string_pretty` with
two-space indent (`capture()`, line 523) over a `serde_json::Map`, which is a
`BTreeMap` here — hence the alphabetised `description` / `label` / `responses`
key order visible in the fixture. Each note and each description occupies exactly
one line, so a hand edit is a literal line replacement. The only escaping
constraint is that the new prose must contain no `"` and no `\`; plain ASCII
prose with backticks and `--`, as all the existing strings use, is copied
verbatim. I verified this by applying both edits to a copy in
`/home/dgazineu/.claude/jobs/aa85aa99/tmp/edited.json` — the resulting diff was
exactly two lines, `-`/`+` on fixture lines 10 and 84, nothing else moved.

*1b, run the regeneration helper.* `regenerate_baseline_fixture`
(`tests/next_response_baseline.rs:681-689`) calls `capture()` and writes the
whole file. `capture()` (lines 445-526) re-runs every step of all thirteen
sequences against the freshly built `koto` binary and re-records every `stdout`.
So it regenerates *everything*: the intended prose, and also any response-body
drift, any added or dropped sequence, and any argv change. Nothing in the file
records which of those happened. A reviewer can tell an intended prose change
from an unintended output change only from the diff, never from the file.

Regeneration here is circular in a way worth naming: it takes a failing
byte-identity test and makes it pass by redefining the expectation. If the
implementation is correct, regeneration is a no-op on every `stdout` — which
means it buys nothing over the hand edit, while carrying the entire risk of
silently absorbing an incorrect one.

**Evidence.** The harness anticipated exactly this case. From the panic message,
`tests/next_response_baseline.rs:560-562`:

> "Note that the document also embeds this file's `NOTES` and the per-sequence
> `description` strings, so editing that prose trips this test too. A diff
> confined to those lines is the harmless case."

And on regeneration, lines 556-559:

> "Regeneration is legitimate in one case only: a deliberate change to the `koto
> next` response format that has nothing to do with this feature, made after it
> has shipped. Then run: cargo test --test next_response_baseline -- --ignored
> --nocapture"

That gate is not met here. This *is* the feature, and the change is to prose, not
to the response format. The doc comment on the helper (lines 669-680) repeats it:
"While the feature is being built, a failing baseline is the finding, and
rewriting the fixture destroys the pre-change record the comparison depends on."
So option 1 is only legitimate in its 1a form; 1b is explicitly ruled out by the
harness's own instructions.

**Verification command.** Primary — prove no recorded response changed,
independent of line formatting:

```
git show origin/main:tests/fixtures/next-response-baseline/instruction-free.json \
  > /tmp/base.json
jq -S '[.sequences[] | {label, responses}]' /tmp/base.json           > /tmp/base_bodies.json
jq -S '[.sequences[] | {label, responses}]' \
  tests/fixtures/next-response-baseline/instruction-free.json        > /tmp/head_bodies.json
diff /tmp/base_bodies.json /tmp/head_bodies.json && echo "bodies and labels unchanged"
```

This catches everything the criterion cares about and more: a changed `stdout`, a
changed `argv`, a dropped or added sequence, a renamed label, a reordering. I ran
it against the current worktree and it prints `bodies and labels unchanged`, so
it is known to work and to be green on an unmodified fixture.

The criterion's own literal form, as a secondary check:

```
git diff origin/main...HEAD -- tests/fixtures/next-response-baseline/instruction-free.json \
  | grep -E '^[-+]' | grep -vE '^(\+\+\+|---)' | grep '"stdout"'
```

Empty output passes. It is sound here only because pretty-printing puts each
`stdout` value entirely on one line (newlines are escaped as `\n`), so "a changed
line inside a `stdout` value" is exactly "a changed line containing `"stdout"`".
Then `cargo test --test next_response_baseline` for the byte-identity check
itself.

**Consequences.** The two stale strings are gone from both files. The fixture
diff is two lines. No `stdout` moves. Satisfies the acceptance criterion at
`docs/prds/PRD-self-loop-suppresses-details.md:339-345` as written.

**How it fails.** In form 1a: a typo makes the two files disagree, and
`instructions_delivered_this_occupancy`— rather, the byte-identity test — fails
immediately with a first-differing-line number (lines 540-545) pointing straight
at the line. Fully detectable in CI, and self-correcting. In form 1b: a real
response-body regression is written into the fixture and the test goes green.
CI's `cargo test` will *not* catch it — the fixture is the expectation. The only
thing standing between that and a merge is the reviewer running the diff check
above. That asymmetry is the whole argument for 1a.

### Option 2: Leave both files alone, carve the fixture out of R18

**What it is / mechanism.** No edit. Add a sentence to the PRD or DESIGN saying
test scaffolding is outside R18's surface because a fixture describes a captured
artifact rather than documenting current behavior. Zero fixture diff, which is
the strongest possible form of "the baseline did not change".

**Evidence.** The PRD has already decided against this, twice and explicitly. The
Surfaces criterion (line 388) names "`tests/next_response_baseline.rs`'s notes and
sequence descriptions" in the list of files that state the shipped rule. Known
Limitations (lines 561-565) confronts the tension head on: "The baseline fixture
embeds prose from its generating test file, so correcting the one sentence in it
that states the old boundary requires editing two files in lockstep and produces
a diff in a fixture whose whole purpose is to have no diff. The acceptance
criterion is written to permit exactly that one change and no other." The
criterion at lines 341-343 then grants the permission in so many words.

**Verification command.** `git diff --exit-code origin/main...HEAD --
tests/fixtures/next-response-baseline/instruction-free.json` reports no change.

**Consequences.** Two committed files keep a sentence that, in the repo's own
published vocabulary, asserts the reversed rule. The acceptance grep at line 389
returns fixture:84 and `.rs`:362, and the reviewer has to talk themselves out of
a real hit — which trains them to dismiss future hits on the same pattern. It
also leaves a trap for the next person who touches the fixture: they will read
"ending one occupancy and beginning another" as current, and the note at
fixture:10 will tell them a rule applied unevenly across those paths is a bug.

**How it fails.** Silently and later. Nothing in CI fails; the failure is a future
reader acting on a false statement, or a future PR "fixing" the self-loop
suppression because the baseline's own notes say the equality across arrival
paths must not be broken by an uneven rule. The carve-out argument is also weak
on its merits: this fixture is unusually documentary — the `NOTES` exist
precisely "so a reader who opens it later knows what it is and what it
deliberately leaves out" (`tests/next_response_baseline.rs:259-260`). Prose
written to be read is documentation, wherever it lives.

### Option 3 (recommended): Reword to state topology only, hand-edited in lockstep

**What it is / mechanism.** Same mechanism as option 1a — hand-edit both files,
never regenerate — but choose wording that makes no claim about delivery,
windows, epochs, or occupancy at all. The description says what the sequence
*does* to the state machine; the note says what the sequence *is for* without
arguing from the rule.

Concretely, two replacements. `tests/next_response_baseline.rs:362` and
`instruction-free.json:84`:

```
old: "`implement` transitions to itself, ending one occupancy and beginning another."
new: "Evidence routes `implement` back to `implement`: the transition's recorded source and target are the same phase."
```

`tests/next_response_baseline.rs:269` and `instruction-free.json:10` — keep the
enumeration, which stays factually true under R14, and replace only the closing
argument:

```
old: "... That is not redundancy to be tidied away. The equality across paths is exactly what a delivery rule applied to one construction site and not the other would break."
new: "... That is not redundancy to be tidied away. These bodies are built at different sites in the response path, and the equality is what a change reaching one site and not the other would break."
```

Label and enumeration note (fixture lines 85 and 7) stay untouched.

**Evidence.** Same harness evidence as option 1 — the panic message's "A diff
confined to those lines is the harmless case" is the sanction, and the
regeneration paragraph is the prohibition on 1b. The additional evidence for
*this wording* is the PRD's Definitions block (lines 105-128), which retires
"occupancy" as ambiguous and splits it into delivery window and epoch. Any
rewording that keeps a boundary word inherits whichever meaning the DESIGN
eventually settles on. "Ending one epoch and beginning another" would in fact be
true under R15 — the epoch boundary does not move — but it commits the fixture to
a term the shipped documents do not yet use, and re-opens the same audit the next
time the vocabulary shifts. Topology-only wording is invariant under every
vocabulary the DESIGN might pick, which matters because this is the one file in
the repo where changing a sentence costs a fixture diff.

**Verification command.** Identical to option 1: the `jq` bodies-and-labels diff
above, then `cargo test --test next_response_baseline`, then the criterion's
`"stdout"` grep. Additionally, confirm nothing is left to re-audit:

```
git grep -nE 'self.transition|occupancy' -- tests/next_response_baseline.rs \
  tests/fixtures/next-response-baseline/instruction-free.json
```

After the edit this returns only the enumeration note (`.rs`:266 /
fixture:7), the label (`.rs`:361 / fixture:85), and the surviving enumeration
inside note 10 — all of which name a call sequence and assert nothing. A reviewer
can clear all of them in one pass without judging the delivery rule.

**Consequences.** Nothing in either file asserts, or argues from, the old
boundary. Nothing in either file asserts the *new* boundary either, so the next
time the rule moves, this fixture is not on the list of files to change — which
is worth something in a file where every prose edit costs a two-file lockstep
edit. Two-line fixture diff, no `stdout` touched, criterion satisfied.

**How it fails.** The same two ways as option 1a, with the same detectability: a
mismatched hand edit fails the byte-identity test loudly with a line number; a
reviewer who skips the diff check would miss a smuggled body change, which the
`jq` command exists to prevent. One new, mild failure mode: topology-only prose
tells a future reader less about *why* the self-transition sequence is recorded.
The note at fixture:7, which lists it among the enumerated call sequences, and
the label itself already carry that, so the loss is small.

### Option 4: Stop embedding the prose in the generated document

**What it is / mechanism.** Change `capture()` to emit only `label`, `argv`, and
`stdout`, dropping `notes` and `description` from the document. The prose then
lives solely in `tests/next_response_baseline.rs`, editable at any time with no
fixture consequence, and the lockstep problem is gone permanently.

**Evidence.** This is a structural change to the fixture, so it can only be
produced by regeneration — the very thing lines 556-559 and 669-680 forbid during
this feature. It also deletes roughly 21 lines of the fixture and every
`description` line, which the acceptance criterion at line 343 prohibits outright:
"nothing else in the fixture may" change. And it defeats the stated purpose of
the notes, which `tests/next_response_baseline.rs:259-260` says exist "so a reader
who opens it later knows what it is and what it deliberately leaves out" — a
reader who opens the fixture is exactly the person who does not have the `.rs`
file in front of them.

**Verification command.** The `jq` bodies check would still pass (bodies
unchanged), which is precisely the problem: the check that matters most would go
green on a change that rewrites a third of the file.

**Consequences.** Solves the recurring cost at the price of the fixture's
self-description and a large diff in a no-diff file, inside the one PR least able
to afford it.

**How it fails.** It does not fail CI at all — `baseline_fixture_covers_every_required_sequence_and_stays_instruction_free`
(lines 576-667) reads only `label`, `stdout`, and `responses`, so it passes
unchanged. The failure is purely one of review burden and lost context. If this
change is wanted, it is a separate PR after this one ships, judged on its own.

### Option 5 (declined): add a new note recording the reversal

Adding a `NOTES` entry saying the delivery boundary moved under this PRD and that
these bodies are unchanged because the template declares no instructions is
permitted by the criterion (it is a `notes` change made in lockstep) and would
serve R19's "record that it was reversed" instinct. Declined: R19 is satisfied by
`docs/prds/PRD-inline-phase-details.md` and
`docs/designs/current/DESIGN-inline-phase-details.md` (criterion at lines
394-396), and every added sentence here is one more string that must be
re-audited on the next rule change, in the one file where prose edits are
expensive. Option 3's reworded note already removes the misleading inference
without adding surface.

## Recommendation

**Option 3.** Hand-edit both strings in both files, to wording that states
topology and nothing about the delivery boundary. Do not run
`regenerate_baseline_fixture`.

The choice between options 1 and 3 is only about wording, and topology-only wins
because the PRD is actively retiring the vocabulary — "occupancy" now means two
different things depending on which document you read, and a fixture that costs a
two-file lockstep edit should not be spending that cost again the next time the
DESIGN renames something. The choice against option 2 is already made by the
PRD, and made correctly: fixture:84 is not a borderline case. In the vocabulary of
`response-shapes.md:38` and `cli-usage.md:82`, "ends one occupancy and begins
another" is a statement that a self-transition re-delivers, which is exactly what
R18 names. Option 4 is a reasonable idea in the wrong PR.

Within option 3, the mechanism matters as much as the wording. Hand-edit, never
regenerate. Regeneration cannot improve on a hand edit when the implementation is
correct — every `stdout` comes back identical by R14 — and when the implementation
is wrong, it is the one action that converts the failure this fixture exists to
produce into a green test. The harness says as much in its own panic message.

I think the acceptance criterion is right as written, with one refinement worth
carrying into the DESIGN: state the verification as the `jq` bodies-and-labels
diff rather than as a grep for changed `"stdout"` lines. The grep is correct only
because of a formatting accident (one `stdout` per line), and it does not catch a
dropped sequence, a renamed label, or a changed `argv` — all of which
regeneration could introduce and all of which the criterion's own prose
("nothing else in the fixture may" change) already forbids. The `jq` form checks
what the criterion means rather than what its current phrasing happens to
describe, and I verified it runs green on the untouched fixture.

## Other golden files checked

I searched every fixture, golden, snapshot, and eval file in the repo for
statements of the delivery rule:

```
git grep -nIiE 'self.transition|self.entry|self.loop|occupanc|re.deliver' \
  -- tests/fixtures test koto-stability-tests benches
```

Four hits, all in `tests/fixtures/next-response-baseline/instruction-free.json`
(lines 7, 10, 84, 85) — the strings analysed above. A broader
`git grep -lIiE 'instructions|details' -- tests/fixtures test koto-stability-tests`
returns that same single file. Specifically clear:

- `tests/fixtures/native-workflows/enriched-shape.json` — the other golden
  (`tests/native_workflows_shape.rs`) — contains no delivery-rule prose.
- `test/functional/` and `koto-stability-tests/` — nothing.
- The plugin eval suites (`plugins/koto-skills/skills/*/evals/evals.json`) do
  state the rule in occupancy terms —
  `koto-user/evals/evals.json:134,136,140,147-155` are the non-advancing-tick and
  rewind evals R20 protects. They are committed files inside R18's surface, but
  they are eval scaffolding rather than golden files, and R20 governs them
  explicitly ("Rewording an explanation that appeals to the boundary this PRD
  moves is in scope; removing an assertion is not"). Out of this decision's
  scope; flagging so no one assumes the fixture sweep covered them.
  `koto-author/evals/evals.json:52,72,78` use "self-loop" to mean the polling
  template pattern, not delivery, and need no change.

So the baseline fixture is the only golden file affected, and no other test data
in the repo has to move.

## Summary

The fixture's `description` at line 84 ("ending one occupancy and beginning
another", mirrored at `tests/next_response_baseline.rs:362`) asserts the reversed
rule in the repo's own published vocabulary, and note 10 argues from it; both must
change, and the PRD's acceptance criterion already permits exactly that two-line
lockstep edit. Recommend rewording both to state topology only — no
occupancy, no window, no epoch — so the fixture survives the vocabulary split the
PRD's Definitions introduce and never has to be re-audited, and hand-editing the
fixture rather than running `regenerate_baseline_fixture`, which reruns the binary
and would silently absorb a real response-body regression into the expectation.
Verify with a `jq`-extracted diff of every label, argv, and stdout against
`origin/main` (verified green on the untouched fixture) plus `cargo test --test
next_response_baseline`; this fixture is the only golden file in the repo that
embeds the rule.
