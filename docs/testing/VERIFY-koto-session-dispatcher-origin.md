# Verification: where a session came from

This records the check of the dispatcher-origin work on the reference host,
the maintainer's machine where the baseline runs in koto#716 were taken: Issue 5
of `PLAN-koto-session-dispatcher-origin` (coordination pull request koto#687).
It was run after koto v0.38.0 (Issues 1 to 3, koto#715) and Issue 4, the
shirabe change that drops the manual dispatcher link step from worker briefs
(shirabe#2117), were both released and installed.

Issue 5 has three checks: AC1, AC3, and AC7 with AC8, which share their runs.
Two of them fail as the PLAN words them, and AC3 wasn't run. The anchor's
origin record works on the dispatched path: every dispatched worker's anchor
reported the dispatching coordinator's anchor on the cloud store, and the
agent wrote nothing to make the anchor say so. The failures come from how the
runs were set up and from what the PLAN assumed about the briefs. Each one
is explained below. The rerun koto#766 asked for is in
[VERIFY-koto-session-dispatcher-origin-rerun.md](VERIFY-koto-session-dispatcher-origin-rerun.md).

## Host and versions

- koto 0.41.0 (`koto version`: `d54c489 2026-11-25T23:45:13Z`), with its
  koto-skills plugin, on the cloud session backend (`backend = "cloud"`).
- shirabe 0.56.0, whose tree contains shirabe#2117's merge commit `a699db22`.
- The launcher is niwa 0.45.0. Workers were dispatched through the
  dispatch-worker script of shirabe's coordinate skill, from the shirabe
  0.56.0 install. The script renders the worker's brief and runs
  `niwa dispatch`, the same launch the baseline runs used (koto#716).

## The worker task

All ten runs are the same task as the three baseline runs on koto#716: a
`/shirabe:explore --auto` worker in this repository that reports by message,
asked the same fixed question:

> Which documents under docs/reference/ in this repository describe the
> session feed, and does each one name the koto version its contract applies
> to? List each document with a one-line answer, from the documents
> themselves.

Run N appended `-after-N` to the topic it derived, because the baseline runs'
sessions still held the derived names (koto#716 records the collision this
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
| 1 | dispatched, `anchor-faaf6575-...` (anchor) | dispatched, `anchor-faaf6575-...` (anchor), via its anchor (its coordinator run was `...T041022Z`) |
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
The series took about four hours. In that time the coordinator handed over
to a successor session three times, and each successor opened a coordinator
run of its own. That is why four anchors and four coordinator runs appear.
The criterion asks for ten runs in a row from one coordinator, and these
runs don't meet that.

The workflow-session column differs because of the brief, not koto. A
`/shirabe:explore` worker's brief still tells it to open its session with
`--dispatched-by <coordinator run>` (see AC8 below). Runs 2 to 10 did, so
their workflow session carries a dispatched-by link of its own, and the verb
reports it first, as Issue 2 specifies (`dispatcher_kind: workflow`). Run 1
opened its session without the flag. Its brief said the run was an
after-change run, and the worker read that as overriding the open command.
It said so in its report and asked whether that was right. Its workflow
session has no link of its own. So the verb followed the held-by link, the
link koto writes from a workflow session to the anchor that held it when it
was opened, and reported the coordinator's anchor. That is the path the PLAN
expected for every run.

So the PLAN's checks conflict here. AC1 asks for the coordinator's anchor on
the workflow session, Issue 4 keeps the `--dispatched-by` open for `/explore`
workers, and Issue 2's precedence reports that link first. A worker that
follows the `/explore` brief can't meet AC1's workflow-session half.

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

Counting follows koto#716. A koto call is a Bash tool call in the worker's
main transcript that runs `koto` directly or through one of the shirabe
scripts that wrap it. In these runs those were the skill-session,
explore-state, resolve-coordinator and render-result scripts. A heredoc body
is not a call. A failed call is one whose tool result came back as an error,
and it counts toward the total. Tokens are those of the assistant messages
that issued the calls. The PLAN's cost measure is (a), input plus output.
(b) also adds cache creation and cache read, and is kept as context, as
koto#716 did. Applied to the three baseline transcripts, this count
reproduces koto#716's call counts and token figures. The one difference is
that koto#716 marks the refused open in baseline 2 as failed, and that tool
result was not an error, so the table shows no failed call for baseline 2.

Runs 1 to 3 are the AC7 runs, because they are the first three after the
change. Runs 4 to 10 were taken for AC1.

| Run | koto calls | Failed | (a) | (b) |
|-----|-----------|--------|-----|-----|
| Baseline 1 | 8 | 0 | 8130 | 673288 |
| Baseline 2 | 9 | 0 | 5632 | 832735 |
| Baseline 3 | 8 | 1 | 5701 | 751988 |
| After 1 | 7 | 1 | 5919 | 687601 |
| After 2 | 10 | 0 | 6430 | 1037913 |
| After 3 | 6 | 0 | 5093 | 598404 |

Medians against the baseline range: (a) 5919 against 5632 to 8130, and (b)
687601 against 673288 to 832735, so both are inside.

The PLAN asks for "the same count of agent koto calls". This record reads
that as each after run's count falling among the baseline counts, since the
baselines themselves differ (8, 9, 8). The counts 7, 10 and 6 fail that
reading, and also fail it compared run for run or by median (7 against 8).

Run 1 counts as an after run here, although it dropped `--dispatched-by`
from its open (see AC1). Leaving it out doesn't change the result. The ten
after runs made 7, 10, 6, 9, 10, 8, 8, 8, 7 and 13 calls, so runs 2 to 4
(10, 6, 9) fail the same reading too. For this task the count isn't stable from run
to run.

The search fails in all ten transcripts. A case-insensitive search for
`dispatched-by`, `link-dispatcher` and `dispatcher_session` finds each
brief's dispatcher block in the brief the worker read at its start. The
block reads "Your dispatcher is the coordinator run that sent you this
brief", then a `dispatcher_session:` line, then the open command with
`--dispatched-by`. In runs 2 to 10 the search also finds the worker running
that command. This is what shirabe#2117 specifies. It removed the link step
for workers whose entry point is `/scope`, `/deliver`, `/execute` or
`/work-on`. It kept the open row for `/explore`, `/decision` and
`/release`, whose `--dispatched-by` open is their own open command, and
shirabe's brief template still renders that row with the dispatcher line for
them. The PLAN's AC7 and AC8 reuse the baseline's `/explore` task, whose
brief Issue 4 leaves unchanged, so this task can't show the link step gone.

## What would close the checks

The rerun these call for is recorded in
[VERIFY-koto-session-dispatcher-origin-rerun.md](VERIFY-koto-session-dispatcher-origin-rerun.md)
(koto#766).

- AC1: ten runs from one coordinator session with no handover in between,
  using workers whose brief has no `--dispatched-by` open.
- AC3: a host with koto, shirabe and the koto-skills plugin installed for the
  user, and no launcher on `PATH`.
- AC7 and AC8: a worker task on an entry point whose link step shirabe#2117
  removed, such as `/shirabe:work-on`. It needs new baseline runs, taken with
  koto before v0.38.0 and shirabe before shirabe#2117. On the reference host
  that would mean replacing the installed versions, so they would be taken on
  a separate host or under a separate pinned install.
