# Agentic Systems Review (Round 2): Cross-Agent Delegation Design

Reviewer role: Agentic systems specialist (tool-use interfaces for AI coding agents).

Reviewed: `docs/designs/DESIGN-cross-agent-delegation.md` (post-round-1 revision)

Reference: `plugins/koto-skills/skills/hello-koto/SKILL.md`, `docs/guides/custom-skill-authoring.md`, `pkg/controller/controller.go`, `pkg/template/compiled.go`, `pkg/template/template.go`

## Round 1 Findings That Were Addressed

The design incorporated these changes from the first review:

1. **Tag vocabulary is now capability-oriented.** `security` was renamed to `specialized-tooling`. The vocabulary table now explicitly describes processing capabilities, not domains. The clarifying sentence "Tags describe processing capabilities, not domains" is present.

2. **Prompt construction guidance appears in the template example.** The `## deep-analysis` section now includes a "When delegating this step, include:" block listing what context to gather.

3. **Single-exchange limitations are documented.** The scope section now includes a three-tier classification (works well / adequately / poorly) with examples.

4. **Prompt file lifecycle is documented.** The design states koto reads but does not delete the prompt file, and recommends `mktemp` in SKILL.md instructions.

5. **Delegate interface contract is defined.** A table specifying input (stdin), output (stdout), working directory, environment, filesystem access (read-write), permissions, and interaction model.

## Focus Area 1: Remaining Agentic Integration Gaps

### 1.1 Delegate response and state progression -- the handoff gap

The round-1 review flagged that there's no mechanism to store the delegate response as evidence for subsequent states. The design still says "the orchestrating agent decides what to do with the response" (out-of-scope item: "Delegate response persistence"). This is acceptable for v1, but the design should acknowledge one practical consequence it doesn't currently mention.

**The problem:** When a delegate runs read-write, there are now two potential outputs -- the stdout response text AND any files the delegate wrote to disk. The orchestrating agent has the stdout response (via `koto delegate run` JSON), but it may not know what files the delegate created or modified. Consider a delegate that's asked to analyze code and writes its findings to `analysis-report.md`. The orchestrating agent has the stdout response but may not know about the file unless the delegate mentions it in stdout.

**Assessment: This is acceptable for v1.** The directive text can instruct the delegate to report file modifications in its stdout response. And the orchestrating agent can check filesystem state after delegation returns. But this should be documented as a known consequence of read-write delegation in the single-exchange scope guide.

**Recommendation:** Add a sentence to the single-exchange scope guide noting that when the delegate writes files, the directive should instruct the delegate to report what it wrote in its stdout response. This keeps the orchestrating agent aware of filesystem changes without adding a file-tracking mechanism to koto.

### 1.2 Delegate stderr handling

The design captures stdout and returns it in the response JSON. It does not mention stderr. Coding agent CLIs use stderr for progress indicators, logging, and diagnostic output. `claude -p` writes status information to stderr. `gemini` may do the same.

Current behavior (implied by the `invokeDelegate` implementation): `cmd.Stderr` is not set, so stderr goes to the parent process's stderr. This means delegate diagnostic output appears in the user's terminal interleaved with koto's own output.

**Assessment: This is fine for v1** -- it matches normal subprocess behavior and lets users see delegate progress. But the design should state this explicitly in the Delegate Interface Contract table. Add a row:

| Aspect | Contract |
|--------|----------|
| **Stderr** | Inherited from koto's process; delegate diagnostic output appears in the user's terminal |

This prevents a future implementer from accidentally capturing and discarding stderr.

### 1.3 The agent's decision tree is still implicit

The first review recommended a "Delegation" section in SKILL.md. The design incorporated prompt construction guidance into the directive text (good), but it didn't update the SKILL.md standard or the custom skill authoring guide to show how a delegation-aware SKILL.md differs from a non-delegation one.

Walking through the agent's decision tree at step 3 of the execution section:

```
koto next -> parse JSON response
  -> if delegation is nil: execute directive directly (existing flow)
  -> if delegation.available is true:
       -> read directive text
       -> gather context (which files? how much?)
       -> write prompt file
       -> koto delegate run --prompt /tmp/prompt.txt
       -> parse delegate response JSON
       -> use response (how? copy to a file? keep in context? both?)
       -> koto transition <next-state>
  -> if delegation.fallback is true:
       -> execute directive with reduced scope (what's reduced?)
       -> koto transition <next-state>
```

None of this branching logic appears in any existing documentation. The hello-koto SKILL.md (which doesn't use delegation) has a linear execution section. The custom skill authoring guide describes seven sections, none of which cover delegation. A skill author writing their first delegation-aware skill has no template to follow.

**Recommendation:** The design's File Change Summary lists "Update SKILL.md with delegation flow documentation" for Phase 5. Make this more concrete by specifying what the updated SKILL.md should contain. At minimum, the design should include a skeleton of a delegation-aware execution section that skill authors can adapt. For example:

```markdown
### 3. Execute the directive (delegation path)

After calling `koto next`, check the `delegation` field in the response.

**If `delegation.available` is true:**

1. Read the directive text. It contains guidance on what context to include.
2. Gather the specified context (files, logs, etc.)
3. Write the delegation prompt:
   ```bash
   prompt_file=$(mktemp)
   # Write gathered context and task description to $prompt_file
   ```
4. Run: `koto delegate run --prompt "$prompt_file"`
5. Parse the response JSON. Check `success` field.
6. [State-specific: what to do with the response]

**If `delegation` is absent or `delegation.fallback` is true:**

Execute the directive directly as in a non-delegated step.
```

This skeleton should appear either in the design document's example section or in the custom skill authoring guide's planned updates.

### 1.4 Prompt file content: who writes it, and with what tools?

The design says the agent writes a prompt to a file, then passes `--prompt /path`. But which agent writes this file? The orchestrating agent -- a coding agent like Claude Code.

Coding agents write files using tool calls (Write tool, editor, shell commands). Writing a multi-kilobyte prompt that includes raw file contents to a temp file is straightforward for agents with file-writing tools. But the SKILL.md instructions need to tell the agent *how* to compose the prompt content. The directive text's "When delegating this step, include:" guidance is the right approach. However, the guidance in the example is a bullet list:

```
When delegating this step, include in the prompt:
- All source files in the affected packages
- The go.mod file for dependency context
- Any test files for the affected packages
- A clear statement of what analysis is expected
```

This tells the agent *what* to include but not *how* to structure it. Should the agent concatenate file contents with filename headers? Should it wrap each file in a code fence? Should it add a preamble explaining the task? Different agents will format the prompt differently, producing variable delegation quality.

**Assessment: This is adequate for v1.** Agents are reasonably good at formatting context when given clear "include X, Y, Z" instructions. The alternative -- a structured prompt template in the template format -- was explicitly deferred to v2 in the round-1 review. The current approach puts the burden on agent intelligence rather than template structure, which is a reasonable v1 trade-off.

**Minor recommendation:** In the example directive, add one sentence about output format expectations. The delegate has no context about what format is useful. The directive should say something like "Return your analysis as structured markdown with sections for each finding." This gives the delegate a format contract that the orchestrating agent can rely on when consuming the response.

## Focus Area 2: Prompt Construction Guidance Depth

### 2.1 Is the in-directive guidance sufficient?

The design added prompt construction guidance to the example directive:

```markdown
## deep-analysis

Analyze the codebase for: {{TASK}}

Think carefully about what changes are needed and why.

When delegating this step, include in the prompt:
- All source files in the affected packages
- The go.mod file for dependency context
- Any test files for the affected packages
- A clear statement of what analysis is expected
```

**Assessment: This is the right pattern, but it has a structural ambiguity.** The directive text serves double duty: it's the instruction for the orchestrating agent when executing directly (no delegation), AND it contains delegation prompt guidance when delegation is active. The agent must parse the directive to separate "what to do" from "what to include in the delegation prompt."

The "When delegating this step" prefix is a reasonable signal. But consider: the orchestrating agent reads this entire directive. In non-delegation mode, the "When delegating this step, include in the prompt:" section is noise that the agent must ignore. In delegation mode, the "Analyze the codebase for: {{TASK}}" part is the task description to include in the prompt, not an instruction to execute.

This dual-use is workable because agents handle conditional instructions well enough. But template authors should understand the pattern: the first part is the task description (used by both the self-executing agent and as content for the delegation prompt), and the "When delegating" block is delegation-only guidance.

**Recommendation:** Document this dual-use pattern explicitly. In the design's example or in the authoring guide, explain:

- The directive text before "When delegating" serves as both the self-execution instruction and the task description for the delegation prompt.
- The "When delegating" block is only relevant when `delegation.available` is true.
- Template authors should write the first part as a task description that works in both modes.

### 2.2 What about multi-tag states?

A state could have `tags: [deep-reasoning, large-context]`. Only one rule matches (first-match). But the directive text's delegation guidance might be written assuming a specific delegate. If the "When delegating" guidance says "include the full codebase" (appropriate for a `large-context` delegate with a 1M token window), but first-match routes to a `deep-reasoning` delegate with a 200K window, the prompt will be too large.

**Assessment: This is an edge case for v1.** The design already says "each state should have exactly one tag representing the primary routing concern" (implicit in the vocabulary table and first-match semantics). But the schema allows multiple tags, and the first-match behavior creates a coupling between directive guidance and config ordering that template authors may not anticipate.

**Recommendation:** Add a note in the Decision 2 section or the vocabulary table: "When a state has multiple tags, the delegation prompt guidance in the directive should be written for the weakest delegate capability. This ensures the prompt works regardless of which tag matches first." Alternatively, document that multi-tag states are a configuration-dependent pattern and template authors should target the most likely routing outcome.

## Focus Area 3: Read-Write Delegate Access

### 3.1 Impact on prompt crafting

The delegate running read-write fundamentally changes the prompt crafting story, but the design doesn't fully surface this.

With a read-only delegate (the original assumption from early in the design process), the prompt must be completely self-contained -- all source code, all context, everything the delegate needs, crammed into stdin. The orchestrating agent bears the entire context-gathering burden.

With a read-write delegate, this burden is dramatically reduced. Instead of serializing 50 source files into the prompt, the orchestrating agent can write:

```
Analyze the Go packages in pkg/engine/ and pkg/controller/ for architectural issues.
The codebase is in the current working directory.
Read the relevant files, perform your analysis, and report findings.
```

The delegate can read the files itself. This changes the prompt from "here are all the files, analyze them" to "here is where to look, go analyze." The prompt becomes a task description rather than a context dump.

**Assessment: The design correctly documents the read-write contract, but the example directive still follows a "context dump" pattern.** The "When delegating this step, include in the prompt: All source files in the affected packages" guidance assumes the orchestrating agent must gather and include files in the prompt. With a read-write delegate, this is unnecessary -- the delegate can read the files from disk.

**Recommendation:** Update the example directive to reflect the read-write reality:

```markdown
When delegating this step:
- Direct the delegate to read source files in the affected packages
- Include the go.mod path for dependency context
- Specify the analysis output format expected
- The delegate has full read-write access to the working directory
```

The key shift: instead of "include X in the prompt," it becomes "direct the delegate to read X." This produces smaller prompts, avoids context window pressure, and uses the delegate's own file-reading capabilities.

### 3.2 Impact on response handling

With a read-write delegate, the response is no longer the only output. The delegate might:

- Write analysis results to a file
- Create or modify source code files
- Generate reports in the working directory

The `koto delegate run` response JSON captures stdout, but filesystem modifications are invisible to the response structure. The orchestrating agent receives `{response: "...", success: true}` but doesn't know if the delegate also wrote `fix.patch` to disk.

This is already noted in section 1.1 above, but the response handling angle deserves emphasis: the `success: true` in the response tells the agent the delegate process completed, but doesn't tell it what the delegate *did* to the filesystem. The agent must either:

1. Trust that the delegate reported its actions in stdout (fragile)
2. Check filesystem state after delegation (reliable but the agent needs to know what to check)
3. The directive tells the delegate to limit itself to stdout-only output (simplest for v1)

**Recommendation:** Add guidance on this three-way choice. For v1, recommend option 3 as the default: directives for delegated states should tell the delegate to report all findings via stdout and not write files unless explicitly instructed. This keeps the response as the single output channel and avoids the "what did the delegate change?" problem. Template authors who want file-writing delegates can opt in by including explicit file-writing instructions in the directive, along with instructions for the delegate to report what it wrote.

### 3.3 Security implication of read-write access with project config

The design correctly separates targets (user-only) from rules (project-allowed with opt-in). But consider this scenario:

1. A project ships `.koto/config.yaml` with `rules: [{tag: specialized-tooling, target: gemini}]`
2. User has `allow_project_config: true` and `targets: {gemini: {command: ["gemini", "-p"]}}`
3. A template in the project has a state tagged `specialized-tooling`
4. The directive text says "Write the analysis results to `results.json`"
5. The delegate (gemini) runs read-write, reads the codebase, and writes to `results.json`

This is all working as designed. The security concern is that the project controls both the tag mapping (via project config rules) AND the directive text (via the template). The project can't choose which binary runs (targets are user-only), but it controls what instructions the delegate receives and what files it's told to write. A malicious template could instruct the delegate to write to sensitive locations, overwrite source files, or exfiltrate data via the delegate's network access.

**Assessment: This is already covered by the existing security analysis.** The design's "Directive text is agent-visible" security note in the authoring guide applies equally to delegation directives. The delegate is just another agent following instructions. The mitigation (code review of templates) is correct.

No new recommendation -- just confirming the existing security model covers this case.

## Focus Area 4: Tag Vocabulary

### 4.1 Is `specialized-tooling` the right name?

The rename from `security` to `specialized-tooling` fixes the domain-vs-capability confusion. But `specialized-tooling` is awkward as a tag name. Tags should be self-describing. `deep-reasoning` clearly means "this step needs deep reasoning." `large-context` clearly means "this step needs a large context." `specialized-tooling` is less clear -- specialized for what? Every tool is specialized for something.

The intent is "this step benefits from a tool with domain-specific capabilities that a general-purpose LLM doesn't have." Examples from the table: static analysis, dependency scanning, specialized linting. These are all cases where a non-LLM tool (or an LLM augmented with specific tools) would outperform a vanilla model.

Alternative names to consider:

- `tool-augmented` -- clearer that the delegate has additional tools beyond raw reasoning
- `domain-tooling` -- emphasizes domain-specific tool access
- `external-tooling` -- emphasizes that the capability comes from external tools

**Assessment: `specialized-tooling` is acceptable for v1.** The vocabulary is documented, not enforced. If a better name emerges from usage, the pattern-validation approach means existing templates using `specialized-tooling` keep working while new templates can use a revised tag. The documentation can note both.

**Minor recommendation:** If the name stays, add a one-sentence clarification in the vocabulary table's "Meaning" column: "Step benefits from domain-specific tools or capabilities beyond general-purpose language model reasoning." The current wording ("domain-specific tools or capabilities") could be read as "tools specific to this domain" (vague) rather than "tools that go beyond what a general model provides" (precise).

### 4.2 Are three tags enough for v1?

The three tags cover the main axes of model differentiation: reasoning depth, context capacity, and tool access. Two additional capabilities that matter for coding agents aren't covered:

1. **Speed / cost sensitivity.** Some workflow steps are simple and don't need a frontier model. A draft step or boilerplate generation step might benefit from routing to a faster, cheaper model. A `fast-generation` or `high-throughput` tag would express this.

2. **Code execution / sandbox.** Some steps need the delegate to run code (tests, build commands, scripts). This differs from `specialized-tooling` because it's about runtime execution, not analysis tooling. A `code-execution` tag would express this.

**Assessment: Three tags are sufficient for v1.** The pattern-validated approach means users can define custom tags (`fast-generation`, `code-execution`) in their own templates and config right now. Adding more official tags should be driven by observed usage patterns, not speculation.

**No recommendation.** The extensibility story is correct as designed.

## Focus Area 5: Skill Documentation

### 5.1 The custom skill authoring guide has no delegation content

The authoring guide at `docs/guides/custom-skill-authoring.md` documents seven SKILL.md sections: Prerequisites, Template Setup, Execution, Evidence Keys, Response Schemas, Error Handling, Resume. None of these mention delegation. The guide's worked example (hello-koto) is a non-delegation workflow.

The design's Phase 5 says "Update SKILL.md with delegation flow documentation" and "Write `docs/guides/delegation.md` user guide." These are the right deliverables. But the custom skill authoring guide itself needs updating -- skill authors writing delegation-aware skills will go to the authoring guide first, not to a separate delegation guide.

**Recommendation:** The design should add the authoring guide to the File Change Summary:

| File | Change |
|------|--------|
| `docs/guides/custom-skill-authoring.md` | Add delegation section: how to write SKILL.md for delegation-aware workflows |

The content should cover:

1. **When to tag states.** A heuristic: tag a state when the task is well-defined enough for a single-exchange prompt but the processing benefits from capabilities the orchestrating agent lacks.

2. **Writing delegation-compatible directives.** The dual-use pattern: task description + "When delegating" block. How to write directives that work in both self-execution and delegation modes.

3. **The delegation execution flow.** The branching logic (delegation available, fallback, absent) as a concrete command sequence.

4. **Response consumption.** How to document what the agent should do with the delegate's response.

5. **Testing delegation-aware skills.** How to eval a skill that has delegation states. The existing eval harness checks for `koto init` and `koto next` commands. Delegation-aware evals should also check for `koto delegate run`.

### 5.2 The hello-koto SKILL.md as reference

The hello-koto SKILL.md is the reference implementation for skill authors. It doesn't use delegation, which is appropriate (not every skill needs delegation). But once delegation exists, authors need a second reference implementation -- a delegation-aware skill that demonstrates the full flow.

The design's example template (`research-and-implement`) fills this role for the template side. But there's no example SKILL.md for `research-and-implement`. The Phase 5 plan says "Ship the research-and-implement reference template" but doesn't mention a companion SKILL.md.

**Recommendation:** Add a research-and-implement SKILL.md to Phase 5 deliverables. This becomes the reference implementation for delegation-aware skills, just as hello-koto is the reference for basic skills.

### 5.3 Template setup instructions for tagged templates

The hello-koto SKILL.md instructs the agent to copy the template to `.koto/templates/hello-koto.md`. For delegation-aware templates, the same copy pattern works. But there's a new consideration: the agent must also ensure the user has delegation config.

Should the SKILL.md include instructions for checking delegation config? Something like:

```markdown
## Prerequisites

- `koto` must be installed and on PATH
- For delegation support, configure `~/.koto/config.yaml` with delegation targets.
  Without config, tagged states run without delegation (the agent handles them directly).
```

**Assessment: This is optional guidance.** The design already specifies graceful degradation -- no config means no delegation, and the workflow still works. But a SKILL.md that says nothing about delegation config will confuse users who expect delegation to "just work."

**Recommendation:** Include a brief note in the Prerequisites section of delegation-aware SKILL.md files pointing to the delegation guide for config setup. Don't make it a hard prerequisite.

## Focus Area 6: End-to-End Flow Walkthrough

Walking through the full flow as a coding agent (Claude Code) receiving a delegation-aware directive.

### Step 1: `koto next`

Agent runs `koto next` and receives:

```json
{
  "action": "execute",
  "state": "deep-analysis",
  "directive": "Analyze the codebase for: refactor authentication module\n\nThink carefully about what changes are needed and why.\n\nWhen delegating this step, include in the prompt:\n- All source files in the affected packages\n- The go.mod file for dependency context\n- Any test files for the affected packages\n- A clear statement of what analysis is expected",
  "tags": ["deep-reasoning"],
  "delegation": {
    "target": "gemini",
    "matched_tag": "deep-reasoning",
    "available": true
  }
}
```

**Gap: How does the agent know it should check `delegation`?** The agent's SKILL.md instructions must tell it to check this field. Without explicit SKILL.md guidance, the agent sees `action: execute` and proceeds to execute the directive directly, ignoring delegation entirely. The `delegation` field is inert metadata unless the SKILL.md teaches the agent to act on it.

This is the most critical gap in the current design. The flow assumes the agent knows to check `delegation`, but nothing in the existing SKILL.md standard or authoring guide teaches this pattern.

**Recommendation:** This reinforces section 1.3 above. The SKILL.md must include explicit delegation flow instructions. Without them, delegation metadata is dead data.

### Step 2: Agent reads directive, recognizes delegation

Assuming the SKILL.md tells the agent to check `delegation`, the agent now needs to:

1. Parse the directive text to separate task description from delegation guidance
2. Identify the "When delegating this step" block
3. Follow the gathering instructions

**Gap: No structured way to separate these.** The agent must parse natural language to find the delegation guidance section. This is fine for frontier models (Claude, Gemini) but less reliable for smaller models. A structured delimiter (like a markdown heading `### Delegation Prompt Guidance` within the directive) would be more reliably parseable.

**Assessment: Acceptable for v1.** The agents that run koto workflows are frontier models. Natural language parsing of "When delegating" blocks is within their capabilities. A structured delimiter would be marginally more reliable but adds template authoring overhead.

### Step 3: Agent gathers context

The agent reads the delegation guidance and gathers files. With a read-write delegate, the agent has two choices:

**Option A:** Gather file contents and include them in the prompt (context dump). This is what the example directive suggests ("include in the prompt: All source files").

**Option B:** Write a task description that tells the delegate to read files from disk. This uses the read-write capability. The prompt is smaller and the delegate uses its own file-reading tools.

The directive example steers toward Option A. But Option B is often better because it avoids context window pressure and uses the delegate's native capabilities. The design doesn't guide the agent (or the template author) on which option to choose.

**Recommendation:** This is covered in Focus Area 3 above. The design should provide guidance on when to use each option. Short heuristic: if the relevant files total less than ~50K tokens, include them directly. If they're larger, direct the delegate to read from disk.

### Step 4: Agent writes prompt to file

```bash
prompt_file=$(mktemp)
```

Agent writes the prompt content to the temp file. This step is straightforward.

**Minor gap:** The agent needs to know it should use `mktemp`. This is a SKILL.md instruction detail -- not a design gap.

### Step 5: `koto delegate run --prompt "$prompt_file"`

Agent runs the delegate command and receives:

```json
{
  "response": "## Analysis Results\n\nThe authentication module has three areas...",
  "delegate": "gemini",
  "matched_tag": "deep-reasoning",
  "duration_ms": 45000,
  "exit_code": 0,
  "success": true
}
```

**Gap: Re-resolution at run time.** The design says `koto delegate run` "re-resolves the delegation target from tags + config (same logic as `Next()`)." This means the config is read again. If the config changed between `koto next` and `koto delegate run` (unlikely but possible), the resolution might differ. The agent received `target: gemini` from `koto next` but `koto delegate run` might resolve to a different target.

**Assessment: Acceptable for v1.** Config changes between commands are an edge case. The re-resolution ensures the delegate run uses current config state, which is arguably correct. But the design should note that the target in the `koto delegate run` response might differ from the target in the `koto next` response if config changed.

**Alternative approach (not recommended for v1):** Accept `--target` on `koto delegate run` to let the agent pass through the target from `koto next`, bypassing re-resolution. This adds a flag and creates a consistency question (what if the passed target doesn't exist in current config?). Not worth the complexity for v1.

### Step 6: Agent uses response

The agent has the delegate's response text. Now what? This is entirely determined by the SKILL.md and the directive for the next state. The `implement` state's directive might reference the analysis output:

```markdown
## implement

Based on the analysis, implement the changes for: {{TASK}}
```

But the analysis output is in the delegate's response, which lives in the agent's context window. It's not in koto's evidence store and can't be referenced via `{{VARIABLE}}` syntax. The agent must carry the response forward in its own context.

**Assessment: This is fine for v1.** The agent's context window is where the response should live. The agent can refer to "the analysis from the previous step" naturally. Stuffing the response into evidence would be premature optimization.

### Step 7: `koto transition implement`

Agent calls transition. Gates are evaluated. This step is unchanged from non-delegation flow.

**No gaps here.**

### Step 8: Cleanup

The agent should remove the temp file. The design notes this but doesn't put it in the execution flow. The SKILL.md should include a cleanup step.

### End-to-End Summary

The flow works. The primary gap is that the SKILL.md instructions for delegation-aware workflows don't exist yet, and the design's Phase 5 description is too vague about what they should contain. The secondary gap is the prompt crafting guidance not accounting for read-write delegate capabilities (it still follows a "context dump" pattern).

## Consolidated Findings

### Must-address before implementation

**F1: SKILL.md delegation instructions are unspecified.** The design lists "Update SKILL.md with delegation flow documentation" as a Phase 5 deliverable but doesn't specify what the updated SKILL.md should contain. Without explicit SKILL.md instructions, agents won't know to check the `delegation` field or how to act on it. The design should include a skeleton of a delegation-aware execution section (see section 1.3 above for a proposed template).

**F2: Prompt guidance doesn't account for read-write delegates.** The example directive says "include in the prompt: All source files" (a context dump pattern). With read-write delegates, the prompt should direct the delegate to read files from disk rather than receiving them inline. The design should update the example to show the read-write pattern and provide guidance on when to use each approach (see section 3.1).

### Should-address before implementation

**F3: Custom skill authoring guide needs delegation section.** The authoring guide is the primary resource for skill authors and currently has no delegation content. The design's File Change Summary should include it (see section 5.1).

**F4: A delegation-aware reference SKILL.md is missing.** Phase 5 ships a `research-and-implement` template but no companion SKILL.md. Skill authors need a full reference implementation including both template and SKILL.md (see section 5.2).

**F5: Stderr handling is undocumented.** The Delegate Interface Contract table omits stderr behavior. Add a row stating stderr is inherited (see section 1.2).

**F6: Delegate file-writing guidance is missing.** With read-write access, the delegate may write files. The design should recommend that delegation directives instruct delegates to report all output via stdout unless file-writing is explicitly required (see section 3.2).

### Observations (no action needed for v1)

**O1: Dual-use directive text.** Directives serve both self-execution and delegation prompt guidance in the same text block. This works but should be documented as an intentional pattern for template authors (section 2.1).

**O2: Multi-tag prompt compatibility.** When a state has multiple tags and the matched delegate differs from what the directive guidance assumed, the prompt may be poorly sized. The single-tag-per-state recommendation should be strengthened (section 2.2).

**O3: Config re-resolution between next and delegate run.** Config is re-read at `delegate run` time. If config changed between calls, the target might differ from what `koto next` reported. Edge case, acceptable for v1 (section Step 5 walkthrough).

**O4: `specialized-tooling` naming.** The tag name is adequate but imprecise. Consider whether `tool-augmented` or `external-tooling` reads more clearly. Not blocking -- the name can evolve since tags are documented, not enforced (section 4.1).
