# AGENTS.md Migration Catalog

Source file: `plugins/koto-skills/AGENTS.md`
Plugin manifest: `plugins/koto-skills/.claude-plugin/plugin.json`

This catalog maps every section and concept in AGENTS.md to its destination in the
koto-user skill. Implementers writing koto-user reference files should use this
alongside the engine and gate catalogs.

---

## Plugin manifest notes

`plugin.json` lists only `./skills/koto-author` in `skills`. The koto-user skill does
not yet exist. Once created, it must be added to this array.

---

## Content catalog

| Content item | Current location | Destination | Notes |
|---|---|---|---|
| "What is koto?" intro paragraph | AGENTS.md § "What is koto?" | koto-user SKILL.md | Good concise orientation; accurate. Include verbatim or lightly reworded. |
| Prerequisites / `koto version` check | AGENTS.md § "Prerequisites" | koto-user SKILL.md | Accurate. Keep as a short prerequisite block. |
| `koto init` syntax and flags (`--template`, `--var`) | AGENTS.md § "koto init" | koto-user references/command-reference.md | Accurate. Covers positional name arg, `--template`, repeatable `--var`. |
| `koto init` success JSON shape `{"name": ..., "state": ...}` | AGENTS.md § "koto init" | koto-user references/response-shapes.md | Accurate. Small distinct shape; document alongside `koto next` shapes. |
| `koto next` overview (three modes) | AGENTS.md § "koto next" | koto-user references/command-reference.md | Accurate. The three-mode framing (no flags / `--with-data` / `--to`) is the right mental model. |
| `koto next <name>` (no flags) — get directive | AGENTS.md § "koto next" | koto-user references/command-reference.md | Accurate. |
| `koto next <name> --with-data '<json>'` — submit evidence | AGENTS.md § "koto next" | koto-user references/command-reference.md | Accurate. |
| `koto next <name> --to <state>` — directed transition | AGENTS.md § "koto next" | koto-user references/command-reference.md | Accurate. |
| `koto next <name> --full` flag description | AGENTS.md § "koto next" | koto-user references/command-reference.md | Accurate. Cross-reference with `details` field section. |
| `--with-data` and `--to` mutual exclusivity note | AGENTS.md § "koto next" | koto-user references/command-reference.md | Accurate. Important constraint; keep as a callout. |
| `koto decisions record` syntax and required/optional fields | AGENTS.md § "koto decisions record" | koto-user references/command-reference.md | Accurate. `choice` and `rationale` required; `alternatives_considered` optional. |
| `koto decisions list` syntax and return shape | AGENTS.md § "koto decisions list" | koto-user references/command-reference.md and references/response-shapes.md | Accurate. Response shape (JSON array of decision objects) should be documented in response-shapes.md. |
| `koto rewind` syntax and return shape | AGENTS.md § "koto rewind" | koto-user references/command-reference.md and references/response-shapes.md | Accurate. Note that repeated calls walk back multiple steps. |
| `koto rewind` "cannot rewind past initial state" note | AGENTS.md § "koto rewind" | koto-user references/command-reference.md | Accurate. Keep as a constraint note. |
| `koto cancel` syntax | AGENTS.md § "koto cancel" | koto-user references/command-reference.md | Accurate. Brief — no return shape documented; flag if engine actually returns JSON. |
| `koto workflows` syntax | AGENTS.md § "koto workflows" | koto-user references/command-reference.md | Accurate. |
| `koto template compile` syntax | AGENTS.md § "koto template compile" | koto-user references/command-reference.md | This is a template authoring command, not a runtime user command. Migrate anyway as a brief entry (agents need it for setup verification). Cross-reference koto-author for full authoring context. |
| Template setup procedure (check, mkdir, copy template) | AGENTS.md § "Template Setup" | koto-user SKILL.md | Accurate. Belongs in the runtime loop guidance section — agents need this before `koto init`. |
| Template path convention `.koto/templates/<name>.md` | AGENTS.md § "Template Setup" | koto-user SKILL.md | Accurate. Establish this as the canonical convention in SKILL.md. |
| `${CLAUDE_SKILL_DIR}/koto-templates/<name>.md` reference | AGENTS.md § "Template Setup" | koto-user SKILL.md | Accurate. Specific to koto-skills plugin distribution; note that each skill's SKILL.md specifies the template path. |
| `action` field dispatch rule — "dispatch on `action` alone" | AGENTS.md § "Response Shapes" intro | koto-user references/response-shapes.md | Accurate and important. Make this the opening rule in response-shapes.md. |
| `action: "evidence_required"` shape with all fields | AGENTS.md § "Response Shapes" | koto-user references/response-shapes.md | Accurate. Includes `expects.fields` type/required/values schema and `expects.options` routing. |
| `expects.fields` field descriptor structure (`type`, `required`, `values`) | AGENTS.md § "evidence_required" | koto-user references/response-shapes.md | Accurate. Document as a sub-schema within the `evidence_required` shape. |
| `expects.options` routing array | AGENTS.md § "evidence_required" | koto-user references/response-shapes.md | Accurate. Explains how evidence values map to target states. |
| `blocking_conditions` on `evidence_required` (gates fail + accepts present) | AGENTS.md § "evidence_required" | koto-user references/response-shapes.md | Accurate. The dual-state behaviour (gates failing but action stays `evidence_required`) is subtle — document clearly with the example. |
| `blocking_conditions` item shape (`name`, `type`, `status`, `agent_actionable`) | AGENTS.md § "evidence_required" example | koto-user references/response-shapes.md | Accurate. Document as a reusable sub-schema referenced by both `evidence_required` and `gate_blocked` shapes. |
| "When `blocking_conditions` is empty, proceed with directive" note | AGENTS.md § "evidence_required" | koto-user references/response-shapes.md | Accurate. Include as a callout — agents must not mistake an empty array for an error condition. |
| `action: "gate_blocked"` shape | AGENTS.md § "Response Shapes" | koto-user references/response-shapes.md | Accurate. `expects` is null; `blocking_conditions` is populated. |
| `status` values for blocking conditions (`failed`, `timed_out`, `error`) | AGENTS.md § "gate_blocked" | koto-user references/response-shapes.md | Accurate. List as an enumeration in the `blocking_conditions` sub-schema. |
| `action: "integration"` shape | AGENTS.md § "Response Shapes" | koto-user references/response-shapes.md | Accurate. `integration.name`, `integration.available: true`, `integration.output`. |
| `action: "integration_unavailable"` shape | AGENTS.md § "Response Shapes" | koto-user references/response-shapes.md | Accurate. `integration.available: false`; proceed manually per directive. |
| `action: "done"` shape | AGENTS.md § "Response Shapes" | koto-user references/response-shapes.md | Accurate. Terminal — no further action. |
| `action: "confirm"` shape with `action_output` fields | AGENTS.md § "Response Shapes" | koto-user references/response-shapes.md | Accurate. `action_output.command`, `.exit_code`, `.stdout`, `.stderr`. |
| `details` field — `<!-- details -->` marker explanation | AGENTS.md § "The `details` Field" | koto-user references/response-shapes.md | Accurate. Belongs as a field-level annotation in response-shapes.md alongside the `evidence_required` shape. |
| `details` field — first-visit vs. subsequent-visit omission | AGENTS.md § "The `details` Field" | koto-user references/response-shapes.md | Accurate. Subtle behaviour — document as a callout. |
| `details` absent (not null) when omitted | AGENTS.md § "The `details` Field" | koto-user references/response-shapes.md | Accurate. Important implementation note; agents must use presence check, not null check. |
| `--full` flag forces `details` inclusion | AGENTS.md § "The `details` Field" | koto-user references/command-reference.md (cross-reference from response-shapes.md) | Accurate. Already in the command reference; add a cross-reference from the `details` field note. |
| `advanced` field — meaning and semantics | AGENTS.md § "The `advanced` Field" | koto-user references/response-shapes.md | Accurate. "Informational only — dispatch on `action`, not `advanced`" is the key point. |
| `advanced` field — four illustrative examples | AGENTS.md § "The `advanced` Field" | koto-user references/response-shapes.md | Accurate. Keep the examples; they prevent a common misuse. |
| Exit code table (0/1/2/3 and associated error codes) | AGENTS.md § "Error Responses" | koto-user references/error-handling.md | Accurate. This is the primary exit-code reference. |
| Structured error object shape (`code`, `message`, `details` array) | AGENTS.md § "Error Responses" | koto-user references/error-handling.md and references/response-shapes.md | Accurate. Document the shape in response-shapes.md; document exit code semantics and agent actions in error-handling.md. |
| Error code table (all 8 codes with exit codes and meanings) | AGENTS.md § "Error Responses" | koto-user references/error-handling.md | Accurate. Complete and correct; use as the canonical error code reference. |
| `invalid_submission` example error JSON | AGENTS.md § "Error Responses" | koto-user references/error-handling.md | Accurate. Good concrete example; include in error-handling.md. |
| Execution loop overview paragraph | AGENTS.md § "Execution Loop" | koto-user SKILL.md | Accurate. The "init → get directive → act → repeat" framing belongs in SKILL.md as the runtime loop section. |
| Simple example: koto-author entry state (3-step init + evidence) | AGENTS.md § "Execution Loop" | koto-user SKILL.md | Accurate. Good minimal loop demonstration. Include or adapt in SKILL.md as the introductory example. Note: references koto-author workflow, so consider whether to keep or replace with a more generic example. |
| Advanced example: work-on workflow (steps 1–7) | AGENTS.md § "Execution Loop" | koto-user references/ (dedicated examples file or within SKILL.md) | Accurate. Demonstrates branching, gates, decisions, multi-state progression. This is the richest example in the file. Create a dedicated `references/examples/work-on-walkthrough.md` or embed as an advanced example in SKILL.md. |
| Step 3 gate-handling explanation (gate_blocked vs. evidence_required with blocking_conditions) | AGENTS.md § "Execution Loop" advanced example | koto-user SKILL.md (runtime loop guidance) | Accurate. The conditional handling of the two gate-failure patterns is important enough to appear in both SKILL.md and response-shapes.md. |
| `koto decisions record` mid-state usage and non-advancing nature | AGENTS.md § "Execution Loop" step 5 | koto-user SKILL.md | Accurate. The note that decisions don't advance the workflow is important context for the runtime loop. |
| Error handling quick-reference list (koto not found, template not found, gate blocked, etc.) | AGENTS.md § "Error Handling" | koto-user references/error-handling.md | Accurate. Useful agent-action guidance; complements the error code table. Migrate as an "agent responses" section within error-handling.md. |
| "State file already exists" / `koto cancel` recovery | AGENTS.md § "Error Handling" | koto-user references/error-handling.md | Accurate. Specific recovery procedure; include in error-handling.md. |
| Resume procedure (koto workflows → koto next → continue) | AGENTS.md § "Resume" | koto-user SKILL.md | Accurate. Belongs in SKILL.md as a resumability / session continuity section. |
| `koto rewind` as recovery from externally-resolved blocking state | AGENTS.md § "Resume" | koto-user SKILL.md | Accurate. Include alongside the resume guidance. |

---

## Phantom commands — do not migrate

Neither `koto status` nor `koto query` appears anywhere in AGENTS.md. No phantom
command content to suppress.

---

## Outdated or needs-update flags

| Item | Issue |
|---|---|
| `plugin.json` `skills` array | Lists only `koto-author`. Add `./skills/koto-user` after the skill is created. |
| `koto cancel` return shape | AGENTS.md documents no return JSON for `koto cancel`. Verify against the engine before documenting; add the shape if it exists. |
| `koto decisions list` return shape | AGENTS.md shows a bare JSON array. Verify field names match the engine's actual output (especially whether decisions carry a `state` or `timestamp` field that isn't shown). |
| Simple example references koto-author workflow | The example in § "Execution Loop" is tied to koto-author's entry state. For koto-user's SKILL.md, consider replacing with a self-contained generic example so it reads independently of koto-author. |

---

## Overlap with koto-author

| Content | Overlap type |
|---|---|
| `koto template compile` command | koto-author covers this fully; koto-user can reference koto-author rather than duplicating. |
| Template Setup section | koto-author covers template authoring and structure; koto-user's coverage is from a consumer perspective (where to put a pre-existing template), so the overlap is superficial — both can cover it from their own angle without duplication. |
| `<!-- details -->` marker explanation | koto-author documents this from the author side (how to write it); koto-user documents it from the consumer side (how to interpret the `details` field). Keep both; link between them. |

---

## Recommended destination file summary

| Destination file | Sections sourced from AGENTS.md |
|---|---|
| koto-user SKILL.md | What is koto, Prerequisites, Template Setup, Execution Loop (both examples or refs to them), Resume, mid-state decisions note, gate-failure handling guidance |
| koto-user references/command-reference.md | All command entries (init, next + all flags, decisions record/list, rewind, cancel, workflows, template compile) |
| koto-user references/response-shapes.md | All `action` shapes, `expects` sub-schema, `blocking_conditions` sub-schema, `details` field annotation, `advanced` field annotation, error object shape |
| koto-user references/error-handling.md | Exit code table, error code table with agent actions, error handling quick-reference list, state-file-exists recovery |
| koto-user references/examples/work-on-walkthrough.md (new) | Advanced work-on example (steps 1–7) — or embed in SKILL.md if a separate file is not desired |

---

## Migration confidence

All content in AGENTS.md is accurate against the current engine as of this review.
No phantom commands are present. The file is self-consistent and fully migratable.
The only items requiring pre-migration verification are the `koto cancel` return
shape and the exact field set of `koto decisions list` output.
