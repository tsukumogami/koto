# Agentic Systems Review: Cross-Agent Delegation Design

Reviewer role: Agentic systems specialist (tool-use interfaces for AI coding agents).

Reviewed: `docs/designs/DESIGN-cross-agent-delegation.md`

Reference: `plugins/koto-skills/skills/hello-koto/SKILL.md`, `pkg/controller/controller.go`, `docs/guides/custom-skill-authoring.md`, `docs/guides/cli-usage.md`

## 1. Agent Integration Flow: Round-Trip Analysis

The proposed flow is six steps:

1. `koto next` -- returns directive + `DelegationInfo`
2. Agent reads directive, gathers context, crafts prompt
3. `koto delegate submit --prompt /tmp/prompt.txt`
4. koto invokes delegate CLI, captures stdout
5. koto returns `{response, success}` to agent
6. Agent uses response, calls `koto transition`

**Assessment: The round-trip count is correct.** Combining delegation resolution into `koto next` (Decision 5) avoids a separate `koto delegate-check` round-trip. The design correctly rejected a separate endpoint. The agent calls three koto commands per delegated step (next, delegate submit, transition) versus two for non-delegated steps (next, transition). One extra command for delegation is reasonable.

**Concern: The agent's role at step 2 is underspecified.** The design says "the agent crafts a self-contained prompt" but doesn't define what "self-contained" means in practice. The agent has just received a directive like "Analyze the codebase for: {{TASK}}." To craft a useful delegation prompt, the agent needs to:

- Decide what codebase context to include (which files? how much?)
- Frame the task for a model that has zero prior context
- Include enough background that the delegate can produce a useful response
- Stay within the delegate's context window limits

This is a significant cognitive load. The agent is essentially acting as a prompt engineer in real time. The design says "the agent crafts" this, but the SKILL.md is where the agent gets its instructions. If the SKILL.md doesn't tell the agent how to craft a delegation prompt, the agent will improvise -- and different agents will improvise differently, producing inconsistent results.

**Recommendation:** The directive text itself (the `## deep-analysis` section in the template) should include guidance for prompt construction when the state is tagged for delegation. The template author knows what the delegate needs. Something like:

```markdown
## deep-analysis

Analyze the codebase for: {{TASK}}

When delegating this step, include:
- All files in the changed module
- The test files for those modules
- The git log for the last 10 commits touching those files
```

This keeps the delegation prompt guidance in the template (where the workflow author has domain knowledge) rather than in the agent's general skill instructions (which are workflow-agnostic).

## 2. Prompt Crafting Realism

**Assessment: The "agent crafts a self-contained prompt" assumption is the design's weakest point.**

Here's what actually happens when a coding agent like Claude Code encounters a delegation step:

1. The agent has accumulated context during earlier workflow states (files read, analysis performed, error messages seen).
2. The agent receives a directive saying "analyze X."
3. The agent must now serialize its accumulated understanding into a standalone prompt for a model that has seen nothing.

This is hard for three reasons:

**Context serialization is lossy.** The orchestrating agent may have read 50 files across three states. It can't dump all of them into a prompt. It must decide what's relevant to the delegate's specific task. Current coding agents are not great at this -- they tend to either include too much (blowing the context window) or too little (producing a vague prompt).

**No prompt template system.** The design provides `DelegationInfo` with `target`, `matched_tag`, and `available` -- all routing metadata. None of this helps the agent write a better prompt. Compare this to how SKILL.md currently works: it gives the agent exact commands, expected JSON shapes, and error recovery steps. For delegation, the agent gets "you should delegate" but no structure for how.

**The delegate has no tools.** The design explicitly scopes out "delegates that need tool access." This means the delegate can't read files, run commands, or check anything. The prompt must contain everything. For a security audit of a codebase, this means the orchestrating agent must extract all relevant source code, dependency manifests, configuration files, and include them in the prompt. That's a lot of work the agent must do correctly every time.

**Recommendation:** Add a `prompt_template` field to the state declaration or to `DelegationInfo`:

```yaml
states:
  deep-analysis:
    tags: [deep-reasoning]
    prompt_template: |
      You are performing a security analysis of a Go codebase.

      ## Task
      {{TASK}}

      ## Source Files
      {{CONTEXT}}

      ## Instructions
      Identify vulnerabilities, unsafe patterns, and missing input validation.
      Return a structured report with severity ratings.
```

The agent fills in `{{CONTEXT}}` with the gathered files. The template author provides the framing. This gives the agent a structure to fill rather than a blank canvas.

Alternatively, if adding `prompt_template` to the template format is too heavy for v1, the SKILL.md documentation should include explicit delegation prompt instructions per-state:

```markdown
### Delegation: deep-analysis

When this step is delegated, construct the prompt as follows:
1. Start with: "You are analyzing [codebase description] for [TASK]."
2. Include the full contents of all .go files in the affected packages.
3. Include go.mod and go.sum.
4. End with: "Provide a structured analysis with findings and recommendations."
```

## 3. Response Handling

The delegate response comes back as:

```json
{
  "response": "...",
  "delegate": "gemini",
  "duration_ms": 12345,
  "success": true
}
```

The agent then "decides what to do with it."

**Assessment: This is sufficient for v1, but creates a quality cliff.**

The response is an opaque string. The agent must:
- Parse it (is it structured? markdown? plain text?)
- Decide what parts are actionable
- Figure out how to use it in subsequent states

For a security audit, the delegate might return a 5000-word analysis. The agent needs to extract findings, prioritize them, and act on them in the `implement` state. Without guidance, the agent might just dump the whole response into its context and hope for the best, or it might discard important sections.

**Concern: No response format contract.** The template author knows what the delegate is supposed to produce, but there's no way to express "the delegate should return JSON with a `findings` array" or "the delegate should return markdown with `## Critical` and `## Warning` sections." The orchestrating agent can't validate the response shape because there's no expected shape.

**Recommendation for v1:** Don't add response parsing to koto. Instead, document in the SKILL.md or directive text what the agent should expect from the delegate and how to use it:

```markdown
## implement

Based on the security analysis from the previous step, implement fixes.

The analysis contains sections labeled Critical, Warning, and Info.
Address all Critical and Warning items. Log Info items but don't fix them.
```

This keeps koto as a pipe (the right choice) while giving the agent enough guidance to use the response effectively.

**Recommendation for v2:** Consider adding the delegate response to the evidence map so that subsequent states can reference it via `{{DELEGATE_RESPONSE}}` or similar. This would let the `implement` state's directive template include: "The analysis found: {{ANALYSIS_RESULT}}". The controller would store the delegate response as evidence automatically after a successful `koto delegate submit`.

## 4. Fallback Behavior

When delegation is unavailable, the response includes:

```json
{
  "delegation": {
    "target": "gemini",
    "matched_tag": "deep-reasoning",
    "available": false,
    "fallback": true,
    "reason": "binary \"gemini\" not found in PATH"
  }
}
```

**Assessment: The signal is clear but the behavior guidance is missing.**

The `fallback: true` flag tells the agent "handle this yourself." But the directive text was written with delegation in mind -- it may say "Analyze the codebase deeply" assuming a model with extended reasoning. If the orchestrating agent is Claude Code running Haiku, "analyze deeply" means something very different than if it's routed to Gemini with a million-token context.

**Concern: Same directive, different capabilities.** The directive text doesn't change between delegation and fallback modes. A directive optimized for a deep-reasoning delegate ("think for 10 minutes about all possible attack vectors") is unhelpful for an agent that needs to handle it in its normal execution flow. Conversely, a directive written for self-execution ("read these files and list issues") undersells what a delegated model could do.

**Recommendation:** Allow states to specify an optional fallback directive:

```yaml
states:
  deep-analysis:
    tags: [deep-reasoning]
    transitions: [implement]
    fallback_directive: |
      Perform a basic security scan. Focus on the most common vulnerability
      categories (injection, auth, data exposure). This is a reduced-scope
      analysis because the preferred deep-reasoning model is unavailable.
```

If fallback is triggered and `fallback_directive` exists, the controller returns it instead of the regular directive. This lets template authors write context-appropriate instructions for both paths. If `fallback_directive` is absent, the regular directive is used (current behavior).

For v1, if this is too much scope, document the pattern in the SKILL.md:

```markdown
### Fallback Handling

If delegation is unavailable (delegation.fallback is true):
- The deep-analysis step runs in the current agent
- Focus on [reduced scope] rather than [full scope]
- Skip [X, Y, Z] which require extended reasoning
```

## 5. Skill Documentation for Delegation-Aware Skills

The design says "Update SKILL.md with delegation flow documentation" (Phase 5). The current SKILL.md (`hello-koto`) has no delegation concepts. The custom skill authoring guide documents seven sections (Prerequisites, Template Setup, Execution, Evidence Keys, Response Schemas, Error Handling, Resume). Delegation introduces new requirements for skill authors.

**Assessment: Skill authors need substantially more guidance than "update SKILL.md."**

A delegation-aware skill author needs to know:

1. **When to tag states.** The vocabulary says `deep-reasoning` is for "security audits, architecture analysis, complex debugging." But what counts as "complex" enough to warrant delegation? If the wrong states are tagged, delegation adds latency and cost for no benefit.

2. **How to write delegation-compatible directives.** The directive is sent to the orchestrating agent, which uses it to craft a prompt for the delegate. The directive needs to work in two modes: as a direct instruction (no delegation) and as a description of the desired output (with delegation). These are different writing styles.

3. **What the orchestrating agent should include in the prompt.** The delegate has no context. The skill author knows what context matters. This belongs in the SKILL.md.

4. **How the agent should handle the delegate response.** Does it paste the response into a file? Use it as input for the next state? Summarize it? The skill author knows the workflow semantics.

5. **How to handle fallback mode.** What's the graceful degradation path for each tagged state?

6. **What the delegation flow looks like in the execution section.** The current SKILL.md execution section shows a linear `init -> next -> execute -> transition` loop. With delegation, step 3 becomes: check if delegation exists, if yes gather context and craft prompt, call `koto delegate submit`, use response. This branching logic needs to be spelled out.

**Recommendation:** Add a "Delegation" section to the SKILL.md standard for koto workflows:

```markdown
## Delegation

### deep-analysis (tags: deep-reasoning)

**Delegation available:**
1. Read `koto next` response. If `delegation.available` is true, proceed with delegation.
2. Gather context: read all .go files in pkg/, read go.mod.
3. Write prompt to /tmp/koto-prompt.txt:
   - System instruction: "You are a security analyst..."
   - Include gathered file contents
   - End with the directive text from koto next
4. Run: `koto delegate submit --prompt /tmp/koto-prompt.txt`
5. Use the response as input for the implement state.

**Delegation unavailable (fallback):**
1. Perform a basic security review yourself.
2. Focus on: [specific items].
3. Skip: [items that need deep reasoning].
```

This makes the agent's decision tree explicit at every branch point.

## 6. Single-Exchange Limitation

The design scopes delegation to "one prompt in, one response out." The stated use cases are security audits and architecture analysis.

**Assessment: This is realistic for v1 but will need extension for the stated use cases to work well.**

**Why single-exchange works for v1:**

- It keeps the interface simple and stateless. No session management, no conversation threading.
- It matches how `claude -p` and `gemini -p` actually work -- they take a prompt and return a response.
- For well-structured prompts with sufficient context, a single exchange can produce useful output. A delegate given 200K tokens of code and a clear "find vulnerabilities" instruction will produce results.

**Why single-exchange will be limiting:**

- **Security audits need follow-up.** A real audit involves: identify a suspicious pattern -> trace data flow -> check if it's exploitable -> recommend a fix. A single prompt can't express "I found something in file A, now I need to check file B to see if it's reachable." The orchestrating agent would need to pre-emptively include everything the delegate might need. With large codebases, that's not feasible even with million-token windows.

- **Architecture analysis is iterative.** "Analyze the architecture" often leads to "I see pattern X, does it extend to subsystem Y?" The delegate can't ask. It either guesses (potentially wrong) or hedges (less useful).

- **The "self-contained prompt" burden grows.** With multi-turn, the orchestrating agent can start with a high-level prompt and let the delegate request what it needs. With single-exchange, the orchestrating agent must anticipate everything. This makes the prompt crafting problem from section 2 even harder.

**Recommendation:** The single-exchange limitation is the right scoping choice for v1. The design already acknowledges this: "Future designs can extend to multi-turn or streaming if needed." But the documentation should be honest about what works and what doesn't in single-exchange mode:

- Works well: Focused analysis tasks with bounded context ("review this 500-line function for vulnerabilities").
- Works adequately: Broad analysis tasks where the relevant context can fit in the delegate's window ("analyze this 10-file package's architecture").
- Works poorly: Investigative tasks requiring iteration ("find the root cause of this intermittent failure across the codebase").

This helps template authors decide which states to tag and how to scope their directives.

## 7. Tag Vocabulary

The initial vocabulary is:

| Tag | Meaning |
|-----|---------|
| `deep-reasoning` | Extended chain-of-thought reasoning |
| `large-context` | Large context window (100K+ tokens) |
| `security` | Security-sensitive analysis |

**Assessment: These tags mix two different dimensions, which will create routing confusion.**

`deep-reasoning` and `large-context` describe **model capabilities** -- what kind of processing the step needs. `security` describes **domain expertise** -- what the step is about.

This mixing creates ambiguity in config rules. Consider: a security audit of a large codebase needs both `deep-reasoning` and `large-context`. But it also needs `security`. Should the state have three tags? First-match routing means only one rule fires. Which tag should match first?

```yaml
states:
  security-audit:
    tags: [security, deep-reasoning, large-context]
```

With the current first-match rule resolution, if the user's config maps `security -> specialized-security-tool` and `deep-reasoning -> gemini`, the step goes to the security tool. Reorder the config rules, and it goes to gemini. This ordering dependency is fragile and surprising.

**The deeper problem:** Tags are trying to express two things at once -- "what this step needs" (capabilities) and "what this step is about" (domain). These map to different routing decisions. You might route `deep-reasoning` to Gemini (because it has extended thinking) and `security` to a specialized scanning tool. But you can't do both for the same step under single-match routing.

**Recommendation for v1:** Keep the three tags but document them as capability-oriented, not domain-oriented. Rename the vocabulary:

| Tag | Meaning |
|-----|---------|
| `deep-reasoning` | Step benefits from extended chain-of-thought |
| `large-context` | Step needs a large context window |
| `specialized-tooling` | Step benefits from domain-specific tools |

Drop `security` as a tag. Security isn't a routing characteristic -- it's a domain. A security audit needs `deep-reasoning`. A security scan needs `specialized-tooling`. The tag should describe what the step needs from the delegate, not what the step is about.

If domain-based routing is genuinely needed (route all security steps to a specific tool regardless of capability needs), add it as a future enhancement with a different matching mechanism (maybe `domain` as a separate field on states, not a tag).

**Alternative recommendation:** If the three tags stay as-is, change the routing from first-match to multi-match. When a state has multiple tags and multiple rules match, all matched rules are returned and the agent (or koto) picks the best one. This would make `DelegationInfo` an array rather than a single struct. But this adds complexity and may not be worth it for v1.

The simpler path: document that each state should have exactly one tag, and that tag represents the primary routing concern. If a security audit needs deep reasoning, tag it `deep-reasoning` and write the directive to focus on security. The tag routes; the directive scopes.

## Additional Observations

### Prompt file lifecycle

The design has the agent write the prompt to `/tmp/prompt.txt` and pass `--prompt /tmp/prompt.txt`. Who cleans up this file? If the agent creates a temp file per delegation, and delegation happens frequently, temp files accumulate. koto could delete the prompt file after reading it, but that's a surprising side effect. The agent could clean up, but SKILL.md instructions rarely cover cleanup.

**Recommendation:** Document that koto does not delete the prompt file. The agent (or OS temp cleanup) is responsible. For SKILL.md instructions, suggest using `mktemp` to avoid collisions:

```bash
prompt_file=$(mktemp)
# ... write prompt to $prompt_file ...
koto delegate submit --prompt "$prompt_file"
rm "$prompt_file"
```

### stdin vs. file for prompt delivery

The design offers both `--prompt /tmp/prompt.txt` and `echo "..." | koto delegate submit --prompt -`. For coding agents, the file path approach is more natural. Agents write files easily. Piping large prompts via stdin in a single shell command is awkward because the prompt may contain characters that break shell quoting. The file approach also lets agents verify the prompt before submission (read back the file to confirm contents).

**Recommendation:** Make `--prompt <file>` the documented primary interface. Keep stdin (`--prompt -`) as an option but don't emphasize it in SKILL.md examples.

### DelegationInfo for untagged states

The design says `Delegation` is nil when there are no tags or no matching rules. But there are three distinct "no delegation" cases:

1. State has no tags (delegation was never intended)
2. State has tags but no config rule matches (user didn't configure routing for this tag)
3. State has tags, rule matches, but delegate unavailable (binary not found)

Case 3 is covered by `fallback: true`. Cases 1 and 2 are both nil `Delegation`. The agent can't distinguish "this state was never meant to be delegated" from "this state could be delegated but the user didn't configure it." This distinction matters for SKILL.md instructions: in case 1, the agent should execute normally. In case 2, the agent might want to suggest that the user configure delegation.

**Recommendation:** For v1, this is fine. Both cases result in the agent executing the directive itself. If user guidance becomes important later, add a `tags_present: true` field to `DelegationInfo` for case 2 (tags exist but no rule matched).

### Interaction with evidence and state progression

The design says "the orchestrating agent decides what to do with the response." But there's a gap: if the delegate response contains findings that should inform the next state, how does it get there? The current koto model passes information between states via evidence (`map[string]string`). But there's no mechanism to store the delegate response as evidence.

The orchestrating agent could manually write the response to a file and set evidence via some future `--evidence` flag on `koto transition`. But the current design doesn't mention this integration, and the existing `koto transition` doesn't support evidence arguments.

**Recommendation:** Note this as a known gap. For v1, the agent consumes the delegate response in its own context and acts on it. For v2, consider having `koto delegate submit` optionally store the response as evidence under a configurable key, so subsequent states can reference it.

## Summary of Recommendations

### Must-address for v1

1. **Provide prompt construction guidance in directive text or SKILL.md.** The "agent crafts a self-contained prompt" assumption is too optimistic without structure. Template authors need to specify what context the delegate needs.

2. **Document the single-exchange limitations honestly.** Help template authors scope their delegation states to tasks that work in one round-trip.

3. **Expand the SKILL.md standard with a Delegation section.** Skill authors need explicit instructions for the delegation branch: what context to gather, how to write the prompt, how to use the response, and what to do in fallback mode.

### Should-address for v1

4. **Clarify tag vocabulary as capability-oriented.** Either rename `security` to something capability-focused, or document that tags describe processing needs (not domains). The current mix will cause routing confusion.

5. **Document prompt file lifecycle.** Clarify that koto doesn't delete prompt files and agents are responsible for cleanup.

6. **Add fallback directive guidance.** Even if `fallback_directive` as a template field is deferred, the SKILL.md should contain per-state fallback instructions.

### Consider for v2

7. **Prompt template field on state declarations.** Let template authors define the delegation prompt structure, with the agent filling in context variables.

8. **Store delegate responses as evidence.** Integrate delegate output into koto's state progression so subsequent states can reference it.

9. **Response format contracts.** Let template authors specify what the delegate should return, so the orchestrating agent can validate and parse responses.

10. **Multi-match tag routing.** If domain tags (like `security`) are retained alongside capability tags, support matching on multiple tags to resolve routing ambiguity.
