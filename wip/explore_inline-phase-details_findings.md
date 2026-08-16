# Exploration Findings: inline-phase-details

## Core Question

koto#90 asks that a workflow phase's full instructions ride inline in the `koto
next` response the first time an agent reaches that phase, and be omitted on
every later visit, with a `koto phase-info` escape hatch for when context
compression drops them. The question is whether that behavior is the right
answer to the overhead it targets, and -- if it is -- what shape it takes in
koto's actual template grammar, visit tracking, and response contract.

## Round 1

### Key Insights

- **Most of koto#90 already shipped.** PR #109 (`feat(cli): redesign koto next
  output contract`, merged 2026-03-30, `Fixes #102`) landed the template
  `details` field, the response field, first-visit gating, and a `--full`
  override. #90 was filed 2026-03-26; #109 merged four days later without ever
  citing it. The mechanism lives at `src/template/types.rs:57` (`TemplateState.details`),
  `src/cli/next_types.rs` (`details: Option<String>` on every non-terminal
  variant), `src/engine/persistence.rs:981` (`derive_visit_counts`), and
  `src/cli/mod.rs:3999-4015` (`if full || count <= 1`). It is a **Done** PRD
  requirement: `PRD-koto-next-output-contract.md` R9. *(leads: first-visit,
  phase-info, premise, contract, alternatives -- five of seven converged on this
  independently)*

- **The template shape is nothing like the issue's sketch.** `details` is not a
  YAML key. `extract_directives` splits a state's markdown body on an HTML
  comment marker, `<!-- details -->`; text before it is the `directive`, text
  after is `details`. First occurrence wins. *(lead: template-grammar)*

- **The issue author has already audited this twice, most recently two hours
  before this exploration started.** Comments on #90 dated 2026-06-07 and
  2026-08-16T16:51Z re-score every acceptance criterion against the shipped
  code. The 2026-08-16 audit measured behavior on koto 0.11.4 (`eb626d9`) with a
  reproducible two-state loop and concluded: AC1, AC2, AC5, AC6 pass; AC3 passes
  for genuine re-entries; AC3 fails for non-advancing re-ticks; AC4 is absent.
  The author's own re-scope is "AC3 remainder plus AC4, and AC4 is the
  higher-value half." *(source: issue #90 comments)*

- **The measured AC3 gap: a tick that does not transition re-sends `details`.**
  Visit counts are derived from state-*entry* events only -- `Transitioned`,
  `DirectedTransition`, `Rewound` (`derive_visit_counts`,
  `src/engine/persistence.rs:981-993`). A `koto next` that evaluates gates,
  fails them, and stays put appends only `gate_evaluated` records, so the count
  never leaves 1 and the first-visit branch keeps firing. The author's table
  shows tick 2 re-sending a 135-char block on a blocked re-tick. The author's
  suggested fix -- count emissions rather than transitions -- names the root
  cause precisely: the counter tracks *state entries*, but the thing worth
  counting is *how many times koto has handed the details to a caller*.

- **The escape hatch gap is real, measured, and the higher-value half.**
  `koto status` is read-only but returns only `name`, `current_state`,
  `template_path`, `template_hash`, `is_terminal` (plus optional `batch`, `leg`,
  `superseded_branches`, `stale_template_source_dir`) -- no directive, no
  details (`handle_status`, `src/cli/mod.rs:4834-4960`). The only path back to a
  phase's procedure is `koto next --full`, which evaluates gates and can
  advance. So an agent that lost its instructions cannot re-read them without
  ticking the machine. The author's recorded case: a 14-week sweep whose node
  carries a 7,140-character procedure emits it once on week 1; weeks 2 through
  14 receive a 101-character directive and nothing else, and one run logged 14
  consecutive gate-blocked ticks with `details` suppressed throughout.

- **`handle_status` is nearly free to extend.** It already reads events, loads
  and parses the compiled template, and does `compiled.states.get(&current_state)`
  to compute `is_terminal`. The `details` string is one field away, and the
  function already follows a "present only when relevant" convention for
  optional keys. It writes nothing. *(verified directly against
  `src/cli/mod.rs:4834-4960`)*

- **Four further breaks in the shipped gating, none of them in the author's
  audit.** All are code-reading inferences, not measured:
  1. `koto rewind` (`handle_rewind`, `src/cli/mod.rs:1984-2081`) appends a
     `Rewound` event, which `derive_visit_counts` counts. A rewind target is by
     construction a state the log already entered, so its count is already >= 1
     and the rewind pushes it to >= 2. `details` are suppressed on exactly the
     state a rewind is telling the agent to redo. *(lead: first-visit)*
  2. F1 cold-restart respawn (`src/engine/respawn.rs:421-557`) and batch child
     retry (`src/cli/retry.rs`) both continue writing to the *same* session log
     under a *new* agent process. A genuinely zero-context agent inherits a "not
     first visit" verdict it never earned. Respawn is invisible to
     `derive_visit_counts` because it rides the reserved `request_store.*`
     audit-kind convention rather than emitting a transition. *(lead: first-visit)*
  3. The `--to` directed-transition path never applies the visit check at all.
     `dispatch_next` (`src/cli/next.rs:50-54`) sets `details` whenever the state
     has any, with no `full` parameter in its signature. So the shipped behavior
     is not uniformly first-visit-only; it is "first-visit-only except under
     `--to`, where it is always-on." No design doc or code comment says whether
     that is a carve-out or an oversight. *(leads: phase-info, alternatives)*
  4. States crossed during multi-state auto-advance never surface `details` even
     on a true first visit, because only `final_state` is ever checked
     (`src/cli/mod.rs:3977-4013`). *(lead: alternatives)*

- **No comparable system does what #90 proposes.** Agent Skills, MCP,
  LangGraph, AutoGen, and CrewAI were all checked; none gates instruction
  delivery on visit history. Agent Skills' progressive disclosure re-decides on
  every task rather than mechanically suppressing after the first load.
  Temporal is the closest analog and is safe precisely because it never treats
  the LLM's context as the source of truth. Anthropic's own context-editing and
  compaction documentation states that tool-result content -- which is exactly
  what a `koto next` response is -- is compaction-eligible and not guaranteed to
  survive past a turn. MCP's 2026-07 caching work (SEP-2549) explicitly rejects
  inferring freshness and requires an explicit contract plus a cheap refetch
  path. *(lead: external)*

- **Nothing in koto's compatibility surface obstructs either fix.**
  `docs/STABILITY.md` and `koto-stability-tests/` cover the session-log wire
  format (`CURRENT_SCHEMA_VERSION`, `StateFileHeader`, `EventPayload`, four
  frozen `SessionBackend` methods), not the `koto next` response or the CLI
  subcommand surface. There is no CLI deprecation policy to satisfy, and the
  session log is append-only and never compacted, so visit history cannot be
  starved. *(leads: contract, first-visit)*

- **An adoption gap sits downstream, outside koto.** shirabe's `work-on.md` --
  27 states, the largest template in the workspace, still edited 2026-08-03 --
  has roughly 10 states whose directive is a literal "read this phase file",
  about 30 KB across 12 files, and has never adopted `<!-- details -->` despite
  the mechanism existing since March. `koto-author`'s own template dogfoods the
  marker on 2 of 9 states. *(lead: premise)*

### Tensions

- **Is `koto next --full` read-only?** The alternatives lead argues a
  non-advancing `koto next` writes nothing and so `--full` is a safe read. The
  issue author measured that it "also ticks the machine." Both can be true of
  different runs: `koto next` evaluates gates and advances when they pass, so
  whether it mutates depends on the state it lands in -- which is exactly what
  makes it unsafe as a recovery path. An agent recovering from context loss
  cannot know in advance whether its own recovery call will move the workflow.
  This tension is what justifies AC4 rather than dismissing it.

- **Counting emissions instead of transitions makes a read into a write.** The
  fix the author suggests requires koto to record that it emitted `details`,
  which means `koto next` writes on emission. `DESIGN-koto-next-output-contract.md`
  already rejected a persisted counter file, explicitly because R9 forbids new
  state files. Whether an *event* in the existing log counts as "no new state
  file" is an open reading of that constraint, and it is the central design
  question the fix turns on.

- **`phase-info` versus koto's own vocabulary.** "state" is the CLI, engine, and
  skill noun (`src/engine/` 427 occurrences vs 3; `src/cli/` 953 vs 36).
  "phase" is a deliberate, documented translation used only inside
  `workflows_surface`, introduced so the `/workflows` render matches Claude
  Code's UI noun. A CLI `phase-info` would be the first leak of UI vocabulary
  into the state-speaking CLI. The author's latest comment already softens to
  "a read-only `phase-info` (or `status --details`)", and `status --details`
  both stays in the existing vocabulary and lands on a function that is already
  read-only and already holds the compiled template.

- **Suppression optimizes tokens; the recovery path carries correctness.** The
  external evidence inverts the issue's framing. Omission is the optimization;
  the escape hatch is the correctness mechanism. That argues for making
  recovery cheap and discoverable enough that agents reach for it
  speculatively, not for treating it as a rarely-used hatch.

### Gaps

- **Nothing was reproduced against current `main`.** The author measured 0.11.4
  (`eb626d9`, 2026-08-05); `main` is now at 0.11.6-dev. Four of the five
  identified defects -- rewind, respawn/batch retry, `--to`, and auto-advance
  intermediates -- are code-reading inferences that no one has run.
- **The emission-counting fix has no established mechanism.** Whether it needs a
  new `EventPayload` variant, whether that requires a schema-version bump, and
  whether it violates R9's "no new state files" have not been settled.
- **The escape hatch's diff footprint is unmeasured.** `status --details` versus
  a new subcommand versus a `next --dry-run` has not been costed against the
  skills, their evals, `cli-usage.md`, and the error-code envelope enumeration.
- **`caps.rs` was ruled out by grep, not by reading it.**

### Decisions

Recorded in `wip/explore_inline-phase-details_decisions.md`.

### User Focus

Running in `--auto`; no live narrowing turn. The nearest thing to author intent
is the issue author's own comment of 2026-08-16T16:51Z, which is unambiguous and
two hours old: the remaining work is the AC3 non-advancing-re-tick gap plus AC4,
and AC4 is the higher-value half because it is what makes AC3 safe on loops
whose iterations outlive the agent's context. This exploration treats that
comment as the authoritative statement of what matters.

## Round 2

Round 2 ran three leads against the gaps round 1 left: an empirical build-and-run
verification, the emission-counting constraint, and the escape hatch's cost. Two
round-1 leads also revised their own conclusions after being told the feature had
shipped.

### Key Insights

- **All five suspected defects reproduce on current source.** The empirical lead
  built `cargo build --release` at `1e3a515` (source-identical to HEAD `1b35372`;
  `git diff --stat` over `src/`, `Cargo.toml`, `Cargo.lock` is empty) and ran each
  case with real templates and real transcripts. Nothing was left as inference.
  *(lead: r2-repro)*

- **The five defects are three root causes plus one inherited limitation.**
  *(lead: r2-repro)*
  1. *The counter measures the wrong thing.* It counts entries into a state, not
     responses sent about a state. A blocked tick enters nothing, so it increments
     nothing and re-sends forever; a rewind is an entry, so redoing a step
     suppresses. This one root cause produces both the author's measured AC3 gap
     and the rewind defect. Any fix that keeps `derive_visit_counts` as its input
     keeps both symptoms.
  2. *The rule lives on one of two code paths.* `handle_next`'s advance path has
     the check; `dispatch_next` (the `--to` path) has none. That is a missing call
     site, not a tuning problem.
  3. *`--full` is not a read.* Measured: it advanced the test workflow.
  4. *Auto-advance discards crossed states* -- and drops their `directive` too, not
     just `details`. This predates the details feature and is different in kind
     from the other four; it should be named as pre-existing rather than counted
     as a details regression.

- **The behavior is close to exactly backwards from the intent.** The mechanism
  exists to spare an agent from re-reading long guidance on repeat visits.
  Empirically it never suppresses on the repeat case that actually matters
  (blocked retries) and always suppresses on the case where the guidance is wanted
  most (a rewind telling the agent to redo a step). *(lead: r2-repro)*

- **`koto next --full` is definitively unsafe as a recovery call.** Three
  independent mutation paths fire regardless of the flag: a pure-routing state
  auto-advances on empty evidence (`resolve_transition`'s guard,
  `src/engine/advance.rs:759`); any state with a `default_action` re-executes its
  shell command and appends `DefaultActionExecuted` on every call
  (`src/engine/advance.rs:286-314`, confirmed intentional by
  `DESIGN-default-action-execution.md:301-302`); and reaching a terminal state
  triggers session cleanup. `--full` only gates whether `details` appears in the
  final response; it never touches `advance_until_stop`. This resolves round 1's
  open tension in the author's favour. *(leads: alternatives, phase-info, r2-repro)*

- **`koto status` is the only true read seam, and it carries neither field.**
  `dashboard --once`'s eight-column contract carries neither; the dashboard's
  `read_detail`/`DetailData` carries `directive` but not `details` and is not
  scriptable. So no existing command is read-only *and* keyed by workflow name
  *and* returns the text. The recovery path is genuinely greenfield.
  *(lead: phase-info, revised)*

- **R9 forbids new state files and schema-version bumps -- not new events.** The
  emission lead read the requirement verbatim and found three R9-compliant ways to
  record a delivery: a new `EventPayload` variant, an `EvidenceSubmitted` riding
  `audit.rs`'s existing reserved-kind convention, or a new additive
  `StateFileHeader` field. All three are precedented: six `request.`-family
  variants and header fields such as `respawn_generation` were added without ever
  moving `CURRENT_SCHEMA_VERSION`, and the header is rewritable in place rather
  than append-only. *(lead: r2-emission)*

- **Two tempting shortcuts are both wrong.** Counting existing `GateEvaluated`
  events is free but incomplete: an ordinary `accepts`-only state awaiting evidence
  writes nothing on a blocked re-tick, which is likely the more common blocked
  shape. Suppressing on the `advanced` flag alone is cheapest but breaks the core
  scenario, because a true first visit via `koto init` and a blocked retry both
  report `advanced == false`. *(lead: r2-emission)*

- **Extending `koto status` is the cheapest recovery path by a wide margin.**
  `handle_status` already reads the events, loads and parses the compiled template,
  and looks up the current state; the read path exists end to end and its only real
  open question is naming (a flag versus unconditional fields). A `phase-info`
  subcommand has a comparable mechanical footprint but adds genuinely new doc and
  skill sections plus the vocabulary cost. `next --dry-run` is the outlier: its
  cost is not documentation but proving that every side channel inside a large,
  side-effect-heavy `handle_next` can be safely skipped. *(lead: r2-hatch)*

- **Discoverability already has a mechanism.** koto already injects
  koto-authored text into `directive` unconditionally -- the leg-abandonment stop
  notice does exactly this. Since `directive` is the one field guaranteed to reach
  an agent on every tick, it is where a pointer to the recovery command can live
  and survive the context loss it is meant to recover from. *(lead: r2-hatch)*

- **Batch-child retry does not reproduce the stale-count problem.**
  `respawn_skipped_child` and `respawn_failed_child` (`src/cli/retry.rs:571-660`)
  both call `backend.cleanup()` to delete the child's log before reinit, so a
  retried child always starts at count 0. Only F1 cold-restart respawn breaks it,
  because `emit_respawn_event` appends to the same requester state file. This
  narrows round 1's claim. *(lead: alternatives, revised)*

- **The rewind fix should live at the call site, not in `derive_visit_counts`.**
  That function is shared with `src/workflows_surface/project.rs:284-286` for an
  unrelated visited-set purpose, so changing its semantics would ripple. The
  boundary-aware checks -- was the most recent entry event a `Rewound`, and have
  we crossed a respawn marker -- belong at `src/cli/mod.rs:4008-4013`.
  *(lead: alternatives, revised)*

### Three findings nobody asked for

Surfaced by the empirical lead while building repro cases. None belongs to #90;
all three are worth filing separately.

- **`koto rewind` ping-pong.** Two consecutive rewinds move you *forward*. Once you
  are past `s1`, `s1` is unreachable by rewinding. A plain correctness bug in
  `handle_rewind`, independent of details. Its interaction with the
  `materialize_children` epoch-branch relocation that `handle_rewind` also drives
  is untested.
- **`accepts:` does not gate advancement.** A transition without a `when:` clause
  fires unconditionally regardless of any `accepts` block. A linear chain of four
  evidence states ran to terminal and self-cleaned in a single `koto next`. This is
  an authoring trap, and it means templates may be auto-advancing through states
  their authors believe are interactive.
- **Migration-scan stderr flood.** Every koto invocation against a real `~/.koto`
  re-runs a migration scan emitting one `migration skipped` line per legacy
  session -- roughly 20 KB of stderr per command on the user's machine. It broke
  two measurement runs. This is likely the same defect as open issue #193.

### Tensions

- **Is the `--to` carve-out deliberate?** No code comment or design doc says. If
  a directed transition is meant to be explicit operator intent that always
  deserves full context, Claim 3 is not a defect and the count is four, not five.
  This needs an author or design answer, not more measurement. It is the one open
  question the research cannot close.
- **Does auto-advance's discarding of crossed states belong in this scope?** It
  drops the directive too, predates the details feature, and is arguably its own
  issue. Naming it as pre-existing keeps a fix from being mis-scoped as "make
  details work on crossed states" when the honest framing is that auto-advance has
  never surfaced intermediate instructions at all.

### Gaps

- Details behavior under `koto next --with-data` combined with gate failure -- the
  accepts-fallthrough path at `src/cli/next.rs:56-69` -- was not measured. It also
  carries `details.clone()` and may have its own visit-count interaction.
- Whether `koto status`'s JSON output has any stability expectation that would make
  unconditional new fields a breaking change. Nothing documents one the way
  `docs/STABILITY.md` documents the Rust surface.

### Decisions

D5 and D6 in `wip/explore_inline-phase-details_decisions.md`.

### User Focus

Still `--auto`. Round 2 confirmed the issue author's 2026-08-16 audit on every
point it overlapped and extended it: the author measured one defect, and the same
root cause produces a second (rewind) that the author had not tested.

## Accumulated Understanding

koto#90 is not a feature request any more. It is a correction to a feature that
shipped underneath it. The template marker, the response field, the visit-count
derivation, and the `--full` override all exist and are documented in a Done PRD
(R9 of `PRD-koto-next-output-contract.md`, delivered by PR #109 on 2026-03-30,
four days after #90 was filed and without ever citing it). The issue was simply
never closed, and its body still describes a YAML template format koto never had.

What the two rounds established is that the shipped mechanism does close to the
opposite of what it was built for, and that this is one defect wearing several
faces. koto counts *entries into* a state when the quantity that matters is
*deliveries of its instructions*. Those two diverge in both directions and the
divergence is measured, not argued. A blocked tick enters nothing, so the count
never moves and the details are re-sent on every retry -- forever, on exactly the
repeat case the feature exists to suppress. A rewind *is* an entry, so redoing a
step suppresses the instructions the agent was just told to redo. One wrong
predicate, two opposite symptoms. Alongside it sits a missing call site: the
`--to` path runs through `dispatch_next`, which has no visit check at all, so the
contract is not "first visit only" but "first visit only, except under `--to`,
where it is always on."

The second half of the work follows from something koto cannot fix by tuning the
predicate. The log records no notion of who is attached to the session, so no
log-derived rule can distinguish an agent that still holds the instructions from
one that does not. F1 cold-restart respawn makes that concrete: a brand-new
zero-context subagent continues on the predecessor's log and inherits its visit
count. Silent context compaction is worse, because it leaves no event at all.
Every comparable system checked -- Agent Skills, MCP, LangGraph, Temporal,
AutoGen, CrewAI -- declined to gate instruction delivery on visit history, and
Anthropic's own documentation states that tool-result content is
compaction-eligible and not guaranteed to survive a turn. So the omission has to
be treated as an optimization whose safety comes from a cheap, non-mutating
recovery path, not as a correctness guarantee.

That path does not exist today, and round 2 killed the hope that it already did.
`koto next --full` was the candidate; it is measurably not a read, because it
evaluates gates, can auto-advance a routing state, re-executes any
`default_action` shell command, and can trigger terminal session cleanup -- none
of which the flag suppresses. `koto status` is the only genuine read seam and it
returns neither the directive nor the details. The good news is that closing the
gap is cheap: `handle_status` already reads the events, already parses the
compiled template, and already looks up the current state, so the text is one
field away from a function that writes nothing. R9 turns out to forbid new state
*files* and schema-version bumps, not new events, and three precedented
R9-compliant ways to record a delivery exist.

So the work is bounded, single-repo, and one coherent feature -- but it is not a
one-liner, and it is not four independent fixes either. It carries a real design
decision about what to record and where (constrained by R9, with two tempting
shortcuts already ruled out on evidence), a naming decision the codebase has a
strong opinion about (`state` outnumbers `phase` 953 to 36 in the CLI, and
`phase` is a deliberate UI-only translation), an open question about whether the
`--to` carve-out is intentional that only the author can answer, a scope
judgment about whether auto-advance's pre-existing habit of discarding crossed
states belongs here at all, and mandatory downstream skill, eval, and
documentation work that koto's own CLAUDE.md requires for any change under
`src/cli/` or `src/engine/`. Three unrelated bugs surfaced along the way and
belong in their own issues.

## Decision: Crystallize
