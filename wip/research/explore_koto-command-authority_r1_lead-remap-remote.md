# Lead: Which writes-remote commands become conversion candidates now that the permission argument is struck, and what does each still need?

## Findings

Scope: every literal `git push`, `git push --force-with-lease`, `gh pr create`, `gh pr edit`, `gh pr ready`, `gh pr close` in `skills/execute/koto-templates/execute.md`, `skills/execute/SKILL.md`, `skills/work-on/koto-templates/work-on.md`, `skills/work-on/references/**` (shirabe, branch `docs/koto-runs-commands`), plus `run-cascade.sh` since `plan_completion` wraps it. Ten distinct line-items, confirmed complete by a final sweep (`grep -rn -E "git push|gh pr (create|edit|ready|close|reopen)"` across all four scopes) that turned up nothing beyond what's tabulated below. Base data: `r2_lead-map-execute.md`, `r2_lead-map-work-on.md`, `r3_lead-middle-path.md`, `r2_lead-template-patterns.md`, all re-read in full before this pass, plus a fresh read of `koto-templates/execute.md`'s frontmatter (gate idiom), `run-cascade.sh:860-893`, and `work-on.md`'s `pr_creation` state (confirmed: no `gates:` block today — line 695 goes straight to `accepts:`).

### The bucket, row by row

| # | file:line — command | state | reversible? | gate-verifiable independent of exit code? | idempotent / safe twice? | still needs | verdict |
|---|---|---|---|---|---|---|---|
| 1 | `execute.md:395` `git push -u origin impl/$PLAN_SLUG 2>/dev/null \|\| true` | `orchestrator_setup` | Yes, cheaply — a fresh branch nothing else has committed to yet; `git push origin --delete` undoes it | Yes: `gh pr list --head impl/{{PLAN_SLUG}} --json number --jq '.[0].number' \| grep -q .` (same gate covers #2 — a PR existing implies the branch landed) | Yes — `2>/dev/null \|\| true` already treats "already exists" as success | nothing | **converted-now** |
| 2 | `execute.md:397` `gh pr create --draft --title "impl: $PLAN_SLUG" --body "..."` | `orchestrator_setup` | Yes — draft PR with boilerplate body not yet read by anyone; `gh pr close` undoes it at near-zero cost | Yes, same gate as #1 | Yes — already guarded by `gh pr list --head ... \| grep -q . \|\|` in the block itself | nothing | **converted-now** |
| 3 | `execute.md:555` `gh pr edit "$PR_NUMBER" --title "feat: $PLAN_SLUG" --body-file "$BODY_FILE"` | `pr_finalization` | Yes, trivially — re-editing overwrites, no git history touched | Structure only: `gh pr view "$PR_NUMBER" --json title --jq .title \| grep -qE '^[a-z]+(\([a-z0-9-]+\))?: '` verifies the conventional-commit title shape (what `shirabe validate --pr-body` enforces in CI); no gate can verify the authored prose is *good*, only that it's *shaped right* | Yes — every run fully overwrites, never appends | a capability koto doesn't have: an action consuming evidence the agent just submitted in the same call (the title/body text itself), not output-routing in the other direction | **converted-after-plumbing** — the apply mechanism (`gh pr edit --body-file`) is mechanical and gate-checkable for structure; title/Part-1 authoring stays agent-run permanently regardless, since no gate reads for quality |
| 4 | `execute.md:596` `run-cascade.sh --push {{PLAN_DOC}}` (internally: `git commit`, `git rm`, `git push` at `run-cascade.sh:884-886`) | `plan_completion` | Reversible with moderate cost — a normal forward commit+push (not force), so `git revert <sha> && git push` undoes the git side, but PLAN deletion + BRIEF/PRD/DESIGN/ROADMAP status flips ripple beyond git and need re-application by hand | Yes, and the script already contains the check: `shirabe validate --lifecycle-chain {{PLAN_DOC}} --mode=ready` (exit 0 = terminal reached), re-run as an independent gate | Yes by design — the pre-cascade probe detects "already at terminal" and emits `cascade_status: skipped` as a no-op | action-output-on-failure — this is the single highest-consequence write in either template (PLAN deletion, ROADMAP deletion, push), and today a failure surfaces only as an opaque gate exit code; separately, the script's own verbosity needs checking against the still-open 64KB pipe-drain defect before any action wraps it | **converted-after-plumbing** — gate exists conceptually today, but per the amended principle's own text ("irreversible outward-facing steps wait for failure output to reach the agent"), this is exactly that case |
| 5 | `execute.md:605` `gh pr ready $(gh pr list --head $(git rev-parse --abbrev-ref HEAD) --json number --jq '.[0].number')` | `plan_completion` (same state as #4) | Yes, trivially — `gh pr ready --undo` flips it back, no data loss | Yes: `gh pr view "$PR_NUM" --json isDraft --jq '.isDraft == false'` | Yes — marking ready twice is a no-op | nothing on its own merits | **converted-now on its own merits, blocked in practice** — bundled into the same `default_action` as #4, so it inherits #4's wait unless split out (see Implications) |
| 6 | `SKILL.md:336` `gh pr edit` (coordination-body re-author, coordinated-path loop step 2) | none — outside any koto state; this loop runs in SKILL.md prose, never through `koto next` | Yes, content-wise | Yes structurally — the SKILL already requires `shirabe validate --coordination-body <file>` on every write before this line fires | Yes — rewritten in full every pass | a koto template that governs the coordinated multi-repo loop at all; none exists | **stays agent-run** — not a capability gap, a structural one: no per-state `default_action` can reach a command that no koto state machine executes |
| 7 | `SKILL.md:371` `gh pr close` (abandonment path) | Same structural situation as #6 | Technically yes (`gh pr reopen`), but reopening after a deliberate abandonment defeats the point | N/A — the state has no koto state to attach a gate to | N/A | Same as #6, plus the trigger itself is an operator judgment call ("elects to abandon rather than resolve") | **stays agent-run** — structural (#6's reason) *and* judgment-gated (the decision to abandon isn't mechanical) |
| 8 | `phase-6-pr.md:20` `git push -u origin <branch>` | `pr_creation` (`work-on.md`) | Yes, cheaply — own feature branch, nobody else depends on it pre-PR | Yes: `git ls-remote --exit-code --heads origin "$(git rev-parse --abbrev-ref HEAD)" >/dev/null 2>&1` | Yes — re-pushing an already-pushed branch is a fast-forward no-op | nothing structurally — no template var needed either, `git rev-parse --abbrev-ref HEAD` self-describes the branch exactly as `execute.md:395`'s pattern does | **converted-now** — not previously broken out as its own site in the prior `/work-on` map (see Surprises) |
| 9 | `phase-6-pr.md:23` `git push --force-with-lease` (after rebase) | `pr_creation` | More sensitive than #8 — rewrites remote history, though `--force-with-lease` refuses a blind overwrite if the remote moved | Yes for "did it land": `[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/$(git rev-parse --abbrev-ref HEAD))" ]` — but that gate can't distinguish *why* a failed attempt failed (stale lease vs. auth vs. branch protection) | Mostly — safe to re-run with nothing new to push; not safe to retry blindly on a lease failure without re-fetching | action-output-on-failure specifically — the raw stderr is what tells the agent which of three different recoveries applies, and guessing wrong compounds a bad state exactly the way the maps flagged for `worktree_discipline_check`'s rebase | **converted-after-plumbing** |
| 10 | `phase-6-pr.md` "Create PR" section — no literal `gh pr create` line exists; prose-only, mechanical title / judgment Part-2 per `pr-body-conformance.md` | `pr_creation` | Yes (`gh pr close`) | Structure only, same shape as #3 | Not guarded today — unlike `execute.md:397`, this prose has no `gh pr list --head ... \| grep -q . \|\|` idempotency check before creating; worth adding regardless of conversion | Same bidirectional evidence-consuming capability as #3 | **converted-after-plumbing** for the apply step; title/Part-2 authoring stays agent-run permanently, same reasoning as #3 |

### Restated yields under the amended principle

**The honest finding is that the numbers barely move — what moves is which numbers are endorsed.** `r2_lead-map-execute.md`'s Wave A (today, zero koto changes, 8/53 ≈ 15%) already included rows #1 and #2 above — the map counted them as technically convertible before the ruling existed. What blocked them wasn't the map; it was `r3_lead-middle-path.md`'s *separate* read-only restriction, which found that applying it line-by-line to Wave A "zeroes out `/execute`'s entire current conversion yield." That restriction is exactly what the Author Ruling struck. So:

- **Today**: `/execute` stays at **15% (8/53)** numerically, but rows #1-#2 move from "technically Wave A, recommended against" to "converted-now, fully endorsed" — the practical, actionable yield for the writes-remote bucket specifically goes from **0 of 2 candidates** to **2 of 2**.
- **After the pipe-drain/migration-warning defect fix**: **62% (33/53)**, unchanged — Wave B was never about permission.
- **After action-output-on-failure reaches the agent** (row #4's need, and #9's for `/work-on`): this is the horizon that newly opens under the amended principle, distinct from Wave C's evidence-consuming-action need. Adding row #4 (`plan_completion`'s cascade) doesn't change the 70%/37-53 Wave C figure — it was already counted there — but it reclassifies *why* it's waiting: not "needs a feature," but "needs the specific diagnosability the amended principle names as the gate for irreversible outward-facing steps."
- **Ceiling**: **79%**, unchanged (11 SKILL.md commands, including rows #6-#7, structurally unreachable by any state action).

**`/work-on` moves by a small, genuine amount, not from the ruling but from finer-grained inventory.** `r2_lead-map-work-on.md`'s Wave A (3/37 ≈ 8%) never broke out `phase-6-pr.md:20/23` as distinct sites — its `pr_creation` row mentions only the `gh pr list --head` existence check and `gh pr create`, not the two `git push` lines above them. Row #8 is a clean, zero-dependency **converted-now** candidate this remap adds to the inventory: **4 of 37 ≈ 11%** today, up from 3. Row #9 adds one more to the after-plumbing horizon. Neither addition depends on the ruling — `git push -u origin <branch>` was never blocked by permission-bypass reasoning in the first place, it was simply never counted as its own row.

### Duplication and gaps flagged along the way

`pr_creation`'s prose (`phase-6-pr.md`, item #10) has no `gh pr list --head ... | grep -q . || gh pr create` idempotency guard, unlike `execute.md:397`'s block. That's worth fixing regardless of whether `gh pr create` itself ever converts — today a retried `pr_creation` state (the template's own `creation_failed_retry` path, up to 3 attempts) can create duplicate PRs on a transient failure that actually succeeded, since nothing checks for an existing PR first.

## Reconciliation with `r1_lead-confirmation.md`

The confirmation lead found `requires_confirmation` executes unconditionally and only
labels the result afterward — it fails issue #71's AC ("preventing auto-execution")
for the one case that motivates the feature, and recommends an authoring rule keeping
irreversible commands out of `default_action` entirely, citing `pr_creation` (agent
runs `gh pr create` itself, koto gate verifies) as the already-defensible pattern.
That conclusion is largely right, and it exposes a real gap in my original table:
I scored "reversible?" on one axis — can the local artifact be reset — and missed a
second, independent axis the confirmation lead's framing makes unavoidable: does the
action fire an externally-visible, un-un-fireable event (a notification, a consumed
identifier, third-party visibility) regardless of what happens afterward. `gh pr
close` undoes a `gh pr create`'s *state*; it does not undo the "opened" notification
every watcher already received. That second axis is what issue #71 and the
confirmation lead actually care about, and my table didn't carry it.

**1. What survives an authoring rule that bans irreversible-outward-facing commands
from `default_action`.** Re-scored on the notification axis, not just the state axis:

- **Survive as originally verdicted** — #1, #8 (`git push` to a branch you solely
  own, before any PR exists). No GitHub notification fires on a bare push to your own
  branch; nobody is watching a ref that isn't yet referenced by a PR. Genuinely
  reversible on both axes.
- **Reclassified to stays-agent-run — I concede these.** #2 (`execute.md:397`, `gh pr
  create --draft`) and #10 (`phase-6-pr.md`'s `gh pr create`) are exactly the
  confirmation lead's own named case. `gh pr create` fires a real, un-recallable
  notification and consumes a PR number the instant it succeeds — closing it after
  the fact (my original "reversible... `gh pr close` undoes it at near-zero cost")
  undoes the *state*, not the *event*. Worse: even with the action-output-on-failure
  plumbing I asked for elsewhere in this bucket, converting `gh pr create` to a
  `default_action` still means koto fires the irreversible event *silently, on the
  happy path, with zero checkpoint* — output-on-failure only helps when the command
  fails; it does nothing for the success path, which is exactly where the
  unrecoverable event lives. That's the same critique the confirmation lead levied
  against `requires_confirmation` itself, and it applies with full force to my own
  "converted-after-plumbing" framing for these two rows. I retract it for both.
- **Also reclassified — #5 (`execute.md:605`, `gh pr ready`).** I originally called
  this "converted-now on its own merits" on the state-reversibility axis alone
  (`gh pr ready --undo` flips it back). Under the notification axis it belongs with
  #2/#10, not with #1/#8: marking a PR ready fires a "ready for review" notification
  to reviewers the instant it succeeds, and undoing the draft state afterward doesn't
  unfire it. I'm moving this to stays-agent-run-or-equivalent-caution, same reasoning
  as #2/#10, not the clean converted-now I gave it originally.
- **Structural rows (#6, #7) are reinforced, not contradicted.** `SKILL.md`'s
  coordination `gh pr edit`/`gh pr close` already stayed agent-run for the structural
  reason (no koto state machine reaches them); `gh pr close` specifically is also a
  clean instance of the confirmation lead's irreversibility category, so the two
  reasons agree rather than compete here.
- **Remaining candidates, with two points of genuine disagreement (not false
  consensus):**
  - #4 (`plan_completion`'s cascade push) and #9 (`force-with-lease`) I'm keeping in
    **converted-after-plumbing**, not sweeping into stays-agent-run, and I want to be
    explicit this is a real disagreement rather than an oversight. Both push commits
    to an *existing* branch with an *already-open* PR — the "opened" notification
    already fired earlier in the run (at #2, or at #8-adjacent creation). A `git
    push` of new commits does trigger a quieter "synchronize" event, not the loud
    "opened"/"ready for review" one #2/#5 fire, so I don't think they belong in the
    same bucket as a fresh `gh pr create`. #9 additionally has a structural guard
    `gh pr create` lacks entirely: `--force-with-lease` fails atomically and
    *before* any history is rewritten if the remote moved — it's a command that can
    refuse itself, not one where every successful invocation is definitionally the
    irreversible event. `gh pr create` has no equivalent self-refusal; the instant it
    returns 0, the notification is already sent. I'd keep #4/#9 on the
    action-output-on-failure path and treat #2/#5/#10 as the ones needing the
    authoring-rule ban — the confirmation lead's own document doesn't address either
    push case, so this is genuinely unadjudicated ground, not a place we've already
    disagreed and I'm re-litigating it.
  - #3 (`execute.md:555`, `gh pr edit` on the run's *own* draft PR, opened earlier in
    the same run) — I'm keeping this closer to reversible than #2/#5/#10. Editing a
    PR you just created doesn't consume a new identifier, and GitHub's notification
    behavior for edits is materially quieter than for opens or ready-for-review (an
    edited title/body doesn't push-notify watchers the way those two do). I'll flag
    this as contestable rather than settled — the confirmation lead didn't evaluate
    it specifically, and "quieter notification" is a judgment call, not a bright
    line the way "no notification at all" (git push, pre-PR) or "loud notification
    every time" (`gh pr create`) are.

**2. Is action-output-on-failure enough on its own for irreversible steps, now that
the permission argument is gone?** No — and the confirmation lead's own argument is
why. For `gh pr create`/`gh pr ready` specifically, action-output-on-failure fixes
*diagnosability of a failed attempt*; it does nothing about the *success* path, where
the irreversible, externally-visible event has already happened with no checkpoint at
all — silently, the instant `koto next` returns. That's structurally identical to what
the confirmation lead found wrong with `requires_confirmation`: a post-hoc signal
cannot gate an event that has already occurred. For the commands where I *am* still
recommending the plumbing-then-convert path (#4, #9), the reasoning is different and
survives: their "irreversibility," to the extent it exists, is bounded and
correctable *after a successful run* (revert-and-repush, a lease that refuses instead
of clobbering) — so what's actually missing for those two isn't a pre-execution
checkpoint, it's clean diagnosis of a *failed* attempt, which action-output-on-failure
does supply. The distinction that matters isn't "does this feature exist yet," it's
"does this command's risk live in a bad success or a bad failure" — `gh pr
create`/`gh pr ready`'s risk lives entirely in a bad (or premature) *success*; #4/#9's
risk lives in an *undiagnosed failure*. Only the second is fixed by output-on-failure.

**3. A correction to the confirmation lead's `pr_creation` example, in support of its
own recommendation.** The lead calls `pr_creation`'s agent-runs-it-with-a-gate pattern
"genuinely defensible." I checked `work-on.md:695` directly for this remap: `pr_creation`
has **no `gates:` block at all** — `pr_creation:` goes straight to `accepts:`, and the
state's only truth is the agent's own self-reported `pr_status` enum
(`created`/`shared`/`creation_failed_retry`/`creation_failed_escalate`). There is no
koto gate independently confirming the PR exists, matching what `r2_lead-map-work-on.md`
already flagged in its own per-state table. This doesn't undercut the recommendation —
keeping `gh pr create` agent-run is still right, for the reasons in point 2 — but the
pattern isn't *yet* the "gate verifying the result" the lead describes; it's currently
agent self-report only. Adding that gate (e.g. `gh pr list --head "$(git
rev-parse --abbrev-ref HEAD)" --json number --jq '.[0].number' | grep -q .`, the same
idiom `execute.md:397`'s own guard already uses) is a small, concrete piece of work
that would make the "genuinely defensible" claim fully true rather than partly true,
and it's cheap regardless of anything else in either document.

### Direct answers to the follow-up (apply the rule, accept/reject it, test durability)

**1. Applying "no `default_action` on irreversible commands" to all ten rows, with counts against the original.**

| # | command | verdict before this exploration | verdict under the blanket rule, read narrowly (my proposed cut, see #2) | verdict under the blanket rule, read maximally (any writes-remote mutation to a shared/external system counts) |
|---|---|---|---|---|
| 1 | `git push` pre-PR (`execute.md:395`) | converted-now | converted-now | converted-now — no external visibility exists at all, no reading of "outward-facing" catches it |
| 2 | `gh pr create` (`execute.md:397`) | converted-now | **stays-agent-run** | stays-agent-run |
| 3 | `gh pr edit` own draft (`execute.md:555`) | converted-after-plumbing | converted-after-plumbing (contestable) | stays-agent-run |
| 4 | cascade push (`execute.md:596`) | converted-after-plumbing | converted-after-plumbing | stays-agent-run |
| 5 | `gh pr ready` (`execute.md:605`) | converted-now | **stays-agent-run** | stays-agent-run |
| 6 | `SKILL.md` coordination `gh pr edit` | stays-agent-run (structural) | stays-agent-run | stays-agent-run |
| 7 | `SKILL.md` coordination `gh pr close` | stays-agent-run (structural) | stays-agent-run | stays-agent-run |
| 8 | `git push` pre-PR (`phase-6-pr.md:20`) | converted-now | converted-now | converted-now |
| 9 | `force-with-lease` (`phase-6-pr.md:23`) | converted-after-plumbing | converted-after-plumbing | stays-agent-run |
| 10 | `gh pr create` (work-on `pr_creation`) | converted-after-plumbing | **stays-agent-run** | stays-agent-run |

**Counts, out of 10**: before this exploration — 4 converted-now, 4 converted-after-plumbing, 2 stays-agent-run. Under my narrower cut — 2 converted-now, 3 converted-after-plumbing, 5 stays-agent-run. Under the maximal reading — 2 converted-now, 0 converted-after-plumbing, 8 stays-agent-run. The three flips (#2, #5, #10) are the ones I already conceded in the section above; #3, #4, #9 are where the narrow and maximal readings disagree, which is exactly the joint question in #2 below.

**2. Do I accept the rule as stated? No — not without sharpening it, and here's the one-sentence version I'd write instead.** "No `default_action` on irreversible commands" is directionally right but `irreversible` is undefined, and a maximal reading sweeps in #4 and #9, which I don't think belong with #2/#5/#10. The mechanism difference: #2/#5/#10 are commands whose *successful exit is itself* the unrecoverable event (a PR is created, a ready-for-review notification fires) — no gate, no later action, nothing downstream un-happens that. #4 and #9 are commands whose only irreversibility is bounded and correctable *after* a successful run (revert-and-repush; a lease that refuses atomically before rewriting anything rather than clobbering blind) — their real failure mode is a *silent bad run*, not an *unrecoverable good one*. I'd write the rule a template author could apply as:

> Keep `default_action` off any command whose successful exit is itself the irreversible, externally-visible event (creating, publishing, or closing a PR; posting a comment; marking ready for review) — but allow it for a command whose only irreversibility is bounded and repairable after a successful run, because those need better failure diagnosis, not a pre-execution veto.

That cuts at "does success itself create the harm" rather than at "does this command touch something remote," which is the joint the blanket rule as stated doesn't name.

**3. Does the sibling's "unrecoverable and undiagnosable" argument survive once action output reaches the failure path — staged constraint or durable rule?** The answer splits cleanly along the same line as #2, and this is the sharpest thing this reconciliation produced: **it depends on which half of "unrecoverable and undiagnosable" is actually doing the work for a given command.**

- For #2/#5/#10 (`gh pr create`, `gh pr ready`): action-output-on-failure fixes *diagnosability of a failed attempt*. It does nothing for a *successful* one, and these commands' entire danger is in an unreviewed success — the notification fires, the PR exists, the instant `koto next` returns 0. So the constraint on these three is **durable, not staged**. No amount of failure-path plumbing changes it, because the plumbing addresses a failure mode these commands don't primarily have. This matches the sibling's own conclusion and I'm not disagreeing with it for these three.
- For #4/#9 (cascade push, force-with-lease): here the actual risk *is* "a failure I can't currently diagnose" — an opaque gate exit code with the real git/script error discarded. There's no separate success-side checkpoint problem the way there is for `gh pr create`: a push landing exactly as intended *is* the desired outcome, full stop, with no distinct "and also this fired an unrecoverable notification regardless of the content" clause riding along. So the constraint on these two is **staged, not durable** — action-output-on-failure is the specific, sufficient fix, and once it lands the case for keeping them agent-run goes away.

So: durable for three of the ten rows, staged for two, and the deciding question for any future row is "does this command's risk live in a bad success or a bad failure" — not "is it labeled writes-remote" and not "is it labeled irreversible" without further qualification.

**4. Where the disagreement is, plainly, and what would settle it.** I disagree with reading the rule to also catch #4/#9, on the mechanism argument in #2/#3 above. This rests on one empirical claim I have not verified against either document or against GitHub's actual notification behavior: that a `synchronize` event (new commits pushed to an already-open PR) is meaningfully quieter, notification-wise, than an `opened` or `ready_for_review` event. If that's wrong — if GitHub notifies PR watchers just as prominently for new commits as for opens — my case for keeping #4 out of the ban weakens a lot, and I'd move it to stays-agent-run alongside #2/#5/#10, converging fully with the sibling's document. `force-with-lease` (#9) I'd defend even then, since its argument doesn't depend on notification volume at all — it rests on the lease's atomic self-refusal, a property `gh pr create` genuinely lacks. That's the concrete thing that would settle it: whichever of us checks GitHub's actual notification tiering for `synchronize` vs. `opened`/`ready_for_review` events resolves #4; #9 stays a live disagreement regardless, since neither document has evidence on it yet.

## Implications

**Superseded by the Reconciliation section above.** This paragraph originally argued row #5 (`gh pr ready`) was the clearest case for splitting `plan_completion` into a converts-now `mark_ready` state and a plumbing-gated `run_cascade` state. After reconciling with `r1_lead-confirmation.md`, row #5 itself moved to stays-agent-run-or-equivalent-caution — it fires an un-un-fireable "ready for review" notification on success, the same category as `gh pr create`, not the git-push category I'd compared it to. The state-split argument still holds as a general lesson (bundling steps of different risk levels inside one `default_action` blocks the safe one), but `plan_completion` is no longer the example: row #4 (the cascade) is the only mechanical step left in that state, and it isn't paired with anything else in this bucket bar to safely split off. I'm leaving the original paragraph's reasoning below for the record rather than deleting it, since the *pattern* — a state split freeing a safe step from a risky sibling — remains a real, general finding even though this specific instance doesn't.

Original argument (state-split pattern, instance now retracted): `gh pr ready` is trivially reversible, cheaply gate-verifiable, and idempotent — it clears every bar for **converted-now** standing entirely on its own — but it's welded into the same `default_action` as row #4's cascade, which is the single highest-consequence write in the whole bucket and genuinely needs the diagnosability plumbing first. Splitting `plan_completion` into a `run_cascade` state (waits on plumbing) and a `mark_ready` state (converts today) would let the safe half ship immediately instead of being held hostage by the risky half it happens to share a YAML block with. This is the same state-granularity lesson `r3_lead-middle-path.md` already drew for `setup_issue_backed`/`setup_free_form` — bundling judgment with mechanical work inside one state blocks the mechanical part — applied here to two *mechanical* steps of very different risk levels bundled together instead.

The two SKILL.md rows (#6, #7) are a different kind of "stays agent-run" than anything else in the bucket: rows #3/#4/#9 wait on a koto capability or on the pipe-drain/diagnosability plumbing, both on a plausible roadmap; #2/#5/#10 now also stay agent-run, but for the confirmation lead's irreversibility reason, not a missing capability — no amount of koto engineering unblocks them short of a real confirm-before-execute primitive, which the confirmation lead separately argued isn't worth building for the one case that needs it. #6/#7 wait on neither: the coordinated multi-repo loop that contains them isn't a koto template at all, it's prose iteration in `SKILL.md`. Three different reasons now converge on "stays agent-run" in this bucket — a capability gap, an irreversibility ban, and a structural gap — and it matters which is which, because only the first two have any roadmap at all.

## Surprises

The permission ruling's practical effect on the *numbers* is smaller than its effect on the *recommendation*. Going in, I expected striking the permission leg to visibly widen the map's yield percentages; instead the map's percentages were already computed on pure technical-reachability grounds in round 2, and the thing the ruling actually reverses is round 3's separate read-only-restriction recommendation, which had proposed shrinking the *endorsed* subset of that same map back down to near zero for `/execute`. The numbers didn't need to move because they were never built on the leg that got struck — but without checking `r3_lead-middle-path.md`'s own line-by-line application of its restriction against `r2_lead-map-execute.md`'s Wave A, it would have been easy to report a number that moved when the real change is upstream of the number, in which recommendation gets to keep it.

`phase-6-pr.md`'s two `git push` lines being absent from `r2_lead-map-work-on.md`'s per-state table at all — not deferred, not classified, just not mentioned as their own rows — is a small inventory gap in a document that was otherwise state-by-state exhaustive. It didn't come from anything about writes-remote commands being harder to spot; the table's `pr_creation` row simply summarized "current ask" as the `gh pr list` check plus `gh pr create` and skipped the two lines above them in the source file.

## Open Questions

- Whether `shirabe validate --pr-body` (cited by both templates as the CI enforcement mechanism for row #3/#10's title/body structure) actually accepts a live PR number or requires a file — if it's file-only, the structural gate I wrote for #3 needs adjustment to fetch-then-check rather than checking `gh pr view` output directly.
- Whether `run-cascade.sh --push`'s typical stdout+stderr volume has actually been measured against the 64KB pipe-drain ceiling anywhere — every prior lead flagged this as open and I didn't find a measurement in any of the four source documents I read for this pass.
- Whether splitting `plan_completion` (row #4/#5's fix) is worth doing before or after the action-output-on-failure plumbing lands — splitting first ships row #5 immediately; splitting after means only one migration instead of two, at the cost of leaving a known-easy win on the table in the meantime.

## Summary
Revised after reconciling with `r1_lead-confirmation.md`: one of the ten writes-remote commands (`orchestrator_setup`'s branch push, `execute.md:395`, plus its `/work-on` twin `phase-6-pr.md:20`) survives cleanly as converted-now — no notification fires on a push to a branch nobody's watching yet, and it's genuinely reversible on both the state axis and the event axis. Three commands (`gh pr create` ×2, `gh pr ready`) that I originally scored as converted-now or converted-after-plumbing don't survive the confirmation lead's irreversibility distinction and I'm conceding that: each fires an externally-visible, un-un-fireable notification the instant it succeeds, and action-output-on-failure — the plumbing I'd pointed them at — only helps diagnose a bad *failure*, not a premature *success*, which is where all three commands' real risk lives. They join `pr_creation`'s existing pattern: agent-run through its own tool layer, koto gate verifying the outcome afterward — though that gate doesn't exist yet in `work-on.md` today (confirmed: `pr_creation` has no `gates:` block, line 695), so the "genuinely defensible" claim needs that small addition to be fully true. Two commands (`plan_completion`'s cascade push, `force-with-lease`) I'm keeping on the plumbing-then-convert path rather than sweeping them in with `gh pr create` — both push to an already-open PR rather than firing an "opened"/"ready" event, and `force-with-lease` specifically has a self-refusal guard `gh pr create` lacks entirely — a genuine, flagged disagreement rather than an oversight. `gh pr edit` on the run's own just-created draft is a contestable middle case I'm leaving in the after-plumbing bucket on the strength of GitHub's quieter edit-notification behavior, not a settled call. The two `SKILL.md` coordination commands stay agent-run for a structural reason (no koto state machine reaches them) that the confirmation lead's irreversibility framing reinforces rather than competes with.
