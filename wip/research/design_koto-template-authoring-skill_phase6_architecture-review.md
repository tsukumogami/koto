# Architecture Review: Koto Template Authoring Skill Design

## Scope

Review of `docs/designs/DESIGN-koto-template-authoring-skill.md` against the existing koto codebase and established patterns. Four questions: clarity for implementation, missing components, phase sequencing, and simpler alternatives.

---

## 1. Is the architecture clear enough to implement?

**Mostly yes, with three gaps that would block or confuse an implementer.**

### 1a. CLI command mismatch (Blocking)

The design references `koto transition <state> --evidence '...'` (line 284) as part of the koto execution loop the SKILL.md will instruct agents to run. This command was removed. The unified koto next design (`DESIGN-unified-koto-next.md`) replaced `koto transition` with `koto next --to <target>` and `koto next --with-data`. The AGENTS.md (the canonical agent-facing CLI reference) documents only `koto next` with `--to` and `--with-data` flags.

The design also shows `koto transition` in the data flow section (line 284): "koto transition <state> --evidence '...'" should be "koto next <name> --with-data '...' or koto next <name> --to <target>".

This doesn't affect the template or state machine design, but an implementer writing the SKILL.md execution loop section would produce a skill that calls a nonexistent command.

**Fix**: Replace all `koto transition` references with the current `koto next --to` / `koto next --with-data` pattern documented in AGENTS.md.

### 1b. Template directory convention divergence (Advisory)

The design prescribes `koto-templates/` as a subdirectory within the skill directory (line 256). The work-on template in shirabe follows this convention (`shirabe/koto-templates/work-on.md`). But the existing hello-koto skill -- the only koto-backed skill shipped in this repo -- uses a flat structure: the template (`hello-koto.md`) sits as a sibling to `SKILL.md` in the same directory.

The custom skill authoring guide (`docs/guides/custom-skill-authoring.md`) also describes the flat pattern: "Both files live in the same directory" (line 9).

The design says the `koto-templates/` convention is "established by existing skills like work-on" but hello-koto contradicts it. An implementer will see two conventions in the codebase: one from the only existing in-repo skill (flat), one from the design and the work-on template design (subdirectory).

**Fix**: Either update hello-koto to use `koto-templates/` (establishing one convention), or acknowledge the flat pattern as valid for simple skills and clarify when each pattern applies. The design's integration check phase (Phase 8) enforces `koto-templates/`, so this needs to be settled before implementation.

### 1c. Self-loop mechanics underspecified

The compile validation state (line 340) specifies a self-loop with "max 3 attempts," but the design doesn't explain how this limit is enforced. Koto templates don't have a built-in retry counter mechanism. Options include:

- A template variable tracking attempt count (the template would need to declare it)
- Agent-side counting in the directive prose (unreliable -- the kind of enforcement gap the design itself identifies with prose-only skills)
- A gate condition that counts evidence submissions (not a current engine capability)

The 8-state table (lines 335-343) shows the self-loop transition but no gate or mechanism for the max-3 limit. An implementer would need to decide how to enforce this, and the answer affects template structure.

**Fix**: Specify whether the 3-attempt limit is enforced mechanically (and if so, how) or is a directive-level instruction to the agent (and if so, acknowledge this is a prose enforcement gap).

---

## 2. Are there missing components or interfaces?

### 2a. Eval case missing from Phase 3 (Advisory)

The implementation phases list plugin.json registration in Phase 3 but don't mention creating an eval case. The custom skill authoring guide (`docs/guides/custom-skill-authoring.md`, lines 443-488) describes eval cases as part of the skill authoring process, and the existing hello-koto skill has one at `plugins/koto-skills/evals/hello-koto/`. The validate-plugins CI workflow runs these on PRs touching `plugins/`.

Without an eval case, the skill would ship without behavioral regression coverage. This is consistent with the guide's process but inconsistent with the design's Phase 4 (which tests end-to-end but through manual invocation, not repeatable CI).

**Fix**: Add eval case creation to Phase 3 or Phase 4 deliverables.

### 2b. MODE variable declaration missing from state table

The design says MODE is tracked as a template variable (line 103), and the Key Interfaces section lists `MODE: "new" or "convert"` (line 269). But the state details table (lines 335-343) doesn't show the entry state capturing MODE as evidence or the template declaring it as a variable.

Looking at the hello-koto template as a reference: variables are declared in YAML frontmatter and supplied via `--var` on `koto init`. But the design's data flow (line 305-307) shows the agent selecting mode during the entry state and submitting it as evidence, which is the evidence routing pattern (accepts/when), not the variable pattern.

These are two different mechanisms. If MODE routes the agent to different behavior within the same state sequence (which is what the design describes), it should be an evidence field with mode-conditional directive prose, not a template variable. If it's a template variable, it needs to be supplied at init time, before the workflow starts.

**Fix**: Clarify whether MODE is a `--var` variable (set at init time, available in all directives via `{{MODE}}`) or evidence submitted at the entry state (captured via accepts/when). This affects how the template's YAML frontmatter is structured.

### 2c. No reference to custom skill authoring guide

The design creates a new `references/template-format.md` condensed authoring guide. A separate `docs/guides/custom-skill-authoring.md` already exists and covers the full SKILL.md authoring process (7 sections, eval setup, deployment options). The design's reference material focuses on the template format, while the existing guide focuses on the SKILL.md structure.

These are complementary, not duplicative. But the design doesn't mention the existing guide or how the two relate. During the SKILL.md authoring phase (Phase 7), agents would benefit from referencing the existing guide's structure rather than reinventing SKILL.md conventions.

**Fix**: Reference `docs/guides/custom-skill-authoring.md` in the SKILL.md authoring phase directive, or note it as supplementary material in the references.

---

## 3. Are the implementation phases correctly sequenced?

**Yes, the sequencing is correct.** Each phase depends only on prior phases:

- Phase 1 (reference material) is standalone
- Phase 2 (koto template) uses Phase 1 output to inform directives
- Phase 3 (SKILL.md + plugin registration) wraps Phase 2's template
- Phase 4 (end-to-end test) exercises everything

No circular dependencies, no phase that could start earlier by reordering. The one sequencing concern is minor: Phase 4 should include eval case creation (see 2a above), but that's additive, not a reordering.

---

## 4. Are there simpler alternatives we overlooked?

### 4a. Reference material scope

The design proposes three graded example templates at increasing complexity (linear, evidence routing, complex). The hello-koto template already exists as the simple example, and the work-on template (17 states) exists as the complex example. Writing two new example templates (simple + complex) from scratch duplicates existing artifacts.

**Alternative**: Use hello-koto as the simple example and a curated excerpt of work-on as the complex example. Write only the medium-complexity evidence routing example, which doesn't exist yet. This cuts Phase 1 work by roughly half.

**Tradeoff**: hello-koto doesn't use the `koto-templates/` convention the design prescribes (see 1b), so it may confuse agents about directory structure. And work-on hasn't been built yet (it's in the shirabe design). If work-on isn't available at implementation time, all three examples would need to be authored.

### 4b. Self-hosting trade-off

The design chooses koto-backed (self-hosted) over prose-only, arguing that the skill should practice what it preaches. This is sound in principle but creates a bootstrapping cost: the 8-state template, its mode-conditional directives, the compile validation self-loop, and the integration check gate all need to be hand-written correctly without the benefit of the skill they're building.

For a skill whose primary near-term use is 7 conversions (shirabe PRD), the bootstrapping cost is paid once and amortized. The living-example benefit is real -- agents using the skill can inspect its own template.

**No simpler alternative recommended.** The prose-only option would work but misses the self-referential value that makes this skill distinctive from the existing skill-creator.

### 4c. Unified vs forked workflow

The design's choice of unified linear workflow with mode-conditional steps (8 states) over forked entry (14+ states) is the right call. The mode differences are in directive content, not state topology. This is validated by the work-on template design, which handles three input modes (issue-backed, free-form, PLAN doc) through a single state graph with evidence routing.

**No simpler alternative recommended.**

---

## Summary of findings

| # | Finding | Severity | Action |
|---|---------|----------|--------|
| 1a | `koto transition` references should be `koto next --to`/`--with-data` | Blocking | Update design lines 284 |
| 1b | `koto-templates/` convention contradicts hello-koto's flat structure | Advisory | Settle one convention before implementation |
| 1c | Self-loop max-3 enforcement mechanism unspecified | Advisory | Specify mechanical or prose enforcement |
| 2a | Eval case missing from implementation phases | Advisory | Add to Phase 3 or 4 |
| 2b | MODE as variable vs evidence not disambiguated | Advisory | Clarify in template state details |
| 2c | Existing custom skill authoring guide not referenced | Advisory | Add as supplementary reference |
| 4a | Two of three example templates may duplicate existing/planned artifacts | Advisory | Consider reuse of hello-koto and work-on |
