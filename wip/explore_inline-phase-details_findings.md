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

## Accumulated Understanding

koto#90 is not a feature request any more. It is a correction to a feature that
shipped underneath it. The template marker, the response field, the visit-count
derivation, and the `--full` override all exist and are documented in a Done PRD;
the issue simply was never closed, and its body still describes a YAML template
format koto never had.

What remains is one coherent piece of work with two halves that share a root
cause. koto counts *state entries* when the quantity that matters is
*deliveries*. Because entries and deliveries diverge, the suppression is wrong
in both directions: it re-sends details to an agent that already has them (the
gate-blocked re-tick the author measured, and every `--to` transition), and it
withholds details from an agent that does not (a rewind target, a respawned or
retried agent on the same log, and any state crossed mid-auto-advance). Neither
direction can be fixed by tuning the predicate, because the log records no
notion of who is attached to the session.

That is the argument for the second half. Since koto cannot reliably know
whether the caller still holds the instructions, the omission has to be treated
as an optimization with an explicit, cheap, non-mutating recovery path rather
than as a correctness guarantee -- which is exactly what the external evidence
says every comparable system concluded, and what Anthropic's own compaction
documentation forces. Today there is no such path: `koto status` is read-only
but does not carry the text, and `koto next --full` carries the text but can
advance the machine. Closing that gap is a small change against a function that
already loads the compiled template and already writes nothing.

The work is bounded, single-repo, and single-feature, but it is not a one-liner:
it carries a real design decision about what to count and where to record it
(constrained by R9's "no new state files"), a naming decision that the codebase
has an opinion about, a set of edge cases that need reproducing before they are
believed, and downstream skill and documentation updates that koto's own
CLAUDE.md makes mandatory for any change to `src/cli/` or `src/engine/`.
