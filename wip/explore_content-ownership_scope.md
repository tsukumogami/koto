# Explore Scope: content-ownership

## Visibility

Public

## Core Question

Should koto own the cumulative context of workflow execution (the files currently stored under wip/) instead of letting agents read and write them directly through the filesystem? If so, what CLI interface and storage model enables agents to submit, retrieve, and query context through koto while supporting multi-agent concurrency, cross-phase reads, skill-to-skill handoffs, and resume logic?

## Context

The session storage feature (PR #84) moved state files from the working directory to `~/.koto/sessions/<repo-id>/<name>/`. But agents still have direct filesystem access to everything in the session directory. The insight: koto should be the gatekeeper for workflow context, not just the address book. Agents submit content to koto and read it back through koto's CLI. This enables content validation, controlled access, format enforcement, and eventually structured queries.

The wip/ surface spans 10+ skills across shirabe and tsukumogami plugins, with ~50 distinct artifact patterns following three lifecycle types: research outputs (create-once, read-later), accumulation files (appended across rounds), and coordination files (updated in-place).

Final deliverables (DESIGN docs, code, PLANs, ROADMAPs) remain agent-managed. Only the workflow's cumulative context — what today lives in wip/ — moves under koto's ownership.

The user's longer-term vision includes agents being able to interrupt workflows to add ad-hoc research context, and koto eventually supporting structured queries on context sections. These are out of scope for this exploration but inform the design direction.

## In Scope

- CLI interface for context submission (pipe, file reference, or both)
- CLI interface for context retrieval (by key, by pattern, listing)
- Multi-agent concurrent context submission without advancing state
- Gate evaluation against koto-owned context (replacing filesystem checks)
- Resume logic based on koto-owned context (replacing wip/ file existence checks)
- Skill-to-skill handoff flows through koto (explore→design, design→plan, etc.)
- Migration surface assessment for existing skills
- Naming and namespacing for context keys
- The three lifecycle patterns: create-once, accumulation, coordination updates

## Out of Scope

- State file internals (already koto-owned by the engine)
- Final deliverables (DESIGN docs, code, etc. — agents own those)
- Partial patches / structured updates on context sections (future)
- Cloud sync of context (separate feature)
- Ad-hoc context injection by users mid-workflow (future)
- The specific name for the CLI subcommand (needs UX exploration, not "evidence")

## Research Leads

1. **What CLI UX works for context submission and retrieval?**
   Piping (`koto ctx add <session> --key research/lead-1.md < file`), file reference (`--from`), inline. Namespaced keys for organization. How does retrieval work for agents that need cross-phase context? What existing CLI tools handle content-addressable storage well?

2. **How should multi-agent concurrent context submission work?**
   The explore skill fans out 3-8 research agents writing simultaneously. The implement skill runs 3 parallel scrutiny agents and 3 parallel reviewers. What's the concurrency model — per-key locking, append-only log, CAS, or something simpler?

3. **How do accumulation and update patterns map to a content-owned model?**
   Findings.md gets appended across discover-converge rounds. Bakeoff files get updated via SendMessage. Coordination.json tracks per-decision status. These aren't replace-only. How do they work when koto owns the content? Is versioning needed?

4. **How does gate evaluation change when koto owns context?**
   Gates currently shell out (`test -f {{SESSION_DIR}}/plan.md`). If koto owns the content, gates need to check koto's storage. Does koto evaluate gates internally, or does it expose a check command for shell gates?

5. **How do skill-to-skill handoff flows work through koto?**
   Explore creates `wip/prd_<topic>_scope.md` for /prd, `wip/design_<topic>_summary.md` for /design. These are cross-skill context transfers. Does the receiving skill read them from the same session, a different session, or through a handoff mechanism?

6. **What's the resume logic migration?**
   Every skill uses `if wip/<artifact> exists → resume at phase N`. If koto owns context, resume detection moves to querying koto. What does this look like? Does koto need a `koto ctx exists <session> --key <key>` command, or does `koto ctx list` suffice?

7. **Is there evidence of real demand for this, and what do users do today instead?** (lead-adversarial-demand)
   You are a demand-validation researcher. Investigate whether evidence supports
   pursuing this topic. Report what you found. Cite only what you found in durable
   artifacts. The verdict belongs to convergence and the user.

   ## Visibility

   Public

   Respect this visibility level. Do not include private-repo content in output
   that will appear in public-repo artifacts.

   ## Six Demand-Validation Questions

   Investigate each question. For each, report what you found and assign a
   confidence level.

   Confidence vocabulary:
   - **High**: multiple independent sources confirm (distinct issue reporters,
     maintainer-assigned labels, linked merged PRs, explicit acceptance criteria
     authored by maintainers)
   - **Medium**: one source type confirms without corroboration
   - **Low**: evidence exists but is weak (single comment, proposed solution
     cited as the problem)
   - **Absent**: searched relevant sources; found nothing

   Questions:
   1. Is demand real? Look for distinct issue reporters, explicit requests,
      maintainer acknowledgment.
   2. What do people do today instead? Look for workarounds in issues, docs,
      or code comments.
   3. Who specifically asked? Cite issue numbers, comment authors, PR
      references — not paraphrases.
   4. What behavior change counts as success? Look for acceptance criteria,
      stated outcomes, measurable goals in issues or linked docs.
   5. Is it already built? Search the codebase and existing docs for prior
      implementations or partial work.
   6. Is it already planned? Check open issues, linked design docs, roadmap
      items, or project board entries.

   ## Calibration

   Produce a Calibration section that explicitly distinguishes:

   - **Demand not validated**: majority of questions returned absent or low
     confidence, with no positive rejection evidence. Flag the gap. Another
     round or user clarification may surface what the repo couldn't.
   - **Demand validated as absent**: positive evidence that demand doesn't exist
     or was evaluated and rejected. Examples: closed PRs with explicit maintainer
     rejection reasoning, design docs that de-scoped the feature, maintainer
     comments declining the request. This finding warrants a "don't pursue"
     crystallize outcome.

   Do not conflate these two states. "I found no evidence" is not the same as
   "I found evidence it was rejected."
