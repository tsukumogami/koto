# Verification: where a session came from

This records the reference-host check of the dispatcher-origin work: Issue 5
of `PLAN-koto-session-dispatcher-origin` (coordination pull request #687). It
was run after koto v0.38.0 (Issues 1 to 3, #715) and the shirabe change that
drops the manual dispatcher link step from worker briefs (shirabe#2117) were
both released and installed.

None of the three checks passes as the PLAN words it. The origin record
itself works: every dispatched worker's anchor reported the dispatching
coordinator's anchor, on the cloud store, with nothing written by the agent.
The checks fail on how the runs were set up and on what the PLAN assumed
about the briefs, and each failure is explained below.

## Host and versions

- koto 0.41.0 (`koto version`: `d54c489 2026-11-25T23:45:13Z`), with its
  koto-skills plugin, on the cloud session backend (`backend = "cloud"`).
- shirabe 0.56.0, whose tree contains shirabe#2117's merge commit `a699db22`.
- The launcher is niwa 0.45.0. Workers were dispatched through shirabe's
  coordinate `dispatch-worker.sh` on plugin root 0.56.0, which runs
  `niwa dispatch`, the same launch the baseline runs used (#716).

## The worker task

All ten runs are the same task as the three baseline runs on #716: a
`/shirabe:explore --auto` worker in this repository that reports by message,
asked the same fixed question:

> Which documents under docs/reference/ in this repository describe the
> session feed, and does each one name the koto version its contract applies
> to? List each document with a one-line answer, from the documents
> themselves.

Run N appended `-after-N` to the topic it derived, because the baseline runs'
sessions still held the derived names (#716 records the collision this
avoids). Each run stopped once its findings were written in its own session,
with no branch, commit or pull request, and the runs went one at a time.

## AC1: ten dispatched workers report the coordinator

**Result: not met as worded.** The anchor half holds in all ten runs. The
runs came from four coordinator runs, not one, and the workflow-session half
reads the coordinator's anchor only in run 1.

`koto session origin` on each worker's anchor (`anchor-<its session id>`) and
on its workflow session, read before each worker was torn down:

| Run | Anchor origin | Workflow session origin |
|-----|---------------|-------------------------|
| 1 | dispatched, `anchor-faaf6575-...` (anchor) | dispatched, `anchor-faaf6575-...` (anchor), via its anchor |
| 2 | dispatched, `anchor-de734f94-...` (anchor) | dispatched, coordinator run `...T051222Z` (workflow) |
| 3 | dispatched, `anchor-de734f94-...` (anchor) | dispatched, coordinator run `...T051222Z` (workflow) |
| 4 | dispatched, `anchor-9943f9ad-...` (anchor) | dispatched, coordinator run `...T063556Z` (workflow) |
| 5 | dispatched, `anchor-9943f9ad-...` (anchor) | dispatched, coordinator run `...T063556Z` (workflow) |
| 6 | dispatched, `anchor-9943f9ad-...` (anchor) | dispatched, coordinator run `...T063556Z` (workflow) |
| 7 | dispatched, `anchor-9943f9ad-...` (anchor) | dispatched, coordinator run `...T063556Z` (workflow) |
| 8 | dispatched, `anchor-ac7f093f-...` (anchor) | dispatched, coordinator run `...T082636Z` (workflow) |
| 9 | dispatched, `anchor-ac7f093f-...` (anchor) | dispatched, coordinator run `...T082636Z` (workflow) |
| 10 | dispatched, `anchor-ac7f093f-...` (anchor) | dispatched, coordinator run `...T082636Z` (workflow) |

Each anchor names the anchor of the coordinator session that dispatched it.
The coordinator session changed three times during the series, which is why
four anchors appear. The criterion asks for ten runs in a row from one
coordinator, and these runs don't meet that.

The workflow-session column differs because of the brief, not koto. A
`/shirabe:explore` worker's brief still tells it to open its session with
`--dispatched-by <coordinator run>` (see AC8 below). Runs 2 to 10 did, so
their workflow session carries a dispatched-by link of its own, and the verb
reports it first, as Issue 2 specifies (`dispatcher_kind: workflow`). Run 1
opened its session without the flag. Its workflow session has no link of its
own, so the verb followed the held-by link to its anchor and reported the
coordinator's anchor, which is the path the PLAN expected for every run.

## AC3: no launcher

**Result: not checked.** It needs a machine with koto and shirabe and no
launcher. Neither available host qualifies: both have niwa on `PATH`. The
koto-skills plugin is also installed per workspace instance on the reference
host, not for the user, so a session started from a plain directory there
gets no start hook at all. Removing niwa or installing the plugin for the user
would change the host, so neither was done. The check needs a host set up for
it.

## AC7 and AC8: cost and the absence of the link step

**Result: not met.** The cost is inside the baseline range. The call count
isn't the same, and every transcript contains the searched terms.

Counting follows #716. A koto call is a Bash tool call in the worker's main
transcript that runs koto directly or through a shirabe script that wraps it
(`skill-session.sh`, `explore-state.sh`, `resolve-coordinator.sh`,
`render-result.sh` among them), and a heredoc body is not a call. Tokens are
those of the assistant messages that issued the calls: (a) input plus output,
(b) also cache creation and cache read. Applied to the three baseline
transcripts, this count reproduces #716's call counts and token figures.

| Run | koto calls | Failed | (a) | (b) |
|-----|-----------|--------|-----|-----|
| Baseline 1 | 8 | 0 | 8130 | 673288 |
| Baseline 2 | 9 | 1 | 5632 | 832735 |
| Baseline 3 | 8 | 1 | 5701 | 751988 |
| After 1 | 7 | 1 | 5919 | 687601 |
| After 2 | 10 | 0 | 6430 | 1037913 |
| After 3 | 6 | 0 | 5093 | 598404 |

Medians against the baseline range: (a) 5919 against 5632 to 8130, and (b)
687601 against 673288 to 832735, so both are inside. The call counts (7, 10,
6 against 8, 9, 8) are not the same. Run 1 counts as an after run here. It
followed its brief except that it dropped `--dispatched-by` from its open, and
swapping in run 4 (9 calls) doesn't make the counts match. Across all ten
after runs the count ranges from 6 to 13, so it isn't stable from run to run
for this task. The baseline counts ranged from 8 to 9.

The search fails in all ten transcripts. A case-insensitive search for
`dispatched-by`, `link-dispatcher` and `dispatcher_session` finds each brief's
dispatcher block ("Your dispatcher is the coordinator run that sent you this
brief", a `dispatcher_session:` line, and the open command with
`--dispatched-by`). In runs 2 to 10 it also finds the worker running that
command. This is shirabe#2117 working as designed. It removed the link step
for workers whose entry point is `/scope`, `/deliver`, `/execute` or
`/work-on`, and kept the open row for `/explore`, `/decision` and `/release`,
whose `--dispatched-by` open is their own open command. The PLAN's AC7 and
AC8 reuse the baseline's `/explore` task, whose brief Issue 4 leaves
unchanged, so this task can't show the link step gone.

## What would close the checks

- AC1: ten runs from one coordinator session with no succession in between,
  with a worker whose brief has no `--dispatched-by` open.
- AC3: a host with koto, shirabe and the koto-skills plugin installed for the
  user, and no launcher on `PATH`.
- AC7 and AC8: a worker task on an entry point whose link step shirabe#2117
  removed, such as `/shirabe:work-on`, with new baseline runs. The old
  baselines can't be retaken, because they need the koto from before v0.38.0.
  If the call count stays unstable, the criterion could compare a range
  rather than an exact count.
