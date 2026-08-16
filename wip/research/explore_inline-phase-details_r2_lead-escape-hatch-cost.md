# Lead: What does the read-only recovery path cost, in each candidate shape?

## Findings

### Candidate A: extend `koto status` with a details/directive payload

`handle_status` (`src/cli/mod.rs:4834-4961`) already does every piece of I/O the recovery
path needs: it reads events, calls `derive_machine_state` (4859), reads and parses the
compiled template into a `CompiledTemplate` (4873-4896), and does
`compiled.states.get(&machine_state.current_state)` (4898-4901) to compute `is_terminal`.
The `TemplateState` it looks up is the exact struct that carries `directive` and `details`
(referenced directly in `src/cli/next_types.rs:8` import list and used at
`src/cli/mod.rs:4001-4014` in `handle_next` — `final_template_state.directive.clone()` and
`final_template_state.details`). Adding the two fields to `status`'s output is a lookup on
data already in scope, not new I/O.

The `Status` command definition (`src/cli/mod.rs:232-235`) is a two-field variant (`name:
String`) with no flags at all today:

```rust
Status {
    /// Workflow name
    name: String,
},
```

Clap supports adding a flag cleanly — same pattern as `Next`'s `--full: bool`
(`src/cli/mod.rs:148`) — but a flag raises a real naming question the lead flagged: should
it be named `--details` (a new word) or `--full` (reusing `next`'s existing flag name for a
different command)? Neither is free of confusion. The **cheaper option structurally** is to
return `directive`/`details` unconditionally, with no flag at all — `status` is already a
snapshot read, and the "present only when relevant" convention that governs `batch`, `leg`,
`superseded_branches`, and `stale_template_source_dir` (`src/cli/mod.rs:4911-4957`, each
commented as populated "when relevant") is about *relevance*, not about a caller-supplied
flag. Under that convention `details` would be present whenever the state has details
content (mirroring `next`'s `Option<String>` — empty details serialize to absent, per
`src/cli/mod.rs:4001-4003`), full stop — no flag, no visit-count gating, since `status`
has no notion of "first visit" to key off of and inventing one (re-deriving
`derive_visit_counts` from the event log, `src/cli/mod.rs:4009`) would be new logic, not
reuse. `directive` should almost certainly ship alongside `details` — a directive with no
details is still useful, and omitting it would make the recovery response strictly weaker
than a first-visit `next` tick for no evident reason.

**Testing implication of the untyped-JSON contract.** `status`'s response is a bare
`serde_json::json!` (`src/cli/mod.rs:4903-4909`), unlike `next`'s response, which is a
typed `NextResponse` enum with a hand-rolled `Serialize` that tracks field counts explicitly
(`src/cli/next_types.rs:62-127`, `372` `let count = 8 + details.as_ref().map_or(0, |_|
1)`). That means: no enum variant to update, no serializer field-count arithmetic to keep in
sync, no `NextResponse` test fixtures to touch. The cost is a couple of new keys in one
`json!` macro call and new assertions in the existing `assert_eq!(json["..."], ...)` style
already used at `tests/integration_test.rs:6895-6963` (`status_active_workflow`,
`status_terminal_workflow`, `status_missing_workflow`). The flip side: because it's an
untyped `json!`, the output contract is enforced nowhere in the type system — same as
today's `batch`/`leg`/`superseded_branches` keys, which are documented only in prose
(`docs/guides/cli-usage.md:929`, `plugins/koto-skills/skills/koto-user/references/
command-reference.md:271-301`). Drift risk for the new fields is identical to the drift
risk `status` already carries for its existing optional keys — not worse, not better.

### Candidate B: new `koto phase-info <workflow>` subcommand

Full registration footprint, using `Status` as the parallel case:

1. New `Command` variant (alongside `Status` at `src/cli/mod.rs:232-235`).
2. New dispatch arm in the giant `match` (alongside `Command::Status` at
   `src/cli/mod.rs:1180-1183`).
3. New `handle_phase_info` function — near-duplicate of `handle_status`'s ~130 lines
   (4834-4961) for the read/parse/lookup path, unless factored into a shared helper (the
   `batch_view` module, `src/cli/batch_view.rs`, is the existing precedent for factoring a
   shared read-only-derivation helper out of `handle_status` — the same pattern would apply
   here).
4. New row in `command-reference.md`'s subcommand table (`plugins/koto-skills/skills/
   koto-user/references/command-reference.md:10-38` lists every subcommand including
   `koto status` at line 25 — a new command needs a new row here; `status` needed no such
   row change since it already has one).
5. New `### koto phase-info` section in both `command-reference.md` (mirroring `## koto
   status` at lines 271-311) and `docs/guides/cli-usage.md` (mirroring the `### status`-
   equivalent content near line 929) — net *more* doc surface than extending `status`,
   which edits an existing section instead of writing a new one.
6. New integration tests (can't extend `status_active_workflow` et al.; needs its own
   `phase_info_active_workflow` / `_terminal_workflow` / `_missing_workflow` trio).

**Does it join the structured-error-envelope list?** `docs/reference/error-codes.md:11`
names exactly three surfaces using the structured `{"error": {"code": ..., ...}}` envelope:
`next`'s domain errors, batch-scoped errors, and the whole `koto request` group. Notably,
`status` itself is **not documented anywhere in error-codes.md** — there is no `### status`
section despite `status` having two real error paths (workflow-not-found at
`src/cli/mod.rs:4836-4842`, corrupt-state/template errors at 4849-4896), both using the
flat `{"error": "...", "command": "status"}` shape. That's a precedent, not a gap this
candidate has to fix: `phase-info`, built the same way `status` is, would stay in the flat-
error camp and would not need to join the three-surface list — matching `status`'s existing
(undocumented) behavior rather than creating new obligation.

**Naming.** This is the candidate with the vocabulary cost the lead flagged in round 1
context: `src/cli/` and `src/engine/` speak "state" almost exclusively (953 vs 36, 427 vs 3
occurrences respectively), and "phase" is otherwise confined to `src/workflows_surface/`
as a deliberate translation for the Claude Code `/workflows` UI screen. A `koto phase-info`
subcommand would be the first UI-vocabulary leak into the CLI's own command namespace —
a naming cost that doesn't show up in a file-touch count but is exactly the kind of
decision a downstream reviewer would push back on given the codebase's own internal
consistency argument.

### Candidate C: `koto next --dry-run`

No direct precedent for a *gate-skipping, state-non-advancing* dry-run. The one `dry_run`
flag in the codebase is `koto workspace prune --dry-run` (`src/cli/mod.rs:294`,
`src/cli/workspace.rs:65-145`), which is a different shape of problem: prune's dry-run
still does the full scan and reports exactly what *would* be deleted, with no gates, no
actions, and no state machine involved at all. It's a precedent for "the flag name and CLI
convention exist," not for "dry-run through an advance loop."

`handle_next` is a large, deeply threaded function (its opening doc comment is at
`src/cli/mod.rs:2867`, with logic continuing past `4517`) covering the advance loop, gate
evaluation, action execution, the retry-outcome thread-local (`RETRY_OUTCOME`,
`src/cli/mod.rs:54-58`), the Issue 7 discovery scan with its per-coordinator cursor file
(mentioned at `src/cli/mod.rs:4019-4025`), and batch/request-store dispatch bookkeeping. A
`--dry-run` that "evaluates nothing, advances nothing, just reports what the current
state's response would be" has to correctly short-circuit *all* of that — gate evaluation
(`src/gate/`), action execution, event-log appends, and the discovery-scan cursor write —
inside a single entry point that was not designed with a no-op path in mind. That's a much
larger and riskier surface than A or B: a missed short-circuit doesn't just produce a wrong
answer, it silently mutates state or advances the workflow under a flag whose name promises
it won't. Given `next`'s domain-error and structured-envelope machinery
(`docs/reference/error-codes.md:35-108`), a dry-run mode would also need its own documented
answer to "which of the nine domain error codes can dry-run still produce" (arguably none,
since gate evaluation is what produces `gate_blocked`/`integration_unavailable`) — a new
doc obligation neither A nor B carries.

### Mandatory downstream work (per koto's own CLAUDE.md skill-assessment rule)

`CLAUDE.md:58-73` requires assessing both `koto-author` and `koto-user` after any change to
`src/cli/`, `src/engine/`, `src/gate/`, or `src/template/`, checking for (1) broken
contracts and (2) new surface neither skill documents. The area table (`CLAUDE.md:66-73`)
maps `src/cli/` to "both" — so any of the three candidates triggers assessment of both
skills, not just `koto-user`.

**Where the `details`/`--full` contract is documented today** (the baseline a new command
or flag must match, and PR #109's precedent set):

- `plugins/koto-skills/skills/koto-user/references/response-shapes.md:15-16, 34-38` — the
  field-presence table and the prose rule ("present on the first visit... absent on
  subsequent visits unless `--full` is passed... always absent on `done`").
- `plugins/koto-skills/skills/koto-user/references/command-reference.md:96` — the `--full`
  flag's one-line description in the `koto next` flags table.
- `plugins/koto-skills/skills/koto-author/SKILL.md:67` — "On first visit to a state, a
  `details` field may contain extended guidance (pass `--full` to force it on repeat
  visits)."
- `docs/guides/cli-usage.md:82, 108, 117` — the `--full` flag description and the
  field-presence legend ("optional" = present on first visit or with `--full`).
- `plugins/koto-skills/.cursor/rules/koto.mdc:168-175` — "## The details Field" section,
  the third and least-obvious location (this file is a Cursor-specific mirror of skill
  content, at `plugins/koto-skills/.cursor/rules/koto.mdc`, not at repo root — the lead's
  guessed path `.cursor/rules/koto.mdc` doesn't exist as such; the real path is nested
  under the plugin).

No `AGENTS.md` exists in this repo at all (checked repo root and via `find`) — the lead's
premise that PR #109 touched it doesn't hold in this worktree; either it was removed since
or never existed here. Drop it from the required-touch list.

**Per-candidate file-touch table** (docs/skills only; source-file footprint above):

| | Candidate A (`status` extended) | Candidate B (`phase-info` subcommand) | Candidate C (`next --dry-run`) |
|---|---|---|---|
| Source files | `src/cli/mod.rs` (Status variant + handle_status body) | `src/cli/mod.rs` (new variant, new dispatch arm, new handler; possible new `src/cli/*.rs` module if factored like `batch_view.rs`) | `src/cli/mod.rs` (handle_next, extensively — gate/action/event-append/discovery-scan short-circuits threaded through an already-large function) |
| `command-reference.md` | edit existing `## koto status` section (271-311) + top table row unchanged | new `## koto phase-info` section + new top-table row (10-38) | edit existing `## koto next` section + flags table (96) |
| `cli-usage.md` | edit existing status section (~929) | new subcommand section | edit existing next section (82, 108, 117) |
| `koto-user` SKILL/refs | `response-shapes.md` (new fields, or note status now carries them), `command-reference.md` status section | `response-shapes.md`, `command-reference.md` (new section + table row), likely a "when to use status vs phase-info vs next --full" note | `response-shapes.md`, `command-reference.md` next section |
| `koto-author` SKILL.md | possible footnote near line 67 | possible footnote near line 67 | possible footnote near line 67 |
| `.cursor/rules/koto.mdc` | edit "## The details Field" (168-175) | add a mention | edit "## The details Field" |
| `error-codes.md` | none required (status undocumented there today; precedent holds) | none required (same precedent) but arguably worth a `### phase-info` addition given it's brand new | needs new prose on which domain error codes dry-run can/can't produce |
| Evals | koto-user (and likely koto-author) — extend/add eval(s) exercising the new field or command; `scripts/check-evals-exist.sh` only requires ≥1 eval per skill, no minimum growth, but the skill content changed so an eval demonstrating correct use is the existing pattern (see `plugins/koto-skills/skills/koto-user/evals/evals.json` id 1, `session-init-first-cycle`) | same, plus likely a discriminability-focused eval (status vs phase-info vs next --full — three similar read paths risk an agent picking the wrong one) | koto-user eval(s) covering dry-run usage and clarifying it's distinct from a real tick |
| Integration tests | extend `tests/integration_test.rs` status tests (6895-6963) | new test trio, can't reuse existing status tests | tests threaded through `handle_next`'s existing large test surface — most invasive to get right given the side-effect surface |

`scripts/run-evals.sh` (top-of-file usage comment) and `scripts/check-evals-exist.sh`
(`for skill_dir in ... evals_file="$skill_dir/evals/evals.json" ...`) confirm evals are
discovered per-skill from `plugins/*/skills/*/evals/evals.json` and CI only enforces
"at least one eval exists" — it does not enforce that skill content changes come with new
evals. That enforcement is social (the `CLAUDE.md:58-64` skill-assessment rule), not
mechanical.

### Discoverability

Per `response-shapes.md`'s field-presence table (lines 11-24): `action`, `state`, and
`advanced` are always present on every `next` response including `done` and `error`.
`directive` is present on **every variant except `done`** (line 15, and reinforced at line
34: "The `done` variant has no `directive` field in its struct — the key is not written at
all"). `details` and `expects`-as-populated-object are conditional. Since an agent that has
lost context is, by construction, mid-workflow (a `done` workflow has nothing left to
recover), `directive` is the one field guaranteed to reach that agent on the very next tick
regardless of what it forgot.

There's already a mechanism for koto-authored text riding inside `directive`
unconditionally: `response-shapes.md:31` documents that an abandoned request leg "prepends
a koto-authored stop notice to `directive`." That's the precedent for injecting a fixed,
system-authored pointer into `directive` on every tick rather than relying on template
authors to remember to mention the recovery command in their own directive text. The
tension: `error-codes.md:298` notes elsewhere in the codebase that a large per-tick string
"is a context-exhaustion problem, not merely a large string" (in the context of the
`--rationale` cap) — the same logic that motivates omitting `details` after the first visit
argues against permanently inflating every `directive` with a recovery-command reminder.
A short one-line, always-present pointer (in the spirit of the leg-abandonment notice) is
the cheapest way to guarantee discoverability without re-inflating every tick to first-visit
size.

### CHANGELOG and release conventions

`CHANGELOG.md` follows Keep a Changelog format with hand-written `### Added` bullet lists
per version (`CHANGELOG.md:67-80` for 0.10.0), e.g. `- koto-stability-tests/ external-
consumer fixture crate. Imports every promised export...`. No changelog automation was
found. No shell-completion generation exists in the codebase (`grep` for
`clap_complete`/`generate_completions` across `src/` and `Cargo.toml` returns nothing) — a
new subcommand does not require regenerating or committing completion scripts, which
removes one potential footprint item from Candidate B that might otherwise be assumed.
`README.md` mentions specific commands in prose (`koto next`, `koto rewind`, `koto session
dir`, etc., lines 5-155) but is not a canonical command index the way `command-reference.md`
is — updating it for any candidate is optional/prose-level, not structurally required.

## Implications

- Candidate A's dominant cost is a naming/flag decision (unconditional fields vs a new
  flag vs reusing `--full`'s name), not implementation size — the read path already exists
  end to end.
- Candidate B's dominant cost is the vocabulary decision (`phase` vs `state`-consistent
  naming) plus genuinely new doc sections rather than edits to existing ones; its
  mechanical registration footprint is otherwise no larger than `status`'s own.
- Candidate C is the outlier: its cost isn't doc/skill surface, it's the risk of adding a
  no-op mode to a large, side-effect-heavy function that was never designed for one. Every
  other candidate's downstream work is roughly proportional to "new field vs new command";
  C's is proportional to "how many of `handle_next`'s side channels can be proven skippable."

## Surprises

- `status` itself is undocumented in `error-codes.md` today despite having real error
  paths — a pre-existing gap, not something either A or B introduces, but worth naming
  since round 1 assumed error-codes.md's "three surfaces" claim was exhaustive over
  *documented* commands rather than over commands that use the structured envelope.
- The lead's brief assumed `AGENTS.md` and `.cursor/rules/koto.mdc` both exist at
  predictable paths per PR #109 precedent. Only the Cursor rules file exists, and at a
  different path than guessed (`plugins/koto-skills/.cursor/rules/koto.mdc`, not repo-root
  `.cursor/rules/koto.mdc`). No `AGENTS.md` exists anywhere in this repo.
- There's already a working precedent for unconditionally injecting koto-authored text into
  `directive` (the leg-abandonment stop notice), which directly answers the discoverability
  question in item 5 with existing machinery rather than a new mechanism.

## Open Questions

- If Candidate A ships fields unconditionally rather than behind a flag, does that count as
  an additive/non-breaking change under whatever JSON-output stability expectations
  `status` consumers currently have (none appear formally documented the way
  `docs/STABILITY.md` documents the `koto::engine::types::*` Rust surface)?
- For Candidate B, would `phase-info`'s error paths get a first-ever `### status`-style
  section in `error-codes.md`, closing the existing gap, or would it inherit the gap by
  precedent?
- Does the discoverability answer (inject a fixed pointer into `directive`) belong in this
  issue's scope at all, or is it a separate, template-independent enhancement layered on
  top of whichever candidate ships?

## Summary

Candidate A (extend `koto status`) is the cheapest: the read path, JSON shape, and test
style already exist, and its only real decision is naming (flag vs unconditional fields).
Candidate B (`phase-info`) has a comparable mechanical footprint to A but adds genuinely new
doc/skill sections plus a vocabulary cost — it would be the CLI's first "phase"-named
surface against 953 "state" occurrences. Candidate C (`next --dry-run`) is the most
expensive and riskiest by far, since it requires proving that every side effect inside a
large, already-complex `handle_next` can be safely skipped rather than just adding new
output.
