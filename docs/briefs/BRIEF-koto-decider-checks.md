---
schema: brief/v1
status: Accepted
problem: |
  koto's decider can only answer a routing question over text koto already
  stores. It can't judge what the agent produced, so the prose rules an agent
  is told to follow are checked by nobody but that agent until a person reads
  the change, and a rule the agent broke surfaces late, if at all.
outcome: |
  For an opted-in user, a workflow won't move past a state while a decider
  judges the agent's work to break one of a few named criteria, and the agent
  is told which one, so it can fix the text or override with a reason.
  Everyone else sees no change, and every verdict is kept for later trust.
motivating_context: |
  shirabe's accuracy spike graded six of its own prose rules through Jev.
  Two of them, "a code comment gives a reason, not a restatement" and "an
  acceptance criterion can be answered yes or no", let no bad or adversarial
  text through in choice form, on inputs under about 2.5 KB. That result
  earns a decider design for those two, but not trust in a pass.
---

# BRIEF: Grading the agent's work against closed criteria

## Status

Accepted

Framing only. How a criterion is declared, how the artifact is extracted, and
what gets recorded are the downstream PRD's and DESIGN's to settle.

## Problem Statement

Workflows built on koto tell the agent to follow rules about the prose it
writes: a code comment should say why, not restate the code; an acceptance
criterion should be answerable with yes or no. Nothing checks those rules
while the workflow runs. A gate can run a script, but these rules need a
reader, and no script can tell a reason from a restatement. So the only
check is the agent's own claim that it complied, and the first independent
reader is a reviewer after the pull request is open, when fixing it costs a
review round instead of an edit.

koto already ships a decider, a typed decision model that users opt into;
the only one it supports today is Jev, from TypeSafe. It can't help here. It answers one kind of question: which value an
`accepts` field should take, judged from context keys and variables that
already exist before the agent acts. It never reads what the agent just
produced, it asks one question per field rather than several rules about one
artifact, and when it disagrees with the agent it says nothing, because it
was built to route, not to object. Its requirements deliberately left
per-item questions and anything that evaluates a check for later.

There's also a trust problem the tool must not paper over. shirabe's Jev
accuracy spike (see References) found that for these two rules a failing answer is reliable and a
passing one hasn't been earned: the numbers are a lower bound on one model
build and short inputs. A check that lets a pass count for anything would
put a model's approval where nothing has shown it belongs. And a check that
treats a garbled answer, or no answer at all, as approval would be worse
than no check.

## User Outcome

A workflow author can name a few closed criteria for something the agent
produces, and for users who've opted in, a state won't be left while the
decider judges that artifact to break one of them. The agent learns which
criterion failed from the same response that already tells it why any other
check failed, fixes the text, and tries again. When the decider is wrong, the
agent overrides the way it overrides any failed check, with a reason on the
record, and that override is visible later as a likely false fail.

A passing verdict changes nothing: the workflow behaves as it would have
without the check. A decider that can't give a verdict never counts as
approval. Users who haven't opted in run the same template and see no
difference at all.

The maintainers who decide, later, whether a decider's pass can ever be
trusted have what they need: every consultation, with its criterion, the
probabilities, the verdict, the mode it ran in and the model that gave it,
tied to the exact input it judged.

## User Journeys

### A template author adds two criteria in shadow

A maintainer of a workflow that has the agent write code comments wants to
know whether the decider would catch restatement comments. They declare the
comment criterion on the state that follows implementation, pointing it at a
command that prints the comments the change added, and leave it in shadow.
Opted-in runs now record a verdict for every visit while the workflow moves
exactly as before, and the maintainer reads those records before deciding to
let the criterion block.

### An agent is stopped by a failed criterion

An agent in an opted-in session finishes a change and calls `koto next`. The
comment criterion, now in veto mode, judges one added comment to restate the
line below it. The state doesn't advance, and the response names the
criterion's id and a reference to the rule, the same way a failed lint check
would. The agent rewrites the comment to give the reason and calls `koto next`
again; the new text is judged afresh and the workflow moves on.

### An agent overrides a false fail

An agent's acceptance criterion is answerable yes or no, but the decider fails
it. The agent records an override for that check through koto's existing
override command, with a sentence saying why the criterion is binary. The
workflow continues, and the override shows up in the record as a candidate
false fail for whoever reviews the criterion's accuracy.

### An agent hits a decider that gave no verdict

The provider times out or returns something koto can't read. In veto mode the
state stays blocked, and the agent is told plainly that no verdict was read,
not that its text broke the rule, so its next move is to try again rather
than to rewrite text that may be fine. In shadow mode the same failure is
logged and nothing blocks.

### A user who never opted in

A contributor runs the same workflow with no decider configured. No request
leaves the machine, no criterion is evaluated, and the state behaves exactly
as if the criteria weren't declared.

## Scope Boundary

### In

- Grading a bounded slice of an artifact the agent produced, extracted by a
  command the template declares, against criteria whose text the template
  fixes.
- Several criteria on one state, each a two-value choice with an escape.
- A veto-only verdict: a fail blocks and reaches the agent as a finding naming
  the criterion; a pass never advances anything.
- A missing or unreadable answer treated as a fail in veto mode, never as a
  pass.
- A per-criterion mode, shadow or veto, with shadow the default.
- A way past a false fail through the override path koto already has.
- A record of every consultation that later accuracy work can join to its own
  judgments.
- The two criteria the spike supports, demonstrated end to end.

### Out

- Letting a decider pass advance a workflow, and any mechanism that promotes a
  criterion toward trusting a pass. Pass-trust is later work that needs its
  own evidence.
- Criteria beyond the two the spike supports, and score or boolean questions.
- Changing how the existing routing decider works, including the compile rule
  that refuses `auto` answers on terminal, confirmation-guarded or
  gate-conditioned routes.
- A registry of rule ids, check severity levels, or retiring a criterion. A
  criterion carries an opaque id and a reference; mapping them is a later
  concern.
- A new override command or flag.
- Changes to shirabe's templates. Adopting the criteria there follows once
  koto ships them.
- Batching criteria in ways the spike didn't measure, unless the design shows
  it's needed for the two criteria.

## References

- shirabe's Jev accuracy spike, `docs/spikes/SPIKE-jev-accuracy.md` in
  tsukumogami/shirabe: the measurements behind the two criteria and the
  choice-form recommendation.
- `docs/prds/PRD-jev-decision-offload.md` and
  `docs/designs/current/DESIGN-jev-decision-offload.md`: the routing decider
  this feature sits beside.
- `docs/guides/decider-authoring.md`: how users opt in today.
