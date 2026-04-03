# Phase 2 Research: Current-State Analyst

## Lead A: AGENTS.md content distribution

### Findings

`plugins/koto-skills/AGENTS.md` is a comprehensive, self-contained agent reference for running koto-backed workflows. It is 550 lines. Its content divides into these categories:

**1. High-level orientation (10–15 lines)**
- What koto is (state machine engine, evidence-gated transitions)
- Prerequisites (koto installed, on PATH, `koto version` check, install URL)

These 2 items are appropriate for root AGENTS.md — any agent working in the repo needs to know koto exists and how to verify it's available.

**2. Command reference with full syntax (lines 20–130)**
- `koto init` with full flags and JSON output
- `koto next` with all flags (`--with-data`, `--to`, `--full`) and mutual exclusivity rules
- `koto decisions record` and `koto decisions list` with full JSON schemas
- `koto rewind`, `koto cancel`, `koto workflows`, `koto template compile`

This is detail-level content. It belongs in koto-user's `references/` directory, not root AGENTS.md. Root AGENTS.md should name the commands and link to the reference, not reproduce each command's flags.

**3. Template setup instructions (lines 135–150)**
- How to find and copy templates to `.koto/templates/`
- Reference to `${CLAUDE_SKILL_DIR}/koto-templates/<name>.md`

This is koto-skills-specific context (it refers to the plugin's directory structure). It belongs in the koto-user skill's SKILL.md or a references file — not in root AGENTS.md, which serves the whole repo.

**4. Response shapes (lines 153–310)**
- Full JSON examples for all 6 action values: `evidence_required`, `gate_blocked`, `integration`, `integration_unavailable`, `confirm`, `done`
- Detailed explanations of `expects`, `blocking_conditions`, `advanced`, `details` fields
- The `--full` flag behavior for `details`

This is the core koto-user runtime content. It belongs in a dedicated `references/response-shapes.md` (or similar) file in the koto-user skill. Too large for root AGENTS.md.

**5. Error responses (lines 313–380)**
- Exit code table (exit 0/1/2/3 with associated error codes)
- Per-error-code table with agent actions
- JSON error shape example

Belongs in koto-user `references/error-handling.md`. Root AGENTS.md might reference exit code categories in 1–2 lines ("exit 1 = retry, exit 2 = fix your call, exit 3 = report to user") with a link.

**6. Execution loop examples (lines 383–550)**
- Simple example: koto-author entry state (init + get directive + submit evidence)
- Advanced example: work-on workflow (branching, gates, decisions, completion)
- Error handling (per-scenario guidance for common errors)
- Resume instructions (koto workflows + koto next after interruption)

Belongs in koto-user skill — these are the examples section of the skill. The resume snippet (2–3 lines: `koto workflows` then `koto next <name>`) could appear in root AGENTS.md as a quick-recovery note.

**Disposition summary:**

| Content block | Root AGENTS.md | koto-user references/ | koto-user SKILL.md |
|---------------|:--------------:|:---------------------:|:------------------:|
| What koto is (2 sentences) | yes | -- | -- |
| Prerequisites + install check | yes | -- | -- |
| Command syntax (all flags) | no | yes | -- |
| Template setup (skills-specific) | no | -- | yes |
| Response shapes (all 6 actions) | no | yes | -- |
| Error exit codes (full table) | no | yes | -- |
| Exit code summary (1 line) | yes | -- | -- |
| Execution loop examples | no | -- | yes (or examples/) |
| Resume instructions | brief note | yes (full) | -- |

**Note on the `koto decisions` commands**: AGENTS.md uses `koto decisions record/list`. The current CLAUDE.local.md quick reference table calls these `koto overrides record/list`. The PRD scope document also references "koto overrides CLI". This is a naming discrepancy — AGENTS.md may be ahead of or behind the CLI. This needs verification against `src/cli/mod.rs` before writing koto-user requirements.

**Note on missing commands**: AGENTS.md does not cover `koto session`, `koto context`, or `koto config` commands that appear in `docs/guides/cli-usage.md`. These are substantive additions since AGENTS.md was written. The koto-user skill will need to address at least `koto session dir` (skills use it to locate artifacts) and `koto context add/get` (the evidence store).

**Note on cursor rules**: `plugins/koto-skills/.cursor/rules/koto.mdc` is a condensed version of AGENTS.md (~200 lines). It covers the same core content (execution loop, response shapes, error handling, resume) but omits the extended examples. It was authored as a fallback for Cursor IDEs that don't support the Agent Skills standard. Its content has the same distribution as AGENTS.md — all detail belongs in koto-user references, none of it in root AGENTS.md verbatim.

### Implications for Requirements

1. **Root AGENTS.md should be short**: the full AGENTS.md content is 550 lines. Root AGENTS.md should be roughly 30–50 lines — orientation, prerequisites, 1-line exit code summary, resume tip, and pointers to the koto-user skill and docs.

2. **koto-user needs at least 3 references/ files**:
   - `references/command-reference.md` — full command syntax for init, next, decisions/overrides, rewind, cancel, workflows, session, context
   - `references/response-shapes.md` — all 6 action values with full JSON examples
   - `references/error-handling.md` — exit codes, error code table, per-scenario guidance

3. **AGENTS.md at plugin root should be deleted or replaced with a pointer**: its content duplicates what koto-user will provide via proper skill references. Leaving both creates a maintenance drift problem.

4. **The `koto decisions` vs `koto overrides` naming discrepancy must be resolved before koto-user requirements are written**: the command name affects every reference in the skill.

### Open Questions

- Is the command `koto decisions record/list` or `koto overrides record/list`? AGENTS.md says `decisions`, CLAUDE.local.md says `overrides`. Needs source verification.
- Should koto-user reference `koto session dir` as a skill-internal detail, or should it be surfaced as something workflow users call directly?
- Does AGENTS.md at plugin root get deleted entirely, or replaced with a 2-line stub pointing to the koto-user skill? (PRD scope says "remove or repurpose".)

---

## Lead B: Root AGENTS.md design

### Findings

**No root AGENTS.md exists** at `/home/dgazineu/dev/niwaw/tsuku/tsukumogami-2/public/koto/AGENTS.md`. The directory listing confirms: `build.rs`, `Cargo.lock`, `Cargo.toml`, `CLAUDE.local.md`, `docs/`, `install.sh`, `LICENSE`, `plugins/`, `README.md`, `scripts/`, `src/`, `target/`, `test/`, `tests/`, `wip/`. No AGENTS.md.

**README.md has an "Agent integration" section** (lines 112–134) that describes the Claude Code plugin, mentions the Agent Skills standard, and explains the basic init/next/submit loop in 5 bullet points. This is the closest thing to agent-oriented orientation content at the repo root.

**CLAUDE.local.md** is the existing agent context file, but it's a Claude Code-specific file (not loaded by Codex, Windsurf, or other platforms). It contains developer-facing shortcuts (build commands, test commands, key concepts) — not a runtime agent reference.

**docs/guides/cli-usage.md** is a complete 714-line CLI reference covering all commands including several not in AGENTS.md (`koto session`, `koto context`, `koto config`, `koto template export`, `koto template validate`). This is the canonical CLI reference but is too large for AGENTS.md.

**What the root AGENTS.md should contain** (based on the user's guidance of "high-level orientation, not full skill content"):

1. **What koto is** (2–3 sentences): state machine engine, evidence-gated transitions, `koto next` is the primary command.

2. **Prerequisites** (3–4 lines): install check, install URL, PATH requirement.

3. **Core concept** (5–7 lines): the loop (init → next → dispatch on action → submit evidence → repeat). No full JSON examples — just the concept and the action values as a brief table or list.

4. **Exit codes** (1 line + table): exit 0 success, exit 1 retry, exit 2 fix your call, exit 3 report to user.

5. **Resume** (2–3 lines): `koto workflows` + `koto next <name>` to resume after interruption.

6. **Pointers** (3–5 lines): link to koto-user skill for the full runtime reference, link to `docs/guides/cli-usage.md` for command reference, link to README for template authoring concepts.

**Right length**: 40–60 lines. The AGENTS.md file is loaded at session start as context — it needs to be short enough that it doesn't exhaust context budget on every session. The full AGENTS.md at 550 lines is too large; the root version should be a fraction of that.

**What root AGENTS.md should NOT contain**:
- Full JSON response examples (that's koto-user `references/response-shapes.md`)
- Per-command flag documentation (that's `references/command-reference.md`)
- Template authoring guidance (that's koto-author)
- Error code per-scenario handling (that's `references/error-handling.md`)

### Implications for Requirements

1. **Root AGENTS.md is a new file to create** — it doesn't exist yet. Requirements must specify its content precisely to avoid it becoming another AGENTS.md clone.

2. **The README's "Agent integration" section is a model**: it's 15 lines and covers the install + loop concept without JSON examples. Root AGENTS.md can be structured similarly but oriented toward runtime behavior rather than plugin setup.

3. **Content boundary is clear**: root AGENTS.md = orientation + concept + pointers. Full detail = koto-user skill references. This boundary should be stated explicitly in the requirements so the file doesn't grow over time.

4. **Platform targeting**: AGENTS.md is read by Codex, Windsurf, and other platforms. Content should not reference Claude-specific concepts (no mention of skills or `/skill` commands — those go in CLAUDE.local.md). It should be platform-neutral.

5. **CLAUDE.local.md stays**: it covers developer workflow (build, test, lint) and is Claude Code-specific. Root AGENTS.md covers agent runtime and is platform-neutral. These are complementary.

### Open Questions

- Should root AGENTS.md mention the koto-user skill by name/path, or just link to the CLI usage guide? (Skill paths are Claude Code-specific; Codex won't know what to do with `plugins/koto-skills/skills/koto-user/SKILL.md`.)
- Should root AGENTS.md include the action dispatch table (6 rows, 2 columns) as a quick reference, or is even that too much detail for a root orientation file?
- Does root AGENTS.md replace CLAUDE.local.md for Claude Code sessions, or do both files get loaded? (Claude Code loads both — AGENTS.md is platform-neutral, CLAUDE.local.md is Claude-specific augmentation.)

---

## Summary

`plugins/koto-skills/AGENTS.md` is a complete 550-line koto-user reference that belongs in the koto-user skill's `references/` directory, not at the plugin root — its full content maps cleanly to 3 reference files (command-reference, response-shapes, error-handling). No root `AGENTS.md` exists at the koto repo root; the README's 15-line "Agent integration" section is the closest precedent and suggests the right length and level of detail (orientation + concept + pointers, roughly 40–60 lines). One blocking discrepancy must be resolved before koto-user requirements are written: `koto decisions` (in AGENTS.md) vs. `koto overrides` (in CLAUDE.local.md) — these appear to be the same command group with different names.
