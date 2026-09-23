# Exploration Findings: jev-decision-offload

## Core Question

How should koto and shirabe evolve so that decisions a typed classifier (Jev)
can make are made inside `koto next` when a key is available, without the
calling agent seeing them, and how should shirabe's workflows be reshaped so
more of their decisions become classifier-shaped?

## Round 1

### Key Insights

- There's one hook point. koto hands a branch decision to the agent only when
  `resolve_transition` returns `NeedsEvidence` on a state with `accepts`
  (`src/engine/advance.rs`). A classifier fits as an automatic evidence
  submitter in that arm. It answers the state's own enum/boolean field, records
  `evidence_submitted` plus an additive classifier event, and lets `when`
  routing proceed unchanged. Success is invisible; every failure mode returns
  today's `evidence_required`. No new response variant. (koto-decision-surface)
- One declaration serves both paths. The field `description` plus new per-value
  `criteria` become the classifier's instructions and the agent's prompt, and a
  `classify` block sets primitive, thresholds and a mandatory escape key. New
  compile rules cover escape hatches, arity, thresholds, inputs and required
  fields. `ExpectsFieldSchema` must start carrying descriptions. Inputs come
  from labelled context-store keys filled by `default_action`. (template-schema)
- The koto and shirabe halves are coupled through state shape. Shirabe decision
  states bundle work and verdict, so they must split into gather / decide / act
  before a classifier can answer them without skipping work. This extends
  shirabe's existing `default_action` campaign. (koto-decision-surface,
  shirabe-workflow-shape)
- Several template "decisions" are deterministic facts the agent computes by
  hand: `batch_outcome`, `pause_decision`, `cascade_status`, verification-map
  matching, panel aggregation, retry caps. They belong in gates regardless of
  any classifier. (shirabe-workflow-shape, shirabe-decision-inventory)
- Of ~45 shirabe decision points, ~12 fit now, ~15 fit after reshaping, and the
  rest are generative, human gates, or deterministic. The largest wins
  (explore's entry-assessment jury and crystallize scoring, doc validation
  juries, AC-discriminability and per-AC/per-finding checks) live in prose
  skills with no koto template, or need variable-length question sets.
  (shirabe-decision-inventory)
- It's operationally cheap. koto already links a sync rustls HTTP client via
  rust-s3 (verified in `Cargo.toml`) and has a credential pattern (env or user
  config only, blocked from project config, redacted). Additive events need no
  schema bump. Results must stick for the epoch like overrides, never re-called
  on polling. Tests need a trait fake, an endpoint override for stub servers,
  and env scrubbing. (koto-operational-constraints)
- There's no dataset yet. Terminal sessions are deleted by default, context
  content isn't versioned, and surviving labels are almost all happy-path
  values. Agent labels measure imitation, not correctness. Shadow mode, a
  cleanup-surviving concordance ledger, per-value thresholds with a
  compiler-enforced `never`, and a layered kill switch are the rollout tools.
  (evaluation-rollout)
- The case against: savings on decision states koto reaches today are seconds
  each (~8% of agent time in surviving logs). The reshaping delivers most of
  the value, the vendor is a week old with inconsistent published numbers,
  calibration on policy-dependent input is unproven, and the change is
  subsystem-sized. It's weak on dependency cost, schema compatibility, per-call
  price and shadow-mode risk. (devils-advocate)

### Tensions

- Embedded auto-decisions (the user's intent) versus the devil's advocate's
  "decider outside core, shadow only". Resolved by the staged posture below.
- Authority "anything the template marks eligible" versus the leads' call for a
  compiler floor (never gates, overrides, irreversible or confirmation-guarded
  edges). These are compatible: authors choose within a floor the compiler
  enforces.
- Where value is versus where koto reaches: the best targets are in prose
  skills, so the payoff grows only as those skills move onto templates.
- Hidden decisions versus auditability and the agent's chance to notice a
  wrong call.
- A factual conflict between leads (no HTTP client versus rust-s3 already
  linked) was resolved by reading `Cargo.toml`: rust-s3 is a regular dependency.

### Gaps

- No calibration data on shirabe's own decisions; independent calibration
  claims were only seen secondhand.
- Variable-length question sets (one question per AC, finding or unit) are not
  designed; koto states have a fixed field set.
- Where aggregation lives (count, argmax, margin, thresholds): koto `when`
  vocabulary or shirabe scorer scripts.
- Main-thread savings in prose skills are estimated, not measured.
- Vendor API terms, versioning and rate limits are unverified.

### Decisions

- Posture: staged in koto. Build the decider inside `koto next` with shadow as
  the default mode; per-value auto only after own-traffic data clears a bar.
  Reshape shirabe in parallel, since it pays off with no key.
- See `wip/explore_jev-decision-offload_decisions.md` for the other round 1
  decisions (authority, no-key parity, generic interface, surface).

### User Focus

The user wants the decision embedded in `koto next` as a step koto executes
when a key exists and hands to the agent otherwise, with no standalone
command. They value context, wall-clock, consistency and cost together. They
asked for the case against to be argued explicitly, and after seeing it chose
the staged posture over full auto or deferral.

## Accumulated Understanding

The change is one feature with two faces. In koto, a template-declared typed
question on an `accepts` field is answered by a pluggable decider (Jev first)
at the `NeedsEvidence` point in the advance loop. The answer is recorded as
evidence plus an additive audit event, sticky per epoch, and falls back to
today's `evidence_required` on low confidence, escape, missing key or error.
Shadow mode ships first. Auto is enabled per value, gated by concordance on
this project's own traffic, with a compiler floor that keeps gates, overrides
and irreversible or confirmation-guarded edges out of reach.

In shirabe, states split into gather / decide / act, deterministic facts move
into gates and captures, and residual judgments are declared once as typed
questions with escape keys. That work improves workflows with no key and
produces the labelled data shadow mode needs. The biggest payoffs need
prose-only skills (explore, plan, review-plan, the validation juries) to
gain templates, and need koto to support per-item question sets and some form
of aggregation.

Open design questions for the next step: field-level versus state-level
declaration (graceful degradation versus loud failure on older koto),
whether the agent ever sees the classifier's distribution on fallback,
per-item question sets, aggregation, the ledger's location, what gets
stored for replay, and which shirabe decisions go first.

## Decision: Crystallize
