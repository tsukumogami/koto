# /scope Handoff: inline-phase-details

## Provenance

Written by `/explore` on 2026-08-16 from
`wip/explore_inline-phase-details_crystallize.md`. Research files:
`wip/explore_inline-phase-details_findings.md`,
`wip/explore_inline-phase-details_decisions.md`, and
`wip/research/explore_inline-phase-details_r*_lead-*.md`.

Two discover-converge rounds, ten agents. Round 1 sent seven leads at what looked
like a greenfield feature request and came back with the opposite: five of the
seven independently found the feature already shipped. That reframing narrowed
the exploration from "design this" to "what is actually still broken", and round 2
ran three leads at the remaining gaps -- one of them an empirical build-and-run
lead that measured every suspected defect against a binary rather than inferring
it from source. The scope narrowed twice more along the way: batch-child retry was
ruled out as a break case, and three incidental bugs were pushed out of the
boundary.

## Problem Statement

koto ships a mechanism that sends a phase's full instructions to an agent on its
first visit and withholds them afterward, and the mechanism does close to the
opposite of what it was built for. It counts entries into a state when the
quantity that matters is deliveries of that state's instructions. Because a
blocked tick enters nothing, the instructions are re-sent on every retry --
forever, on exactly the repeat case the suppression exists to prevent. Because a
rewind *is* an entry, the instructions are withheld from an agent that has just
been told to redo the step. Underneath both sits a harder constraint: the session
log records nothing about who is attached to it, so no log-derived rule can
distinguish an agent that still holds the instructions from one whose context was
compacted or which respawned onto the predecessor's log. That makes the
suppression an optimization that needs a cheap, non-mutating recovery path to be
safe -- and koto has no such path today.

## Scope Boundary

### In scope

- The predicate that decides whether `details` ships: what it counts and where
  that is recorded, under R9's constraint.
- The blocked-re-tick defect: a `koto next` that evaluates gates, fails them, and
  does not transition re-sends `details` indefinitely. Measured.
- The rewind defect: `details` are suppressed on the state a rewind directs the
  agent back to. Measured.
- The `--to` path: `dispatch_next` applies no visit check at all, so the contract
  is not uniformly first-visit-only. Measured. Whether this is a defect or a
  deliberate carve-out is an open question for the chain, not a settled one.
- F1 cold-restart respawn: a zero-context subagent continues on the predecessor's
  log and inherits its visit count.
- A read-only recovery path that returns the current state's instructions without
  mutating the session -- issue #90's AC4, and the half its author called the
  higher-value one.
- Discoverability of that path: how an agent that has lost its instructions
  learns the recovery command exists.
- The mandatory downstream work koto's CLAUDE.md requires for changes under
  `src/cli/` and `src/engine/`: the `koto-author` and `koto-user` skills, their
  evals, and `docs/guides/cli-usage.md`.
- Rewriting issue #90's acceptance criteria, which describe a template format
  koto never had and criteria that have been met since March.

### Out of scope

- **`koto rewind` ping-pong.** Two consecutive rewinds move forward rather than
  back, so an early state becomes unreachable. A correctness bug in
  `handle_rewind`'s target selection, unrelated to `details`. Adjacent enough to
  flag, because a rewind-aware fix touches the same function.
- **`accepts:` does not gate advancement.** A transition without a `when:` clause
  fires unconditionally regardless of any `accepts` block; a four-state chain ran
  to terminal in one tick. A template-grammar and documentation problem.
- **Migration-scan stderr flood.** Every invocation against a populated `~/.koto`
  re-runs a migration scan emitting one skip line per legacy session. Very likely
  the same defect as open issue #193.
- **Retrofitting `<!-- details -->` onto existing templates.** The largest
  template in the workspace still uses a read-a-file-per-phase directive pattern
  and has never adopted the marker. That is adoption work in another repository,
  downstream of anything koto changes here.
- **Changing `derive_visit_counts` itself.** It is shared with
  `src/workflows_surface/project.rs` for an unrelated visited-set purpose, so its
  semantics are not the place to fix this. This is a constraint on the solution,
  not an excluded goal.
- **Auto-advance discarding crossed states** is deliberately *not* placed on
  either side. See Coverage Notes.

## Decisions Already Settled

From `wip/explore_inline-phase-details_decisions.md`:

- **The feature is not greenfield (D1).** PR #109 shipped the template marker,
  the response field, the visit-count derivation, and a `--full` override,
  codified as R9 of `PRD-koto-next-output-contract.md` (status Done). Issue #90's
  "Proposed Template Format" is fiction: `details` is delimited by an HTML comment
  marker inside a state's markdown body, not by a YAML key. Any requirement
  written downstream must correct this rather than inherit it.
- **The issue author's 2026-08-16 audit governs the remaining scope (D2)**, not
  the issue body. It is the most recent statement, the only empirical evidence
  that existed before this exploration, and it comes from the author.
- **The work proceeds rather than closing as superseded (D3).** Two acceptance
  criteria are demonstrably unmet.
- **It is one feature, not four bug fixes (D5).** Three of the defects share one
  root cause; the fourth is what makes any suppression rule safe.
- **The three incidental bugs are filed separately (D6).**
- **Two candidate fixes are eliminated on evidence.** Counting existing
  `GateEvaluated` events is incomplete -- an `accepts`-only state awaiting
  evidence writes nothing on a blocked re-tick. Suppressing on the `advanced`
  flag alone is wrong -- a true first visit via `koto init` and a blocked retry
  both report `advanced == false`.
- **`koto next --full` is not a viable recovery path.** Measured: it advanced a
  test workflow. It gates only whether `details` appears in the response and never
  touches the advance loop, so gate evaluation, `default_action` shell
  re-execution, auto-advance of routing states, and terminal session cleanup all
  still fire.

## Coverage Notes

Four things the exploration deliberately did not settle, each of which the chain
should:

- **Is the `--to` carve-out intentional?** No code comment or design doc says
  either way. A directed transition could reasonably be read as explicit operator
  intent that always deserves full context. If it is deliberate, there are four
  defects rather than five. This needs an author answer, not more research.
- **Does auto-advance's discarding of crossed states belong in this feature?** A
  `koto next` that advances through an intermediate state surfaces neither its
  `details` nor its `directive`. This predates the details mechanism and is
  broader than it, so folding it in silently would mis-scope a fix as "make
  details work on crossed states" when the honest framing is that auto-advance
  has never surfaced intermediate instructions at all. Left explicitly undecided.
- **What records a delivery, and where.** Three R9-compliant mechanisms were
  identified with costs but not ranked. See Shape Signals.
- **Two measurements were not taken.** `details` behavior under `koto next
  --with-data` combined with gate failure -- the accepts-fallthrough path at
  `src/cli/next.rs:56-69`, which also carries `details.clone()` and may have its
  own visit-count interaction. And whether `koto status`'s JSON output carries any
  stability expectation that would make new unconditional fields breaking;
  nothing documents one the way `docs/STABILITY.md` documents the Rust surface.

## Upstream Observations

`docs/prds/PRD-koto-next-output-contract.md` (status Done) is the requirement
this work amends. Its R9 is the source of both the shipped behavior and the "no
new state files or schema changes" constraint that shapes any fix.
`docs/designs/current/DESIGN-koto-next-output-contract.md` carries the matching
Decision 3 on visit-count computation, including a rejected alternative -- a
persisted counter file -- turned down on exactly that constraint. Reading R9
verbatim showed it forbids new state *files* and schema-version bumps, not new
events, which is what reopens the design space.

`docs/prds/PRD-native-workflows-phase-detail.md` and its BRIEF share the words
"phase detail" with this topic and are about something else: projecting a
session's structure into Claude Code's `/workflows` render. They matter here only
as the origin of the `phase` vocabulary and as evidence that the split from
`state` is deliberate rather than drift.

`docs/STABILITY.md` and `koto-stability-tests/` scope to the session-log wire
format, not the `koto next` response or the CLI subcommand surface, so neither
constrains this work.

No ROADMAP exists in this repository -- `docs/roadmaps/` is absent -- so no
`--upstream` flag accompanies the command below.

## Framing-Shift Answer

**Pre-supplied answer:** yes, the framing shifted -- substantially, and twice.

**Evidence:** The exploration began from issue #90's framing, which is that koto
lacks a way to deliver phase instructions inline and needs one built. Round 1
established that the mechanism shipped in PR #109 on 2026-03-30 and that the
problem is a defective predicate, not a missing capability (D1). That inverts the
problem statement from "build this" to "this is wrong in both directions". Round 2
shifted it again by measurement: the surviving hope that `koto next --full` could
serve as the recovery path died when the flag demonstrably advanced a test
workflow, which promoted the recovery command from a nice-to-have escape hatch to
the thing that makes any suppression rule safe at all. The success criterion moved
with it -- from "reduce token overhead" to "never withhold instructions from an
agent that does not have them, while not re-sending to one that does". The issue
author's own 2026-08-16 comment reflects the same shift, calling AC4 "the higher-
value half".

Note that this answer is `/explore`'s reading, recorded in an `--auto` run with no
live author turn. `/scope` should surface it for confirmation rather than adopt it.

## Shape Signals

### Architectural alternatives left open

- **What records that details were delivered.** Three R9-compliant options, none
  ranked. A new `EventPayload` variant -- precedented by six `request.`-family
  variants added without moving `CURRENT_SCHEMA_VERSION`, but it is new public
  surface on the event enum. An `EvidenceSubmitted` riding the reserved-kind
  convention in `src/engine/audit.rs` -- adds no variant and no schema bump, and
  reuses a pattern already used for synthetic pseudo-states, but overloads a
  record type that means something else. A new additive `StateFileHeader` field --
  precedented by `respawn_generation`, and the header was confirmed rewritable in
  place rather than append-only, but header state is a different durability story
  from event state.
- **Where the recovery read lives.** Extending `koto status` is the cheapest by a
  wide margin: `handle_status` already reads the events, parses the compiled
  template, and looks up the current state, so the text is one field away from a
  function that writes nothing; its only real question is a flag versus
  unconditional fields. A new `phase-info` subcommand has a comparable mechanical
  footprint but adds genuinely new documentation and skill sections. A
  `next --dry-run` is the most expensive and riskiest, because its cost is proving
  that every side channel inside a large, side-effect-heavy `handle_next` can be
  safely skipped rather than merely adding output.
- **What the recovery command is called.** The issue says `phase-info`. The
  codebase says `state`: 427 occurrences to 3 in `src/engine/`, 953 to 36 in
  `src/cli/`, and the skills and CLI guide use `state` throughout. `phase` is a
  deliberate, documented translation confined to `src/workflows_surface/` so the
  `/workflows` render matches Claude Code's own noun, which means a CLI
  `phase-info` would be the first leak of UI vocabulary into the state-speaking
  CLI. The author's most recent comment already softens to "`phase-info` (or
  `status --details`)".
- **Where the boundary-aware checks live.** `derive_visit_counts` is shared with
  `src/workflows_surface/project.rs` for an unrelated purpose, so rewind-awareness
  and respawn-awareness point at the call site in `src/cli/mod.rs` rather than at
  the shared function.
- **How an agent discovers the recovery command.** An agent that lost its
  instructions also lost any instruction naming the recovery path. `directive` is
  the one field guaranteed to reach an agent on every tick, and koto already
  injects koto-authored text into it -- the leg-abandonment stop notice does
  exactly this -- so the machinery exists. Whether that belongs in this feature or
  is layered on afterward is open.

### Complexity signals

- Contested trade-offs needing settlement rather than research: three recording
  mechanisms, three recovery surfaces, and a naming choice where the issue's
  wording and the codebase's convention disagree.
- Two defects share one root predicate and cannot be fixed independently without
  keeping both symptoms, which forces a sequencing constraint rather than allowing
  parallel fixes.
- The fix touches an already-Done PRD's requirement, so the requirements work
  includes amending a settled contract rather than writing a fresh one.
- The change lands in `src/cli/` and `src/engine/`, which triggers koto's
  mandatory skill-assessment rule and its eval requirements -- documentation and
  eval work is not optional cleanup here, it is part of the deliverable.
- Every claimed defect is reproduced with a transcript against a built binary, so
  acceptance criteria can be written against measured behavior rather than
  inferred behavior. That lowers risk on the verification side considerably.
