# Lead: What is the strongest case for not doing this?

This is the adversarial lead. It argues against embedding a hosted classifier
(TypeSafe Jev first, behind a generic decider trait) inside `koto next`, and
against reshaping shirabe around it. It tries to be honest about where that
case is thin.

## Findings

### 1. Are the savings real? (confidence: Medium-High that they are small)

**Measured decision time is a rounding error next to the generative work.**
I walked the 37 koto session logs that survive on the maintainer's machine
(191 `evidence_submitted` events) and measured, for each submission, the gap
between the previous transition/evidence event and the agent's answer. That
gap is an upper bound on "time the agent spent at this state", reading
included.

- Pure decision states that round 1 rated "good now" are already fast:
  `plan_validation` median 3.5 s (7 samples, 0.2 min total), `pre_pr_evidence`
  median 2.3 s, `finalize` 4 s, `chain_proposal` 6.5 s (and that one is a human
  reply).
- The states with real time in them bundle gathering with deciding:
  `worktree_discipline_check` median 20 s, `fold` median 25 s. `fold`'s
  verdict doesn't even change routing (both values go to the same target,
  per the workflow-shape lead); the time is the agent reading two documents,
  which it must still do to perform the absorb/keep work afterwards.
- Excluding states that just wait on children (`hop_*`, `spawn_and_await`),
  agents spent about 3.8 hours at evidence states in these logs. The
  classifier candidates account for about 18 minutes of that (roughly 8%),
  and about 14 of those minutes are `fold`, whose cost is reading, not
  deciding. The pure decisions a classifier could actually remove come to a
  few seconds each. Implementation, review, scrutiny and QA states run 270 to
  500 s median each; scope hops run 30 to 40 minutes.

Caveats: small sample, one user, surviving logs only (terminal sessions are
deleted by default), and prose-only skills (explore, plan, design) leave no
log at all. The numbers are directional.

**The agent usually still needs the decision's inputs.** Round 1's own
surprise: most shirabe decision states bundle "do the work" with "report the
verdict". After the split, the classifier needs a pruned state that someone
produced. Either a script produced it (then the agent's decision over that
same pruned fact sheet is also cheap), or the agent produced it (then the
agent already paid the reading cost and has the answer in hand). For
`task_validation` the agent must read the task anyway to implement it. For
`post_research_validation` the agent wrote the summary. Taking the final
enum out of the agent's hands saves one tool call and perhaps 100 to 300
output tokens.

**The round trip isn't the bottleneck.** The agent calls `koto next` after
every state regardless. An auto-decided state removes one agent turn
(a few seconds, a few hundred tokens of directive plus JSON) from a run
that takes tens of minutes and hundreds of thousands of tokens. Main-thread
context: the directive text of a decision state is 150 to 250 words
(`task_validation`, `plan_validation` in `work-on.md`), and prompt caching
makes re-reading it cheap. Cost: at frontier prices an in-context enum
decision is on the order of a cent; saving ten of them saves ~$0.10 on a run
that costs dollars.

**Where the savings are real, koto isn't where they'd happen.** The inventory
lead's biggest wins (explore's three-agent entry jury, the doc-validation
juries, the AC-discriminability pre-screen, never loading the 19 KB
crystallize framework) all live in prose-only skills with no koto template.
Getting them "inside `koto next`" requires porting those skills onto
templates first, a project much larger than the classifier and unrelated to
it. Those savings are also subagent savings, not main-thread savings, and
replacing a jury with a single classifier removes the thing the jury exists
for (independent redundant judgment).

**On-path latency can be negative.** koto is a fresh process per call, so
every classifier call pays a TLS handshake with no connection reuse, on top
of Jev's published 110 ms p50 / 340 ms p95. When the answer falls below
threshold (rate unknown, and likely high on minority-class states), the call
is pure added latency and the agent decides anyway.

### 2. Does the reshaping deliver most of the value without the classifier? (confidence: High)

Round 1 already says so in several places:

- Workflow-shape lead: "Every pattern above except the classifier call itself
  is a win on today's koto." Deterministic prefilters mean "most runs skip
  the question entirely" (`impact: none` from `git diff` overlap).
- Inventory lead: at least eight template "decisions" are really
  deterministic facts the agent computes by hand (`batch_outcome`,
  `pause_decision`, panel aggregation, verification map matching, retry caps,
  `cascade_status`, `research.context_gathered` which routes nowhere, release
  bump). Moving them to gates or `when` clauses removes turns and removes a
  class of silent errors (a mis-set `batch_outcome` advances the run today).
- Crystallize is "count signals minus anti-signals, demote, tiebreak": the
  valuable fix is moving the arithmetic into a script, which Jev can't do
  anyway.
- Per-value `criteria` in `accepts` (template-schema lead) helps the agent
  with no key, and fixes the real bug that `ExpectsFieldSchema` drops field
  descriptions today.
- A narrow question over a precomputed fact sheet is easy for the agent
  too; it's the same question, answered by a model that has the rest of the
  run in context.

After that work lands, what remains for a classifier is the residual semantic
enum on a pruned state: a few seconds per decision (finding 1). The
classifier looks like the last 10 to 20% of the value for most of the
complexity. It does keep one distinct benefit (consistency across runs and
agents, since a fixed classifier is less variable than an agent's mood), but
the reshaping captures much of that too by narrowing the question.

### 3. Quality risk of ~91% agreement (confidence: Medium-High that it's material)

**Compounding.** If auto-decisions were independent at 91.5% agreement, a run
with 5 of them agrees with the frontier judge end to end 64% of the time;
10 gives 41%; 20 gives 17%. The inventory lead estimates 80 to 120 atomic
questions in a six-issue `/execute`. Thresholding raises accuracy on the
auto-taken subset, and many disagreements are benign, so these are worst-case
shapes, not predictions. But the compounding structure is real, and nobody
has measured the thresholded rate on koto-shaped decisions.

**The 91.5% figure doesn't transfer.** It comes from rubric checks on
customer-interaction data against a frontier model as the reference, not
from workflow routing on code artifacts. And the evaluation lead found the
labels koto would be measured against are almost all the happy path
(`review_outcome` passed 4/4, `plan_validation` proceed 7/7, every scope hop
landed). On skewed decisions, 91.5% raw agreement can be worse than
"always pick the majority class". What matters is minority-class recall, and
there are almost no minority examples to measure it with.

**Calibration fails exactly where koto would use it.** The whole fallback
design rests on confidence being trustworthy. Secondary reporting of
independent studies (published within the first week, not verified
first-hand here) says:

- On 900 synthetic tickets whose labels depended on an internal policy absent
  from the text, expected calibration error was about 0.107 against a 0.024
  noise floor, roughly 4.4x; the score primitive hit 44.7% accuracy at a mean
  stated probability of 0.74. The authors' own warning: "calibration you can
  see on public benchmarks is not the calibration you will get on your
  traffic."
- A pre-registered study found that when the criteria descriptions were
  wrong, accuracy "collapsed to 16.7%, below the 25% random floor", and the
  model couldn't tell the criteria were wrong.

shirabe decisions are the "internal policy absent from the text" case almost
by definition: whether an outline item is "clear enough", whether drift is
"intent-changing", whether a doc should be absorbed all hinge on workspace
conventions. And template criteria are authored prose that will drift out of
sync with directive prose over time; the failure mode for stale criteria is
confident wrong answers, not low-confidence fallbacks.

**Errors nobody sees.** The design's selling point, that the agent never
sees an auto-decision, is also its failure mode. Today a wrong enum is
written by the agent that then acts on it, and often notices ("wait, this
isn't docs-only"). An invisible classifier choice gives the agent the next
directive with no reason attached; the agent can't catch what it can't see,
and there's no `rationale` field (the template-schema lead proposes dropping
or synthesizing it, and E-CLASSIFY-REQUIRED bites every shirabe verdict that
pairs with a required rationale). Downstream prose that consumes that
rationale (e.g. `validation_exit` explaining to the user why a task wasn't
ready) loses its input.

**Debugging cost.** "Why did koto pick that?" becomes: find the
`ClassifierEvaluated` event, fetch the content-addressed state payload (only
if `--record-classifier-state` was on, since `ctx/` is overwrite-only),
and re-run against a hosted model whose `jev-latest` alias may have moved.
The answer is a probability vector, not a reason.

### 4. Vendor and maturity risk (confidence: High)

- **Days old.** Launched September 15, 2026; today is September 23. A web
  check shows it was waitlist-gated at launch and the waitlist was reportedly
  removed September 21. There's a `jev-latest` alias, a bespoke client (not an
  OpenAI-compatible base URL), and no published deprecation policy,
  versioning commitment, rate limits, or SLA that I could find.
- **The source document is weak evidence.** It's a secondary-source synthesis
  whose citations are mostly SEO explainers, video summaries and vendor
  material. It contains leftover generation artifacts (`[span_267](start_span)`),
  and internal contradictions:
  - The same 91.5% figure is attributed to two different studies (a Langfuse
    benchmark and a Good Start Labs benchmark).
  - The stated price ($0.042 per 1M input tokens) doesn't match its own
    example response (482 tokens billed at $0.000020244, which is about $42
    per 1M, a 1000x gap). Separately, the vendor's own reported average of
    about $0.0004 per decision implies roughly 10k tokens per decision at list
    price.
  - Latency is described both as "tens of milliseconds" and as 70 to 500 ms.
  - Adoption claims ("fastest-adopted model in Vercel AI Gateway history",
    "nearly 13% of paid engineering teams" within 24 hours) are marketing, not
    evidence.
  None of this means Jev is bad. It means the design would rest on numbers
  nobody on this project has checked first-hand.
- **Churn cost for a public OSS tool.** A vendor-specific client in the engine
  binary puts koto's release cadence on the vendor's schedule: wire-format
  changes, auth changes, model deprecations, and pricing changes each become
  a koto release. The category is one week old; it's likely there will be
  competing "decision models" with different primitives within months, and
  the generic trait will have been shaped around the first one's
  choice/score/noul vocabulary. Templates that encode Jev-shaped
  `criteria`/`bands`/`true_above` are the real lock-in, because they ship in
  shirabe and live in users' repos.

### 5. Complexity and maintenance (confidence: High)

Tallying what round 1 proposes:

- Engine: an injected classifier closure in `advance_until_stop`, per-epoch
  stickiness, "classified fields as one unit" fallback, score banding
  arithmetic, escape-key stripping on fallback, a new `condition_type`, and
  a reserved namespace so the agent can't forge classifier results.
- Events: `ClassifierEvaluated` (or `classifier_evaluated`) with ~15 fields,
  possibly `ClassifierDeclined`, registration in the session-feed contract.
- Template: a `classify` block per field, per-value `criteria`, `inputs`
  with `max_bytes`, `bands`, thresholds, `auto_min` per value with `never`,
  `mode: shadow|auto|off`; at least nine new compile rules (E-CLASSIFY-ESCAPE,
  -ESCAPE-ROUTED, -CRITERIA, -ARITY, -THRESHOLD, -INPUTS, -REQUIRED, two
  warnings) plus a rule rejecting auto on override/irreversible edges.
- Config: `classifier.*` keys, key blocklisting and redaction, a kill-switch
  env var, three-way most-restrictive mode resolution.
- Storage/eval: content-addressed state payload storage, a cleanup-surviving
  concordance ledger next to `_terminal_index.jsonl`, a
  `koto classifier report` command, sampled shadow slices in auto mode,
  promotion bookkeeping keyed to `template_hash`.
- Tests: trait fake, endpoint override, Go/godog stub server steps, recorded
  fixtures, feature-gated live tests, env scrubbing across both test
  harnesses.

That's a subsystem, not a feature. For scale: the closest precedent, "koto
runs the mechanical commands" (#205, merged Aug 21), was followed within four
days by roughly ten fix PRs (#209, #210, #213, #223, #226, #229, #230, #231,
#233) about substitution, captures and edge cases. 13 of 41 commits since
August 1 are fixes. That was a deterministic, local feature. A
non-deterministic, networked, vendor-backed one will have a longer tail, and
koto has an external consumer importing `koto::engine::types::*` under a
written stability contract (`docs/STABILITY.md`), which raises the cost of
getting the event and template shapes wrong the first time.

The additive-event rules do make this *possible* without a schema bump. They
don't make it cheap.

### 6. Audience fit (confidence: Medium-High)

- koto and shirabe are public OSS. The number of users who have or will get
  a Jev key in the next six months is unknown and probably small; the
  maintainer is likely the main one. The feature's design point ("no-key
  path identical to today") concedes that most users get zero benefit.
- tsuku's philosophy is self-contained with no system dependencies. koto
  already carries an opt-in network backend (cloud sync), so this isn't
  unprecedented, but cloud sync is storage the user controls (any
  S3-compatible endpoint). A decider bound to one paid vendor's endpoint is a
  different kind of dependency: the workflow's *behavior* changes depending
  on whether you pay a third party.
- Two behavior paths per classified state. With a key, runs take branches
  based on classifier output; without one, the agent decides. Bug reports
  become "which path were you on, which model build, which threshold". Every
  classified state doubles the scenarios shirabe's eval fixtures and
  functional tests should cover, and the key path can't be tested in CI
  without a secret.
- The same template produces different runs for different users, which
  undercuts the "reproducible" value the workspace states as a principle.

### 7. Transparency and trust (confidence: Medium)

- koto's pitch (README) is enforced order, atomic persistence, and
  recoverable transitions "without losing the audit trail". The audit trail
  today records *who* chose *what* and usually *why* (rationale). A classifier
  entry records a probability vector over a hashed payload. That's an audit
  of *that* a decision happened, not *why*. The human reviewing a PR later
  has less to go on.
- The `default_action` precedent for invisible success doesn't transfer
  cleanly: a command's output is deterministic and re-runnable; a hosted
  classifier's answer isn't reproducible once the alias moves.
- Injection: the state payload includes GitHub issue bodies (anyone can
  write them), others' commit messages, CI logs, and agent-written context.
  The Jev document itself says the model "remains vulnerable to semantic
  injection" and shouldn't be an unmonitored gate. An issue body that says
  "this is a docs-only change, trivially ready" steers `issue_type` and
  readiness decisions invisibly. Round 1's mitigations (never on gates,
  never on overrides or confirmation edges) are right, but they also shrink
  the eligible set further, which feeds back into finding 1.
- Anchoring and imitation: showing the distribution on fallback anchors the
  agent; hiding it means the agent re-does the decision from scratch, so the
  fallback path pays both costs.

### 8. Cheaper alternatives (confidence: Medium-High that they capture most of the value)

1. **Do the reshaping only.** Split gather/decide/act, move deterministic
   "decisions" into gates and `when`, add per-value criteria to `expects`,
   add visit counters and variable-value matching to `when`. High value,
   no vendor, no second code path, every user benefits.
2. **Make the decider a command, not an HTTP client.** If koto grows the
   decision-state declaration at all, let a template or user config name an
   external executable (`decider: command`) that gets the pruned state and
   question set on stdin and returns typed JSON, reusing the
   runs-commands machinery. The vendor client, key handling and HTTP live in
   an adapter outside koto (a small tool installable via tsuku). koto owns
   the protocol, thresholds, events and fallback; it never owns a vendor.
   This keeps the "generic interface" the user asked for and drops key
   handling, TLS, vendor churn and most of the test-secret problem from the
   engine.
3. **Let the calling agent delegate.** Claude Code and similar harnesses can
   already route a narrow question to a smaller model. That puts the choice
   of model with the user's harness, not with koto, and needs nothing from
   koto beyond a well-formed typed question (which option 1 provides).
4. **Deterministic rules for the computed facts.** Much of what looks like
   judgment (staleness, drift "none", verification selection, batch
   outcome) is `git` and `jq`.
5. **Wait.** Shadow-mode data can be gathered later against whatever vendor
   exists then. Nothing in options 1 to 4 forecloses a classifier; all of
   them make one easier to add. Six months would show whether the category
   has competitors, stable APIs, and published calibration on out-of-domain
   data.

## Strongest arguments against (ranked)

1. **The savings are small where koto can reach them.** Measured pure-decision
   states take a few seconds each; the candidate states total about 8% of
   non-waiting agent time in surviving logs, and most of that is reading the
   agent must do anyway. The large wins (juries, crystallize) live in prose
   skills with no koto template.
2. **The reshaping is most of the value and needs no classifier.** Round 1
   agrees. The classifier is the marginal slice on top.
3. **Confidence-based fallback rests on calibration that independent work
   says breaks on unfamiliar, policy-dependent input**, which is what shirabe
   decisions are. Stale criteria produce confident wrong answers, and
   invisible decisions remove the agent's chance to notice.
4. **It's a subsystem with a long maintenance tail**, landing in an engine
   with a written stability contract and an external consumer, modeled on a
   precedent (#205) that needed about ten follow-up fixes while being
   deterministic and local.
5. **Vendor maturity**: one week old, no published versioning or SLA, key
   numbers unverified and internally inconsistent in the source document, and
   template syntax shaped around one vendor's primitives.
6. **Audience fit**: most OSS users won't have a key; those who do get a
   different workflow from the same template, and the key path can't be
   tested in public CI without a secret.

## Where the case against is weak

- **Dependency cost is near zero.** koto already links a sync rustls HTTP
  client through rust-s3 and already has an opt-in network backend with the
  exact key-handling pattern needed. "koto shouldn't touch the network" isn't
  a real objection.
- **The stability contract allows it.** Additive events and optional fields
  don't bump the schema; older readers degrade gracefully. The design fits
  the rules.
- **Cost per call is genuinely negligible**, whichever of the inconsistent
  price figures is right.
- **Consistency is a real benefit** the reshaping only partly delivers: a
  fixed classifier gives the same answer to the same pruned state across
  agents and sessions, and gives a probability instead of a vibe.
- **High-multiplicity checks are different.** Per-AC, per-finding and
  per-unit checks (tens to ~100 per `/execute` or review-plan run) are where
  per-decision savings add up and where a cheap pre-screen that clears
  confident "ok"s could matter. My timing data doesn't cover them because
  they mostly live in prose or subagents.
- **Shadow mode is low risk.** Logging a classifier's answer next to the
  agent's, with no effect on routing, costs little and produces the evidence
  this whole argument says is missing. The case against auto-deciding is much
  stronger than the case against measuring.
- **The measurement is thin.** 37 surviving sessions from one machine, and
  gap timing is an upper bound that can't separate reading from deciding.

## What would change the verdict

Doing it (in auto mode, inside koto) becomes clearly right if:

- Shadow data on koto-shaped decisions shows minority-class recall and
  in-band calibration holding up on this project's own traffic (not vendor
  or public benchmarks), for specific questions, across a few hundred paired
  observations including real minority cases.
- The prose skills with the big wins (explore's entry assessment and
  crystallize, the validation juries, AC discriminability) are already on koto
  templates for their own reasons, so the classifier plugs into existing
  states instead of motivating a port.
- Measured main-thread savings on a reshaped workflow (after deterministic
  facts moved to gates) still show decision turns as a meaningful share of
  tokens or wall-clock, not a few seconds each.
- The vendor publishes versioning, deprecation and rate-limit terms, and at
  least one competing provider exists, so the generic interface is shaped by
  two implementations rather than one.
- The decider lives behind a command/adapter boundary, so koto core carries
  no vendor client, and the no-key path is the tested default in public CI.

## Summary

The strongest case against is that the savings koto can reach are small (pure decision states in surviving session logs take a few seconds each, and the big wins like jury replacement and crystallize live in prose skills without koto templates), while round 1's own findings show the gather/decide/act reshaping and gate-computed facts deliver most of the value with no classifier, no vendor and no second code path. Against that marginal gain sit a subsystem-sized engine change (new events, nine-plus compile rules, config, key handling, ledger, stub servers) in a stability-contracted engine, a model one week old whose headline numbers are unverified and internally inconsistent in the source document, and independent reporting that its confidence calibration breaks on unfamiliar, policy-dependent input, which is what shirabe's decisions are and what the invisible, confidence-gated fallback depends on. The case against is weak on dependency cost, schema compatibility, per-call price and shadow-mode measurement, so the defensible reading is "do the reshaping now, keep any decider behind a command boundary outside koto core, run shadow mode if anything, and revisit auto-decisions once own-traffic calibration and vendor maturity are demonstrated."
