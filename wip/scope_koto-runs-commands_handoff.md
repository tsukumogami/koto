# /scope Handoff: koto-runs-commands

## Author Ruling — read first

Recorded 2026-08-20, after the exploration concluded, in response to the
author's direct correction. It overturns a conclusion two leads reached
independently, so it sits above everything else in this file.

The exploration found that a `default_action` runs `sh -c` from the koto binary
rather than through the agent's tool layer, so a user's allow/deny/ask rules for
`git push` or `gh pr create` never see it. It treated that as the strongest
objection to the whole direction. The mechanism is real; the conclusion was
wrong.

**That behavior is the intent.** Loading a skill that drives koto is itself the
broad grant — invoking a koto-backed workflow authorizes every command that
workflow bakes in, deliberately, with the risk acknowledged and accepted.
Consent moves from per-command prompting to the decision to run the workflow at
all, and that relocation is the feature: it is what lets koto carry mechanical
work without interrupting the agent at every step.

Do not re-derive this as an objection. Anywhere below where the permission
surface is described as a constraint, this section supersedes it. What follows
from the ruling:

- The permission argument is struck as a reason to keep any command with the
  agent, and no longer constrains scope.
- The carve-out for remote mutations survives on narrower grounds only —
  irreversibility, and the fact that failure diagnosis is currently blind.
  Both are fixable, the second by plumbing this chain already scopes.
- Conversion scope is therefore **wider** than the figures recorded here.
  Treat every published yield as a floor.
- `requires_confirmation` matters more, not less: it is the only in-band
  checkpoint left, which makes its after-the-fact firing a defect worth fixing.
- Untouched by the ruling: action output is persisted to an event log committed
  to feature branches, so a command whose output carries a secret leaks it.
  That is about what gets written down, not who authorized the command.

## Provenance

Written by `/explore` on 2026-08-20 from
`wip/explore_koto-runs-commands_crystallize.md`.

**The research lives in the shirabe repository, not this one.** The exploration
started from a symptom in shirabe's templates and was authored on the
`docs/koto-runs-commands` branch of `tsukumogami/shirabe`. Crystallize placed
the chain here because every open design question is koto's. The files:

- `wip/explore_koto-runs-commands_findings.md` (shirabe) — accumulated findings
  across three rounds, plus the running synthesis.
- `wip/explore_koto-runs-commands_decisions.md` (shirabe) — what was settled and
  why, per round.
- `wip/explore_koto-runs-commands_crystallize.md` (shirabe) — the scoring.
- `wip/research/explore_koto-runs-commands_r{1,2,3}_lead-*.md` (shirabe) —
  twenty-one lead files plus two rounds of orchestrator probes run against the
  shipped `koto 0.11.6` binary.

Three discover-converge rounds. Round 1 established what exists. Round 2
designed the koto-side changes, mapped both templates state by state, and ran an
adversarial lead against the whole premise. Round 3 adjudicated the resulting
disagreement, swept for missed surface, and sequenced the work. The author
narrowed along the way from "should koto run these commands" to a stated
principle for which commands qualify, and promoted two defects found in passing
above the conversion question itself.

## Problem Statement

koto's `default_action` lets a template state run a command during `koto next`.
It shipped in March 2026, works in the current binary, and no template in the
workspace uses it — so shirabe's workflows hand agents mechanical shell in prose
("then run `git rev-parse --abbrev-ref HEAD` to get the current branch") that
the engine could run itself. The mechanism is not what is missing. What is
missing is everything around the command: koto captures the output and discards
it, so a command whose value is its output has nowhere to put it; a failing
action changes nothing at all, so there is no failure path to fall back from;
and the command runs in whatever directory `koto next` was invoked from, with no
binding between a session and the tree it was created in. Two defects in the
shared execution layer sit underneath all three.

## Scope Boundary

### In scope

- koto's command-execution surface end to end: what runs, where it runs, what
  happens to the output, and what happens when it fails.
- The two defects in `run_shell_command` and the session-migration path that
  make the surface unreliable as it stands.
- A durable statement of which commands an engine should run and which belong to
  the agent, so template authors can apply it without re-deriving it.
- Authoring documentation for `default_action`, which currently amounts to one
  table row and a Rust integration test.

### Out of scope

- shirabe's template rewrite. It depends on this work and warrants its own run
  in that repository afterward; the per-state maps are already written and
  waiting in the research files.
- Retiring the eight retry-clearing blocks in `/work-on`. Blocked behind a
  shirabe design decision, not behind engine work — see Coverage Notes.
- The hardcoded-command surface in shirabe's eighteen non-koto skills.
  `default_action` is structurally inert outside a koto-backed template, so no
  koto change reaches it.
- CI monitoring as a typed integration. koto's own design names that as the
  right long-term home for `ci_monitor`, but it is a separate feature.

## Decisions Already Settled

- **This is an unused capability plus three engine gaps, not poor use alone.**
  Verified running: the capability works, shirabe uses it zero times, and output
  routing, failure propagation, and execution anchoring each block a distinct
  part of the goal.
- **No prior rejection exists to respect.** No design, issue, or PR in either
  repo records a decision to skip `default_action`. It was dropped, not declined.
- **No `on_failure:` schema field.** Gates were always the intended arbiter of
  success. The real gaps are that the failure response variants do not carry the
  action's output, and that a state with an action and no gates has no failure
  detection at all.
- **`requires_confirmation` is not the failure mechanism.** It fires
  unconditionally, after execution, on success and failure alike — so an
  irreversible action declared with it has already run by the time anyone is
  asked.
- **`capture_stdout_as:` is the working answer for output routing**, over
  populating `action_output` on every stop reason. Once auto-advance chains
  through several states, the acting state and the stopping state diverge, which
  makes the latter the more invasive change despite looking smaller.
- **Conversion is scoped by a principle, not a percentage** (as amended by the
  Author Ruling). koto runs a step when it is isolated to its own state and
  gate-verifiable independent of the action's own exit code. Reversible,
  repo-local steps convert now. Irreversible outward-facing steps — PR creation,
  comments, pushes to shared branches — convert once failure output reaches the
  agent; they are deferred on diagnosability and irreversibility, never on
  authorization. Only commands needing per-repo knowledge to know what to run
  stay agent-run, and that set shrinks once a `TEST_COMMAND` style variable
  carries the answer.
- **A blanket read-only restriction is rejected.** Applied line by line it
  collapses the yield to nothing and cannot be enforced at compile time.
- **Engine-run commands bypass the agent's permission layer by design.** No
  preview-before-execution mechanism exists in koto, and building one would
  reproduce the prose-plus-gate pattern already in place. Per the Author Ruling
  above, this is the intended trade, not a constraint on scope.
- **Defect severity is stated as latent, not active.** Measured: `go test ./...`
  across 63 packages on the tsuku monorepo emits 3,793 bytes, well under the
  trigger, and only one of eleven shipped gates writes captured stdout at all.

## Coverage Notes

- **Whether koto-store writes count as side effects** under the conversion
  principle is unresolved, and it is the hinge for how much of `/work-on`
  remains convertible. The principle names git and remote mutations; a write to
  koto's own context store is a different risk class and nobody decided which.
- **The retry-clearing question needs a shirabe decision this chain cannot
  make.** `docs/designs/current/DESIGN-work-on-retry-clearing.md` is marked
  Current and chose manual clear-and-verify deliberately, reasoning that a
  uniform superset-clearing rule beats a per-edge mechanism, at a time when
  `context_assignments` was unimplemented. Implementing koto issue #204 retires
  nothing in shirabe until that doc is revisited on its own terms. Either
  outcome is legitimate; implementing the primitive because the issue exists is
  not.
- **Three design questions on execution anchoring are open**: whether the anchor
  defaults silently or requires an explicit flag, whether pre-existing sessions
  refuse until bound or warn once, and whether the check is root-equality or
  root-containment.
- **Two design questions on `capture_stdout_as`**: silent-skip versus hard-fail
  when a captured value fails the variable pattern, and single-trimmed-string
  versus multi-value capture.
- **A defect is reproduced but unfiled**: a nested `koto next --with-data` inside
  an action advances its session to terminal while the outer invocation returns
  `advanced: false` with the original state, and a follow-up `koto status`
  reports the session missing.
- **`check-staleness.sh` could not be read** — it is not in shirabe's tree — so
  one gate's exposure to the output defect is unverified. Its stdout is consumed
  internally by `jq -e`, so the only open channel is the script's own stderr.
- **There is no existing test coverage to extend for the output defect.** A
  search of `tests/integration_test.rs` and `action.rs`'s own unit tests found
  zero tests exercising output above the buffer or the truncation path at all,
  so the fix carries new tests rather than modified ones. The polling loop and
  the default-action path both call the same function, so it is one fix and not
  several; the only other piped spawn in `src/` reads its child line by line
  while it runs and is a different shape.
- **Where the authoring documentation belongs** is unsettled: extending
  `template-format.md`'s existing layer structure, or a new layer, since
  `default_action` changes engine behavior rather than being a routing primitive.

## Upstream Observations

`docs/designs/current/DESIGN-default-action-execution.md` (this repo) is the
design that shipped the capability, spawned from issue #71 and parented by
`DESIGN-shirabe-work-on-template.md`. It states the automation-first principle
and claims roughly 42% of skill instructions could be eliminated. That claim
does not hold today and is comfortably exceeded after the output defect is
fixed, so the document is worth reading as the origin of the intent rather than
as a current estimate. Its Consequences section already concedes the two risks
the exploration's adversarial lead raised independently — action output landing
in a committed event log, and the engine's inability to determine reversibility
automatically — mitigated only by documentation.

`DESIGN-shirabe-work-on-template.md` (this repo) carries the three-path model
this exploration verified is reachable today, and names `setup_issue_backed` and
`setup_free_form` as conversion targets. Round 3 found both bundle mechanical
work with genuine judgment, so those two named targets do not survive scrutiny
as drawn.

koto issue #193 already tracks the migration-warning volume, filed from a
different angle — log noise during direct CLI use — with no mention of the
deadlock it triggers. koto issue #204 tracks `context_assignments` being
silently discarded while koto's own W5 warning recommends it.

`docs/designs/current/DESIGN-work-on-retry-clearing.md` (shirabe, status
Current) is named here because it constrains one item; it is not an upstream
this chain consumes.

No ROADMAP covers this topic, so no `--upstream` flag applies.

## Framing-Shift Answer

**Pre-supplied answer:** yes, the framing shifted.

**Evidence:** The exploration opened as "is this a missing koto feature or poor
koto use in shirabe" and that question was settled in round 1 — neither, exactly:
a shipped capability nobody adopted, plus three specific gaps. The framing moved
twice more after that. Round 2's adversarial lead reframed the question from
"how much can we convert" to "which commands should an engine run at all",
producing a permission-surface finding that the author has since ruled is the
intended design rather than a constraint. Round 3 then moved the centre of gravity off conversion entirely:
the two highest-priority items are defects in the shared execution layer that
predate the question, and the item that most directly answers the original
concern about side effects in the wrong place — execution anchoring — is
independent of `default_action` adoption altogether, because gates already run
against the same unguarded working directory. The problem this chain should
specify is narrower and more fundamental than the one the exploration was sent
to look at.

## Shape Signals

### Architectural alternatives left open

- **Output routing**: four options were costed and one recommended, not settled.
  `capture_stdout_as:` with an additive `VariableCaptured` event reaches a later
  state's prose and avoids the response-contract surface, but carries a
  same-tick staleness trap — variables must be rebuilt after the advance loop,
  not before, or it silently fails on exactly the auto-advance case it exists
  for. Populating `action_output` on every stop reason is simpler to describe
  and more invasive to build: five variants, the hand-rolled `Serialize` impl,
  and three exhaustive-match combinators. Merging output into per-state evidence
  is cheap and useful as a same-state guard but cannot reach a later state.
  Writing to the context store is cheap and reintroduces the manual step the
  work exists to remove.
- **Failure detection for a gate-less action**: synthesizing a failed gate
  result under a reserved key feeds the existing block logic unchanged and
  leaves every state that already declares gates untouched. The alternative —
  changing what a non-zero exit means generally — contradicts the design's own
  model, in which gates are the arbiter.
- **Execution anchoring**: record-and-refuse versus a required `working_dir`
  with a containment check versus an explicit `--cwd` on every call versus a
  session-bind verb. The recommendation combines record-and-refuse with
  join-and-canonicalize and adds a bind verb for the deliberate case, but the
  backward-compatibility behavior for sessions with no anchor is unsettled, and
  that choice determines whether this ships as a silent improvement or a
  breaking change.
- **Where the containment and spawn failures surface**: bundled into the
  anchoring change as a structured error type, or shipped first as a narrow
  standalone fix so any spawn failure becomes distinguishable from a generic
  `exit_code: -1`.

### Complexity signals

- The work spans two repositories with a real dependency between them, and one
  item requires arguing against a design document currently marked Current in
  the other repo.
- Two items touch the event log and one touches the state-file header, which is
  where koto's compatibility discipline concentrates — there is a response
  baseline test, a session-feed spec, and a convention that comparably narrow
  features in this area each got their own design doc.
- The estimate of what is convertible moved three times during the exploration
  as the diagnosis of one defect was corrected, from "roughly half", to "32%
  ceiling, structurally excluded", to a three-horizon range: 15% today, 62%
  after the output defect is fixed, 70% with a further capability, ceiling near
  79%. An estimate that unstable under a single corrected fact is itself a
  complexity signal.
- Two of the adversarial lead's three objections stand; the third has been
  ruled on. A compiled command string cannot span the tooling variety of the
  repositories a shipped plugin targets, and an incident already on record —
  twelve child workflows dispatched against a branch nobody created, because an
  error was filtered away — is the exact silent-failure shape that converting a
  step without the failure plumbing would reproduce. Its permission-surface
  objection does not stand: see the Author Ruling above.
- The conversion principle needs to survive contact with real states. The two
  states koto's own design named as targets both fail it, which means adoption
  implies splitting states rather than annotating them, and state granularity is
  a template-design question this chain should answer even though the rewrite
  happens elsewhere.
