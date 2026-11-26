# Verification rerun: where a session came from

This records the rerun of the four checks that
[the first verification](VERIFY-koto-session-dispatcher-origin.md) left
unsettled for Issue 5 of `PLAN-koto-session-dispatcher-origin`: AC1, AC3, and
AC7 with AC8 (koto#766). It was run on 2026-11-26 on the same reference host,
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

The criteria are read as the PLAN words them on koto#687's branch
`docs/session-dispatcher-origin`.

RESULT-SUMMARY-PENDING

## Host and versions

| Runs | koto | shirabe CLI | shirabe plugin | koto-skills plugin | Where |
|------|------|-------------|----------------|--------------------|-------|
| Baselines b1 to b3 | 0.37.2 (dde9edb) | v0.55.0 | 0.55.0 | 0.37.2 | scratch HOME `$S`, local koto store |
| AC3 | 0.41.0 (d54c489) | v0.56.0 | 0.56.0 | 0.41.0 | scratch HOME `$S3`, local koto store, no niwa |
| Probes p1 to p10 | PROBE-VERSIONS-PENDING | | | | instances the coordinator provisioned, cloud store |

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

The task text holds none of the three search terms (koto#716's rule). Each
run's brief was rendered by its own shirabe version's `render-brief.sh` from
the same input: entry point `work-on`, run mode `--auto`, review level
light, and two checkpoints. The run stops once its branch is pushed, with no
pull request. The 0.55.0 brief carries the dispatcher block and its link step,
and the 0.56.0 brief doesn't, which is the change shirabe#2117 made.

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

The counter is the one Issue 5 used. Its built-in wrapper list predates the
`/work-on` scripts, so for every run the wrapper set adds each shirabe script
the run invoked that runs koto. In these runs that's `review-level.sh`, which
runs `koto context exists`, and `work-on-open.sh`, which opens the session
through `koto-open.sh`. The same rule applies to baseline and after runs.
With the built-in list alone, b1 counts 13 calls instead of 14.

| Run | koto calls | Failed | (a) input + output | (b) with cache |
|-----|-----------:|-------:|-------------------:|---------------:|
| b1 | 14 | 1 | 5435 | 907580 |
| b2 | 14 | 1 | 5364 | 910083 |
| b3 | 9 | 1 | 3677 | 590391 |
| p1 | P1-PENDING | | | |
| p2 | P2-PENDING | | | |
| p3 | P3-PENDING | | | |

Baseline range: 9 to 14 calls, median 14; (a) 3677 to 5435, median 5364.
In each baseline the failed call is the one Bash command that ran the link
step and then `resolve-coordinator.sh`. The link exited 0, and the resolve
exited 1 because the stand-in coordinator run holds no run record. b3 made
fewer calls because its worker batched several `koto next` steps into fewer
Bash commands.

AC7-RESULT-PENDING

## AC8: no trace of the link step

AC8-PENDING

## AC1: ten dispatched workers report the coordinator

AC1-PENDING

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

AC3 is met.

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

SCRATCH-DISPOSITION-PENDING
