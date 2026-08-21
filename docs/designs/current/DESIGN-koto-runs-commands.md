---
schema: design/v1
status: Current
upstream: docs/prds/PRD-koto-runs-commands.md
problem: |
  `default_action` runs a command and then loses everything about it: the
  output is discarded by the advance loop, a non-zero exit changes nothing
  when the state declares no gates, and the command runs in whatever
  directory `koto next` happened to be typed in. Underneath sit two defects
  in the shared command runner that reach gate authors in the current
  release -- the parent waits on the child before draining its pipes, so a
  large-output command deadlocks and is misreported as a timeout, and a
  migration cleanup that fails on its skip branch reprints the same warning
  on every invocation forever.
decision: |
  A `capture_stdout_as:` field on the action declares a name; the engine
  appends an additive `VariableCaptured` event and keeps a live overlay the
  advance loop and the final response both read, so a captured value reaches
  a later state's prose within the same tick. A failing action -- non-zero
  exit, spawn failure, timeout, or a capture that cannot be delivered --
  synthesizes a reserved gate result that feeds the existing block path,
  stops the tick before the state's own gates run, and returns the command's
  facts in `blocking_conditions` with the action's `fallback:` prose spliced
  onto the directive. Sessions record a canonical `execution_dir` at init,
  every tick must be at or beneath it, and every command runs at the anchor
  itself. The shared runner drains its pipes on reader threads and reports a
  typed failure kind instead of an overloaded `-1`.
rationale: |
  Routing through variable substitution reuses a path that already exists,
  already validates values against a shell-safe allowlist, and already
  rejects undeclared names at compile time -- so it costs one event variant
  and no change to the seven-variant response contract, its hand-rolled
  Serialize impl, or its three exhaustive combinators. Synthesizing a gate
  result rather than redefining what a non-zero exit means keeps gates the
  arbiter of success, as PRD decision D5 requires. The beneath-the-anchor
  test plus execute-at-anchor was chosen over root equality because it refuses the
  wrong-tree case just as firmly while making a command's behavior
  independent of which subdirectory the developer stood in.
---

# DESIGN: koto runs the mechanical commands

## Status

Current

## Upstream Design Reference

`docs/designs/current/DESIGN-default-action-execution.md` shipped the
`default_action` capability this design extends. Two of its claims are
superseded here, and both supersessions are deliberate.

Its "safety via reversibility" constraint (lines 55-58, 71, 442-445) holds that
only reversible actions auto-execute and that `requires_confirmation` is what
keeps an irreversible action from running unattended. The flag does not do
that: it runs the command and only then asks. This design does not fix the flag
— renaming or reworking it stays out of scope per the PRD — but it replaces the
reversibility framing with the published rule of R21, which asks whether a
command's risk lives in a bad success or only in a bad failure. That rule
classifies commands before they are written into a template, which is where the
decision actually belongs.

Its advance-loop ordering (lines 119-120) has the action's confirmation check
fire before gate evaluation, and gate evaluation stop the loop on failure.
Decision 3 below inserts a failure check ahead of the confirmation branch and
short-circuits the state's gates when the action failed.

## Context and Problem Statement

`PRD-koto-runs-commands.md` states four obligations and deliberately declines
to pick mechanisms for most of them (PRD decision D4). This design picks them.

The starting position, verified against the tree at the time of writing:

- `run_shell_command` (`src/action.rs:26-107`) spawns `sh -c` with piped
  stdout and stderr, then calls `child.wait_timeout(...)` at line 60 and only
  reads the pipes at lines 62-79, inside the `Ok(Some(status))` arm. A child
  that writes more than the OS pipe buffer blocks on its own `write`, the
  wait expires, the process group is killed, and the `Ok(None)` arm at lines
  86-100 returns empty stdout, empty stderr, and `exit_code: -1` with the
  text "command timed out". Every byte is lost and the cause is misreported.
- The same `-1` is returned for three unrelated conditions: spawn failure
  (line 53), timeout (line 96), and a wait error (line 102). The gate
  evaluator disambiguates them by substring-matching the stderr text
  (`src/gate.rs:209-221`), which is the only signal available.
- The advance loop discards the action's output. `src/engine/advance.rs:291-293`
  matches `ActionResult::Executed { .. }` and keeps none of the three fields.
- The gate block is guarded by `if !template_state.gates.is_empty()`
  (`src/engine/advance.rs:326`), so a state with an action and no gates has no
  failure detection at all.
- `requires_confirmation` is checked after execution regardless of outcome
  (`src/cli/mod.rs:4038-4051`), so a failing command produces a confirm stop
  rather than a failure stop.
- Variable substitution is built once, before the loop.
  `Variables::from_events` folds exactly one `WorkflowInitialized` event
  (`src/engine/substitute.rs:62-77`); `handle_next` calls it at
  `src/cli/mod.rs:3177`; `advance_until_stop` runs at `src/cli/mod.rs:4053`;
  the response's directive is substituted with that same pre-loop snapshot at
  `src/cli/mod.rs:4282-4284`. The loop's own `vars.*` when-clause map is
  likewise built before the first iteration (`src/engine/advance.rs:202-210`).
- `handle_next` reads `std::env::current_dir()` fresh on every tick
  (`src/cli/mod.rs:3082`) and hands it unchecked to the gate closure
  (`src/cli/mod.rs:3954`) and the action closure (`src/cli/mod.rs:3979-3983`).
  Nothing in `StateFileHeader` (`src/engine/types.rs:223-...`) records a
  working tree.
- `migrate_if_needed` (`src/session/local.rs:657-720`) prints
  "migration skipped" on a name collision without moving anything, so
  `migrated_count` stays zero, the old-layout directory stays non-empty, the
  trailing `fs::remove_dir` fails silently, and the next invocation walks the
  same directory and prints the same line again. Forever.

Two details in the tree correct the upstream research. The PRD cites
`src/gate/mod.rs` and `docs/template-format.md`; the live paths are
`src/gate.rs` and
`plugins/koto-skills/skills/koto-author/references/template-format.md`.
Neither changes any conclusion.

## Decision Drivers

- **The response contract is the expensive surface.** `NextResponse` has
  seven variants with a hand-rolled `Serialize` impl and three exhaustive
  combinators (`with_substituted_directive`, `with_directive_prefix`,
  `with_details_suppressed_unless_full`), plus a byte-pinned baseline in
  `tests/next_response_baseline.rs`. A design that adds a field to five
  variants pays four sweeps and risks the baseline. A design that reuses an
  existing `serde_json::Value` payload pays nothing.
- **Additive event variants are cheap and unversioned.** `docs/STABILITY.md`
  states that additive change does not move `CURRENT_SCHEMA_VERSION`. A new
  `EventPayload` variant costs four fixed touchpoints.
- **Substituted values land in `sh -c`.** `VALUE_PATTERN`
  (`src/engine/substitute.rs:29`) is an allowlist precisely because a
  substituted value can reach a shell command. Anything that becomes
  substitutable must pass the same allowlist.
- **Compile time beats run time.** koto already rejects a `{{KEY}}` that
  names no declared variable (`src/template/types.rs:782-841`). Whatever the
  capture mechanism is, it must keep that check working, because R4's typo
  case depends on it.
- **R6 is unconditional.** "A failing `default_action` stops the workflow at
  the state that ran it" admits no exception for states that declare gates.
  Any design where a passing gate can rescue a failed action violates it.
- **No template declares a `default_action` today.** The PRD establishes this.
  It means a behavior change confined to states with an action reaches zero
  shipped content, which widens what counts as additive under R24.
- **A nested `koto` call deadlocks on the session lock.** The exploration's
  empirical probe established that any design routing state through a
  subprocess `koto` invocation cannot work. Everything here writes state
  in-process.

## Considered Options

### Decision 1: how a command's output reaches a later state

| Option | Reaches a later state | Response contract | Cost |
|--------|----------------------|-------------------|------|
| A. `capture_stdout_as:` -> additive event -> variable substitution | Yes | Untouched | One field, one event variant, one compile check, a live overlay |
| B. Populate an action-output field on every response variant | No, not on its own | Five variants, Serialize, three combinators, every construction site | Largest |
| C. Merge output into the same state's gate evidence | No | Untouched | Small |
| D. Write output to the context store | Only via a manual `koto context get` | Untouched | Small |

**Chosen: A.**

Option B is the one that looks smallest and is not. The state that ran the
action is usually not the state the loop stops in — step 5 falls through to
gates and then to an unconditional transition, all inside one
`advance_until_stop` call, so "the action's output" and "this response's
state" diverge on exactly the auto-advance path R2 targets. Making the field
meaningful requires threading a most-recent-action value through
`AdvanceResult`, five `NextResponse` variants, the hand-rolled `Serialize`,
three exhaustive combinators, and every construction site in `next.rs` and
`mod.rs` — and at the end of it the agent still has to copy the value into the
next state's evidence by hand, which is the manual step this feature exists to
delete. B is rejected as the mechanism for R1/R2. A narrow slice of it is
adopted for the failure path under Decision 4, where the acting state and the
stopping state are the same state by construction.

Option C is genuinely useful and genuinely insufficient: `current_evidence` is
reset to an empty map on every transition (`src/engine/advance.rs:514`,
`:558`), so gate-evidence merging can never cross a state boundary. It is not
adopted; a template that needs same-state routing on the command's result gets
it through Decision 3's reserved condition instead.

Option D reintroduces the manual retrieval step and is rejected on that alone.

Option A's cost is one `Option<String>` field on `ActionDecl`, one additive
`EventPayload::VariableCaptured` variant with its four fixed touchpoints, one
compile-time check, and the staleness fix described below. It touches no
response field, no `Serialize` arm, and no combinator.

**The staleness trap, verified.** Wiring `VariableCaptured` into
`Variables::from_events` and stopping there passes a mental test and fails in
practice. `variables` is built at `src/cli/mod.rs:3177` from the events read
before the loop; the loop runs at `:4053`; the response directive is
substituted with that same binding at `:4282-4284`. An event appended during
the loop is on disk and invisible to the in-memory binding. A state that
captures `BRANCH` and auto-advances into a state whose directive reads
`{{BRANCH}}` — the whole point of R2 — would render the stale value or the
raw token.

Verification also found a second staleness site the research did not name:
`advance_until_stop` builds its own `workflow_variables` map before the first
iteration (`src/engine/advance.rs:202-210`) and uses it for `vars.*`
when-clause evaluation on every subsequent iteration. And a third: the gate
closure (`src/cli/mod.rs:3942-3950`) and action closure (`:3978`) substitute
into command strings using the same pre-loop binding, so a *later* state's
gate command or action command in the same tick would also see stale values.

Rebuilding after the loop, as the exploration recommended, fixes only the
first of the three. The design therefore uses a live overlay rather than a
rebuild — see Solution Architecture.

### Decision 2: where a capture name is declared, and what an undelivered name does

| Option | R4 typo case | R4 unset case | R5 duplicate case |
|--------|--------------|---------------|-------------------|
| A. Reuse the template's `variables:` block, empty default | Compile error, free | Renders empty string — **violates R4** | Undefined |
| B. `capture_stdout_as:` declares the name in its own namespace | Compile error, via a union check | Typed run-time stop | Compile error |
| C. No declaration; names appear at runtime | Not caught | Raw token — violates R4 | Undefined |

**Chosen: B.**

Option A is what the exploration sketched, and it does not survive contact
with R4. Declaring `BRANCH` in the `variables:` block with an empty default
means `koto init` materializes it (Issue #141), so a run that never enters the
producing state renders `{{BRANCH}}` as the empty string — which R4 forbids by
name. Option A also leaves `koto init --var BRANCH=main` accepted for a name
the engine is about to overwrite, and says nothing about two states declaring
the same name.

Under B, `capture_stdout_as: BRANCH` is itself the declaration. The compiler
collects every capture name in the template and validates `{{KEY}}` references
against the union of the `variables:` block, the capture names, and
`RUNTIME_VARIABLE_NAMES` — so the existing check at
`src/template/types.rs:782-841` keeps rejecting typos with the same message
shape. The compiler rejects a capture name that collides with a declared
variable, with a reserved runtime name, or with another state's capture name,
which answers R5's duplicate question at compile time. `koto init` rejects
`--var` for a capture name for the same reason it rejects an unknown variable.

An unset capture name reaching substitution is a typed run-time stop naming
the variable and the state that would have delivered it, not an empty string
and not a raw token. Option C is listed only to record that the current
`substitute` behavior — pass the literal token through unchanged
(`src/engine/substitute.rs:136-141`) — is what R4 exists to prevent for these
names, and must be overridden for them specifically. Declared `variables:`
keep their current pass-through behavior; nothing about existing templates
changes.

### Decision 3: failure detection, and what happens to the state's own gates

| Option | Gate-less state | Gated state | Keeps gates the arbiter |
|--------|-----------------|-------------|-------------------------|
| A. Synthesize a reserved failed gate result; skip the state's real gates | Detected | Detected, gates not run | Yes |
| B. Synthesize a reserved gate result; still run the state's real gates | Detected | Gates can override — **violates R6** | Yes |
| C. Redefine non-zero exit as a stop throughout the engine | Detected | Detected | No — contradicts PRD D5 |
| D. Add an `on_failure:` policy field to the schema | Author's choice | Author's choice | No — PRD D5 forbids it |

**Chosen: A.**

C and D are closed by PRD decision D5 and are recorded here only so the
rejection is visible: gates were always koto's intended arbiter of success,
and a general redefinition of exit-code semantics would change behavior for
every state that already declares gates.

The live choice is A versus B, and the exploration recommended B — "synthesize
a failed gate result for the gate-less case, leaving every state that already
declares gates untouched." That recommendation is not compatible with R6.
Under B, a state whose action exits non-zero and whose gates then pass would
advance, which is precisely the silent advance R6 forbids, and the failure
would be invisible in the response. R6 admits no exception for gated states,
so the synthesized failure has to short-circuit.

A therefore stops the tick at the state that ran the failing command and does
not evaluate that state's gates. The rule is one sentence an author can hold:
**a state's gates judge the work the action did; when the action did not
happen, there is nothing for them to judge.** It also means one code path and
one documented behavior instead of two, and it avoids running gate commands —
which may be slow or have side effects of their own — after the step they were
written to check has already failed.

The behavior change reaches zero shipped templates, because no template
declares a `default_action`.

The reserved condition name is `__action__`. The compiler rejects a gate
declared with that name so an author cannot collide with it.

**What counts as a failure.** Per PRD decision D8, four conditions, all of
which produce the same stop:

1. a non-zero exit,
2. a command that could not be spawned,
3. a command killed for exceeding its timeout,
4. a declared capture whose value cannot be delivered (Decision 5).

**Ordering against `requires_confirmation`.** The failure check runs before
the confirmation branch. Today `requires_confirmation` fires on success and
failure alike (`src/cli/mod.rs:4038-4051`), so a failing command produces a
confirm stop that carries no indication anything went wrong. After this
change, a failing action produces an action-failure stop whether or not the
flag is set; the confirm stop is reached only on success. This does not rename
or remove the flag — that stays out of scope per the PRD.

### Decision 4: how the command's facts and the fallback prose reach the agent

| Option | R8 facts | R9 prose | Response contract |
|--------|----------|----------|-------------------|
| A. Reserved `BlockingCondition` payload + directive prefix | In `blocking_conditions[].output` | Spliced onto `directive` | Untouched |
| B. New `ActionFailed` response variant | Dedicated fields | Dedicated field | Eighth variant, Serialize arm, three combinators, new baseline |
| C. Field on `GateBlocked` and `EvidenceRequired` only | Dedicated fields | Dedicated field | Two variants, Serialize, three combinators |

**Chosen: A.**

Decision 3 routes an action failure through `StopReason::GateBlocked`, which
already produces a `NextResponse::GateBlocked` carrying
`blocking_conditions: Vec<BlockingCondition>`. `BlockingCondition` already has
an `output: serde_json::Value` (`src/cli/next_types.rs:770-784`) and a
`condition_type` string. A condition named `__action__` with
`condition_type: "action"` gives R10 its machine-readable discriminator, and
its `output` object carries the command string, the exit status, stdout,
stderr, the typed failure kind, and the truncation flag. That is R8 satisfied
with no new struct field anywhere.

The prose is spliced onto the directive with `with_directive_prefix`
(`src/cli/next_types.rs:245`), the same mechanism the recovery pointer and the
leg-abandonment notice already use. Prose goes on the directive rather than
into `details` because `details` is subject to
`with_details_suppressed_unless_full`, and a fallback the agent may not
receive is not a fallback.

B is the honest, legible option and costs an eighth variant plus four sweeps
plus a new baseline fixture for a stop that is, structurally, a blocked state.
C splits the payload across two variants and still pays the combinator sweeps.
Neither buys anything A does not deliver.

`ActionRequiresConfirmation` keeps its existing `action_output` field
unchanged; nothing about the confirm path's wire shape moves.

### Decision 5: where the fallback prose lives in the template

PRD decision D7 settles that the fallback is distinct from the directive and
leaves naming and shape to this design.

| Option | Reads as |
|--------|----------|
| A. `fallback:` on the action declaration | The action's fallback |
| B. `on_failure:` on the state | A handler — collides with PRD D5's wording |
| C. A second directive variant on the state | A per-outcome directive; larger schema change |

**Chosen: A** — `default_action.fallback`, a prose string alongside `command`,
`working_dir`, `requires_confirmation`, `capture_stdout_as`, and `polling`.

The failure being described is the action's, so the prose belongs on the
action. `on_failure:` is rejected on its name: PRD decision D5 says in so many
words that there is no `on_failure:` field, meaning no per-state failure
*policy*, and reusing the name for a prose field would read as reopening a
settled decision. A state that declares no `fallback` still compiles and still
stops on failure; the response simply carries no prefix.

### Decision 6: recording the anchor

| Option | R24 additive | Breaks existing callers |
|--------|--------------|-------------------------|
| A. Default to the `koto init` cwd, `--execution-dir` to override | Yes | No |
| B. Require an explicit flag on `koto init` | No | Every existing invocation and every skill |

**Chosen: A.** A new header field `execution_dir: Option<PathBuf>`, canonical,
recorded at init, following the established additive pattern
(`#[serde(default, skip_serializing_if = "Option::is_none")]`) that
`template_source_dir`, `intent`, and the request-store fields already use.

Requiring a flag is a breaking CLI change that buys no safety: the init cwd is
the correct answer in every case anyone has named, and a flag that must be
passed is a flag that gets passed wrongly. `--execution-dir` exists for the
case where a caller genuinely knows better.

**Pre-existing sessions** are settled by PRD decision D2: adopt on first tick
with a one-time notice. The notice is delivered by appending an
`ExecutionAnchorAdopted` event and splicing a line onto the directive with
`with_directive_prefix`. Exactly-once follows from the event: the next tick
finds `execution_dir` recorded and takes the ordinary path.

**Child sessions (R16)** inherit the parent's anchor. A child is created during
a parent tick (`src/cli/init_child.rs:469`), and under this design that tick is
already executing at the parent's anchor, so the child's anchor is the
parent's. Documented as the rule, not as an implementation accident. The
rebind verb works on a child exactly as on any other session.

### Decision 7: what "satisfy the anchor" means

| Option | Wrong-tree case | Developer in a subdirectory | Command behavior |
|--------|-----------------|---------------------------|------------------|
| A. Beneath the anchor; execute at the anchor | Refused | Accepted | Same from anywhere |
| B. Root equality; execute at the cwd | Refused | Refused | Same by construction |
| C. Beneath the anchor; execute at the cwd | Refused | Accepted | Varies by subdirectory |

**Chosen: A**, departing from the exploration's recommendation of B.

B refuses a developer who did `cd src/` before ticking, which is an ordinary
thing to do and has nothing to do with the hazard R12 exists to close. The
hazard is a *different tree*, and a different checkout is not beneath the
anchor, so the beneath test refuses it just as firmly.

C is what "beneath" naively implies and is worse than either: the tick is
accepted but the command runs somewhere the session never agreed to, so
`cat README.md` means different things depending on where the developer stood.
Executing at the anchor removes that variance entirely — and removing it is
squarely in the spirit of the PRD's third problem, which is that today the
command runs wherever `koto next` was typed.

**Comparison is byte-exact over canonicalized paths.** `fs::canonicalize`
resolves `.`, `..`, and symlinks and strips trailing slashes, so those three
acceptance cases follow from canonicalization rather than from special
handling. It does not case-fold, so a path differing only in case is a
different directory and is refused, on every platform. That is stated in the
documentation rather than left to the filesystem.

**A recorded anchor that does not resolve (R15)** is a distinct refusal with
its own machine-readable code, pointing at the rebind verb. koto already has
the shape for this: `check_template_source_dir` and `TemplateSourceStatus`
(`src/engine/template_source_status.rs`) exist precisely to answer "does this
recorded directory exist, and on whose machine", and the same module is
extended rather than duplicated.

### Decision 8: `working_dir` resolution

An action's `working_dir` is today `PathBuf::from(variables.substitute(...))`
(`src/cli/mod.rs:3979-3983`) — absolute paths are used verbatim and relative
paths resolve against the process cwd. Under anchoring it is joined to the
anchor.

`Path::join` with an absolute argument silently discards the base, so a join
alone would let an absolute `working_dir` escape the anchor while appearing to
sit beneath it. The rejection has to happen **before** the join:

1. **Compile time**: a literal absolute `working_dir` is a compile error.
2. **Run time, after substitution**: an absolute result — reachable when the
   value came from a variable — is an action failure under Decision 3, with a
   message naming the field.
3. **Only then** join against the anchor and canonicalize, and refuse a result
   that escapes the anchor via `..`.

R17 requires this to be described honestly: it bounds where a command
*starts*, not where it can *reach*. An authorized command can `cd` anywhere or
name any absolute path, and nothing here stops it.

### Decision 9: the pipe-buffer deadlock

| Option | Fixes deadlock | Output on timeout | New dependency |
|--------|----------------|-------------------|----------------|
| A. Drain both pipes on reader threads, then wait | Yes | Yes, partial | No |
| B. Non-blocking poll loop over both fds | Yes | Yes | No, but hand-rolled and platform-specific |
| C. Redirect to temporary files | Yes | Yes | No, but adds filesystem lifecycle |

**Chosen: A.** Two `std::thread`s take the child's stdout and stderr and read
to end into bounded buffers; the parent then calls `wait_timeout` as it does
today and joins the readers. On timeout the process group is killed, which
closes the pipes, which ends the readers — so the timeout path returns
whatever the command produced before it was killed instead of two empty
strings. That fixes R18 and the part of R8 that asks a timed-out command's
output to reach the agent, in one change.

The readers **keep draining after the bound is reached** and retain only the
first `N` bytes, setting a truncation flag. Stopping the read at the bound
would reintroduce the deadlock for anything larger.

B is the same fix with more platform-specific code. C works but puts temp-file
creation and cleanup on a path that currently has none.

### Decision 10: typed failure kinds

`exit_code: -1` currently means spawn failure, timeout, or wait error, and the
gate evaluator tells them apart by searching stderr for "timed out"
(`src/gate.rs:209-221`). PRD decision D8 requires the response to say which
happened rather than reporting a synthetic status.

`CommandOutput` gains a `failure_kind` discriminator alongside the existing
`exit_code`, which keeps its current values so gate evidence stays
byte-identical on the success path. Gate evidence for the three failure
conditions gains a `failure_kind` key — additive, and the existing
`{"exit_code": -1, "error": "timed_out"}` shape is preserved so nothing
downstream that reads `error` moves. The stderr substring match is deleted.

**Amendment: the set is five kinds, not four.** Planning enumerated the runner's
`-1` arms against the tree and found three, not two: spawn failure
(`src/action.rs:53`), timeout (`:96`), and a `wait_timeout` error (`:102`). The
third is real and is routed today by falling through the stderr match to
`GateOutcome::Error`. Deleting that match without a kind for it would either lose
the routing or silently report a wait error as a timeout — the exact conflation
this decision exists to end. So the runner reports `wait_failed` alongside
`nonzero_exit`, `spawn_failed`, and `timed_out`; the gate evaluator maps it to
`GateOutcome::Error`, preserving current behavior exactly; and at the action level
it is an action failure like the others, reported as `failure_kind: "wait_failed"`
in the `__action__` payload of Decision 4. This adds a value to an enumeration and
changes no decision.

### Decision 11: the repeated migration warning

| Option | Bounded per session | Root cause fixed |
|--------|--------------------|------------------|
| A. Quarantine the colliding directory so the migration completes | Yes | Yes |
| B. Write a marker file so the warning prints once | Yes | No |
| C. In-process dedup set | No — per invocation, not per session | No |

**Chosen: A.** The warning repeats because the collision branch moves nothing,
leaving the old-layout directory non-empty; the trailing `fs::remove_dir`
(`src/session/local.rs:718`) can only remove an empty directory and its error
is discarded, so the directory survives and the next invocation walks it
again.

On a collision, the colliding session directory is moved to
`<base>/.migration-conflicts/<repo-id>/<name>/` and the warning names that
destination. `list()` skips it, because listing requires a state file at
`<dir>/<state_file_name(dir_name)>` (`src/session/local.rs:111-114`) and a
dot-prefixed container has none. The old-layout directory then drains,
`remove_dir` succeeds, and the condition is gone — one message per colliding
session, ever, and two colliding sessions produce two messages.

C is what "bound the warning" suggests and does not satisfy R20, which asks
for a bound per session across invocations, not within one.

R20's general half is a stated convention rather than a mechanism: a new
diagnostic that describes a durable condition is fixed at its source or
recorded on the session, not printed per invocation.

## Decision Outcome

The four obligations resolve into one coherent shape.

An author writes a command, a name for its output, and prose for when it
fails:

```yaml
detect-branch:
  default_action:
    command: "git rev-parse --abbrev-ref HEAD"
    capture_stdout_as: BRANCH
    fallback: |
      Determine the current branch by hand and record it, then continue.
  transitions:
    - target: implement
```

```markdown
## implement

Work on branch `{{BRANCH}}`. Do not commit to main.
```

On the happy path the engine runs the command at the session's anchor, trims
and validates the output, appends `VariableCaptured`, updates a live overlay,
auto-advances into `implement`, and returns a directive that already names the
branch. No agent turn was spent on `git rev-parse`, and the state that needed
the value never had to ask.

On the unhappy path — the tool is missing, the command exits non-zero, it
times out, or the output cannot be delivered under the declared name — the
tick stops at `detect-branch`. The state's gates do not run. The response is
a `GateBlocked` whose `blocking_conditions` carries one `__action__` condition
with the command, its typed failure kind, its exit status where one exists,
and its stdout and stderr; and whose directive is prefixed with the fallback
prose. The agent does the step by hand in the same turn.

From a tree the session is not bound to, the tick refuses before any of that
happens, and names the tree it is bound to.

Underneath, the shared runner no longer loses a large command's output and no
longer misreports it as a timeout — which matters to gates that ship today,
independent of any of the above.

## Solution Architecture

### Components

| Component | File | Change |
|-----------|------|--------|
| Command runner | `src/action.rs` | Reader threads; `failure_kind`; `truncated`; bounded retention |
| Gate evaluator | `src/gate.rs` | Consume `failure_kind` instead of matching stderr; emit it in evidence |
| Action declaration | `src/template/types.rs` | `capture_stdout_as`, `fallback`; capture-name namespace and collision rules; absolute `working_dir` rejection; `__action__` reserved |
| Event log | `src/engine/types.rs` | `VariableCaptured`, `ExecutionAnchorAdopted`, `ExecutionAnchorRebound` variants |
| Header | `src/engine/types.rs` | `execution_dir: Option<PathBuf>` |
| Substitution | `src/engine/substitute.rs` | Fold `VariableCaptured` in event order; overlay type; unset-capture error |
| Advance loop | `src/engine/advance.rs` | Capture and validate; synthesize `__action__` on failure; short-circuit gates; read variables through the overlay |
| `handle_next` | `src/cli/mod.rs` | Anchor check; execute at anchor; overlay wiring; failure-condition construction; fallback prefix |
| Anchor status | `src/engine/template_source_status.rs` | Extend the resolve-check to `execution_dir` |
| Rebind verb | `src/cli/session.rs` | `koto session rebind` |
| Migration | `src/session/local.rs` | Quarantine on collision |

### The variable overlay

The three staleness sites share one fix. `handle_next` creates a
`RefCell<HashMap<String, String>>` holding captures made during this tick, and
passes it to every consumer:

- the gate closure and the action closure, which consult it before the
  pre-loop `Variables` binding when substituting a command string;
- `advance_until_stop`, which reads it at each iteration in place of the
  once-built `workflow_variables` map for `vars.*` when-clause evaluation;
- the final `with_substituted_directive` call, which consults it before the
  pre-loop binding.

The advance loop writes to it when an action's capture succeeds, in the same
step that appends the `VariableCaptured` event, so the on-disk record and the
in-memory view never diverge. The overlay is per-tick and lives only as long
as the call; the event log is the durable record, and a later tick's
`Variables::from_events` reconstructs everything from it.

A rebuild-after-the-loop approach was considered and rejected: it fixes only
the response directive and leaves a later state's gate command and a later
state's `vars.*` when clause reading pre-loop values within the same tick.

Lookup order is fixed and documented: runtime names (`SESSION_DIR`,
`SESSION_NAME`) substitute first, as they do today
(`src/cli/vars.rs:19-26`), then the overlay, then the `WorkflowInitialized`
bindings. Because captures resolve in the final layer, a captured value
containing a `{{...}}` token is never re-expanded. The allowlist forbids
braces anyway; the layering means that is a second defense rather than the
only one.

### Capture delivery and its three failure cases

After the command runs and before anything else happens with its result:

1. Trim trailing and leading whitespace from stdout.
2. If the result is empty, the capture fails.
3. If the result exceeds `MAX_CAPTURE_BYTES` (4096), the capture fails.
4. If `validate_value` rejects the result, the capture fails.
5. Otherwise append `VariableCaptured { key, value }` and write the overlay.

All three failures are action failures under Decision 3 — the same stop, the
same response shape, the same fallback prose — with a `capture_error` field in
the `__action__` payload naming which case fired and, for the allowlist case,
the offending value's first rejected character position.

This departs from the exploration's recommendation of validate-then-skip,
which argued that a hard failure turns a loose command into an outage. Three
reasons the departure is right. R3 says output is never silently dropped, and
a skip is a silent drop. A skipped capture does not make the problem go away;
it defers it to the reading state, where the error names a variable instead of
the command that failed to produce it — a strictly worse diagnostic, arriving
later. And treating a failed capture as an action failure means one failure
model rather than two, so an author who has written `fallback:` prose gets it
delivered for every reason the step did not work.

The 4096-byte capture bound is deliberately far below the 64KB response bound
(`MAX_ACTION_OUTPUT_BYTES`, `src/cli/mod.rs:61`). A capture is a token that
lands in prose and possibly in a shell word; the allowlist already rules out
newlines, so anything approaching the bound is a template mistake. The two
bounds are separately stated in the authoring documentation.

The size bound and truncation marking of R19 apply to the *response and event
log* copies of stdout and stderr, for gates and actions alike, and are carried
by the runner's `truncated` flag rather than by after-the-fact string
inspection.

### Lifetime and identity of a captured value (R5)

Derived from the append-only log, so the answers fall out rather than being
invented:

- **Re-entering the producing state** appends a second `VariableCaptured` for
  the same key. Later wins; substitution folds in event order.
- **Two states declaring the same name** is a compile error (Decision 2), so
  precedence between them never arises at run time.
- **A rewind past the producing state** appends a `Rewound` event and removes
  nothing (`src/engine/persistence.rs:722-752` derives the current epoch by
  scanning backwards; the log itself is never truncated). The captured value
  survives the rewind. This is documented, not changed, and matches the PRD's
  Known Limitation that a rewind does not unwind what a command already did.

### The anchor check

At the top of `handle_next`, before the template is compiled and before any
gate or action closure is built:

```
read header
  execution_dir absent  -> adopt current cwd (canonical), append
                           ExecutionAnchorAdopted, prefix a notice, continue
  execution_dir present -> canonicalize; if it does not resolve, refuse with
                           the unresolvable code and point at rebind
                        -> if cwd (canonical) is not the anchor or beneath it,
                           refuse with the wrong-tree code, naming the anchor
                        -> otherwise, use the anchor as the working directory
                           for every gate and every action this tick
```

Two distinct machine-readable codes, per R12 and R15. Both are refusals with
no action executed, no gate evaluated, and no transition — the check runs
before any of those are reachable, so that property is structural rather than
maintained by discipline.

`koto session rebind <name> [--to <dir>]` canonicalizes the target (defaulting
to the current directory), writes it to the header, and appends
`ExecutionAnchorRebound { from, to }`. It is the only verb that changes an
anchor, and it works on sessions created by other sessions.

### The action-failure condition

The `__action__` condition's `output` payload:

```json
{
  "command": "git rev-parse --abbrev-ref HEAD",
  "failure_kind": "nonzero_exit",
  "exit_code": 128,
  "stdout": "",
  "stderr": "fatal: not a git repository",
  "truncated": false,
  "state": "detect-branch"
}
```

`failure_kind` is one of `nonzero_exit`, `spawn_failed`, `timed_out`,
`wait_failed` (see the amendment under Decision 10), or `capture_failed`. `exit_code` is present only for `nonzero_exit`; the other
three carry no meaningful status and say so by omitting it rather than
reporting `-1`. `capture_failed` adds a `capture_error` object naming the key
and the case. `agent_actionable` is false and `category` is `"corrective"`.

## Implementation Approach

Ordered so each phase is independently shippable and the two shared-path
defects — which affect gates in the current release — land first.

**Phase 1: the shared execution path.** Reader threads in
`run_shell_command`; bounded retention with a truncation flag; typed
`failure_kind`; delete the stderr substring match in `src/gate.rs`; additive
`failure_kind` in gate evidence. New tests at slightly above the measured pipe
buffer and at several megabytes, for a gate and for an action. No test in koto
exercises this today, so these are new files rather than extensions.

**Phase 2: the migration warning.** Quarantine on collision; the old-layout
directory drains and is removed. Tests for one colliding session across many
invocations, and for two colliding sessions each producing their own message.

**Phase 3: anchoring.** Header field; adoption with notice; the per-tick check
with its two codes; execute-at-anchor; `working_dir` rejection then join then
the beneath check; `koto session rebind`; extend the resolve-check module. Ships
without any of the output or failure work, and closes Story 3 on its own.

**Phase 4: the failure path.** `fallback:` on the action declaration; the
`__action__` reserved name and its compile-time protection; the synthesized
condition and the gate short-circuit; ordering against
`requires_confirmation`; the directive prefix.

**Phase 5: output routing.** `capture_stdout_as:`; the capture-name namespace
and its three compile-time rules; `VariableCaptured`; the overlay threaded
through the loop, both closures, and the final substitution; the unset-capture
stop. Depends on Phase 4 for the failure model that carries a failed capture.

**Phase 6: documentation and skills.** The bad-success-versus-bad-failure rule
with worked examples on both sides, including `gh pr create` on the
permanent-agent side and a worked engine-runnable example; complete
`default_action` authoring documentation covering the six points of R22;
anchoring's guarantee and its explicit non-guarantee; `koto-author` and
`koto-user` updated, including the dispatch-table drift; the `grep -ri` sweep
for containment language; `session-feed.md` entries for the three new event
types.

## Security Considerations

**A captured value can reach `sh -c`.** This is the sharpest new surface. A
command's output becomes a `{{KEY}}` value, and a later state's gate command
or action command may interpolate it. The existing allowlist
(`src/engine/substitute.rs:29`: alphanumerics, `. _ - / : @`, and space) is
the control, applied to captured values by the same `validate_value` function
that guards init-time variables — reused deliberately rather than
reimplemented, so a future widening of the allowlist is a single reviewed
change that both paths inherit. Shell metacharacters, newlines, backticks,
`$`, and braces are all rejected. A rejected value is not delivered and the
workflow stops.

**Double substitution is structurally prevented.** Captures resolve in the
last substitution layer, so a captured value containing `{{OTHER}}` is emitted
literally rather than expanded. The allowlist independently forbids braces.

**The anchor is not containment, and the documentation must not imply it is.**
An authorized command can `cd` out or name absolute paths. R17 requires every
koto document, error string, skill, and release note to avoid describing
anchoring as sandboxing, isolation, containment, or a restriction on what a
command can touch, and requires a `grep -ri` sweep for `sandbox`, `contain`,
`isolat`, and `restrict` to confirm it. What anchoring does provide is
narrower and real: a session cannot be advanced from a tree it is not bound
to, checked on every tick.

**An absolute `working_dir` must be rejected before the join.** `Path::join`
with an absolute argument discards the base silently, so a design that joins
first and checks afterwards would vouch for a path it never verified. The
rejection is at compile time for a literal and at run time after substitution
for a variable-derived value, both ahead of the join, with a `..` beneath-the-anchor
check after canonicalization.

**Captured output is written to the event log.** A command whose stdout
carries a secret records it, bounded at 4096 bytes for a capture and 64KB for
the response copies, unredacted. This is a real exposure and it is narrower
than the one already present: the post-substitution command string, gate
override payloads, and init-time variables are already logged unbounded and
unredacted. Bounding the log's wider content is out of scope by the PRD's
boundary, and the PRD records that this feature owes a follow-up issue on it —
to be filed before the first template declares an action, since today's zero
exposure is exactly the condition this feature ends.

**The permission model is deliberate and settled.** A `default_action` runs
from the koto binary and does not pass through the agent harness's
allow/deny/ask rules. Loading a workflow is the grant: invoking a koto-backed
workflow authorizes the commands that workflow declares. This is the intent,
recorded here so a future reader does not re-derive it as a defect. It is why
the authoring rule of R21 exists — the boundary between what the engine runs
and what the agent keeps is enforced by published guidance and template
review, not by a runtime prompt.

**Reader threads and resource use.** Two threads per command execution,
joined before the call returns; the process group is still killed on timeout,
which closes the pipes and ends the readers. Draining past the retention bound
means a command emitting gigabytes costs I/O but not memory.

## Consequences

### Positive

- The motivating case works within a single tick, which is the case R2 names
  and the one a rebuild-after-the-loop fix would have silently missed.
- The response contract, its hand-rolled `Serialize` impl, its three
  exhaustive combinators, and its byte-pinned baseline are untouched.
- One failure model covers a bad exit, a missing tool, a timeout, and an
  undeliverable capture, so an author writes `fallback:` once.
- The two shared-path defects ship in phases 1 and 2 and reach gate authors
  without waiting on any of the rest.
- Timed-out commands now return the output they produced before the kill,
  which they never did.
- A command's behavior no longer depends on which subdirectory the developer
  was standing in.
- R4's typo case, R5's duplicate-name case, and the absolute-`working_dir`
  case are all compile-time errors rather than run-time surprises.

### Negative

- A state that declares an action and gates no longer evaluates those gates
  when the action fails. This is a behavior change; it reaches no shipped
  template, and R6 does not permit the alternative.
- A failing action now stops rather than producing a confirm stop when
  `requires_confirmation` is set. Same reasoning, same zero blast radius.
- A capture that fails the allowlist stops the workflow rather than being
  skipped, so a template author writing a multi-line command discovers it as
  a stop. The `fallback:` prose is the intended answer, and the authoring
  documentation states the constraint up front.
- Three staleness sites must stay wired to the overlay. A future consumer of
  `Variables` inside the advance loop that forgets the overlay reintroduces
  the bug in a narrow form.
- Two threads per command execution where there were none.
- Quarantining a colliding session directory moves a developer's data. It is
  moved, never deleted, and the message names the destination.

### Mitigations

- The overlay is passed as an explicit parameter rather than read from a
  global, so a new consumer that ignores it is visible in review; the
  auto-advance capture-then-read case is covered by a test that fails if any
  of the three sites regresses.
- The gates-after-failure rule and the confirm ordering are stated in the
  `default_action` authoring documentation as behavior, not as internals, and
  R22's acceptance criterion checks that the documented behavior matches the
  engine.
- The capture allowlist and both size bounds are stated in the authoring
  documentation rather than discovered by hitting them, per R25.
- The quarantine path is exercised by a test asserting the session's contents
  are intact at the new location.
