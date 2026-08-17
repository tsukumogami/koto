# Lead: Exactly which sentences in the BRIEF, PRD and DESIGN encode the old occupancy definition, and what does this repo's documentation lifecycle say about changing a Done PRD and a Current DESIGN?

## Findings

### 1. Every passage that defines an occupancy, describes self-transition behaviour, or states the delivery rule

#### `docs/briefs/BRIEF-inline-phase-details.md` (status Done)

**The brief contains no occupancy definition and never uses the words
"occupancy", "self-transition" or "self-loop".** `grep -i` for all three over
the file returns nothing. Its delivery-rule statements are all
delivery-vs-entry framings that survive the ruling unchanged:

- Frontmatter `problem:` (lines 4-10): "koto decides whether to send a phase's
  long-form instructions by counting entries into a state rather than deliveries
  of its instructions, so it re-sends to an agent that is standing still and
  withholds from one told to redo a step."
- `## Problem Statement` (lines 40-43): "It makes that decision by counting how
  many times the workflow has *entered* the state. What it needs to know is how
  many times it has *delivered* that state's instructions to whoever is asking."
- `## User Outcome` (lines 84-87): "It receives the procedure when it reaches a
  phase for the first time, stops receiving it once it demonstrably has it, and
  gets it back -- reliably, and without moving the workflow -- whenever it no
  longer does."
- Journey "An agent stuck on a gate stops re-reading the same instructions"
  (lines 112-115): "After this feature, the second and every later blocked tick
  carries the directive and omits the procedure, because koto is tracking that
  it already delivered it rather than that the workflow moved."
- Journey "An agent told to redo a step is given the step's procedure" (line
  125): "After this feature, arriving at a phase by rewind delivers the
  procedure."
- Journey "A template author stops routing procedures through a separate file"
  (lines 149-151): "After this feature the author puts the procedure in the
  phase and stops maintaining a parallel file, because the delivery rule holds
  for as long as the loop runs."

None of these six is falsified by "a self-loop suppresses". The last one is the
only sentence that even brushes it, and "the delivery rule holds for as long as
the loop runs" reads as a reliability claim, not a per-arrival claim. **The
BRIEF needs no change.**

#### `docs/prds/PRD-inline-phase-details.md` (status Done)

This is where the old definition is normative. Four passages:

**(a) `## Requirements` -> `### Definitions`, lines 140-147 -- the definition
itself, verbatim:**

> **Occupancy.** A phase's occupancy begins when a state-entry event names that
> phase as its target, and ends when the next state-entry event names any phase,
> including the same one. A self-transition therefore ends one occupancy and
> begins another, which makes it behave exactly like a loop-back through other
> phases: the instructions are delivered again on arrival. This is the same
> answer the criteria already require for a loop-back, and treating a
> self-transition differently would make the rule depend on the shape of the
> loop rather than on whether the workflow re-entered the phase.

**(b) `### Functional -- the delivery rule`, R1 and R3, lines 159-169:**

> - **R1.** The decision to include a phase's instructions in a response is
>   keyed on whether koto has already delivered that phase's instructions to a
>   caller during the current occupancy of that phase, not on how many times the
>   workflow has entered it.
> - **R3.** The first response of a phase's occupancy carries that phase's
>   instructions, however the occupancy began: a conditional transition, an
>   unconditional transition, a directed transition, a self-transition, a
>   rewind, or workflow initialization at the initial state.

R1 survives the ruling untouched (it is entry-vs-delivery, not shape-of-loop).
**R3's enumeration is the load-bearing clause: it names "a self-transition"
explicitly in the list of occupancy-starting arrivals.**

**(c) `## Acceptance Criteria` -> `### The delivery rule`, two criteria:**

Lines 260-262:

> - [ ] A phase whose transition targets itself: the arrival response after the
>       self-transition carries the instructions, matching the loop-back case
>       above and the occupancy definition.

Lines 272-275:

> - [ ] Two consecutive directed transitions into the same phase -- reachable
>       only when the template declares a self-transition, since the directed
>       handler validates the target against the current phase's declared
>       transitions -- both carry the instructions, because each begins a new
>       occupancy.

Both criteria assert delivery on a self-arrival and both cite the occupancy
definition as their justification. These are the two checkboxes that flip.

(For completeness, the neighbouring criterion at lines 258-259 -- "after the
gate passes and the workflow later loops back to that phase, carries the
instructions again on the arrival response" -- is about a loop-back *through
other phases* and is unaffected.)

**(d) `## Status` prose is silent on this; the frontmatter `goals:` block
(lines 14-21) says only "arrive again when the agent is sent back to redo a
phase, and follow one rule across every path a workflow moves by" -- no
self-transition claim.**

#### `docs/designs/current/DESIGN-inline-phase-details.md` (status Current)

Five passages:

**(a) Frontmatter `rationale:`, lines 27-32:**

> Recording a delivery costs one event and makes rewind, self-transition,
> directed transition and multi-hop auto-advance all fall out without special
> cases, because the predicate keys on position relative to the last entry event
> and every one of those paths appends one.

**(b) `## Decision Outcome`, lines 217-222:**

> The predicate keys on position relative to the most recent entry event, and
> every way of arriving at a phase -- conditional transition, unconditional
> transition, directed transition, self-transition, rewind, initialization --
> appends one. So each of those starts a fresh occupancy with no special case in
> the predicate, which is precisely the uniformity R3 and R4 ask for and
> precisely what a visit count cannot give.

**(c) `## Decision Outcome` -> "A contradiction in the PRD was corrected",
lines 246-255 -- the most consequential paragraph, because it records a
deliberate upstream edit made *in the opposite direction* to the ruling:**

> Its Definitions made a self-transition begin a new occupancy -- so
> instructions must be delivered -- while an acceptance criterion required a
> second consecutive directed transition into the same phase to omit them. Those
> two are only jointly reachable when a template declares a self-transition,
> since the directed handler validates its target against the current phase's
> declared transitions (`src/cli/mod.rs:3304-3322`), and that path appends
> `DirectedTransition { from: X, to: X }`, which is a new occupancy by the PRD's
> own definition. The Definitions are normative and R3 is explicit, so the
> criterion was rewritten to test what it was plainly reaching for: a directed
> transition followed by a non-advancing tick.

The `## Status` prose (lines 47-50) points at the same event: "the
cross-validation surfaced a contradiction in the PRD's own acceptance criteria,
which was corrected upstream before this document was written."

**(d) `## Solution Architecture` component table, line 264** names the predicate
`instructions_delivered_this_occupancy(events, state) -> bool` -- the identifier
itself encodes the definition.

**(e) `## Implementation Approach`, Phase 1, lines 357-361** lists the unit
tests: "no prior delivery, a delivery in the current occupancy, a delivery
before the most recent entry event, a rewind entry, **a self-transition entry**,
and a multi-hop advance where the delivery belongs to an intermediate phase."

Also `## Security Considerations` line 404 ("bounded by occupancy count rather
than tick count") uses the term but does not depend on the self-transition
reading.

#### Two adjacent facts worth having on the table

**The pre-existing Done PRD says the opposite, and koto#90 AC 3 is quoting it.**
`docs/prds/PRD-koto-next-output-contract.md` (status Done) R9, lines 128-131:

> - **First visit** to a state: both `directive` and `details` are present.
> - **Subsequent visits** (retries, self-loops, polling): `directive` is
>   present, `details` is absent (omitted from JSON, not null). The caller
>   already received the full instructions on the first visit.

That is the exact wording of koto#90's AC 3. So the ruling does not invent a new
rule -- it restores the one an earlier Done PRD already stated, and the
inline-phase-details PRD's Definitions paragraph silently overturned it without
naming it. Both the BRIEF (lines 206-208) and the PRD (`motivating_context`,
lines 24-30) describe this work as *amending* R9: "This feature amends it rather
than replacing it" / "This PRD amends that requirement rather than introducing a
new one."

**Downstream committed docs that also encode the definition** (the callsites
lead owns these; listed so the scope of churn is visible):
`plugins/koto-skills/skills/koto-user/references/response-shapes.md` (lines 38-45,
107, 168-171, 550), `.../koto-user/references/command-reference.md:96`,
`.../koto-user/evals/evals.json` (lines 134-155),
`plugins/koto-skills/skills/koto-author/SKILL.md:67`,
`docs/guides/cli-usage.md` (lines 82, 117), `docs/reference/session-feed.md:683`.

---

### 2. The documented lifecycle for these artifact types

koto's own `CLAUDE.md` says nothing about doc lifecycle. The authority is
shirabe, wired into CI by two reusable workflows this repo calls at `@main`:

- `.github/workflows/validate-docs.yml` -> `tsukumogami/shirabe/.github/workflows/validate-docs.yml@main`
  (per-file, changed-files-only, `--diff-filter=ACMR`).
- `.github/workflows/lifecycle.yml` -> `tsukumogami/shirabe/.github/workflows/lifecycle.yml@main`,
  which runs `shirabe validate --lifecycle .` **against the whole tree**. Its own
  comment states the READY posture: "READY PRs run in ready posture and require
  single-pr chains to be at their terminal state (PLAN deleted, BRIEF/PRD Done,
  DESIGN Current)."

#### Statuses and required frontmatter (from `crates/shirabe-validate/src/formats.rs`)

| Type | schema | required fields | valid statuses |
|---|---|---|---|
| BRIEF | `brief/v1` | `status`, `problem`, `outcome` | Draft, Accepted, Done |
| PRD | `prd/v1` | `status`, `problem`, `goals` | Draft, Accepted, In Progress, Done |
| DESIGN | `design/v1` | `status`, `problem`, `decision`, `rationale` | Proposed, Accepted, Planned, Current, Superseded |

Structural checks that run on any changed doc carrying `schema:`: FC01 required
fields, FC02 status in enum, FC03 frontmatter status equals the *entire first
non-blank line* under `## Status`, FC04 all required sections present, FC15
sections in canonical order. DESIGN's nine required sections and their order are
fixed (`Status, Context and Problem Statement, Decision Drivers, Considered
Options, Decision Outcome, Solution Architecture, Implementation Approach,
Security Considerations, Consequences`).

#### Superseding a Current DESIGN -- the documented procedure

`skills/design/references/design-format.md` ("Lifecycle" section):

> | any -> Superseded | A successor DESIGN names this one as `superseded_by:` | None; the doc stays where it is |

**That table row is wrong about the directory.** The implementation
(`crates/shirabe-validate/src/transition.rs:444-463`) declares for Design:
`Moves { Current -> docs/designs/current, Superseded -> docs/designs/archive }`
and `ExtraInput::SupersededBy { required: true, ... }`. So the real procedure is:

```
shirabe transition docs/designs/current/DESIGN-x.md Superseded \
  --superseded-by docs/designs/current/DESIGN-successor.md
```

which (verified against the unit test at `transition.rs:2154-2200`):
1. sets `status: Superseded` in frontmatter,
2. adds `superseded_by: <path>`,
3. rewrites the body `## Status` first line to
   `Superseded by [DESIGN-successor.md](docs/.../DESIGN-successor.md)`,
4. `git mv`s the file to `docs/designs/archive/` (staged, not committed),
5. repoints inbound references tree-wide (`repoint::repoint_references`); a
   repoint failure fails the whole transition.

The DESIGN status rule is `Rule::MembershipOnly` -- there is no transition
graph, so `Current -> Superseded` needs no precondition beyond the required
`--superseded-by`.

#### Amending a Done PRD in place -- allowed, but undocumented and unrecorded

`skills/prd/references/prd-format.md` states the lifecycle
`Draft -> Accepted -> In Progress -> Done` and then:

> **No "Superseded" state.** If requirements change fundamentally, create a new
> PRD and mark the old one as Done (with a note that it was replaced).

There is **no "Edit Rules" section for PRD and none for DESIGN.** Only BRIEF has
one, and it is explicit:

> ### Edit Rules
> Accepted briefs can be edited in place. The framing a brief carries is durable
> but not frozen -- if the problem or outcome shifts materially before the
> downstream PRD lands, edit the brief and note the change in the Status section
> prose.

So the repo's written rules give: an explicit in-place-edit licence for BRIEF
with a place to record it (Status prose); a "make a new document" instruction
for a fundamental PRD requirements change; and nothing at all for a DESIGN short
of the Superseded machinery. No artifact type has a changelog, revision-history
or `amended:` frontmatter convention -- I grepped `skills/` and `references/`
for "amend" and the only hits are unrelated (`L13 amendment`, `R9 amendment`).
The transition tooling likewise has no "amend" verb.

PRD is also `Rule::MembershipOnly`, so `shirabe transition <prd> "In Progress"`
would mechanically succeed on a Done PRD -- but the whole-tree lifecycle check
would then fire L01, because a chain whose PLAN is absent is at-merge posture and
at-merge requires PRD Done. Reopening the PRD is therefore CI-blocked.

---

### 3. Precedent in this repo's history

**Superseding a Current DESIGN: one precedent, four files, done by hand.**
Commit `70ba97c` ("docs: audit and clean up docs/ (#169)") moved
`DESIGN-koto-engine.md`, `DESIGN-koto-cli-tooling.md`,
`DESIGN-koto-installation.md` and `DESIGN-koto-template-format.md` from
`docs/designs/current/` to `docs/designs/archive/`. Each diff is three lines:
`status:` set to `Superseded` and a new
`superseded_by: docs/designs/current/DESIGN-migrate-koto-go-to-rust.md`. No body
text was touched -- `DESIGN-koto-engine.md`'s `## Status` section still reads
`**Planned**` today, which is both stale and FC03-shaped-wrong. It survives only
because those four files carry no `schema:` field, so the validator's schema gate
skips them, and because `docs/designs/archive` is not in the lifecycle walk's
`ARTIFACT_DIRS` (`docs/briefs`, `docs/prds`, `docs/designs`,
`docs/designs/current`, `docs/plans`, `docs/roadmaps`; the walk is per-directory
`read_dir`, not recursive). Commit `16dbd34` (#195, "declare schema on 30
documents") deliberately did not touch the archive.

**Amending a Current DESIGN in place: precedent exists, and it is substantive
prose.** The same commit `70ba97c` rewrote
`docs/designs/current/DESIGN-koto-template-authoring-skill.md`'s
`## Security Considerations` -- replacing a paragraph claiming koto interpolates
`{{VARIABLE}}` into command-gate strings with one stating it does not, citing
`evaluate_command_gate` in `src/gate.rs` -- plus two supporting sentences
elsewhere. Status stayed `Current`, the file did not move, nothing was marked as
amended, and no successor doc was created. The record of the change lives
entirely in the PR and commit message: "A full audit of `docs/` against the
current code... Every fix was verified against `src/`."

**Amending a Done PRD in place: precedent exists, but never for requirement
prose.** The Done-PRD edits in this repo's history are: status transitions and
acceptance-checkbox ticks (`70ba97c` on `PRD-session-persistence-storage.md`,
`PRD-hierarchical-workflows.md`, `PRD-unified-koto-next.md`), and schema/heading
repairs (`16dbd34`). Commit `16dbd34`'s message states the discipline explicitly:

> Every changed document is content-identical to its previous version apart from
> the inserted `schema:` line and the heading casing. That is checked
> mechanically, by comparing each file's sorted case-folded token stream against
> `origin/main`.

**No precedent for a Done PRD whose requirement or acceptance-criterion text was
rewritten after shipping.**

**A live precedent for leaving a Current DESIGN stale.** PR #197
(`b7b0799`) changed `koto next`'s delivery rule from
`full || visit_count <= 1` to the delivery-record predicate, and did **not**
touch `docs/designs/current/DESIGN-koto-next-output-contract.md` (status
Current) or `docs/prds/PRD-koto-next-output-contract.md` (status Done), even
though both the new BRIEF and the new PRD say this work amends R9. That design's
"Decision 3: Visit count computation" still reads "The `details` field should be
included on first visit to a state and omitted on subsequent visits" and
describes `derive_visit_counts` feeding response construction (lines 121-153,
162, 213-215). So the repo already carries one Current DESIGN describing
delivery behaviour the code no longer has -- created by the very PR now under
correction.

---

### 4. Schema and validation: what would make an in-place amendment fail

Nothing in the schema blocks an in-place amendment of any of the three documents.
FC01/FC02 are frontmatter-only; FC03 compares the `## Status` first line (all
three are bare words today and would stay so); FC04/FC15 require the section set
and order, which an in-place edit preserves as long as no heading is added,
removed or reordered. There is no check on requirement numbering, on
acceptance-criterion checkbox state, or on consistency between a PRD and its
DESIGN. FC19 (orphaned `R<n>` citations) fires only for documents declaring
`absorbed:`, which none of these do.

I confirmed the current baseline is clean:

```
$ shirabe validate docs/designs/current/DESIGN-inline-phase-details.md \
    docs/prds/PRD-inline-phase-details.md docs/briefs/BRIEF-inline-phase-details.md
exit=0
$ shirabe validate --lifecycle .
exit=0
```

(local `shirabe v0.18.0`; CI builds from `@main`, which may be newer.)

**The supersession path, by contrast, has a live tooling conflict.** The body
line `shirabe transition ... Superseded` writes is not the bare status word, and
FC03 compares the whole first line. I reproduced it on a synthetic doc outside
the repo:

```
$ shirabe validate /tmp-scratch/DESIGN-fc03probe.md
::error ...::[FC03] frontmatter status "Superseded" does not match ## Status body
  "Superseded by [DESIGN-new.md](docs/designs/current/DESIGN-new.md)"
exit=2
```

The archived file is a rename, and `validate-docs.yml` diffs with
`--diff-filter=ACMR`, so the moved file *is* in the validated set. A supersession
performed with the documented tool on a `schema:`-carrying design therefore turns
`validate-docs` red unless the body line is hand-edited back to the bare word
`Superseded` (which then discards the successor link the tool wrote). shirabe's
own repo has never superseded a design -- `grep -rl "status: Superseded" docs/`
in shirabe returns nothing -- so this path has never been exercised on a schema'd
document anywhere in the workspace.

**Second-order effect of superseding, for whoever weighs the options.** Archiving
removes the design from the lifecycle index entirely (archive is not in
`ARTIFACT_DIRS`), so L01/L02/L07 stop applying to it. But a *successor* DESIGN
declaring `upstream: docs/prds/PRD-inline-phase-details.md` joins the chain, and
with no PLAN on disk the chain is at-merge posture, so the successor must be
`status: Current` in `docs/designs/current/` by the time the PR is ready -- which
is exactly what PR #197 did for the current design. That is mechanically doable
in one PR; it just means the successor is born Current rather than walking
Proposed -> Accepted -> Planned -> Current.

---

### 5. Exact frontmatter as it stands today

**`docs/briefs/BRIEF-inline-phase-details.md`** (lines 1-23): `schema: brief/v1`,
`status: Done`, `problem: |` (7 lines), `outcome: |` (5 lines),
`motivating_context: |` (7 lines). No `upstream:`, no `source_issue:`.

**`docs/prds/PRD-inline-phase-details.md`** (lines 1-31): `schema: prd/v1`,
`status: Done`, `problem: |` (10 lines), `goals: |` (8 lines),
`upstream: docs/briefs/BRIEF-inline-phase-details.md`, `source_issue: 90`,
`motivating_context: |` (7 lines).

**`docs/designs/current/DESIGN-inline-phase-details.md`** (lines 1-39):
`schema: design/v1`, `status: Current`,
`upstream: docs/prds/PRD-inline-phase-details.md`, `problem: |` (9 lines),
`decision: |` (13 lines), `rationale: |` (12 lines). No `spawned_from:`, no
`user_visible_surface:`, no `superseded_by:`.

Note the DESIGN's `decision:` and `rationale:` blocks both restate the predicate
("the suppression predicate becomes 'has a delivery been recorded since the most
recent entry into this phase'"; "makes rewind, self-transition, directed
transition and multi-hop auto-advance all fall out without special cases"), so a
frontmatter edit is in scope for any in-place amendment, not just body prose.

## Implications

The blast radius inside the durable artifacts is small and precisely located:
one definition paragraph and one clause of R3 in the PRD, two acceptance
criteria in the PRD, and four passages plus two frontmatter blocks in the
DESIGN. The BRIEF is untouched by the ruling.

The repo's written rules do not cover the case. BRIEF has an edit licence, PRD
has "write a new PRD if requirements change fundamentally", DESIGN has only the
Superseded machinery. But the repo's *practice* has one clean precedent for
exactly this shape of change -- `70ba97c` corrected a Current DESIGN's security
prose in place against the real code, kept the status, and recorded the change in
the PR message. Nothing structural distinguishes that from correcting the
occupancy definition here.

Supersession is the more expensive option on every axis: it requires authoring a
whole successor DESIGN (nine sections, jury-shaped) for a change that alters one
predicate clause, it moves the file out of the validated corpus, and the
documented tool that performs it produces a document that fails FC03 today. It
also buys the wrong thing: the design's four decisions -- record delivery as an
event, extend `koto status`, share a combinator, splice the pointer -- all remain
correct. Only the occupancy boundary moves.

The strongest argument for *not* leaving it alone is that #197 already created
one stale Current design (`DESIGN-koto-next-output-contract`) by declining to
touch it, and the dispatch brief's rule -- "Do not silently leave a durable
design doc describing behaviour the code no longer has" -- is aimed at exactly
that pattern repeating.

One point of care for an in-place amendment: the DESIGN's paragraph "A
contradiction in the PRD was corrected" is a *record of a decision that is now
being reversed*. Deleting it erases the audit trail; leaving it as-is makes the
document self-contradictory. It should be rewritten to say what actually
happened -- the contradiction was resolved toward delivery in #197, koto#90's AC
3 (and the older R9 it quotes) ruled the other way, and the definition was
changed to match.

## Surprises

1. **The BRIEF never defines an occupancy.** The term enters the chain at the
   PRD. That halves the amendment surface.
2. **The pre-existing Done PRD already said self-loops suppress.**
   `PRD-koto-next-output-contract.md` R9's "Subsequent visits (retries,
   self-loops, polling): `directive` is present, `details` is absent" is the
   literal source of koto#90's AC 3. The ruling restores an older written
   contract rather than overriding one.
3. **`shirabe transition ... Superseded` writes a body line that fails FC03.**
   Reproduced (exit 2). The documented supersession path is broken for any
   design carrying `schema:`, and has never been run on one anywhere in the
   workspace.
4. **design-format.md's lifecycle table is wrong about the directory move** --
   it says Superseded stays put; the implementation `git mv`s to
   `docs/designs/archive/`.
5. **The existing archive is itself inconsistent.** `DESIGN-koto-engine.md` has
   `status: Superseded` in frontmatter and `**Planned**` in its body Status
   section, invisible only because the archive is excluded from both the schema
   gate (no `schema:` field, skipped by #195) and the lifecycle walk.
6. **PR #197 left `DESIGN-koto-next-output-contract` (Current) describing the
   visit-count rule it replaced**, despite both new upstream docs saying this
   work amends it. The problem the brief warns about already exists once in the
   tree.

## Open Questions

- Does the fix also want `DESIGN-koto-next-output-contract`'s Decision 3 and
  `PRD-koto-next-output-contract`'s R9 corrected, or is that separate work? R9
  is the document the ruling vindicates, so it may need nothing; the design's
  Decision 3 describes a mechanism the code no longer uses at all.
- If the amendment lands in place, where does the "this was changed and why" note
  go? BRIEF's convention (Status-section prose) is the only precedent for
  recording an edit inside a document, and PRD/DESIGN have no equivalent
  convention. CHANGELOG.md plus the PR body is the alternative, matching how
  `70ba97c` recorded its in-place design correction.
- Human call: does correcting a normative Definitions paragraph and two
  acceptance criteria in a Done PRD count as "requirements change fundamentally"
  (prd-format's trigger for writing a new PRD)? My read is no -- R1, R2 and R4-R25
  are unchanged and R3 loses one item from an enumeration -- but the rule is
  written for a human to apply, not a validator.
- Should shirabe's FC03-vs-supersede conflict be filed upstream regardless of
  which option is chosen here? It is a real defect in a public repo's tooling.

## Summary

The old definition lives in exactly six places: the PRD's Definitions paragraph,
R3's enumeration, and two acceptance criteria, plus four passages and both
frontmatter blocks in the DESIGN -- the BRIEF never mentions occupancy or
self-transitions and needs no change. The repo's rules give no in-place-edit
licence for a Done PRD or a Current DESIGN, but its practice does: commit
`70ba97c` rewrote a Current design's security prose against the real code with no
status change and no successor, while supersession would demand a whole new
nine-section DESIGN and, worse, the documented `shirabe transition ... Superseded`
writes a `## Status` line that fails FC03 today (reproduced, exit 2) on any
design carrying `schema:`. The biggest open question is human: whether correcting
a normative Definitions paragraph in a Done PRD crosses prd-format's "requirements
change fundamentally, write a new PRD" line -- and where the record of the
amendment should live, since no PRD or DESIGN convention exists for noting one.
