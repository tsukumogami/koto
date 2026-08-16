# Decision D2: the read-only retrieval surface

## Question

Where does the read-only retrieval that PRD-inline-phase-details.md's R7–R13 requires live
on koto's CLI surface, and what is it called?

## Decision drivers

- **R7**: keyed by workflow name alone, no argument the caller would have had to memorize.
- **R8/R9**: must return the current phase's identifier, substituted directive, substituted
  instructions, and the `expects` schema when the phase declares one.
- **R11**: an exhaustive no-side-effects list — no event append, no gate evaluation, no
  shell execution, no transition, no terminal cleanup, no request-store write, no discovery
  cursor advance.
- **R12**: must not block on a lock another process holds.
- **R13**: error shape must match koto's existing structured-error conventions; a phase with
  no instructions and a terminal phase are both normal success responses, not errors.
- Minimize new registration footprint (clap surface, dispatch arm, docs, evals) — the PRD
  imposes no requirement to add ceremony beyond what R7–R14 demand.
- Vocabulary consistency: koto's CLI/engine surface says "state" (953 occurrences in
  `src/cli/`, 427 in `src/engine/`, vs. 36 and 3 for "phase"); "phase" is a deliberate,
  documented translation confined to `src/workflows_surface/` for Claude Code's `/workflows`
  UI (see `BRIEF-native-workflows-phase-detail.md:48`, which glosses "phase" as "(the
  template's states)" on first use).

## Considered options

### A. Extend `koto status`

`handle_status` (`src/cli/mod.rs:4834-4961`, verified against current source) already does
every read the retrieval needs and nothing else:

- `backend.exists(name)` (4835) → flat `{"error": "workflow '<name>' not found", "command":
  "status"}` at exit 2 if not.
- `backend.read_events(name)` (4845) → same flat-error shape at `exit_code_for_engine_error`
  on corruption.
- `derive_machine_state(...)` (4859) → `"corrupt state file: cannot derive current state"` at
  `EXIT_INFRASTRUCTURE` if it returns `None`.
- Reads and parses the compiled template (4873-4896) into a `CompiledTemplate`, then looks up
  `compiled.states.get(&machine_state.current_state)` (4898-4901) — this is the exact
  `TemplateState` struct that carries `directive` and `details` (the same struct
  `next_types.rs` imports and `handle_next` reads at `mod.rs:4001-4014`).
- Computes `is_terminal` from that same lookup (4898-4901) and already returns it as a normal
  field in a normal success response — terminal is not an error path today.
- Calls `lock_state_file` **nowhere** in the function body. The only `lock_state_file` call in
  `src/cli/mod.rs` is inside `handle_next` at line 3770, gated by `state_is_batch_scoped`
  (`mod.rs:2211-2213`) — batch-parent states only. `status` inherits `read_events`'s
  documented advisory-read tolerance (`persistence.rs:312`: "concurrent readers via
  `read_events` do NOT take a lock and are unaffected"), so it satisfies R12 by construction,
  against both the batch-lock case and the ordinary-session mid-tick race.
- Appends no event, evaluates no gate, executes no shell command, transitions nothing, and
  never reaches `finish_terminal_tick`/`backend.cleanup` — none of those code paths are
  reachable from `handle_status` at all. R11 is satisfied because the function structurally
  cannot reach the excluded operations, not because a flag suppresses them.

**What's missing today**: `directive`, `details`, and `expects` are not read at all.
`response = serde_json::json!({...})` (4903-4909) carries only `name`, `current_state`,
`template_path`, `template_hash`, `is_terminal`. Closing R8/R9 means:

- Reading `template_state.directive` / `.details` off the same `TemplateState` `handle_status`
  already has in scope after 4898-4901 — a lookup on data already loaded, not new I/O.
- Running the same two-layer substitution `next` uses (`with_substituted_directive`,
  `next_types.rs:159-251`): `crate::cli::vars::substitute_vars(d, &runtime_vars)` then
  `variables.substitute(&d)`. Both inputs are reachable read-only: `runtime_vars` needs only
  `backend.session_dir(&name)` and `name` (no I/O); `variables` comes from
  `Variables::from_events(&events)` (`substitute.rs:60-75`), a pure scan over the event log
  `handle_status` has already read at line 4845. No new file is opened to satisfy R8.
- Calling `derive_expects(state: &TemplateState) -> Option<ExpectsSchema>`
  (`next_types.rs:774`), a pure function over the same `TemplateState`, with no I/O — already
  used identically by the live `dispatch_next` path. Closes R9 with zero new machinery.

**Registration footprint**: none of clap `Command` enum, dispatch match, or help text change.
`Status { name: String }` (`mod.rs:232-235`) already has the only argument R7 requires and no
flags. `Command::Status { name } =>` (`mod.rs:1180`) is the existing, unchanged dispatch arm.
The change is entirely inside `handle_status`'s body plus its response `json!` literal.

**Structured-error list**: `docs/reference/error-codes.md:11` names exactly three surfaces
using the structured `{"error": {"code": ...}}` envelope — `next`'s domain errors,
batch-scoped errors, and `koto request`. `status` is not one of them today (confirmed: no
`### status` section in `error-codes.md`, and `handle_status`'s three error sites above all
use the flat `{"error": "...", "command": "status"}` shape). Extending `status`'s success
payload doesn't touch its error paths, so it doesn't newly qualify for the structured-envelope
list and imposes no new documentation obligation there.

**Terminal and no-instructions cases (R13)**: `status` already returns `is_terminal` in a
normal success envelope — the terminal case is handled today, for free. For consistency with
`NextResponse::Terminal`, which never carries a `directive` field at all
(`next_types.rs:458-471`), the recommendation is that `status` omit `directive`/`details`/
`expects` when `is_terminal` is true, rather than returning stale-looking instructions for a
phase with nothing left to do. A phase with an empty `details` string already serializes as
absent under the existing `Option<String>` convention `next` uses (`mod.rs:4001-4003`) — same
rule applies here, no new logic.

**Stability**: `koto-stability-tests/` was grepped for any reference to `status` and returned
none — it pins the wire format and four `SessionBackend` trait methods (per
`docs/STABILITY.md`), not `status`'s JSON shape. Adding keys to an already-open, additively-
consumed JSON object is not a breaking change under any contract this repo currently enforces.

### B. A new top-level subcommand (`phase-info`, `state-info`, or a noun-group placement)

**Naming.** `phase-info` is the name the source issue proposed. Two independent leads
(`explore_inline-phase-details_r1_lead-phase-info-command.md`,
`explore_inline-phase-details_r2_lead-escape-hatch-cost.md`) measured the same signal against
current source: "state" is the CLI/engine's vocabulary (953:36 in `src/cli/`, 427:3 in
`src/engine/`); "phase" is confined to `src/workflows_surface/`, where it is a deliberate,
documented translation for Claude Code's `/workflows` screen, not an alternate spelling of the
same concept available for reuse elsewhere. `koto phase-info` would be the CLI's first
UI-vocabulary leak into its own command namespace. `koto state-info` avoids that cost and
reads as the natural read-only twin of `koto status`/`koto next`, but see the case against
option A below for why even this name is worse than not minting a command at all.

A noun-group placement (`koto session state-info`, `koto template state-info`) doesn't fit
koto's own grouping convention either: noun-groups (`session`, `context`, `template`,
`config`, `decisions`, `overrides`, `workspace`, `request`) exist for families of related
CRUD-style actions on a shared resource; a single current-phase lookup keyed by workflow name
is not a member of any of those families — it's a sibling of the bare-verb commands
(`status`, `next`, `cancel`, `rewind`, `dashboard`), which take a workflow name positional and
nothing else.

**Registration footprint**, using `Status` as the structurally closest precedent:

1. New `Command` variant next to `Status` (`mod.rs:232-235`).
2. New dispatch arm next to `Command::Status` (`mod.rs:1180-1183`).
3. New handler — a near-duplicate of `handle_status`'s ~130-line read/parse/lookup path
   (4834-4961), unless factored into a shared helper (`batch_view.rs` is the precedent for
   that kind of extraction, but extracting it *from* `handle_status` and reusing it in both
   places is strictly more work than adding three fields to the one function that already
   exists).
4. New row in `command-reference.md`'s subcommand table (line 10-38 area) plus a wholly new
   `## koto <name>` section (mirroring `## koto status` at lines 271-311) — net *more* new doc
   surface than option A, which edits an existing section.
5. New `### <name>` section in `docs/guides/cli-usage.md`, versus an edit to the existing
   status paragraph at line 929 for option A.
6. A new integration-test trio (`<name>_active_workflow`, `_terminal_workflow`,
   `_missing_workflow`) — can't extend the existing `status_active_workflow` et al.
   (`tests/integration_test.rs:6895-6963`).
7. A near-certain new skill note in `koto-user` explaining "when to use `status` vs `<name>`
   vs `next --full`" — a three-way discrimination an agent has to get right, that doesn't
   exist if the retrieval is just `status`'s existing, familiar output getting richer.

R7, R11, R12, and R13 are all satisfiable by option B exactly the way option A satisfies them
— it would be built from the same read/parse/lookup path — so B is not weaker on the PRD's
functional requirements. It is strictly more expensive to build and document, and it fails
the vocabulary driver that A does not even have to clear.

### C. `koto next --dry-run`

Round 2 of the escape-hatch-cost research (`explore_inline-phase-details_r2_lead-escape-hatch-
cost.md`) traced this option's implicit premise — that `koto next --full` already is a safe
read-only re-fetch — end to end against `handle_next` (`mod.rs:2892` onward) and found it
false, which I re-verified against current line numbers:

- The batch-parent lock acquisition (`mod.rs:3768-3776`) runs before any dispatch logic, gated
  only by `state_is_batch_scoped`, with **no dependency on any dry-run-style flag** — a
  `--dry-run` would need to skip this entirely to satisfy R12, which is a change to the same
  code path `--full` never touches.
- `full` is consulted exactly once in the whole function, at the point where `details` is
  included in the already-computed response — it has zero effect on whether gates evaluate,
  whether a `default_action` executes, whether the workflow auto-advances, or whether terminal
  cleanup runs. A bare `koto next --full` on a gated state with a passing gate and no
  `accepts` block silently advances the workflow; on a terminal landing, it deletes the
  session directory.

A `--dry-run` that is safe against R11 would have to correctly short-circuit gate evaluation,
default-action execution, event-log appends, the discovery-scan cursor write, and terminal
cleanup — all inside a single large entry point that was never designed with a no-op path in
mind, and where a missed short-circuit doesn't produce a wrong answer, it silently mutates
state or advances the workflow under a flag whose name promises it won't. This is materially
riskier than A or B, whose entire read paths are already side-effect-free by construction
(they simply never call the mutating primitives). I confirm the prior round's judgment: C is
the worst-costed option and there is no new evidence in this pass that changes that.

### D. Anything else

No fourth candidate surfaced independently in this pass. The dashboard's `read_detail` /
`DetailData` seam (`dashboard_data.rs:649`, `272-296`) is a real, proven read-only path but is
TUI-internal with no scriptable JSON output, and it doesn't even carry the `details` field
(only `directive`) — extending it to a CLI-scriptable surface with `details` added would be a
strictly larger effort than extending `status`, for the same destination shape. Not
independently pursued as a serious option.

## Recommendation

**Extend `koto status <name>`. No new flag, no new command, no new clap variant.**

`response` (`mod.rs:4903-4909`) gains three keys, following the same "present only when
relevant" convention `status` already uses for `batch`, `leg`, `superseded_branches`, and
`stale_template_source_dir` (`mod.rs:4911-4957`, each populated conditionally with no flag to
opt in):

```json
{
  "name": "...",
  "current_state": "...",
  "template_path": "...",
  "template_hash": "...",
  "is_terminal": false,
  "directive": "<substituted directive text>",
  "details": "<substituted instructions text, omitted when empty>",
  "expects": { "...": "..." }
}
```

- `directive` and `details` are populated from `compiled.states[current_state]`'s
  `TemplateState`, substituted via the identical two-layer pipeline `next` uses
  (`substitute_vars` then `Variables::from_events(&events).substitute`), so recovered text is
  byte-identical to what `next` would have produced on first arrival — satisfies R8.
- `expects` is populated via `derive_expects(&state)`, present exactly when the phase declares
  an `accepts` schema — satisfies R9.
- All three keys are **absent**, not null, when `is_terminal` is true — matching
  `NextResponse::Terminal`'s existing behavior of never carrying a `directive` field, and
  giving R13's "terminal is a normal success response" requirement a concrete shape rather
  than an implied one.
- `details` is absent when the phase's instructions are empty, mirroring the existing
  `Option<String>` convention `next` already uses — satisfies the "phase declares no
  instructions is not an error" half of R13.
- No flag is introduced. Round 1 of the phase-info research considered `--details` against
  `--full`, and concluded (independently reconfirmed here) that `status` has no notion of
  "first visit" to key a flag off of, and `status` is not called every tick the way `next` is
  (`command-reference.md` and `cli-usage.md` describe it as a spot-check tool — batch-parent
  progress, `superseded_branches` after a rewind — not a per-tick poll), so the token cost of
  an unconditional `details` field is paid only when an agent deliberately reaches for
  recovery, not on some hot loop.
- No lock is acquired (R12): `handle_status` never calls `lock_state_file`; the only call site
  in the codebase is inside `handle_next`, gated to batch-parent states. `status` inherits
  `read_events`'s documented advisory-read tolerance unconditionally.
- No new entry in `docs/reference/error-codes.md`'s structured-envelope list: `status`'s three
  existing error paths (unknown workflow, corrupt state file, unreadable/unparseable template)
  already use the flat `{"error": ..., "command": "status"}` shape and are unaffected by this
  change.

This is the cheapest option on every axis the PRD or koto's conventions care about: zero new
clap surface, zero new dispatch arm, edits to one existing `## koto status` doc section
instead of new ones, extension of the existing `status_active_workflow` / `_terminal_workflow`
/ `_missing_workflow` integration tests instead of a new trio, and no vocabulary cost, because
it introduces no new noun at all.

## Case against the recommendation

**"`status` was a snapshot command; now it's also the recovery command, and that's two jobs
wearing one name."** This is the strongest objection and I don't think it survives. `status`'s
own doc comment already frames it as "read-only view of the current state," and R8's payload
— identifier, directive, instructions, evidence schema — is exactly "what is the current state
of this workflow," not a different question. The PRD's own problem statement independently
arrives at `status` as the natural site: "`koto status` is genuinely side-effect-free and
returns neither the directive nor the instructions" is written as a gap in `status`, not as a
reason to look elsewhere. Nothing in R7–R14 asks for a *distinct* verb; R7 only asks that the
retrieval be keyed by workflow name alone, which `status` already is.

**"An unconditional multi-KB `details` field bloats every `status` call, including ones that
only wanted `current_state`."** This is real but bounded: `status` is not in koto's per-tick
loop (that's `next`'s job); it's reached for deliberately, for batch-progress checks and
post-rewind inspection today, and for recovery going forward. The cases where an agent wants
`current_state` alone and doesn't want to pay for `details` are exactly the cases the existing
optional-field convention already handles for `batch`/`leg`/`superseded_branches` — those are
also unconditionally computed and merged in when present, at whatever cost `derive_batch_view`
and `derive_superseded_branches` already impose. This isn't a new class of cost the design is
introducing; it's the same cost model `status` already accepted.

**"A dedicated command name is more discoverable to an agent than a flag-less extension to an
existing command."** Possibly true in the abstract, but R14's pointer mechanism (a
koto-authored sentence spliced into `directive` via the existing `with_directive_prefix`
machinery, precedented by the leg-abandonment notice at `next_types.rs:268-357`) solves
discoverability independent of naming — the pointer text just says "run `koto status <name>`
to recover this," which is no less discoverable than a bespoke verb, and is one fewer thing
for an agent to learn is a distinct concept from `status`.

I judge the recommendation survives.

## Consequences

- `handle_status` (`mod.rs:4834-4961`) gains a directive/details/expects computation block
  using `with_substituted_directive`-equivalent logic and `derive_expects`, guarded by
  `is_terminal`.
- The R14 discoverability pointer, wherever it's specified, should read `koto status <name>`
  as the command name — this decision fixes that string for whichever decision owns the
  pointer's exact wording and channel.
- `command-reference.md`'s existing `## koto status` section and `docs/guides/cli-usage.md`'s
  existing status paragraph (line 929) both need editing to document the three new fields;
  neither needs a new section.
- `koto-user`'s `response-shapes.md` needs a new row/paragraph for `status`'s `directive`/
  `details`/`expects` fields, most naturally adjacent to its existing description of the same
  fields on `next`'s responses, to make clear they carry identical semantics.
- No entry needed in `docs/reference/error-codes.md`.
- Integration tests extend the existing `status_active_workflow` / `status_terminal_workflow`
  / `status_missing_workflow` trio (`tests/integration_test.rs:6895-6963`) rather than adding
  a new one.

## Open questions for cross-validation

- Does the discoverability decision (wherever R14's pointer text and channel are finalized)
  agree that `koto status <name>` is the right string to point at, or does it independently
  favor a different phrasing that assumes a dedicated verb exists? This decision assumes the
  pointer references `status` by name.
- Should `blocking_conditions` (last-known, derivable read-only via
  `derive_last_gate_evaluated`, `persistence.rs:844`) also ride on the extended `status`
  response? R8/R9 don't require it, and it introduces a staleness caveat `status` doesn't
  otherwise carry. I left it out of the recommended payload as out of this decision's scope —
  worth a explicit ruling from whichever decision covers the full response-shape contract, so
  it isn't silently added or silently dropped without a stated reason.
