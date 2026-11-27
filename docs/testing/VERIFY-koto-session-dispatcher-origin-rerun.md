# Verification rerun: where a session came from

This records the rerun of the four checks that
[the first verification](VERIFY-koto-session-dispatcher-origin.md) left
unsettled for Issue 5 of `PLAN-koto-session-dispatcher-origin`: AC1, AC3, and
AC7 with AC8 (koto#766). It was run from 2026-11-26 to 2026-11-27 (UTC) on the same reference host,
the maintainer's machine. Each check changed in the way that document's
closing section asked for:

- AC1 takes its ten dispatches from one coordinator session with no handover,
  and its workers run `/shirabe:work-on`, whose brief has no
  `--dispatched-by` open since shirabe#2117.
- AC7 and AC8 use a `/shirabe:work-on` task, with three new baseline runs
  taken under a separate pinned install: koto before v0.38.0 and shirabe
  before shirabe#2117, in an isolated scratch HOME.
- AC3 runs in a second isolated HOME on the reference host, with koto, shirabe
  and the koto-skills plugin installed for that HOME's user and no launcher on
  its `PATH`. It did not run on a separate machine.

The criteria are read as the PLAN words them on koto#687's branch,
docs/session-dispatcher-origin.

Thirteen worker runs carry the figures. The baselines, b1 to b3, ran before
the change. The probes, p1 to p10, are the ten AC1 dispatches. The first
three probes, p1 to p3, are also the AC7 and AC8 after runs, because they are
the first three after the change, as in the first record.

Results, each explained in its section below:

- **AC1: not met as worded.** The anchor half holds in all ten runs, and all
  ten came from one coordinator session. Every workflow session reports the
  coordinator's workflow run instead of its anchor. The cause is now koto,
  not the brief: under `--koto-leg`, koto writes the workflow session a
  dispatched-by link of its own, naming the session that sent the request.
- **AC3: met** on the reference host, in an isolated HOME with no launcher on
  its `PATH`.
- **AC7: not met.** The after runs made 21, 22 and 17 koto calls against 14
  in each baseline, and cost more. The two sides also differ in more than
  shirabe#2117, so this pair can't isolate that change's cost.
- **AC8: not met as worded, met within the scope stated below.** The raw
  search is not empty.
  Every hit in the three after transcripts is the loaded `/shirabe:work-on`
  skill text naming its open script's `--dispatched-by` option, or a copy of
  that text. Nothing comes from the brief or from a command the agent ran.

## Host and versions

| Runs | koto | shirabe CLI | shirabe plugin | koto-skills plugin | Where |
|------|------|-------------|----------------|--------------------|-------|
| Baselines b1 to b3 | 0.37.2 (dde9edb) | v0.55.0 | 0.55.0 | 0.37.2 | scratch HOME `$S`, local koto store |
| AC3 | 0.41.0 (d54c489) | v0.56.0 | 0.56.0 | 0.41.0 | scratch HOME `$S3`, local koto store, no niwa |
| Probes p1 to p7 | 0.41.0 | v0.56.2 | 0.56.2 | 0.41.0 | one niwa instance per probe, provisioned by the coordinator, cloud store |
| Probes p8 to p10 | 0.41.0 | v0.56.3 | 0.56.3 | 0.41.0 | same |

The probes ran the shirabe plugin the host had installed when each one
started. That was 0.56.2 for the first seven and 0.56.3 for the last three,
not the 0.56.0 the rerun was planned around. All ten ran Claude Code 2.1.332
and niwa 0.45.0. The plugin version is read from each transcript: the loaded
skill names its base directory,
`~/.claude/plugins/cache/shirabe/shirabe/<version>/skills/work-on`, and that
is the shirabe plugin root each probe ran from. The other two columns are
inferred rather than read, because teardown removed each probe instance's
own records. The host's shirabe CLI moved from v0.56.2 to v0.56.3 between
p7 and p8, at the same point as the plugin. Every niwa instance created on
the host during the series got koto-skills 0.41.0, whose root is
`~/.claude/plugins/cache/koto/koto-skills/0.41.0`.

The baselines' plugin roots are `$S/.claude/plugins/cache/koto766-scratch/shirabe/0.55.0`
and `$S/.claude/plugins/cache/koto766-scratch/koto-skills/0.37.2`. AC3's are
`$S3/.claude/plugins/cache/koto766-ac3/shirabe/0.56.0` and
`$S3/.claude/plugins/cache/koto766-ac3/koto-skills/0.41.0`.

koto 0.37.2 is the last release before v0.38.0, and shirabe v0.55.0 is the
last release without shirabe#2117. Both scratch HOMEs got their koto and
shirabe CLIs from `tsuku install` into their own `.tsuku`, and their two
plugins from a local marketplace built from copies of the plugin cache's
directories for those versions. The shirabe 0.55.0 copy is identical to the
cached one.

## The worker task

Baseline and after runs ran the same task, in the sandbox repository
tsukumogami/dms-verdict-sandbox, with only the label changing (`b1` to `b3`
for baselines, `p1` to `p10` for probes):

> Add the file notes/koto766-probe-LABEL.md to this repository, holding the
> one line: Probe LABEL for the koto#766 session origin rerun. Commit it on a
> new branch named chore/koto766-probe-LABEL and push that branch to origin.
> Open no pull request.

The task text holds none of the three search terms (koto#716's rule). Every
brief was rendered by shirabe's coordinate `render-brief.sh` from the same
input: entry point `work-on`, run mode `--auto`, review level light, and two
checkpoints. The baselines' briefs came from shirabe 0.55.0's script, and
the probes' from shirabe 0.56.0's, which the coordinator ran. The run stops
once its branch is pushed, with no pull request. The 0.55.0 brief carries the
dispatcher block and its link step, and the 0.56.0 brief doesn't, which is
the change shirabe#2117 made.

The coordinator launched each probe with shirabe 0.56.0's coordinate
`dispatch-worker.sh --session <coordinator run>`. That script runs
`niwa dispatch "<brief>" --name koto766-probe-pN --detach` with the lineage
and grant arguments it adds. The coordinator pointed it at a small wrapper
that adds `--profile worker` to `niwa dispatch` and passes every other niwa
call through. niwa then created the probe's instance and started the worker
with `--permission-mode bypassPermissions` (from the host's niwa
configuration) and with session messages accepted.

## Baselines

Each baseline was launched from its own clone of the sandbox repository
inside `$S`, one after another, with a cleaned environment:

```
env -i HOME="$S" TSUKU_HOME="$S/.tsuku" \
  PATH="$S/.tsuku/tools/current:$S/.tsuku/bin:$S/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  USER LOGNAME SHELL=/bin/bash TERM LANG TMPDIR TSUKU_NO_TELEMETRY=1 \
  GH_TOKEN ANTHROPIC_API_KEY \
  claude -p "<the 0.55.0 brief for bN>" --model opus \
    --permission-mode bypassPermissions \
    --output-format stream-json --verbose --include-hook-events
```

The two credentials are passed into the cleaned environment by name and
nothing else from the outer shell crosses over: no `KOTO_*`, `CLAUDE_*` or
niwa variable, and no koto cloud configuration, so every run's koto store is
`$S/.koto`. The brief's dispatcher is a stand-in coordinator run created in
that local store, so the link step has a session to link to.

| Run | Session | Branch head | Result |
|-----|---------|-------------|--------|
| b1 | `task_koto766-probe-b1` | 5a668db6 | exit 0, one commit holding only the note, no pull request |
| b2 | `task_koto766-probe-b2` | fec24102 | exit 0, same |
| b3 | `task_koto766-probe-b3` | b5a5c1b8 | exit 0, same |

No baseline run produced a subagent transcript. koto 0.37.2 has no
`koto session origin`, so the baselines carry no origin read.

## AC7: the same koto calls at the same cost

The count follows koto#716 and the first verification's AC7 section: a koto
call is a Bash tool call in the worker's main transcript that runs koto
directly or through a shirabe script that wraps it. A heredoc body is not a
call. A call whose tool result is an error counts and is marked failed.
Tokens are those of the assistant messages that issued the calls: (a) is
input plus output, the PLAN's measure, and (b) adds cache creation and cache
read as context.

The counter is the script the first verification used, which reproduces
koto#716's figures. It isn't in this repository, and the transcripts it read
aren't either, so the table below can't be recomputed from here. Its
built-in wrapper list predates the
`/work-on` scripts, so one wrapper set, applied to all six runs, adds every
script a run invoked that runs koto. From shirabe, that's `review-level.sh`
(`koto context exists`), `work-on-open.sh` (the session open) and
`check-branch-output.sh` (`koto context get`). The workers' own helpers are
added too. Several workers wrote a small script that writes the evidence
file and runs `koto next`, and then ticked through it: b3's `b3tick.sh`, and
the probes' `tick.sh` and `tickf.sh`. A call through such a helper runs koto
as surely as a direct one. Without them b3 would count 9 calls and p3 11.

| Run | koto calls | Failed | (a) input + output | (b) with cache |
|-----|-----------:|-------:|-------------------:|---------------:|
| b1 | 14 | 1 | 5435 | 907580 |
| b2 | 14 | 1 | 5364 | 910083 |
| b3 | 14 | 1 | 4903 | 945149 |
| p1 | 21 | 0 | 9107 | 2203334 |
| p2 | 22 | 4 | 8343 | 2586857 |
| p3 | 17 | 1 | 6847 | 1822251 |

Baselines: 14 calls in every run, so the range is 14 to 14 with median 14.
(a) runs from 4903 to 5435, median 5364. After runs: median 21 calls and
median (a) 8343. In each baseline the failed call is the one Bash command
that ran the link step and then `resolve-coordinator.sh`. The link exited 0,
and the resolve exited 1 because the stand-in coordinator run holds no run
record. p2's four failed calls come from its session refusing a tick from
the niwa worktree it had moved into (`execution_anchor_mismatch`), until it
rebound the session there. p4, p5 and p6 did the same. That behaviour is
tracked on shirabe#792.

**Result: not met.** The after runs make more koto calls than the
baselines, not the same number, and their median (a) is outside the
baseline range. This pair can't attribute the difference to shirabe#2117,
because the two sides differ in more than that change:

- p1 to p3 ran shirabe 0.56.2, which adds more `/work-on` states and
  checks than shirabe#2117 alone.
- The probes ran in niwa instances with the cloud store, and some of them in
  a worktree. The baselines ran in a plain clone with a local store.
- The probes' briefs came through the coordinator's dispatch. The baselines'
  briefs were rendered by hand from the same input.

Removing the link step doesn't remove a counted call here: in the baselines
it shares one Bash command with `resolve-coordinator.sh`, which the after
runs still make. So the raw gap stands: the after runs make 3 to 8 more
calls than 14. A comparison that isolates the change
would take baselines and after runs in the same kind of environment, with
shirabe releases on either side of a699db22 and nothing else between them.

## AC8: no trace of the link step

The search is case-insensitive, for `dispatched-by`, `link-dispatcher` and
`dispatcher_session`, over each probe's whole main transcript file, every
record included. No probe produced a subagent transcript. The raw result:

| Run | dispatched-by | link-dispatcher | dispatcher_session |
|-----|--------------:|----------------:|-------------------:|
| p1 | 4 | 0 | 0 |
| p2 | 4 | 0 | 0 |
| p3 | 4 | 0 | 0 |
| p4 to p7, p9, p10 | 4 each | 0 | 0 |
| p8 | 8 | 0 | 0 |

Where each hit comes from:

- **Skill text the harness loaded: 2 in every run.** When `/shirabe:work-on`
  starts, Claude Code writes the skill's whole `SKILL.md` into the
  transcript as a user record marked `isMeta`. That text names
  `--dispatched-by` twice: once in `work-on-open.sh`'s usage line and once
  in the description of what the option does. Every shirabe version since
  0.38.0 has both lines.
- **A copy of that skill text: 2 in every run.** The transcript also keeps
  one record of the API request body, and it carries the same two lines.
- **Tool output: 2 more in p8, plus their copy.** p8 listed the context keys
  of two other probes' sessions with `koto context list`. The output
  includes `koto/links/dispatched-by/<coordinator run>`, the key koto stores
  a dispatched-by link under (see AC1). The transcript repeats that tool
  result in its own copy, which makes four.
- **The brief, and commands or text from the agent: none in any run.** Each
  brief, as the coordinator recorded it at dispatch, matches none of the
  three terms.

The scope AC8 is judged by is what the brief and the agent add: the brief,
every command the agent ran, the agent's own text, and tool output. It
leaves out the skill text the harness loaded, and that text's copy. The
criterion is about the link step: whether the worker was told to link its
session to a dispatcher, or did. A worker can't run `/shirabe:work-on`
without loading the skill, and the skill's own reference to the option
says nothing about whether the link step ran. Within that scope the three
after transcripts, p1 to p3, find nothing. p8's tool-output hits fall inside
the scope, but p8 isn't one of the after runs. They come from a read of other
sessions' links, not from any link step.

**Result: not met as worded, met within the stated scope.** The PLAN asks
that the search find nothing, and the raw search finds 4 hits in each after
transcript, all of them the skill text or its copy.

## AC1: ten dispatched workers report the coordinator

**Result: not met as worded.** The anchor half holds in all ten runs, and
the series meets the PLAN's conditions. The workflow-session half reads the
coordinator's workflow run, not its anchor, in all ten.

One coordinator session made the ten dispatches, one at a time, from the
first probe's start at 2026-11-26 21:03Z to the last origin read at
2026-11-27 04:01Z. p1 ran from 21:03:36Z to 21:08:07Z. The coordinator reaped each probe before
dispatching the next. That was the status breadcrumbs coordinator, whose
anchor is `anchor-19188424-6743-4c42-9909-0ca5315f65fe` and whose
coordinator run is `coordinate-roadmap-shirabe-status-breadcrumbs-20261126T200307Z`.
No successor session took over between the first dispatch and the tenth:
every probe's anchor names that one anchor, and every workflow session names
that one run. Each probe's koto session directory holds the cloud store's
sync files, so the cloud store was in use. No brief matches any of the three
search terms, and so none carries a link step or a `--dispatched-by` open.
Each probe pushed `chore/koto766-probe-pN` in the sandbox repository, adding
only its note, and opened no pull request.

The coordinator ran `koto session origin` on each probe's anchor
(`anchor-<its session id>`) and its workflow session
(`task_koto766-probe-pN`), before that probe's reap:

| Run | Claude session | Branch head | Read at | Anchor origin | Workflow session origin |
|-----|----------------|-------------|---------|---------------|-------------------------|
| p1 | f5ce77d0 | 9b9f7105 | 11-26 21:08Z | dispatched, the coordinator's anchor (anchor) | dispatched, the coordinator run (workflow), via none |
| p2 | ffc71913 | 7c36cb92 | 11-26 21:31Z | same | same |
| p3 | a94b3d79 | 793070e7 | 11-26 22:15Z | same | same |
| p4 | 90b8f2b2 | ee634424 | 11-26 22:35Z | same | same |
| p5 | e26bf19d | 3d2ffa31 | 11-26 22:55Z | same | same |
| p6 | f381392a | b67fc803 | 11-26 23:26Z | same | same |
| p7 | 7888d003 | 7ebea4ec | 11-26 23:51Z | same | same |
| p8 | d58b5b88 | 9611ca73 | 11-27 02:59Z | same | same |
| p9 | 9f95ab83 | f8ee5411 | 11-27 03:28Z | same | same |
| p10 | c0380d51 | ad1c40c9 | 11-27 04:01Z | same | same |

In the table, "(anchor)" and "(workflow)" are the read's `dispatcher_kind`.
"via none" is its top-level `via` field: it is null when the read stops at
the session's own link and doesn't follow a held-by link to an anchor. That
field isn't the same as the `via` inside the link record described next.

This time the workflow sessions didn't get their link from the brief.
Each probe's workflow session holds a `koto/links/dispatched-by/<coordinator
run>` record that koto wrote itself while opening the session, before its
held-by link (in p1, two seconds after the session was created). The record says `"via":"leg"`. The
probes opened their sessions with `--koto-leg`, and koto recorded the
request's requester, which is the coordinator run, as the dispatcher. Issue 2's precedence reports a session's own dispatched-by link
first, so the read names the coordinator run with `dispatcher_kind:
workflow` and never follows the held-by link to the anchor.

Each workflow session still names the coordinator that dispatched it, by
its run instead of its anchor. AC1 asks for the anchor. A coordinator that
dispatches `/shirabe:work-on` workers with a request leg can't meet that
half while koto records the leg's requester this way. Whether the check or that
record should change is a question for the PLAN's owner.

## AC3: no launcher

`$S3` had koto 0.41.0 and shirabe v0.56.0 installed by `tsuku` into
`$S3/.tsuku`, and the koto-skills 0.41.0 and shirabe 0.56.0 plugins
installed for that HOME's user. Inside the cleaned environment
`command -v niwa` prints nothing, and its `PATH` holds no directory that
contains niwa. The session started from `$S3/plain2`, a fresh git repository
with one empty commit and nothing else, outside any workspace:

```
claude -p 'Run `/shirabe:explore "What files are in this directory, if any?" --auto`. ...' \
  --model opus --permission-mode bypassPermissions \
  --output-format stream-json --verbose --include-hook-events
```

The skill opened its session, and the agent ran the reads:

```
$ koto session origin explore-directory-contents
{"session":"explore-directory-contents","via":"anchor-ff1c7e01-f687-4844-a1da-6cebbb8f87fd","origin":{"state":"none"}}
$ koto session origin "$KOTO_ANCHOR"
{"session":"anchor-ff1c7e01-f687-4844-a1da-6cebbb8f87fd","via":"anchor-ff1c7e01-f687-4844-a1da-6cebbb8f87fd","origin":{"state":"none"}}
```

The koto-skills SessionStart hook exited 0 with empty stdout and stderr, and
the session's stderr was empty. A case-insensitive search of every tool
result and every line the agent wrote for `error`, `warn`, `install` and
`not found` finds nothing.

Two earlier attempts in the same HOME didn't reach the reads. Neither one
bears on the origin result:

- The first had no `gh` on `PATH`, and `/shirabe:explore` stopped at its own
  prerequisite check with a prompt to install `gh`. That prompt is shirabe's
  requirement for `gh`, not koto's or a launcher's. `gh` was linked into
  `$S3/bin` for the later attempts.
- The second started from an empty directory that wasn't a git repository,
  and `/shirabe:explore` refused to open its session (`refused=no_branch`),
  since it works on a branch. Its anchor read `none` on
  `koto session origin`, as did the first attempt's.

**Result: met**, read more narrowly than the PLAN words it. The check ran
on the reference host in an isolated HOME, not on a separate machine, and
niwa stays installed on the host, off that HOME's `PATH`. It started from a
fresh git repository rather than a bare directory, because
`/shirabe:explore` needs a branch. The hook checked is koto-skills', the
only plugin in that HOME with a SessionStart hook.

## Isolation

- The live plugin cache, `~/.claude/settings.json` and the marketplace were
  unchanged by these runs. Two plugin bookkeeping files under `~/.claude`
  changed during the window. Every new entry in them belongs to another
  session provisioning its own niwa instance at the same time, and neither
  file names either scratch HOME. The plugin cache gained only the in-use
  marker of the host's own background Claude process.
- `~/.tsuku/tools` was unchanged. Every install went into a scratch HOME's
  own `.tsuku`.
- Neither scratch HOME has a koto cloud configuration. Their stores are local
  directories under the HOME, so no baseline or AC3 session read or wrote the
  real koto store or the bucket.
- Both scratch HOMEs and every scratch file lived under `$TMPDIR`, outside
  `~/.claude`, and no `.niwa` directory exists in either.
- niwa stays installed and resolves on the real `PATH`.

The probes ran in instances the coordinator provisioned and tore down
through niwa. They didn't run in either scratch HOME, and nothing above
covers them.

Both scratch HOMEs were removed once the figures above had been read from
them.
