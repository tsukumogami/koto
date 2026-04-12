# Walkthrough Process Guide

## Goal

Validate the batch-child-spawning design by role-playing a complete
workflow execution. The user plays the agent (the unpredictable actor);
the assistant plays koto (showing exact JSON responses). The walkthrough
surfaces design gaps by letting the user make unexpected choices at each
step and seeing whether koto's response contract handles them.

This already surfaced Decisions 7 and 8, plus the `type: tasks`
replacement for `type: json`, and the single-state fan-out requirement.
More gaps are expected as we walk through failure paths, retry, and
dynamic task addition.

## Presentation rules

The user has strong preferences about how information is presented.
Follow these exactly:

1. **One artifact at a time.** Show the template, explain it, wait for
   acknowledgment. Then show the plan, explain it, wait. Then show the
   first koto interaction. Never skip ahead or bundle multiple artifacts
   in one message.

2. **Explain, then wait.** After showing each artifact or interaction,
   explain what it does and why, then stop and wait for the user to
   respond. Do NOT use the AskUserQuestion tool to prompt the user --
   just present the material and let them respond naturally. The user
   found AskUserQuestion prompts disruptive during this exercise.

3. **Full JSON responses.** Show the complete `koto next` response JSON
   for each interaction, not a summary. The user wants to see exactly
   what the agent receives. Explain each field's purpose after the JSON.

4. **Explain the "switch" explicitly.** At each transition point
   (parent to child, child to parent, child to sibling), explain what
   in koto's response tells the agent to change which workflow it's
   driving. The response IS the signal -- there is no separate "switch"
   command.

5. **Let the user drive.** After explaining a koto response, the user
   decides what the agent does next. They may:
   - Follow the happy path (drive the suggested child, submit expected
     evidence)
   - Test edge cases (submit malformed evidence, call koto next on a
     blocked child, submit retry evidence prematurely, etc.)
   - Ask questions about the design (which may surface gaps that need
     new decisions)

6. **When the user surfaces a gap, stop the walkthrough.** Run a
   /decision if needed, update all artifacts, then resume from where
   we stopped.

7. **No "proceed" prompts.** Don't ask "ready to continue?" or present
   multiple-choice options. Just show the next piece and explain it.
   The user will say when they want to move on or when something needs
   discussion.

## Artifact order

Present these in sequence, one per message, with explanation and a
pause after each:

1. Parent template (`coord.md`) -- the single-state fan-out pattern
2. Child template (`impl-issue.md`) -- what each spawned child looks like
3. Plan document (`PLAN-batch-schema.md`) -- the upstream artifact the
   agent reads to build the task list
4. Task list (`tasks.json`) -- what the agent produces from the plan,
   showing how the task entry schema maps to the plan's issue outlines
5. Interaction 1: `koto init coord --template coord.md --var plan_path=PLAN-batch-schema.md`
6. Interaction 2: `koto next coord` (get the planning directive)
7. Interaction 3: `koto next coord --with-data @tasks.json` (submit the batch)
8. Interaction 4: `koto next coord.issue-1` (drive the first child)
9. Interaction 5: Agent completes issue-1, submits evidence
10. Interaction 6: `koto next coord` (re-tick parent, unblock issue-2 and issue-3)
11. Interaction 7: Drive issue-2 and issue-3 (parallel or sequential, user's choice)
12. Interaction 8: Both complete, re-tick parent, batch finishes
13. Failure scenario: what happens if issue-2 submits `{"status": "blocked"}`
14. Retry scenario: `retry_failed` evidence action

## Current state

The walkthrough was started in the previous session. We presented:

- The parent template (step 1) -- user reviewed and provided feedback
  that led to Decision 8 (default_template + item_schema) and the
  type: tasks replacement for type: json
- The child template was NOT yet shown
- No interactions were walked through (we showed mock responses during
  the design discussion but did not do the slow step-by-step walkthrough)

**Resume point: step 2 (child template).** Start from the child
template and proceed through the full sequence.

## Files in this folder

- `walkthrough.md` -- the full walkthrough document with all templates,
  plan, task list, and interaction sequences. This is the reference
  material; the session walks through it step by step.
- `PROCESS.md` -- this file. Describes the presentation process.

## Design artifacts (parent folder)

The walkthrough references these design artifacts in `wip/`:

- `design_batch-child-spawning_summary.md`
- `design_batch-child-spawning_decisions.md`
- `design_batch-child-spawning_coordination.json`
- `design_batch-child-spawning_cross_validation.md`
- `design_batch-child-spawning_decision_[1-8]_report.md`
- `explore_batch-child-spawning_*.md`
- `research/explore_batch-child-spawning_r1_lead-*.md`
- `research/design_batch-child-spawning_phase[5-6]_*.md`

And the design doc itself at `docs/designs/DESIGN-batch-child-spawning.md`.

## What to do when a gap is found

When the user's response reveals a design gap:

1. Acknowledge the gap explicitly
2. Analyze whether it needs a new decision or is a clarification
3. If new decision: run `/decision` or add Decision N+1 to the design,
   update all artifacts (design doc, walkthrough, coordination manifest),
   commit and push
4. If clarification: update the relevant section of the design doc,
   update the walkthrough if affected, commit and push
5. Resume the walkthrough from where we stopped

Every change gets committed and pushed before resuming so the user
can review the diff on GitHub.
