# Decision D4: the discoverability pointer

## Question

R14 requires a pointer to the read-only retrieval to reach an agent that has
lost its context, riding a channel present in every non-terminal response for
a phase that declares instructions. R6 requires that a phase declaring no
instructions produce byte-identical responses to today's. Which channel
carries the pointer, and how is it worded, so that both requirements hold at
once without costing back a meaningful fraction of what R1-R6's suppression
saves?

## Decision drivers

- R14: present on every non-terminal response for an instruction-bearing
  phase; reaches an agent with nothing retained.
- R6 / the Decisions entry "the discoverability pointer is scoped to phases
  that declare instructions": no pointer, no byte, on a phase with no
  instructions.
- R15: the pointer must not displace or truncate the phase's own directive.
- Token cost: the pointer rides on every applicable tick, including the
  gate-blocked repeat loop the PRD's motivating numbers describe (14
  consecutive ticks in the recorded run). A verbose pointer erodes the saving
  the whole feature exists to capture.
- Honesty about durability: the PRD's own "Known Limitations" section
  concedes the retrieval "is only as good as an agent's willingness to reach
  for it," and that there is no published evidence on whether agents notice a
  missing procedure versus confabulating one. Any option has to be judged
  against that admission, not against an assumption that a pointer solves the
  problem by existing.

## Considered options

### A. Reuse the existing directive-prefix splice

`NextResponse::with_directive_prefix` (`src/cli/next_types.rs:268-357`)
prepends a `prefix: &str` to the `directive` field only, leaving `details`
and every other field untouched. It is matched over all six `NextResponse`
variants and is a no-op on `Terminal` and `Error` — both are returned
unchanged because neither carries a `directive` field to prepend to
(`next_types.rs:230-232`, `354-355`). The five variants it does touch —
`EvidenceRequired`, `GateBlocked`, `Integration`, `IntegrationUnavailable`,
`ActionRequiresConfirmation` — are exactly the five R14's own acceptance
criterion enumerates as instructions-carrying. That is not a coincidence to
verify by hand each time; the match arms in `with_directive_prefix` and the
non-terminal branch of `with_substituted_directive` (`next_types.rs:159-251`)
are the same five, so the mechanism's coverage and R14's scope are the same
set by construction, not by an added check someone could forget to update
when a seventh variant is added later.

The mechanism already has a live caller: the leg-abandonment stop notice.
`discover_abandoned_leg` finds an abandoned leg, and the natural-advancement
path splices its notice in unconditionally when found
(`mod.rs:3374-3380`, mirrored on the directed-transition path at
`mod.rs:4204-4210`):

```rust
let resp = resp.with_substituted_directive(|d| {
    let d = crate::cli::vars::substitute_vars(d, &runtime_vars);
    variables.substitute(&d)
});
let resp = match &abandoned_leg {
    Some(a) => resp.with_directive_prefix(&a.directive_prefix()),
    None => resp,
};
```

**Ordering is load-bearing, and it is the same ordering the pointer would
need.** The splice runs strictly after `with_substituted_directive`. The
doc comment at `mod.rs:3361-3368` explains why: the substitution helper does
a sequential replace over a map, so koto-authored text spliced in before
substitution would itself be rescanned for `{{...}}` tokens by a later key.
A discoverability pointer is koto-authored text with no legitimate
`{{...}}` content of its own; splicing it before substitution would be a
latent injection surface (a template variable named to collide with the
pointer's wording) for no benefit, so it must follow the same
after-substitution ordering the abandonment notice already established.

**Byte cost, quantified against the existing precedent.** The abandonment
notice's prefix (`mod.rs:2733-2744`) is roughly 540 characters — it is
meant to seize attention once, at a rare, effectively-terminal event, and
its own doc comment frames it as retaining "the state directive below...
for context only," i.e. it is designed to dominate the read. A
discoverability pointer cannot reuse that register; it rides every
applicable tick, not a rare one. A pointer sized for that constraint —

> `(Lost this phase's instructions? \`koto phase-info <workflow>\` returns them, read-only.)\n\n`

— is about 95 characters, roughly a fifth of the abandonment notice's length
and, at a rough 4-characters-per-token estimate, on the order of 24 tokens.
Set against the PRD's own recorded run (a 7,140-character procedure
suppressed across 13 of 14 ticks, saving on the order of 90,000+ characters
over the sweep), a 95-character pointer on each of those 13 ticks adds back
roughly 1,235 characters — about 1.3% of what the suppression saved on that
run. That is the shape of cost the requirement can tolerate; a pointer
written at the abandonment notice's length (540 × 13 ≈ 7,000 characters)
would claw back roughly 8% of the saving on the same run, which starts to
matter. The mechanism does not force a length; the wording discipline does,
and it is worth stating as an explicit constraint in the implementation
task rather than trusting it to fall out of the reused function.

**Does it survive compaction?** No, and the honest answer matters here: a
pointer spliced into `directive` lives inside the same tool-result content
block as the instructions it is meant to help recover. The PRD's own problem
statement calls this content out by name as "compaction-eligible and not
guaranteed to survive a turn." Nothing about `with_directive_prefix` changes
that — the prefix is not a separate, more durable object; it is more bytes
in the same JSON string that gets discarded together if the platform
discards the tool result. A design that claimed this pointer "survives
compaction" would be wrong. What it actually does is something narrower and
still useful: because R14 requires it on *every* non-terminal response for
an instruction-bearing phase — not only the first delivery — the pointer is
re-asserted on the very next tick after whatever compacted it away, as long
as the agent keeps calling `koto next` at all. It does not need to survive
any single compaction event; it needs to be cheap enough to repeat
indefinitely, which is a different and weaker property than durability, and
one the existing research (`explore_inline-phase-details_r1_lead-external-progressive-disclosure.md`,
finding 5) found is exactly the tradeoff Claude Code's own harness makes
for its task-tool reminders — "re-inject based on a heuristic, accept the
repetition cost" — with users pushing back on the repetition being *too*
frequent, not reporting that a one-shot version would have served them
better. That is second-hand evidence the pattern gets noticed, not proof
this specific pointer will.

**Terminal and error variants.** `with_directive_prefix` is a no-op on both,
which matches R14's own text ("every non-terminal response") and the
acceptance criterion's explicit exclusion of the terminal and error variants
("carry no instructions field and are excluded"). There is no coverage gap
to patch here the way there is for the abandonment notice — that notice
needed a second channel (the `sibling()` envelope field, `mod.rs:2713-2719`)
specifically because an abandoned leg's audience needs the notice *at* a
terminal state too. A recovery pointer has no equivalent need: there is
nothing to recover once a workflow is terminal or has errored, so the gap
the abandonment notice had to work around does not exist for this pointer.

**Does it change any documented response contract?** Yes, but the change is
exactly the one R20 already mandates: `response-shapes.md`'s field-presence
table and its per-scenario JSON examples currently show `directive` without
a koto-authored prefix on the ordinary (non-abandonment) case. Adding the
pointer means every worked example in that document that shows an
instructions-bearing variant needs its `directive` string updated to include
it, and the document needs a short paragraph explaining the pointer the same
way it currently explains the abandonment notice. This is a documentation
update the PRD already requires (R20), not a new obligation introduced by
choosing option A specifically — option B would require the identical
documentation update for a new field instead of an amended one.

### B. A new structured sibling field (e.g. `recovery` or `hint`)

Add a field, `Option<serde_json::Value>` or similar, present only when the
current phase declares instructions, on the same five variants. Checked
against `docs/STABILITY.md`: the frozen-surface protocol it defines governs
the *state-file* wire format (`SessionBackend`, `EventPayload`,
`StateFileHeader`) consumed by bunki BK2, not the `koto next` stdout JSON
shape — `koto-stability-tests` has no reference to `NextResponse` or
`next_types` anywhere in its source. So R17 does not block adding a field to
the response envelope; the only governing contract is `response-shapes.md`
itself, which R20 already requires touching.

That removes the objection that would have been sharpest against B. What
remains is a comparison on the merits:

- **Coverage and R6.** Identical to A: add the field only on the five
  variants R14 names, omit it entirely (not `null`) when the phase declares
  no instructions, matching the pattern `details` already uses
  (`Option<String>`, key omitted when `None`, per
  `response-shapes.md`'s "not written as `null`" convention for `options`
  and similar fields). No advantage or disadvantage relative to A here — both
  can satisfy R6 by construction.
- **Byte cost.** A minimal structured form, `"recovery":"koto phase-info
  <workflow>"`, costs about 45 bytes of JSON scaffolding beyond the command
  string itself. That is smaller than option A's prose sentence in raw
  bytes. But raw bytes are not the only cost that matters here: the reader
  is an LLM-driven agent, not a machine parser with a fixed field to look
  for. A's pointer lands inside the one field the koto-user skill already
  tells the agent is authoritative (`next_types.rs:259-262`: "`directive` is
  the one field the agent-facing skill declares authoritative"), where the
  agent's attention is already trained to land on every tick. B's pointer
  lands in a field the agent has to have been separately told to check —
  which is exactly the same "did the agent retain the instruction that told
  it this channel exists" problem R14 exists to solve, now recursively
  applied to the pointer's own field name instead of to the phase's
  instructions. This is not a fatal flaw — the skill docs (R20) will
  document the field regardless — but it is a real, if second-order, cost
  A does not carry.
- **Durability under compaction.** No different from A. It is still bytes
  inside the same tool-result JSON object; nothing about being a named
  sibling field instead of a directive prefix changes what the platform's
  compaction boundary discards. Anyone arguing B is "more durable" than A
  because it is "structured" is not describing a real property of how
  compaction works.
- **Implementation cost.** Higher than A: five variant structs each need a
  new field, five `Serialize` impl arms need updating (`next_types.rs`
  currently hand-rolls `Serialize` per the field-presence table's
  variant-by-variant asymmetry — see `next_types.rs:360-538`), and the
  read-only retrieval's own response type would need to decide whether it
  also carries the field (it should not — R10 already makes the retrieval's
  entire response the answer to "what do I do," a self-referential recovery
  pointer inside the recovery response is noise). A reuses a function that
  already exists, is already tested via the abandonment-notice call sites,
  and needs no new field, no new `Serialize` arm, and no schema decision
  about whether the field is a bare string or a structured object.

Net: B is not blocked by any hard requirement, but it costs more to build,
costs more attention from the reader it is written for, and buys no
durability advantage over A. It would be the right choice only if a
non-agent consumer needed to key off the pointer's presence programmatically
without parsing prose — no requirement calls for that, and the PRD's stated
audience throughout (R7-R14, the user stories) is the agent itself.

### C. Carry it in `expects`

Disqualified on inspection, not on preference. `expects` already has a
defined, narrower meaning — the evidence schema the current state accepts,
which R9 separately requires the retrieval to return for the same reason.
Per `response-shapes.md`'s field-presence table, `expects` is documented as
**always `null`** on the `gate_blocked` variant. `GateBlocked` is precisely
the variant the PRD's own motivating example centers on — the 14-tick
gate-blocked loop that sat with instructions suppressed throughout. Routing
the pointer through `expects` would mean it cannot reach the one response
shape it is needed most on without first changing what `expects: null` means
for that variant, which is a larger and more confusing change to an already
load-bearing, separately-documented field than either A or B. No further
analysis is needed; this option fails before reaching the cost questions the
others were judged on.

### D. No pointer — rely on the koto-user skill's standing instructions

This deserves a genuine hearing rather than a quick dismissal, because the
underlying intuition is sound: skill content lives in the system prompt
region loaded at session start (per
`explore_inline-phase-details_r1_lead-external-progressive-disclosure.md`,
finding 1), not inside a tool result, and Anthropic's compaction contract
treats those as different classes of content — compaction drops content
*prior to* the compaction boundary and the developer-facing tooling
(`pause_after_compaction`) exists specifically to let developers re-inject
what they know must survive; system-level instructions are a more common
target for that kind of protection than an arbitrary tool result. If the
koto-user `SKILL.md` said, in prose, "if you ever find yourself without a
phase's instructions, call `koto phase-info <workflow>`," a compacted agent
that still has its skill content loaded would have everything it needs
without any per-response pointer at all — and this sidesteps the R6 scoping
puzzle entirely, since a skill-level instruction is not a response byte and
cannot make a no-instructions phase's response non-byte-identical.

It fails as the *exclusive* answer for two independent reasons, and both are
sharp enough that D cannot substitute for a response-carried channel:

1. **It does not satisfy R14 as written.** R14 requires the pointer to ride
   "a channel present in every non-terminal response... for a phase that
   declares instructions." Standing skill documentation is not a response
   channel; it is a separate context source the requirement does not name
   and, per the PRD's framing throughout ("where the retrieval lives... is
   the DESIGN's decision," never "whether R14 holds is the DESIGN's
   decision"), is not this design's to override. Recommending D-only would
   be recommending against a numbered requirement, which is a scope
   objection to raise explicitly, not something to resolve unilaterally
   inside a decision write-up.
2. **The failure R14 targets is exactly the failure that also removes the
   skill.** The PRD names three routes into the same hole: context
   compaction, a cold-restart respawn, and — the one D does not cover at
   all — an agent that never loaded `koto-user` in the first place, because
   it is a different orchestrator, a differently-configured agent identity,
   or a human operator scripting against the CLI directly. `lead-external`'s
   own research flags this precisely: third-party "Governance Decay"
   findings and a live opencode issue both report standing in-context
   instructions getting dropped by compaction specifically because
   compaction "treats standing policies as low-salience content" — so even
   granting D's premise that skill content is *more* durable than a tool
   result, "more durable" is not "durable," and the PRD's own "Known
   Limitations" section already concedes the retrieval's whole value rests
   on an agent's *willingness* to reach for it, which a never-loaded skill
   cannot supply by definition.

D is not wasted, though — R20 and R21 already require `koto-user` and
`koto-author` to document the retrieval's existence and contract as shipped.
That documentation should say what D proposes, in addition to, not instead
of, the response-level pointer: an agent that has both the skill loaded and
the per-tick pointer gets two independent chances to notice the retrieval
exists; an agent missing one still has the other. Treating D as a required
complement rather than a competing option is the correct scope for it.

### E. Other channels considered and set aside

- **A dedicated `warnings` or `notices` array** distinct from `recovery`,
  generalizing beyond this one pointer. Rejected for the same reason as B's
  implementation cost, amplified: it invents a new general-purpose surface
  for a PRD that needs exactly one specific pointer, and nothing in the
  requirements calls for a general notices channel.
- **Exit-code or stderr signaling.** Considered and dropped immediately —
  `koto next`'s JSON goes to stdout and its consumers (the koto-user skill,
  scripting callers) are not documented to read stderr for anything but
  human-facing warnings (`mod.rs:2925-2933`'s config warnings are the
  existing precedent, and those are explicitly *not* part of the machine
  contract). An agent recovering from context loss has no standing reason
  to inspect stderr any more than it does `expects`.

## Recommendation

**Option A: reuse `with_directive_prefix`, applied unconditionally whenever
the current phase declares instructions, spliced after variable
substitution exactly as the abandonment notice is, with the pointer text
held to roughly one short sentence (target: under 150 characters).**
Combine it with option D as a required complement, not a substitute — the
skill documentation R20/R21 already mandate should also state the recovery
call exists, so an agent that still has its skill content loaded gets a
second, independent notice beyond the per-tick one.

This is the cheapest correct answer available: the mechanism already
exists, is already exercised in production for a structurally identical
problem (koto-authored text that must reach the agent through the one field
it treats as authoritative, on every applicable tick, without touching
`details` or the variants that carry no directive), and its coverage of the
five instructions-bearing variants is the same set R14 names by
construction rather than by a parallel check that could drift. The one
discipline it requires that the existing precedent does not — keeping the
pointer short — is a wording constraint on the implementation, not a gap in
the mechanism, and the PRD's own recorded numbers show that discipline is
easily met: a sub-100-character pointer costs on the order of 1% of what a
14-tick suppression run saves, while a pointer written at the abandonment
notice's length would cost closer to 8%.

## Case against the recommendation

The sharpest attack is the one the task brief poses directly: **does adding
this pointer actually change agent behavior, or is it a gesture that makes
the design look complete without being tested?** The honest answer is that
no direct evidence exists either way for this specific case. The PRD's own
"Known Limitations" section says so explicitly — there is no published data
on whether agents notice a missing procedure and reach for a stated recovery
path, versus confabulating a plausible one instead. What evidence does
exist is indirect and mixed:

- **In favor:** the repeat-every-tick pattern is the one comparable systems
  research actually found in production (Claude Code's own task-tool
  reminders), and the public friction on that pattern is that it fires *too
  often*, not that a single delivery would have sufficed — which is weak but
  real evidence that repeated short nudges get read rather than tuned out
  entirely. It's also the only design compatible with the fact that the
  pointer's payload itself is compaction-eligible: since it cannot be made
  durable, making it cheap and repeated is the only lever left, and A is the
  cheapest way to pull that lever.
- **Against:** nothing in this repository or the cited external research
  measures whether an LLM agent, mid-task and not specifically looking for
  it, actually reads a one-sentence prefix on a field it may already treat
  as "the directive I've seen before, skip to the interesting part" —
  especially once the pointer itself becomes routine background text that
  appears on every tick. There's a real risk the pointer becomes exactly as
  invisible through repetition as the phase-visit heuristic it's compensating
  for; frequency defeats staleness but can also induce the same kind of
  banner-blindness reminder fatigue that `anthropics/claude-code#26038`
  documents users complaining about for a *different* repeated nudge.

Given that uncertainty, A survives the attack not because it is proven
effective, but because it is the design that costs the least — in tokens,
in implementation surface, and in new documented contract — for a
requirement (R14) that is already written into the PRD regardless of this
design's opinion on its ultimate efficacy. If the PRD's numbered
requirement is wrong to want a pointer at all, that is a case to make
against R14 itself, upstream of this decision, not a reason to pick a more
expensive channel for the same unproven bet. Post-launch, the same kind of
transcript-level verification the phase 2 recovery-contract exploration
already used to validate side-effect-freedom (R11) is the right instrument
to actually answer the effectiveness question — not something this design
can resolve on paper.

## Consequences

- `with_directive_prefix` gains a second caller. Its existing doc comment
  and ordering guarantees (splice after `with_substituted_directive`) need
  no change; the new call site follows the same pattern as
  `mod.rs:3374-3380` and `mod.rs:4204-4210`.
- The pointer text is a new piece of koto-authored prose that needs to be
  written once, kept short by explicit review (not by the mechanism), and
  referenced by name from `response-shapes.md` (R20) alongside the existing
  abandonment-notice paragraph.
- `koto-user/SKILL.md` and `koto-author`'s references gain the D-style
  standing-instruction sentence as a required complement (R20/R21), not as
  an alternative implementation path.
- Because the directed-transition path already calls
  `with_substituted_directive` and, when applicable,
  `with_directive_prefix` in the same order as the natural-advancement path
  (`mod.rs:4199-4210`), the pointer splice needs no special-casing between
  the two paths — it inherits R4's "one rule, not two" property for free.
- No change to `docs/STABILITY.md`, `koto-stability-tests`, or the
  state-file schema version: the pointer lives entirely in the `koto next`
  stdout JSON, which that contract does not govern.

## Open questions for cross-validation

- **Ordering with the abandonment notice.** If a session is both bound to an
  abandoned leg and sitting on an instructions-bearing phase, two prefixes
  now compete for the front of `directive`: the abandonment notice (which
  explicitly wants to dominate — "the state directive below is retained for
  context only") and the new discoverability pointer. Splicing the recovery
  pointer *before* the abandonment notice would bury a stop instruction
  under a routine nudge; splicing it *after* would put it inside the "for
  context only" zone the abandonment notice already tells the agent to
  deprioritize. This needs an explicit ordering decision (likely: recovery
  pointer never precedes an abandonment notice) and a test case, and it
  belongs wherever the emission-order decision for repeated splices gets
  made.
- **Prepend vs. append.** `with_directive_prefix` only prepends. The
  abandonment notice wants that — it is meant to seize attention. A routine
  discoverability pointer arguably wants the opposite: the phase's own
  directive should keep primacy on every ordinary tick, with the pointer
  reading as a footnote rather than a preamble. Consider whether this
  pointer should instead ride a small new `with_directive_suffix`, or
  whether prefixing is acceptable given the pointer's short, fixed length.
  This is worth resolving before implementation; it does not change the
  channel decision (A) either way.
- **Exact pointer wording is not fixed here.** This report sizes a target
  length and gives one candidate sentence; the literal string is copy the
  design or plan phase should own, ideally reviewed against R15 (must not
  displace or truncate `directive`) and against whatever the retrieval
  command is ultimately named (a decision this report treats as already
  settled elsewhere in the design, per the PRD's note that naming was
  costed separately in exploration).
