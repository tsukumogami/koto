# Lead: Is the premise of koto#90 real -- do koto-backed skills pay a separate Read call per phase, and how large is the payload?

## Findings

### The feature koto#90 asks for is already shipped

`docs/designs/current/DESIGN-koto-next-output-contract.md` documents a design decision made and implemented before #90 was ever addressed by any PR: a `<!-- details -->` marker inside a state's markdown section splits directive text into two parts. Content before the marker is `directive` (always returned); content after is `details` (returned only on first visit to the state, or when the caller passes `--full`). This is not aspirational documentation -- it is implemented end to end:

- `src/template/types.rs`: `TemplateState` carries a real `details: String` field.
- `src/template/compile.rs`: `extract_directives` splits state body sections on the first `<!-- details -->` marker (documented in `plugins/koto-skills/skills/koto-author/references/template-format.md:92-124`).
- `src/cli/next_types.rs`: every non-terminal `NextResponse` variant carries `details: Option<String>`, serialized only when `Some`.
- `src/engine/persistence.rs`: `derive_visit_counts` scans the JSONL event log (`Transitioned`, `DirectedTransition`, `Rewound`) to count how many times a state has been entered -- exactly the "derive first-visit from the existing log, no new state files" mechanism #90's acceptance criteria demand.
- `src/cli/mod.rs:3999-4015` (in `handle_next`): after the advancement loop lands on a final state, it computes `details` as `Some(...)` only when `full || count <= 1`, else `None`. `full` is the `--full` CLI flag on `koto next`.

This is exactly #90's acceptance criteria "Workflow template states support an optional `details` field alongside `directive`", "First visit ... includes `details`", "Subsequent visits ... omit `details`", and "Phase visit tracking uses existing JSONL state (no new state files)" -- all four are true today.

Landed in PR #109 ("feat(cli): redesign koto next output contract", merged 2026-03-30T15:18:10Z, closing #102), four days after #90 was filed (2026-03-26T22:00:43Z). #90 is not linked as closed or referenced by that PR -- it appears to have been overtaken by unrelated work rather than deliberately addressed, then left open.

### The one piece #90 asks for that does NOT exist: `koto phase-info`

There is no `koto phase-info` subcommand anywhere in the CLI (`grep -rn "phase-info\|phase_info"` across all `.rs` and `.md` files returns nothing outside this exploration's own wip scope doc). The equivalent escape hatch that does exist is the `--full` flag on `koto next`, which forces `details` back into the response on a repeat visit regardless of visit count. Functionally this covers #90's stated need ("let agents explicitly re-read details if context compression dropped them") without a new command -- `koto next <name> --full` already does it. Whether a dedicated `phase-info` verb is still worth adding (read-only, no side effects, explicit "just show me the details" semantics vs. `--full` riding on the same command that also may re-evaluate gates/advance) is a naming/API-surface question for another lead, not a gap in capability.

### Do the shipped skills tell agents to read a separate file per phase?

**koto-user/SKILL.md** (the generic "how to run any koto workflow" skill) does not instruct per-phase file reads at all. Its loop is: call `koto next`, read `directive` (and `details` on first visit), act. Reference material (`command-reference.md`, `response-shapes.md`, `error-handling.md`, `batch-workflows.md`) is explicitly "Read these on demand, not upfront... Consult a reference file only when you hit the specific situation it describes" (SKILL.md:467-474) -- that's meta-documentation about the CLI contract, not per-phase task instructions, and is out of scope for #90.

**koto-author/SKILL.md** documents the details mechanism directly: "Read the `directive` for instructions. On first visit to a state, a `details` field may contain extended guidance (pass `--full` to force it on repeat visits)" (SKILL.md:67). Its own reference material (template-format.md, batch-authoring.md, example templates) is likewise consult-on-demand, tied to specific *states* in the koto-author workflow itself (state_design, template_drafting), not a blanket per-phase pattern.

So neither shipped skill instructs "read a phase file every time you enter a phase." The premise as stated in #90's Context section ("the directive tells it to read a phase file for instructions... forces a Read tool call on every first visit") does not describe koto-user or koto-author. It describes a pattern found in **downstream templates that individual skills author for themselves** -- see below.

### Real-world templates: the premise IS real, just not in the shipped skills, and not solved by adoption

Searched `koto/plugins/`, `koto/test*/`, `koto/.claude/`, and the sibling repos `public/shirabe`, `public/niwa`, `public/tsuku` for files with `initial_state:` in frontmatter (the unambiguous koto-template signature). Results:

| Template | Location | States | `<!-- details -->` uses | Size |
|---|---|---|---|---|
| `koto-author.md` | koto's own shipped skill | 9 | 2 | 13,195 B |
| `complex-workflow.md` | koto-author example | ~6 | example-only | 2,897 B |
| `batch-coordinator.md` | koto-author example | ~5 | example-only | 3,378 B |
| `execute.md` | shirabe `/execute` skill | 12 | **0** | 40,224 B |
| `work-on.md` | shirabe `/work-on` skill | 27 | **0** | 42,049 B |

No `initial_state:`-bearing template found under `public/niwa` or `public/tsuku`.

`work-on.md` is the clearest, largest real-world instance of the pattern #90 describes. Roughly a third of its 27 states have a directive body whose entire content is a one-line pointer, e.g. (line numbers in `public/shirabe/skills/work-on/koto-templates/work-on.md`):

- `context_injection` (806): `Read \`references/phases/phase-0-context-injection.md\` for detailed steps.`
- `setup_issue_backed` / `setup_free_form` / `setup_plan_backed` (880, 887, 924): `Read \`references/phases/phase-1-setup.md\` for branch naming and baseline format.`
- `introspection` (955): `Read \`references/phases/phase-2-introspection.md\` for steps and evidence options.`
- `analysis` (962), `implementation` (984), `scrutiny` (1003), `review` (1011), `qa_validation` (1019), `finalization` (1048), `pr_creation` (1087), `ci_monitor` (1097): same pattern, one `Read \`references/phases/phase-N*.md\`` line each.

That's a genuine, measured Read-tool-call-per-phase pattern -- in production, in the org's own flagship implementation skill, four-plus months after the engine mechanism that would eliminate it (`<!-- details -->` + first-visit gating) had already shipped (PR #109 merged 2026-03-30; `git log --follow` on `work-on.md` shows edits as recent as 2026-08-03 that still didn't adopt the marker).

The dozen referenced phase files under `public/shirabe/skills/work-on/references/phases/` total 30,216 bytes across 12 files, averaging ~2,518 bytes (~630 tokens at a rough 4 bytes/token) each, ranging from 751 B (`phase-2-introspection.md`) to 5,389 B (`phase-4-implementation.md`, ~1,350 tokens). `execute.md` uses the same pointer style twice (`worktree-discipline.md`, `phase-6-pr.md`, ~2-4 KB each) but otherwise inlines its directive text directly in the template body already -- it's a mixed case, not a clean example either way.

### Quantifying the overhead this would eliminate

For `work-on.md`: roughly 9-10 of 27 states carry a "Read `phase-N.md`" directive. Each such state costs, on a first visit: one `koto next` round trip (tool call + JSON result) *plus* one `Read` tool call (tool call + full file content back into context, 751-5,389 bytes). Inlining via `<!-- details -->` collapses that second call entirely -- the same bytes ride inside the `koto next` JSON response's `details` field instead of a second tool round trip. Per the design doc and implementation, this costs nothing extra on repeat visits (self-loops, retries) since `details` is omitted once `count > 1` (unless `--full` is passed) -- exactly the token-saving half of #90's ask.

A typical `/work-on` run that touches all ~10 file-pointing states would go from 10 extra Read calls (10 tool-call round trips, ~30 KB of file content plus whatever framing/tool-result overhead the harness adds per call) to zero, at the cost of those same ~30 KB being embedded directly into 10 `koto next` responses instead -- a wash on raw bytes, but a savings on tool-call count and the fixed overhead each Read call carries (the file path resolution, the "cat -n" line-number framing Read adds per the tool's own contract, and the extra conversational turn). The mechanism to realize this saving already exists; it's unused by the org's largest template.

### Is there a response-size ceiling?

No size ceiling was found on `directive` or `details` content in `src/template/compile.rs`, `src/template/types.rs`, or the PRD/design docs. The only size limit found anywhere near this surface is unrelated: `--with-data` evidence payloads submitted *to* koto are capped at 1 MB (`koto-user/SKILL.md:108`), and `--inputs` on `koto session start` is capped at 1 MiB / 128 nesting levels (`koto-user/SKILL.md:207`). Nothing constrains how large a compiled template's `directive`/`details` text can be, so an enormous details block would simply ride through uncompressed in the `koto next` JSON -- no engine-side guard exists against an author pasting a 50 KB reference doc into `<!-- details -->`.

### Counter-evidence: nothing argues against inlining

Found no doc, CHANGELOG entry, or closed issue arguing for keeping directives short as a deliberate constraint. To the contrary, `docs/designs/current/DESIGN-koto-next-output-contract.md` explicitly rejected the two alternatives that would have kept directives *short and external*:

- **YAML `summary` field** (short inline summary + external body) -- rejected because it splits content across two locations and fights YAML multiline ergonomics.
- **`details_file` (external file reference, inlined at compile time)** -- rejected because it "breaks the single-file template model, adds file resolution complexity to the compiler, and creates the highest maintenance burden for template authors."

Both rejections favor *more* inlining, not less -- the design explicitly chose to keep everything in one file and to let `details` be as long as an author wants, gated only by visit count. There is no evidence anywhere in the repo of a deliberate "keep directives short" design principle that would resist #90's goal.

## Implications

The core question this lead was asked to answer -- "is the premise real" -- has two different answers depending on which artifact you check. Against the shipped `koto-user`/`koto-author` skills, the premise as literally stated in #90 ("the directive tells it to read a phase file for instructions... pure overhead") is not accurate: those skills already ship the exact mechanism #90 wants, and have since PR #109 (merged 2026-03-30, four days after #90 was filed). Against real downstream usage -- specifically `work-on.md`, the org's most complex production template -- the premise is accurate and measurable: ~10 of 27 states pay a genuine Read-tool-call-per-phase tax, moving ~30 KB across 12 files that could collapse into the existing `<!-- details -->`/first-visit mechanism with zero new engine work.

This reframes the whole exploration. There is no engine gap to design around for the "inline on first visit, omit on repeat" behavior -- that's DESIGN-koto-next-output-contract.md's Decision 1, already built, already tested (PR #109 test plan: "404 unit tests pass... including new serialization, error code, visit count, and template splitting tests"). What's missing is adoption: `work-on.md` and `execute.md`, the two real templates that would benefit most, don't use the marker. The `koto phase-info` half of #90 (an explicit re-read command) has no engine equivalent besides `--full`, which is close but not identical (it rides the same command that may also advance state / re-evaluate gates, rather than being a pure read-only query).

Given this, the highest-leverage next steps are not "design a new feature" but: (1) confirm with the other leads whether `--full` fully substitutes for a dedicated `phase-info` verb or whether a read-only variant is still warranted, and (2) treat #90 as substantially an *adoption/migration* issue against `work-on.md`/`execute.md` rather than a *net-new engine capability* issue. That changes the acceptance criteria, complexity classification, and likely the issue's own title and scope.

## Surprises

- The single biggest surprise: the feature request in #90 was essentially already built by the time #90 would plausibly have been triaged, via an unrelated PR (#109, "Fixes #102") that never referenced #90. #90 was filed 2026-03-26, PR #109 merged 2026-03-30 -- four days later, same week, but no cross-reference exists. This looks like two people/agents independently converging on the same design without linking the artifacts, and #90 was simply never revisited to check whether it had been overtaken.
- The org's own largest, most actively-maintained template (`work-on.md`, edited as recently as 2026-08-03) has *never* adopted the `<!-- details -->` marker despite the capability being available for over four months at that point. That's a strong signal the mechanism's existence alone hasn't been enough -- either nobody who authored/maintains `work-on.md` knows about it, or there's friction in retrofitting it onto an already-large template with many one-line pointer directives.
- `koto-author`'s own SKILL.md documents the mechanism correctly (SKILL.md:67) and its own template dogfoods it on 2 of 9 states (`state_design`, `template_drafting` per the design doc) -- so the authoring skill that's supposed to teach this pattern to template authors does it right. The gap is between "the authoring skill teaches it" and "the flagship consuming template (work-on.md) uses it," which points at either an authoring-time discoverability problem or work-on.md predating awareness of the pattern (its earliest commit, 2026-03-26, is literally the same day #90 was filed, four days before the mechanism existed).

## Open Questions

- Was `work-on.md`'s pointer-per-phase pattern a deliberate choice (e.g., keeping the compiled template's `directive` array small for some other reason, like the `/workflows` native rendering surface) or pure oversight? Worth checking with whoever maintains shirabe, or checking shirabe's own design docs for a stated reason.
- Does `--full` interact safely with gate re-evaluation / state advancement in a way that would make a dedicated read-only `phase-info` verb meaningfully safer or more correct, or is `--full` already side-effect-free enough that a new verb is pure API-surface duplication? (Handoff to the lead investigating template grammar / response contract, if not already covered.)
- Would retrofitting `<!-- details -->` onto `work-on.md`'s ~10 pointer states be in scope for whatever comes out of this exploration, or is that separate follow-up work against the shirabe repo (which this session cannot modify)?
- Is there a reason `execute.md` and `work-on.md` reference *separate phase files* rather than inlining text directly, beyond directive length -- e.g., are those phase files also read/reused by non-koto tooling, which would make "just inline it" a partial solution that still needs the standalone file to exist for other consumers?

## Summary

The exact behavior koto#90 requests -- `details` field inline on first visit, omitted on repeat visits, derived from the existing JSONL log with no new state files -- already shipped in PR #109 four days after #90 was filed, and koto's own shipped skills correctly document and use it. The real, measurable version of the premise lives downstream: shirabe's `work-on.md` (27 states, the org's largest template, still being edited as of 2026-08-03) has ~10 states doing a literal "Read `references/phases/phase-N.md`" per-phase file read totaling ~30 KB across 12 files, and has never adopted the `<!-- details -->` mechanism that exists specifically to eliminate that cost. The open question for the rest of the exploration is whether this becomes an adoption/migration effort against real templates plus a possible `phase-info` read-only verb, rather than new engine design.
