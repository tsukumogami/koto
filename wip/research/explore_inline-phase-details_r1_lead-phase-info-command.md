# Lead: What would `koto phase-info <workflow>` be, does an existing command already cover it, and how should state-vs-phase naming be resolved?

## Revision note (round 2)

The team lead confirmed the rest of koto#90 (template `details` field, `NextResponse.details`,
first-visit-only gating, `--full`) shipped in PR #109 (merged 2026-03-30). That leaves `koto
phase-info` as the one unimplemented acceptance criterion, and it sharpens the crux question:
**is `koto next --full` actually safe as a read-only re-fetch, or does it risk mutating/advancing
the workflow?** Round 1 of this lead treated `--full` as if it already covered the escape-hatch
need. Round 2 traced the dispatch path in full and that conclusion does not hold: `--full` is not
read-only. See "Round 2: is `koto next --full` read-only?" below, which supersedes the
"already covered" framing in the original Findings section for the purposes of the phase-info
decision. The rest of round 1's findings (naming, CLI conventions, output envelope, stability)
still stand and are reused in the round-2 recommendation.

## Round 2: is `koto next --full` read-only?

**No. `--full` only changes whether the `details` field is included in the response; it does not
change dispatch behavior in any way. A bare `koto next <name> --full` (no `--with-data`, no
`--to`) runs the exact same mutating advancement loop as a bare `koto next <name>`.**

Tracing `handle_next` (`src/cli/mod.rs:2892`) end to end for the no-`--with-data`, no-`--to` case:

1. **Startup side effects run unconditionally, before any dispatch logic, regardless of flags.**
   `src/cli/mod.rs:2948-3013`: every `koto next` call — `--full` or not — runs cursor GC
   (`gc_stale_cursors`), stale compact-lock recovery, terminal-index compaction
   (`maybe_compact_terminal_index`, which can rewrite `_terminal_index.jsonl` on disk when over
   threshold), and a "wake-candidates" pass that "emits a `RequesterWoken` event... invokes the
   substrate-wake primitive... and unlinks the child's claim sidecar" (comment at
   `mod.rs:2992-3005`). These are best-effort and scoped to the workflow's role as a coordinator
   over children, but they are real writes (event appends, file rewrites, sidecar unlinks) that
   happen on every tick with zero relationship to `--full`.
2. **The `--with-data` block is skipped** when `with_data` is `None` (`mod.rs:3487-3488`
   `if let Some(ref data_str) = with_data`), so no `EvidenceSubmitted` event is appended directly
   from a bare call. This part is genuinely inert without `--with-data`.
3. **`advance_until_stop` runs unconditionally** (`mod.rs:3962-3972`), passing live closures for
   `append_event`, `evaluate_gates`, `invoke_integration`, and `execute_action`
   (`mod.rs:3820-3959`). Nothing in this call is gated on `full`; `full` is read later, only when
   building the `details` field (`mod.rs:4001-4015`).
4. **Inside `advance_until_stop`** (`src/engine/advance.rs:168`), for the current state, in order:
   - **Step 5, default action execution** (`advance.rs:286-314`): if the current state has a
     `default_action` and the current epoch has no evidence yet (`has_evidence` false — true for
     an agent that hasn't submitted anything, which is exactly the "I lost context, let me
     re-read my instructions" scenario), `execute_action` runs for real — `action::run_shell_command`
     (`mod.rs:3930`) or polling (`mod.rs:3913-3928`) — an actual subprocess execution, and appends
     a `DefaultActionExecuted` event (`mod.rs:3938-3945`). A bare `--full` call on a state with a
     default action will **re-execute that action's command**.
   - **Step 6, gate evaluation** (`advance.rs:316` onward, evaluation call and append at
     `advance.rs:389` `append_event(&gate_evaluated_payload)`): whenever the state has gates, they
     are evaluated for real (including command gates, which run subprocesses) and a
     `GateEvaluated` event is appended to the state file — a genuine mutation of the workflow's
     own event log, on every tick, independent of `--full`/`--with-data`/`--to`.
   - **Auto-advance**: if the state has no `accepts` block (or gates already pass with no new
     evidence needed) the loop resolves a transition and appends a `Transitioned` event
     (`advance.rs:509`, `551`), moving `current_state` forward — i.e. a bare `koto next --full`
     call **can advance the workflow to a different state**, with no evidence submitted and no
     `--to` given, purely because the auto-advance fallback in `dispatch_next`
     (`src/cli/next.rs:107-124`, "Fallback: state has no accepts... auto-advance candidate") and
     the corresponding loop in `advance.rs` don't check `full` at all.
5. **Terminal cleanup**: if the loop lands on a terminal state, `finish_terminal_tick` runs
   (`mod.rs:4486`) and calls `backend.cleanup(name)` (`mod.rs:2586`) **unless `--no-cleanup` was
   passed** — deleting the session directory. `--full` does not suppress this; only `--no-cleanup`
   does.

**Conclusion:** `full` is consulted exactly once in the whole function, at `mod.rs:4010`
(`if full || count <= 1`), purely to decide whether to include `details` in the already-computed
response. It has zero effect on whether gates are evaluated, whether a default action executes,
whether the workflow auto-advances, or whether a terminal session gets cleaned up. **An agent that
lost context and calls `koto next --full` hoping to safely re-read its current instructions is
running the full mutating tick** — if the state has a default action, it re-executes; if gates
newly pass, the workflow silently advances past the phase the agent meant to re-read; if that
lands on terminal, the session gets deleted out from under it (absent `--no-cleanup`, which the
agent would have to know to add defensively). This is exactly the risk the team lead's question
named, and it's real: **a separate read-only command (or a genuinely read-only flag combination)
is justified.**

## Round 2: does any existing read-only command return the current state's details today?

**No. Nothing in the current CLI can return a live session's current-state `directive` or
`details` without either (a) running the mutating `koto next` tick, or (b) an agent manually
reading and parsing an internal template-cache JSON file it isn't documented to touch.**

Walked every read-only candidate:

- **`koto status <name>`** (`src/cli/mod.rs:4834`, docstring at `mod.rs:231-235`: "read-only, no
  state changes") is the closest fit in shape — takes only the workflow name, JSON output, no
  mutation anywhere in `handle_status`. But its response body (`mod.rs:4903-4909`, plus optional
  `batch`/`leg`/`superseded_branches`/`stale_template_source_dir` sections through line 4957) is
  exactly: `name`, `current_state`, `template_path`, `template_hash`, `is_terminal`. **No
  `directive`, no `details` field anywhere in `handle_status`.** It does return
  `machine_state.template_path` (`mod.rs:4906`) — the path to the *compiled* template JSON on
  disk — which is the only thread an agent could pull to reconstruct the current state's
  directive/details, but `status` itself does not do that extraction, and using that path requires
  the agent to read and manually parse a cache-artifact JSON file (`states[current_state].directive`
  / `.details`) that has no documented stability guarantee (`docs/STABILITY.md` covers the wire
  format and `SessionBackend` trait, not template cache file paths or shape).
- **`koto template compile <source>`** (`TemplateSubcommand::Compile`, `mod.rs:618-628`) takes a
  *source* markdown/YAML path, not a workflow name, and a running agent that only has the
  workflow name in hand has no way to know the source path from `koto status` alone (`status`
  gives the *compiled* cache path, not the original source). Even if the agent had the compiled
  JSON path, `template compile` assumes a source file and would try to recompile it as markdown —
  it isn't built to load an already-compiled JSON and print one state's fields.
  `template validate <path>` (`mod.rs:631-634`) only validates structure; it prints nothing about
  a specific state's directive/details either. Neither subcommand is usable with "just the
  workflow name."
- **`koto session dir <name>` / `session list`** print a path / list metadata; no directive or
  details content.
- **`koto dashboard --once`** (`src/cli/dashboard.rs:98-188`, `format_once_line`) emits an
  8-column tab-separated line: `session_id, current_state, elapsed, status_bucket, intent,
  template_name, idle, liveness` (`dashboard.rs:177-187`). No directive, no details — the
  once-mode contract deliberately doesn't carry either field.
- **`koto dashboard` (interactive TUI)** does load `read_detail` -> `DetailData`
  (`dashboard.rs:254`, `state.detail_cache = dashboard_data::read_detail(&path, id)`), and
  `DetailData.directive` (`dashboard_data.rs:272-296`) *is* populated from the compiled template's
  current-state directive. But this only happens inside the live terminal UI loop — there is no
  scriptable/JSON output path that exposes `DetailData`, so it's unusable by an agent driving
  `koto` non-interactively. And critically, **even `DetailData` has no `details` field at all** —
  its struct (`dashboard_data.rs:272-296`) carries `directive` but not the extended-instructions
  `details` text that PR #109 added to `TemplateState`/`NextResponse`. So even a hypothetical
  scriptable version of the dashboard's detail seam would not, by itself, answer "give me back the
  `details` I lost" — it would need to be extended to read `template_state.details` too.

**What an agent can actually do today, with only the workflow name, to recover instructions
without risking a mutating tick:** nothing through a documented, first-class command. The only
options are (a) call `koto next` (or `--full`) and accept the mutation risk documented above, or
(b) `koto status <name>` for `template_path`, then read that JSON file directly off disk and
extract `.states[<current_state>].directive` / `.details` by hand — an undocumented, cache-internal
path with no stability guarantee, not a supported workflow.

## Round 2: best-fitting command surface, given koto's noun-verb convention

Given (1) `--full` is not safe as a read-only re-fetch, and (2) no existing command exposes
`details` read-only, a new or extended read surface is justified. Two live options, ranked:

1. **Extend `koto status <name>` with an opt-in flag (e.g. `--details`) rather than mint a new
   top-level command.** `status` is already exactly the right shape: read-only by contract
   (`mod.rs:231-235`), keyed by workflow name only, JSON output, already loads
   `derive_machine_state` + the compiled template (`mod.rs:4859-4896`) — the same two objects
   that hold `directive`/`details` for `current_state`. Adding `details`/`directive` fields to the
   existing `handle_status` response (present-when-relevant, following the same convention
   `status` already uses for `batch`/`leg`/`superseded_branches`/`stale_template_source_dir`,
   `mod.rs:4911-4957`) is a few-line change with zero new mutation surface, zero new top-level
   verb, and zero vocabulary drift — the response already says `current_state`, not `phase`. This
   is the minimal-surface option and reuses code instead of duplicating `handle_status`'s
   read-and-derive path in a new function.
2. **If a separate, more discoverable command is preferred** (e.g., because bundling into
   `status` behind a flag is less obvious to an agent that just wants "my instructions back" than
   a dedicated verb), the best-fitting name given the noun-verb survey from round 1 is a **bare
   top-level verb**, not a noun-group subcommand — `status`, `next`, `cancel`, `rewind`,
   `dashboard`, `version` are all single verbs taking a `name` positional; noun-groups
   (`session`, `context`, `template`, `config`, `decisions`, `overrides`, `workspace`, `request`)
   exist for families of related CRUD-style actions, which a single details-lookup is not. Given
   that, `koto state-info <name>` fits the existing "state" vocabulary (matches `current_state`,
   `TemplateState`, `--to <state>` everywhere else in the CLI) and reads as the explicit read-only
   twin of `next`/`status`, without introducing "phase" — the UI-only vocabulary confined to
   `workflows_surface` per round 1's naming findings. `koto phase-info` remains the weaker choice
   on naming grounds alone (see round 1's naming section), and is now also not obviously better
   than option 1 on functional grounds, since option 1 delivers the same read with less new
   surface.

**Recommendation:** favor option 1 (extend `status`) unless there's a discoverability reason
(e.g., prompting/skill-guidance considerations outside this lead's scope) to prefer a standalone
verb — in which case `koto state-info <name>` is the better-fitting name over `koto phase-info
<name>`.

## Findings (round 1)

### The headline finding: the "first-visit-only inline details, `--full` escape hatch" behavior already exists and already ships

`koto next` already implements almost exactly what koto#90 describes, under the name `details`, not `phase-info`:

- `TemplateState` (`src/template/types.rs:54-57`) has both `directive` (short, always present) and `details` (extended instructions, optional).
- `src/cli/mod.rs:3999-4015` (the natural-advancement success path, inside `handle_next`) computes `details` by re-reading events, calling `derive_visit_counts(&post_events)` (`src/engine/persistence.rs:981`), and including `template_state.details` only when `full || count <= 1`. Every `StopReason` branch below it (`GateBlocked`, `EvidenceRequired`, `Integration`, `IntegrationUnavailable`, `ActionRequiresConfirmation`, `SignalReceived`) reuses the same precomputed `details` value (`src/cli/mod.rs:4040-4189`), so the suppression is consistent across all non-terminal response shapes on that path.
- `koto next --full` (`src/cli/mod.rs:147-148`, doc: "Always include the details field in the response, regardless of visit count") is the existing escape hatch. It is documented as covering exactly the scenario koto#90 names for `phase-info`: "context recovery (new session, context compression dropped the instructions)" (`docs/prds/PRD-koto-next-output-contract.md:131`).
- This is not an ad hoc implementation detail — it's a **Done** PRD requirement: `docs/prds/PRD-koto-next-output-contract.md` (source_issue 102, status Done), R9 "Directive split: summary and details," spells out first-visit/subsequent-visit/`--full` semantics verbatim, and the acceptance criteria (lines 186-189) require exactly this behavior, tested.
- It's fully documented downstream too: `docs/guides/cli-usage.md:82` ("By default, `details` is included on first visit to a state and omitted on subsequent visits. This flag forces inclusion every time"), and the koto-user skill (`plugins/koto-skills/skills/koto-user/references/response-shapes.md:37-38`, `command-reference.md:96`) teach agents this exact contract.

**Implication:** koto#90, as titled, is largely already shipped. The gap is not "koto lacks a way to get full phase instructions on demand" — it's narrower and needs to be re-scoped once this is known.

### The one real gap found: the directed-transition (`--to`) path does not suppress `details`

`koto next <name> --to <state>` (`src/cli/mod.rs:3286-3355`) does NOT go through the visit-count-aware code at line 3999+. It calls `dispatch_next` (`src/cli/next.rs:32-124`) directly, which unconditionally sets `details: details.clone()` whenever `template_state.details` is non-empty (`src/cli/next.rs:50-54` and every branch after) — no visit-count check, no `full` parameter in its signature at all. So a directed transition into a state always includes `details`, even on a repeat directed-transition into an already-visited state. This is a real, narrow inconsistency in the existing implementation, not a new command's problem to solve — but it's relevant background: it's the kind of edge case a `phase-info` design should account for (does `phase-info` need to exist partly *because* this path can't be trusted to always carry details? No — `--full` still works everywhere `koto next` is callable; `--to` merely never *needs* the escape hatch because it always includes details already).

### Does any command need a NEW subcommand, or does `koto next --full` already serve as "phase-info"?

Given the above, `koto next <workflow> --full` **is** the existing `phase-info` equivalent. It:
- Takes a workflow name (same argument shape as the proposed `phase-info <workflow>`).
- Returns the full response including `details` for whatever state the workflow is currently in.
- Is idempotent and side-effect-observing-safe for the *current* state (it does not itself advance the workflow — advancement only happens via the auto-advancement loop when new evidence/gates clear, and `--full` doesn't submit evidence).
- Is a read of current position, already exposed with zero new surface.

One real difference from a hypothetical dedicated `phase-info`: `koto next --full` is coupled to the "get the current directive/expects/blocking_conditions/etc. for the current state" contract — it returns the *entire* `NextResponse` envelope (state, directive, details, expects, blocking_conditions, advanced, unassigned_children, etc.), not a details-only payload. If the actual ask is "just the phase instructions, nothing else," `--full` still works (details is one field in it) but is not a minimal read. `koto status <name>` (`src/cli/mod.rs:232-235`, "read-only, no state changes") is the other read candidate, but `status` is not shown to return `details` at all in the docs skim — it's a distinct, thinner surface (worth a design-phase check of `handle_status`, not fully read here).

### `read_detail` -> `DetailData` (the dashboard's seam) is a second, richer read path — but scoped to TUI/dashboard, not CLI JSON

`src/cli/dashboard_data.rs:649` (`pub fn read_detail(path: &Path, session_id: &str) -> Option<DetailData>`) derives, per session: `current_state`, `directive` (from compiled template), `gate_name`/`command`/`result`/`elapsed` (most recent gate eval), `evidence` (current-epoch `EvidenceEntry`s), `intent`, `template_name`, `history` (full current-epoch event list for the History tab), and `remaining` (unvisited state names in topological order, for the Remaining tab) — see `DetailData` struct at `dashboard_data.rs:272-296`.

This is the exact seam `PRD-native-workflows-phase-detail.md` (Done) reuses to enrich the `/workflows` single-session JSON contract with ordered phases, active phase, per-phase evidence/gate outcomes, and blocked status (`docs/prds/PRD-native-workflows-phase-detail.md:44-51, 111-115`). It is a real, reusable, already-proven read seam — but it is dashboard/TUI-internal (`src/cli/dashboard.rs`, `dashboard_data.rs`) and workflows_surface-internal (`src/workflows_surface/`), not a CLI JSON command today. Notably `read_detail`/`DetailData` does NOT include the *template's* `details` field (extended instructions) at all — it exposes `directive` only. So it is not, by itself, a superset of what `koto next --full` already returns; the two seams (next's `details` field vs dashboard's `read_detail`) currently answer different questions ("what should I do" vs "what is the session's structure/history").

### Naming: "state" is the dominant, load-bearing vocabulary everywhere except `workflows_surface` and its doc family

Raw occurrence counts (grep, whole-word, case-insensitive-ish on `state`/`State` vs `phase`/`Phase`):
- `src/engine/`: state 427 vs phase 3 — state totally dominates; this is the substrate (`TemplateState`, `StateFileHeader`, `current_state`, `derive_state_from_log`, `EngineError`, etc.).
- `src/cli/`: state 953 vs phase 36 — CLI surface follows the engine's vocabulary almost everywhere (`koto next` uses `state`, `current_state`; `--to <state>`; error messages say "state '...' not found in template").
- `src/workflows_surface/`: state 70 vs phase 59 — roughly balanced, and this is deliberate: `BRIEF-native-workflows-phase-detail.md:48` explicitly translates — "koto's per-session model has the ordered phases (the template's states)" — i.e., `workflows_surface` is a translation layer that renames koto's internal "state" concept to "phase" for the `/workflows` operator-facing screen, because Claude Code's `/workflows` UI itself uses "phase" as its vocabulary (a tree of phases and steps). `PRD-native-workflows-phase-detail.md` and `BRIEF-native-workflows-phase-detail.md` both use "phase" as their primary noun throughout, consistently glossing it as "(the template's states)" on first use.
- koto-user/koto-author skills and `docs/guides/cli-usage.md`: "state" is the vocabulary for CLI commands and response shapes (`command-reference.md`, `response-shapes.md` — "present on the first visit to a **state**"). No file under `plugins/koto-skills/` was found using "phase" as CLI vocabulary in the grep above.

**Conclusion on naming:** "state" is the CLI/engine/skill vocabulary; "phase" is a UI-rendering-only vocabulary confined to `workflows_surface` and its BRIEF/PRD/DESIGN docs, introduced specifically to match Claude Code's `/workflows` screen's own noun. A new CLI subcommand named `phase-info` would be the *first* place "phase" leaks into the CLI/engine-facing surface (`koto next`, `koto status`, `koto session`, `koto context`, error messages, koto-user skill) that has used "state" exclusively until now. That's a real vocabulary collision: an agent calling `koto next --to <state>` and then `koto phase-info <workflow>` has to know these refer to the same underlying concept under two different nouns, in a tool whose entire other surface says "state." (Round 2 correction: `koto next --full` does **not** actually cover the escape-hatch use case safely — see "Round 2: is `koto next --full` read-only?" above — so the naming argument below now applies to whichever *read-only* surface is chosen, not to `--full` itself.) The naming argument leans toward **not** introducing "phase" into the CLI at all, favoring a "state"-vocabulary name (or, better, extending `status`) rather than adding a differently-named command under a different noun.

### CLI conventions: noun-verb grouping is used, but `phase-info` doesn't fit it

Confirmed noun-verb pattern for grouped concerns: `koto session {start,dir,list,cleanup,resolve,update}`, `koto context {add,get,exists,remove,list}`, `koto template {compile,validate,validate-feed,export}`, `koto config {get,set,unset,list}`, `koto decisions {record,list}`, `koto overrides {record,list}`, `koto workspace {prune}`, `koto request {create,bind,get,wait,list,...}` (ten subcommands), `koto workflows {publish}` (plus bare `koto workflows` for listing).

Top-level bare verbs exist too, for the core workflow-driving actions: `koto init`, `koto next`, `koto cancel`, `koto rewind`, `koto status`, `koto dashboard`, `koto version`. These are one-word, no noun-group, because they're each a single well-known verb on "the workflow" (positional `name` arg).

A standalone `koto phase-info <workflow>` would land in this second category (bare verb-ish noun, single positional arg) — structurally plausible next to `status`. But: (a) it introduces "phase" where the sibling `status`/`next` commands say "state"; (b) it would need to answer "info about which phase — the current one? All of them?" which invites scope creep toward the `read_detail`/`DetailData`/dashboard surface (ordered phases, evidence, gate outcomes) rather than staying a thin re-fetch of `details`. If a new command is still wanted after weighing the `--full` overlap, closer-fitting candidates that don't collide on vocabulary: `koto next --full` (already exists, zero new surface), `koto state-info <workflow>` (matches existing "state" nomenclature, mirrors `koto status`), or extending `koto status <name>` to optionally include `details` via a flag (needs verification of what `handle_status` currently returns — not read in this pass).

### Output/error envelope conventions any new (or extended) command must follow

`docs/reference/error-codes.md:1-11`: two envelope shapes coexist by design. The default flat shape is `{"error": "<string>", "command": "<string>"}` for pre-dispatch I/O errors. Three surfaces use a **structured** envelope instead — `error` is an object with a machine-readable `code` — and carry no `command` field: `koto next`'s domain errors, batch-scoped errors, and the whole `koto request` noun group. `koto next` success responses are the `NextResponse` enum (`src/cli/next_types.rs`) serialized with per-variant custom `Serialize` impls (seen at `next_types.rs:372-401` etc.) — each variant explicitly lists which fields are present (`details` conditionally, via `if let Some(d) = details { map.serialize_entry("details", d)?; }`), which is exactly the "optional = present on first visit... absent on subsequent visits" contract from `cli-usage.md:117`. Any new command returning `details`-shaped content would need to either reuse `NextResponse`'s existing variants (favoring `koto next --full` reuse over a new command) or define a new bespoke envelope and register it in `docs/reference/error-codes.md`'s three-structured-envelope list — a documented, load-bearing enumeration that a new command joining it should not do lightly.

### Stability contract: adding a subcommand is not covered by `docs/STABILITY.md` at all

`docs/STABILITY.md` is entirely about the **wire format** (`CURRENT_SCHEMA_VERSION`, `StateFileHeader`, `EventPayload`, four frozen `SessionBackend` trait methods for bunki BK2). It says nothing about CLI subcommand/flag stability, versioning, or deprecation for the `koto` binary's argument surface itself. `koto-stability-tests/` (a separate crate, `Cargo.toml` + `src/`) appears scoped to the same wire-format/backend-trait contract (not independently verified beyond directory listing in this pass — would need a follow-up read of its test files to confirm it doesn't also pin CLI flags). So: adding `koto phase-info` commits the project to nothing under the existing stability contract — there is no CLI-surface deprecation policy to satisfy. This lowers the cost of adding a command, but also means there's no existing process artifact enforcing that a new command doesn't duplicate an old one — that check is purely a design-review judgment call, which is what this exploration is for.

## Implications

1. **(Superseded by round 2 — kept for record.)** Round 1 concluded `koto next --full` already satisfies koto#90's escape-hatch ask. Round 2 traced the full dispatch path and found `--full` is not read-only: it runs the same mutating advancement loop as a bare `koto next` (gate evaluation with real appends, possible default-action re-execution, possible auto-advance past the phase the agent meant to re-read, possible terminal cleanup). So a genuinely read-only current-state details lookup is NOT already covered, and is the real, justified gap — see the round-2 sections above for the concrete recommendation (extend `koto status`, or `koto state-info` as a fallback name).
2. The naming conclusion from round 1 still holds and now applies more directly: whatever read-only surface is chosen, it should stay in the "state" vocabulary the rest of the CLI/engine/skills use, not import "phase" from the `workflows_surface` UI-rendering layer. `phase-info` would be the first CLI-facing use of that noun and would fragment vocabulary for callers who already read `koto-user`'s skill docs in terms of "state" and "visit."
3. The `--to` directed-transition gap (unconditional `details` inclusion, no visit-count check in `dispatch_next`) is a small, real, pre-existing inconsistency worth flagging to whoever scopes this — independent of the phase-info decision.
4. `read_detail`/`DetailData` is a proven reusable read seam for the dashboard, but (a) it's only reachable interactively, not from a scriptable/JSON path, and (b) it doesn't even carry the `details` field — only `directive`. It is not a shortcut to the read-only surface this issue needs without further extension work of its own.

## Surprises

- Round 1's biggest surprise doesn't survive round 2 unmodified: koto#90's escape-hatch ask *sounds* like it's covered by the Done `PRD-koto-next-output-contract.md`/`--full` (word-for-word matching justification: "context compression dropped the instructions"), but tracing the actual dispatch code shows `--full` was never built to be safe as a standalone read — it just controls one field in a response built by a call that still does everything `next` normally does. The PRD's R9 describes the field-suppression contract; it says nothing about mutation safety of the retrieval path, and nobody appears to have checked that assumption before filing koto#90 with `phase-info` as the ask. That gap between "the field behavior is spec'd" and "the retrieval mechanism is safe to call defensively" is the actual finding underneath koto#90, more than the naming question.
- The `state`/`phase` vocabulary split is not accidental drift — it's a deliberate, documented translation boundary (`workflows_surface` exists specifically to speak "phase" because Claude Code's `/workflows` UI speaks "phase"), not a naming inconsistency to "fix." Introducing `phase-info` into the CLI would be crossing that boundary in the wrong direction (UI vocabulary invading the engine-facing CLI) rather than resolving an existing inconsistency.
- No existing command — not `status`, not the dashboard's `--once` mode, not `template compile/validate` — exposes `directive` or `details` for a live session read-only with just the workflow name. The dashboard's interactive-only `DetailData.directive` is the closest thing that exists anywhere in the codebase, and even that lacks `details` entirely.

## Open Questions

- Was koto#90 filed with awareness of `PRD-koto-next-output-contract.md`/`--full`? If not, the issue may need to be re-scoped or closed as already-addressed, with only the `--to` path gap and/or a discoverability improvement (e.g., docs, or a shorter alias) left as real work. This needs a human/issue-author check, not more code reading.
- What does `koto status <name>` actually return today (`handle_status` was not read in this pass) — does it already expose `details`/directive for the current state, making it an even closer existing match than `--full`?
- Does `koto-stability-tests/` pin anything about the CLI flag/subcommand surface (as opposed to just the wire format), which would matter if a new command or flag is added?
- If the team decides `--full`'s full envelope actually is the discoverability problem (agents don't know to reach for it after context compression, precisely because they've lost the instructions that would remind them), is the real fix a *thinner, separate* read (favoring a new minimal command) or better prompting/protocol guidance embedded in the `directive` field itself (e.g., "if you're missing details, call `koto next <name> --full`") — the latter needs zero new CLI surface.

## Summary

`koto next --full` already implements almost exactly what koto#90 asks for: `details` (full phase instructions) rides inline on first visit to a state, is omitted on repeat visits, and `--full` is the documented, tested, Done escape hatch for when context compression drops it (`PRD-koto-next-output-contract.md` R9, source_issue 102) — the one real gap is that the `--to` directed-transition path (`src/cli/next.rs` `dispatch_next`) never suppresses `details` in the first place, so it never needs the hatch. The biggest naming risk in adding a `phase-info` command is vocabulary collision: "state" is the CLI/engine/skill-wide term (`src/engine/`, `src/cli/`, koto-user skill), while "phase" is a deliberate, docs-confirmed translation used only by `workflows_surface` to match Claude Code's `/workflows` UI noun, so a CLI `phase-info` would be the first leak of UI vocabulary into the state-speaking CLI. The open question that most needs a human decision is whether koto#90 was filed without knowledge of the already-Done `--full` contract, which would make this exploration's real deliverable a reconciliation/re-scope rather than a new-command design.
