# Reviewer: content quality

## Verdict
PASS

The document names a real, measured defect rather than a solution, ties every concrete fact back to the exploration record, and holds the brief's altitude without smuggling requirements or interface decisions downstream work is meant to own.

## Per-rubric findings

### 1. Problem Statement: problem, not smuggled solution

PASS. The section diagnoses a defective decision rule and its consequences, and stops short of prescribing a fix. Its closest approach to a solution is: "It needs a way for an agent to ask for the instructions back, and koto does not have one" -- naming a need, not a command, a flag, or an interface shape. It correctly declines to name `phase-info`, `status --details`, or any other candidate, all of which the supporting research explicitly left as an open naming question.

It stands alone: a cold reader gets the mechanism ("koto decides per response whether to send them... by counting how many times the workflow has entered the state"), the divergence ("delivered" vs. "entered"), and both failure directions (blocked re-tick over-sends, rewind under-sends) without needing the roadmap or the issue. The one place it leans on existing internals -- naming `koto status` and `koto next --full` and what each does -- is diagnosis of why no existing command already closes the gap, not a proposed design; it earns its place because the problem itself is "an existing shipped mechanism is wrong," which requires citing what exists.

Quoted evidence: "So the withholding cannot be made correct on its own. It needs a way for an agent to ask for the instructions back, and koto does not have one."

### 2. User Outcome: outcome-shaped, matches frontmatter

PASS. The section states what changes for the agent ("can rely on having the current phase's procedure"), not what gets built, and closes each of the three problem-statement failures with an outcome rather than a mechanism: "An agent sitting on a blocked gate is not re-sent the same block of text on every tick." It names two distinct users whose experience changes -- the driving agent and the template author -- matching the format's requirement that an outcome name a receiver.

Body prose and frontmatter `outcome` carry the same content: both open on "has the current phase's procedure whenever it needs it," both close on the recovery call being state-inert. No divergence.

Quoted evidence: "An agent driving a koto workflow can rely on having the current phase's procedure. It receives the procedure when it reaches a phase for the first time, stops receiving it once it demonstrably has it, and gets it back... whenever it no longer does."

### 3. User Journeys: concrete and distinct

PASS. All four journeys lead with a `###` name heading and each names a concrete user (a coding agent; an operator or coordinator plus the agent it rewinds; an agent recovering from context loss; a template author), a trigger (gate fails and re-ticks; a rewind command; compaction or respawn; attaching a procedure and running the loop), and an outcome shape (directive-only on later blocked ticks; procedure delivered on rewind-arrival; recovery without state mutation; observed behavior matching documented behavior).

The four are genuinely different entry points -- three walk the mechanism from the executing agent's side under three distinct triggers, and the fourth walks it from the authoring side before any of the other three happen. None restates another with a different name.

Quoted evidence (journey 3, trigger and outcome together): "It makes a single read-only call keyed by the workflow name, receives the current phase's directive and procedure, and continues from exactly where the workflow was."

### 4. Scope Boundary: real in/out exclusions

PASS. Every OUT item is something a downstream author could plausibly have assumed was folded in, and each carries the specific reason it isn't: auto-advance's discarding of crossed phases is adjacent because it "predates this mechanism and is broader than it"; the rewind ping-pong bug is adjacent because "a rewind-aware change here touches the same function"; `accepts:` not gating is a distinct authoring-trap problem; the migration-scan flood is likely a pre-existing filed issue; template retrofitting is named as downstream adoption work in other repos; and changing the shared visit-count derivation is explicitly framed as "a constraint on the solution rather than a target of it" rather than a vague exclusion. None reads as filler ("not building X unrelated thing").

The IN list is equally specific -- it names the predicate, the three concrete break cases (blocked re-tick, rewind, cold-restart respawn), the directed-transition gap, the read-only recovery path, its discoverability, and the mandatory downstream skill/eval/doc work -- giving a downstream PRD author a legible edge.

Quoted evidence: "**Auto-advance discarding the phases it crosses.** ... It predates this mechanism and is broader than it -- the honest framing is that auto-advance has never surfaced intermediate instructions at all, and folding it in here would disguise that as a details regression."

### 5. Open Questions

N/A -- section correctly absent. The one live open question from the supporting research (whether the `--to` carve-out is a defect or deliberate operator-intent carve-out) is handled inside Scope Boundary instead, explicitly deferred: "Making the contract uniform is in scope; which reading wins... is the DESIGN's to settle and record." This is a legitimate choice for a question whose resolution changes an implementation reading rather than the brief's own framing, and it doesn't block Draft -> Accepted since there's no unresolved Open Questions list to clear.

### 6. Content boundaries

PASS, with one item close to the line. Scanned every section for PRD-level requirements, acceptance criteria, user stories, DESIGN-level architecture, implementation tasks, and feature sequencing:

- No specific interface is named anywhere -- no command name, no flag, no field name for the recovery path, no response schema. The research left `phase-info` vs. `status --details` vs. a new subcommand as an open design question, and the brief correctly never picks one.
- "What koto records in order to decide and where that record lives" is named as an in-scope *topic*, with the answer explicitly deferred: the Status section states "the DESIGN owns what koto records and where the read-only recovery lives." That's scope framing, not a decision.
- The recovery path is described functionally ("read-only," "keyed by the workflow name," "changes no workflow state and triggers no side effect") rather than as an interface shape. This is the one place the language reads closest to acceptance-criteria phrasing (see Optional improvements below), but it stops short of naming a command or a response shape, so it does not cross into PRD or DESIGN territory.
- No acceptance criteria, no numbered requirements, no implementation task breakdown, no sequencing of this feature against others appears anywhere in the document.

Quoted evidence (the closest-to-the-line phrase): "A read-only way to retrieve the current phase's directive and instructions, keyed by the workflow name, that changes no workflow state and triggers no side effect."

## Findings outside the rubric items

- **Numeric claims check out.** The 7,140-character / 101-character / fourteen-consecutive-blocked-ticks figures in the Problem Statement match the exploration findings' "14-week sweep" case exactly in substance; the brief generalizes "week" to "iteration," which is a reasonable abstraction away from one template's domain vocabulary and not a factual drift.
- **No claim found that the supporting context doesn't support.** Cross-checked the PR/issue provenance (#90 filed 2026-03-26, PR #109 merged 2026-03-30, R9 of the Done PRD), the three break cases, the `--full`-is-not-a-read finding, and the References section's characterization of the two upstream docs -- all match the handoff and findings documents.
- **No vague prose where a concrete fact was available.** Where the source material had a measured number, the brief used it (character counts, tick counts); where the source material left something genuinely unresolved (recording mechanism, recovery-surface naming), the brief correctly stayed abstract rather than inventing false precision.

## Required changes

None.

## Optional improvements

- The scope-boundary phrase "keyed by the workflow name, that changes no workflow state and triggers no side effect" and its near-duplicate in the Problem Statement ("The only remaining options are to pass `--full` on every tick... or to read the template file directly") edge toward the specificity of an acceptance criterion. Not a required change -- it stays short of naming an interface -- but a downstream PRD author should feel free to restate these as formal requirements rather than treating the brief's phrasing as binding language.
- Journey 4 (the template author) is the weakest of the four in journey-shape terms: its "outcome" is largely a checklist recap of journeys 1-3 rather than a new outcome in its own right. It still passes the distinctness bar because its entry point (authoring/design-time) differs from the other three (run-time), but a tighter version could lead with something the author specifically learns or decides, rather than what they observe matching the other journeys.
