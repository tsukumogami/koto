---
schema: prd/v1
status: Done
problem: |
  Agents on one machine drive many koto sessions at once, often from separate
  workspaces. A few workflow steps, such as a full local test suite, use most
  of the machine for minutes at a time, and koto can't tell them apart from
  cheap steps: when several sessions reach one together they all start, the
  machine overloads, suites fail on timing and the 30-second action limit
  cuts unrelated work. People serialize koto by hand to compensate.
goals: |
  A template author names the host pool a heavy step draws from, an operator
  sizes that pool once per machine, and koto queues the step across every
  session and workspace on the machine, holding a slot for as long as the
  work really runs and giving it back when the work ends or crashes. A
  machine with no pool defined behaves exactly as today.
absorbed:
  - docs/briefs/BRIEF-host-resource-pools.md
---

# PRD: host resource pools

## Status

Done

Absorbed [BRIEF-host-resource-pools](docs/briefs/BRIEF-host-resource-pools.md); carried in Absorbed Brief.

## Absorbed Brief

The brief named in Status is folded into this section and its file was
removed when this PRD absorbed it; this section is the brief's framing, and
there is no separate file to look for.

The feature was framed before these requirements existed. koto can't tell that
a step uses something scarce on the host, so when several sessions on one
machine reach steps that run heavy test suites, all of them start, the machine
overloads, suites fail on timing and the 30-second action limit cuts unrelated
work, and people queue koto by hand because no one can predict which tick will
start a suite. The framed outcome: a template author marks a step with a host
pool's name, an operator sizes the pool once per machine, sessions from any
workspace queue for it on their own and resume without failing, the slot is
held for the work's real length and returns when it ends or crashes, and a
machine with no pool defined, or an older koto, runs as before. The framing's
five journeys (an author marking the heavy step, an operator sizing a pool on a
shared machine, a session reaching the step while the pool is full, a holder
crashing, and an operator checking who holds what) are among the User Stories
and Acceptance Criteria below. Its boundary put in: naming a pool on a state's
action, per-machine pool sizes in the user's config with an undefined pool
meaning no limit, slots shared across sessions and workspaces and released on
crash, heavy work holding its slot past the tick, queued rather than failed,
fair ordering and visibility. It put out: pools across machines or users,
sizes in templates or project config, CPU and memory limiting, the per-session
write lock and the script-level suite lock; it also left the shirabe
verification migration to follow, which Out of Scope below carries along with
the rest. The brief's three open questions are closed under Decisions and
Trade-offs.

## Problem Statement

koto runs many sessions side by side on one machine, each driven by its own
agent and often from a workspace that knows nothing about the others. Most
steps are cheap. A few are not: a step that runs a project's full local test
suite can take from a few minutes to most of an hour and use most of the
machine while it does.

koto treats every step the same. A template can't say that a step uses
something the host has little of, and koto has no notion of a resource shared
across sessions. When two or three sessions reach such a step together, all of
them start; the machine overloads, the suites fail each other on timing, and
koto's fixed 30-second limit on ordinary actions starts cutting steps that pass
on an idle machine. Because a single `koto next` can pass through several
states whose gates already pass and land on the heavy one, nobody can predict
which tick will start a suite, so coordinators queue every koto command through
a person. That costs a hundred or more messages per run and leaves sessions
idle behind each other.

The heavy step also outlives the command that starts it. Since koto gives an
action 30 seconds, long suites are launched by a quick action into a detached
process today, and a later tick reads the result. A limit that holds a slot only
while the action runs would release it the moment the suite starts.

## Goals

- A heavy step never runs beside more copies of itself on a machine than the
  machine's operator allows, whichever session, workspace or tick reaches it.
- A session that has to wait is told it is queued and by whom, and resumes on a
  later tick without failing the run.
- A step's slot is held for the real length of its work and returns when the
  work ends, fails, times out or crashes.
- Nobody has to configure anything for koto to keep working as it does today.

## User Stories

- As a **template author**, I want to mark a state's action with the name of a
  host pool, so that every machine that limits that pool queues the step while
  machines that don't, and older koto releases, run my template unchanged.
- As a **template author** with a step longer than 30 seconds, I want koto to
  run it in the background and report its result on a later tick, with or
  without a pool, so that I don't need a hand-written supervisor.
- As an **operator** running several agent sessions on one workstation, I want
  to set a pool's size once in my koto config, so that every workspace on the
  machine, including ones created later, shares those slots.
- As an **operator**, I want to change a pool's size or remove it while
  sessions hold slots, so that I can adjust the machine's limit without
  breaking running work.
- As an **agent** ticking a session, I want a tick that reaches a full pool to
  answer "queued, held by these sessions, retry in N seconds" instead of
  failing or blocking for minutes, so that I can keep driving the run.
- As an **agent** whose session syncs through koto's cloud backend, I want koto
  to refuse to move my session to another machine while its background step
  runs, so that the step's result isn't lost or duplicated.
- As a **person diagnosing a stuck queue**, I want one command that lists each
  pool's holders and waiters, so that I can see why sessions are waiting.

## Terms

- **Pool**: a named limit defined in the user's koto config, with a size.
- **Slot**: one unit of a pool's size. A pool of size N has N slots.
- **Visit**: one stay of a session in a state, from entering it to leaving it.
- **Tick**: one `koto next` invocation on a session.
- **Pooled action**: a state's `default_action` whose `pool` names a pool
  defined in the user's config, valid or not (R7 covers an invalid one).
- **Runner**: the koto process koto starts for a background action. It starts
  the command, waits for it, records the result and holds the action's slot.
- **Holder**: the process that holds a slot. For an ordinary pooled action it
  is the koto process running the tick; for a background action it is the
  runner.
- **Waiter**: a session queued for a slot of a full pool.
- **Claim period**: how long a freed slot stays reserved for the head waiter
  before the next waiter may take it.
- **Lapse period**: how long a waiter may go without ticking before it loses its
  place.
- **Pools directory**: `<koto root>/pools/`, where koto keeps the slot and queue
  files.

## Requirements

### Declaring a pool

- **R1.** A state's `default_action` can name one host pool with a `pool` field.
  A pool name matches `^[a-z0-9][a-z0-9-]{0,62}$`: lowercase letters, digits
  and hyphens, starting with a letter or digit, at most 63 characters.
- **R2.** `koto template compile` refuses, with an error naming the state and
  the field: a pool name that doesn't match R1; a `pool` value holding a
  template variable; a `size`, `claim_secs`, `lapse_secs` or `wait_secs` key
  anywhere under a state; `timeout_secs` without `background: true`;
  `background: true` without `timeout_secs`; and `timeout_secs` that is not a
  whole number from 1 to 86400.
- **R3.** A template that doesn't use `pool`, `background` or `timeout_secs`
  compiles to the same compiled output and template hash as on the previous
  release.
- **R4.** A koto release without this feature ignores a `pool` field and runs the
  action unthrottled, as it does today.

### Defining pools on a host

- **R5.** Pools are defined only in the user's koto config, the `config.toml` at
  the koto root (`$KOTO_HOME` when set, else `~/.koto`), as `[pools.<name>]`
  tables. `size` is required. `claim_secs` (default 120), `lapse_secs` (default
  600) and `wait_secs` (default 10) are optional. A project's config can't
  define or change a pool: such keys are ignored and listed by
  `koto config list` among dropped keys.
- **R6.** A pool named by a template but not defined in the user's config imposes
  no limit: the action runs exactly as if `pool` were absent, writes no pool
  event and creates nothing under the pools directory.
- **R7.** `size` is a whole number from 1 to 1024; `claim_secs` and `lapse_secs`
  are whole numbers from 1 to 86400; `wait_secs` is a whole number from 0 to 60.
  An invalid or missing value makes that pool invalid. A tick reaching a pooled
  action on an invalid pool stops with error code `pool_config_invalid`, naming
  the config file and the key, and runs nothing. `koto pool status` reports
  each invalid pool with the same code and exits 1. Every other command, and a
  tick that doesn't reach that pool, works as if the pool were undefined.
- **R8.** Pools are read from the config each time a pooled action is reached,
  so a pool added or resized takes effect on the next tick in every workspace.
  Removing a pool lets new arrivals and every waiter run unthrottled on their
  next tick; current holders keep running. Lowering a size below the number of
  holders keeps the holders and admits nobody new until fewer than `size`
  slots are held.

### Sharing slots

- **R9.** All sessions under one koto root on a machine share each pool's slots,
  whatever workspace, worktree or niwa instance they run from. At most `size`
  holders hold a pool's slots at any moment, including when several sessions
  try to take the last slot at the same instant.
- **R10.** A slot is held by every process of the work it was taken for: the
  koto process or runner, the command, and the command's descendants. It is
  released within 5 seconds after the last of them exits for any reason,
  including `kill -9`, without anyone removing a file by hand. When a runner is
  killed and processes of its command survive, the slot stays held until they
  exit, and koto doesn't signal them (R20, R22).
- **R11.** Waiters are served first come, first served. A session arriving at a
  pool queues behind existing waiters even when a slot is free. A freed slot is
  reserved for the head waiter for the claim period; if the head waiter doesn't
  tick within it, the slot goes to the next waiter and the head keeps first
  place for the following free slot. Time a waiter spends inside a tick's
  in-tick wait (R15) counts as ticking for both the claim and the lapse
  period.
- **R12.** A waiter that doesn't tick for longer than the lapse period loses its
  place, and the waiters behind it move up. Its next tick queues it again at
  the back. A waiter whose session leaves the pool state (a transition, a
  rewind, completion), is cancelled, or is removed leaves the queue before the
  command that caused it returns.
- **R13.** koto marks the environment of every command it runs under a slot with
  the pool's name and the koto root. A koto process, or a pooled action, that
  inherits the marker for the same pool and the same koto root runs on the
  outer slot without waiting and without taking a second slot. A marker for a
  different pool or root is ignored. Detached descendants inherit the marker;
  the marker is cooperative and isn't a security boundary between processes
  of the same user.
- **R14.** If the pools directory can't be created or written, a tick reaching a
  pooled action stops with error code `pool_dir_unavailable`, naming the path,
  and runs nothing.

### Waiting

- **R15.** When a tick reaches a pooled action whose pool has no slot for it,
  koto waits inside the tick for up to the pool's `wait_secs`. If a slot comes
  free in that time the tick takes it and runs the action. Otherwise koto runs
  neither the action nor the state's gates and returns exit 0 with
  `action: "gate_blocked"` and one blocking condition: name `__pool__`, type
  `pool`, status `queued`, category `temporal`, `agent_actionable: false`.
- **R16.** The queued condition's output carries `pool`, `size`, `position`
  (1-based), `waiting` (waiters, this session included), `waited_secs` (since this session joined the queue),
  `retry_after_secs`, and a `holders` list with `session`, `workspace`,
  `state` and `held_secs` for each holder. `retry_after_secs` is
  `min(15, claim_secs)`, so a whole number from 1 to 15; nothing else changes
  it.
- **R17.** Progress a tick made before reaching the full pool is kept: a tick that
  passes through earlier states and stops at the pool state records those
  transitions and reports the pool state as current.

### Ordinary and background actions

- **R18.** An ordinary pooled action (without `background`) takes its slot when it
  starts and releases it when the action ends, under koto's usual 30-second
  action limit.
- **R19.** A `default_action` may declare `background: true` with `timeout_secs`,
  with or without `pool`. koto starts the command through a runner in a new
  process group and returns from the tick within 2 seconds without waiting for
  the command. A session never runs two copies of a visit's command at once. A
  background action that names a pool takes the slot before its command
  starts. When the command ends or times out, the runner stops the command's
  remaining descendants. The user's config can switch background actions off
  (`[background] enabled = false`, refused at the tick with
  `background_disabled`) and caps `timeout_secs` at `[background]
  max_timeout_secs` (default 7200).
- **R20.** While a background command runs, every tick in that visit returns exit
  0 with `action: "gate_blocked"` and one blocking condition: name `__action__`,
  type `action`, status `running`, category `temporal`,
  `agent_actionable: false`, with `run_id`, `elapsed_secs` and `timeout_secs`.
  It starts no second copy and evaluates no gates. When the runner has died but
  processes of the run survive, the condition's status is `orphaned` instead,
  and it stays so until they exit.
- **R21.** The first tick after a background run ends records the result in the
  session log. An exit 0 delivers the action's `capture`, when declared, and
  runs the state's gates and transitions as an ordinary successful action
  would. Otherwise the tick reports an `__action__` failure with
  `failure_kind` `nonzero_exit` (with `exit_code`), `timed_out` (the runner
  killed the process group within 2 seconds after `timeout_secs`; carries
  `timeout_secs`), or
  `lost` (the runner is gone with no result; no `exit_code`). A result carries
  the command's stdout and stderr under the bound koto applies to every action
  today: the first 64 KiB of each stream (`MAX_ACTION_OUTPUT_BYTES`), with
  `truncated: true` when either was cut. A failure doesn't transition as a
  pass.
- **R22.** After a failed or lost run, the next tick in the same visit starts a new
  run, as koto re-runs a failed ordinary action today; an orphaned run must end
  first. Entering the state again starts a new run once any earlier run of that
  session has stopped: a live runner is stopped first, as by `koto cancel`.
- **R23.** `koto cancel` on a session, and removing a session, stop its live
  background runner, which stops the command and its descendants, and remove it
  from any queue, before the command returns. `koto cancel` appends `pool_released`
  with reason `cancelled`; a removed session has no log left to append to.
- **R24.** Moving, relocating or rebinding a session to another machine while a
  background run is in flight is refused with error code
  `background_run_in_flight`, naming the run. A background run recorded as
  started on another machine reads as lost on this one.

### Visibility

- **R25.** `koto pool status` lists every pool in the user's config with its size,
  held, free (`size` minus held, never below 0) and waiting counts, each holder (session, workspace, state,
  held time) and each waiter in queue order (session, workspace, waited time).
  `--json` prints the same as one JSON object. It takes no lock, writes
  nothing, and works while any session is being ticked. With no pools defined
  it says so and exits 0.
- **R26.** These events are appended to the session's log, each naming the pool
  where one is involved: `pool_queued` once when the session joins a queue (not
  on each queued tick); `pool_acquired` when it takes a slot;
  `pool_released`, with a reason of `done`, `failed`, `timed_out`, `lost`,
  `cancelled`, `lapsed`, `left_state` or `pool_removed`, when it releases a
  slot or leaves a queue; `background_started` and `background_finished` around each background
  run, carrying `run_id`. A koto release without the feature reads a log
  holding these events without error.

### Non-functional

- **R27.** The feature works on Linux and macOS. On other platforms a pooled
  action on a defined pool stops with `pool_unsupported_platform`, and
  `background: true` is refused at compile time.
- **R28.** On a state whose action names an undefined pool, the median of 50
  ticks is at most 5 ms above the median of 50 ticks on the same state with no
  `pool` field, both measured in one run of the repository's benchmark on the
  Linux CI runner.

## Acceptance Criteria

Every criterion is checked on Linux and macOS CI, except the two in Platform
and cost, which say where they run.

### Compile and compatibility

- [ ] A template that uses none of `pool`, `background` or `timeout_secs`
  compiles to byte-identical compiled output and the same template hash as on
  the previous release.
- [ ] `koto template compile` accepts the pool names `a`, `test-suite` and a
  63-character name, and refuses `""`, `Test`, `-a`, a 64-character name, a
  name containing `/` or `..`, and `{{POOL}}`, each with an error naming the
  state and the `pool` field.
- [ ] `koto template compile` refuses each of `size: 2`, `claim_secs: 5`,
  `lapse_secs: 5` and `wait_secs: 5` under a state, `timeout_secs`
  without `background: true`, `background: true` without `timeout_secs`, and
  `timeout_secs` of 0, -1, 1.5 and 86401, each naming the state and field, and
  accepts `background: true` with `timeout_secs: 60` and no `pool`.
- [ ] A template with `pool: test-suite` compiles and runs on the previous koto
  release, which ignores the field.
- [ ] A session log holding all five new event types is read by `koto status` on
  the previous release without error.

### Zero config and pool definitions

- [ ] With no `[pools]` table, a template whose action declares
  `pool: test-suite` produces the same `koto next` JSON as without `pool`
  once every `timestamp` and `since` field is removed, appends no pool event, and leaves the pools
  directory absent.
- [ ] A template naming `pool: other` while only `[pools.test-suite]` is defined
  runs its action as without `pool`, and `koto pool status` doesn't list
  `other`.
- [ ] With `KOTO_HOME` set, pools are read from `$KOTO_HOME/config.toml` and the
  pools directory is `$KOTO_HOME/pools`; with it unset, from
  `~/.koto/config.toml` and `~/.koto/pools`.
- [ ] A project config setting `pools.test-suite.size = 1` changes nothing, and
  `koto config list` lists the key among dropped keys.
- [ ] `[pools.test-suite] size = 1` with no other key behaves with
  `claim_secs` 120, `lapse_secs` 600 and `wait_secs` 10, and `size = 1024`,
  `claim_secs = 1`, `lapse_secs = 86400` and `wait_secs = 0` are accepted.
- [ ] For each of `size = "two"`, `0`, `-1`, `1.5`, `1025`, a missing `size`,
  `claim_secs = 0`, `lapse_secs = 86401` and `wait_secs = 61`:
  `koto status`, `koto config list` and a tick that doesn't reach the pool all
  succeed; a tick reaching the pooled action stops with `pool_config_invalid`
  naming the config file and the key and runs nothing; `koto pool status`
  reports the pool with that code and exits 1.
- [ ] Adding `[pools.test-suite] size = 1` while two sessions in different
  workspaces are past the pool state makes the next session reaching it queue
  behind one holder, with no restart of anything.
- [ ] Removing the pool from config while one session holds it and another
  waits lets the waiter's next tick run the action; the holder keeps running to
  completion.
- [ ] Lowering `size` from 2 to 1 while two sessions hold slots keeps both
  running and makes a third session wait until both have released.

### Sharing slots

- [ ] With `[pools.test-suite] size = 2` and three sessions in three different
  workspace directories under one koto root reaching the pool state, recorded
  start and stop times show at most two commands running at any instant.
- [ ] Twenty sessions racing for the last free slot of a size-1 pool produce
  exactly one `pool_acquired`.
- [ ] An ordinary pooled action on a size-1 pool releases its slot when it
  exits, and when koto cuts it at the 30-second limit: a second session's next
  tick takes the slot.
- [ ] `kill -9` of the koto process running an ordinary pooled action frees the
  slot within 5 seconds with no file removed by hand.
- [ ] `kill -9` of a background runner's whole process tree frees the slot
  within 5 seconds with no file removed by hand, and the head waiter's next
  tick takes it.
- [ ] After `kill -9` of a background runner whose command survives, the session's
  ticks report `__action__` status `orphaned` and the slot stays held, no
  signal reaches the surviving command, and once it exits the next tick reports
  `failure_kind: "lost"`, frees the slot and the tick after starts a new run.
- [ ] A background command that starts a child in its own session (`setsid`)
  and exits leaves no process of the run alive on Linux once the runner ends,
  and the slot is free.
- [ ] With `[background] enabled = false`, a background action stops with
  `background_disabled`; with `max_timeout_secs = 10`, an action declaring
  `timeout_secs: 60` is killed after 10 seconds and its `running` condition
  shows `timeout_capped: true`.
- [ ] With size 1 and waiters W1, W2 and W3 queued in that order, the slot goes to
  W1, then W2, then W3, even when W3 ticks every second and the others every
  10 seconds.
- [ ] A session that arrives while a slot is free but W1 is waiting queues behind
  W1.
- [ ] With `claim_secs = 5`, when a slot frees and W1 doesn't tick for 6 seconds,
  W2's next tick takes the slot, and W1 takes the following free slot.
- [ ] With `lapse_secs = 5`, a waiter that doesn't tick for 6 seconds is gone from
  `koto pool status`, the waiters behind it move up, and its next tick queues
  it at the back; with no `lapse_secs` set, a waiter idle for 590 seconds keeps
  its place.
- [ ] A waiter whose session is rewound out of the pool state, and separately one
  whose session completes, is absent from a `koto pool status` run right after
  the command returns, and its log holds `pool_released` with reason
  `left_state`.
- [ ] A background command under a size-1 pool that runs a nested koto session
  reaching a pooled action on the same pool, and separately runs a pooled
  action directly, finishes without waiting and without a second
  `pool_acquired`; a nested action on a different defined pool, and a nested
  koto run with the same pool but another `KOTO_HOME`, queue normally.
- [ ] With the pools directory made unwritable, a tick reaching a pooled action
  stops with `pool_dir_unavailable` naming the path and runs nothing.

### Waiting

- [ ] The third session's tick in the size-2 case exits 0 with
  `action: "gate_blocked"` and a blocking condition named `__pool__` of
  `type: "pool"`, `status: "queued"`, `category: "temporal"`,
  `agent_actionable: false`, whose output holds `pool: "test-suite"`,
  `size: 2`, `position: 1`, `waiting: 1`, `waited_secs`, `retry_after_secs: 15`,
  and two `holders` entries with `session`, `workspace`,
  `state` and `held_secs`.
- [ ] With `claim_secs = 5`, a queued tick reports `retry_after_secs: 5`; a
  fourth session queued behind the third reports `position: 2` and
  `waiting: 2`.
- [ ] Two queued ticks 3 seconds apart report `waited_secs` and each holder's
  `held_secs` larger by at least 2 on the second.
- [ ] With `wait_secs = 5` and a holder finishing 2 seconds into the waiter's
  tick, that tick takes the slot and runs the action without returning queued;
  with no holder finishing, it returns queued after 5 to 7 seconds.
- [ ] A state whose gate writes a marker file when evaluated never writes it while
  its session is queued or its background run is running.
- [ ] A tick that starts in state A, passes two states whose gates pass, and
  reaches pool state D while the pool is full returns state D with
  `advanced: true` and the queued condition; the log holds the transitions from
  A to D, and D's command hasn't run.
- [ ] After a holder finishes, the queued session's next tick takes the slot and
  starts the command with no other command run in between.

### Background actions

- [ ] A `background: true` action whose command exits 0 after 90 seconds holds its
  slot for the whole 90 seconds; the tick that started it returns within 2
  seconds; every tick meanwhile reports `__action__` `running` with the same
  `run_id`, growing `elapsed_secs` and the declared `timeout_secs`, and starts
  no second copy; the first tick after it ends delivers the declared `capture`,
  evaluates the state's gates and advances.
- [ ] With size 1, a second session can't start its command until the first
  session's background command has exited.
- [ ] The background command's process group id differs from the process group
  id of the koto process that ran the starting tick.
- [ ] A `background: true` action with no `pool` and a 45-second command runs to
  completion and advances, with no pool event in the log.
- [ ] A background command exiting 3 yields, on the next tick, `failure_kind:
  "nonzero_exit"` with `exit_code: 3`, its slot is free, its log holds
  `pool_released` with reason `failed`, and the tick after starts a new run.
- [ ] A background command printing 100 KiB to stdout and exiting 1 yields a
  failure whose stdout is its first 64 KiB, with `truncated: true`.
- [ ] A background command that outlives `timeout_secs: 5` has its process group
  killed within 7 seconds, its slot freed, and the next tick reports
  `failure_kind: "timed_out"` with `timeout_secs: 5`.
- [ ] Re-entering the pool state while a previous visit's runner is still alive
  stops that runner, and its command's processes are gone, before the new run
  starts.
- [ ] `koto cancel` on a holder stops its background runner, whose command's
  processes are then gone, frees the slot and appends `pool_released` with
  reason `cancelled`; removing the session does the same and frees the slot; `koto cancel` on a waiter removes
  it from `koto pool status`; each is done before the command returns.
- [ ] `koto session` moves, relocates and rebinds to another machine refuse with
  `background_run_in_flight` while a background run is in flight; a run whose
  log records it as started on another machine reads as `lost` on this one.

### Visibility

- [ ] `koto pool status --json` lists each configured pool with `size`, `held`,
  `free`, `waiting`, each holder and each waiter in queue order; the plain form
  prints one block per pool with the same values; neither changes any file's
  modification time, and both complete while another process holds a session's
  state lock; with no pools configured it reports none and exits 0.
- [ ] A queued, acquired, then released background run appends `pool_queued`,
  `pool_acquired`, `background_started`, `background_finished` and
  `pool_released` (reason `done`) in that order, each pool event naming the pool
  and both background events carrying the same `run_id`; five consecutive
  queued ticks append one `pool_queued`.
- [ ] A lapsed waiter, a lost run and a timed-out run each append
  `pool_released` with reason `lapsed`, `lost` and `timed_out`.

### Platform and cost

- [ ] A unit test that forces the platform check to report an unsupported
  platform sees a pooled action on a defined pool stop with
  `pool_unsupported_platform` and `background: true` fail to compile; runs on
  Linux CI.
- [ ] The repository's benchmark, on the Linux CI runner, reports the median of
  50 ticks on a state naming an undefined pool at most 5 ms above the median of
  50 ticks on the same state with no `pool` field.

## Out of Scope

- **Pools across machines, or across users on one machine.** A pool is per koto
  root on one machine; distributed locking is a different problem.
- **Pool sizes in templates or project config.** Only the machine's operator
  sizes a pool, so one repository can't take over a shared machine.
- **Per-workspace caps below the host size.** Deferred: the host's size is the
  only limit in this version (see Decisions and Trade-offs).
- **Limiting CPU or memory directly.** A pool counts concurrent steps, not the
  resources they use.
- **Pools on command gates.** Gates poll and return quickly; only actions take
  slots.
- **The per-session state-log write lock.** Two writers on one session are a
  separate problem with its own fix.
- **Moving shirabe's work-on verification step onto the feature.** This PRD
  covers koto only. The migration needs this
  feature released and shirabe's koto floor raised, because a koto release
  without `background` would run the suite inline and cut it at 30 seconds; it
  follows as shirabe work once koto ships, using a background pooled action
  with `capture` (R19, R21).

## Decisions and Trade-offs

- **Pool sizes live in the user's koto config, not a project's or a
  template's.** Alternatives were template-declared sizes (Buildkite's shape)
  and project config. The shape that fits shared machines, used by Airflow
  pools, Argo semaphores, Prefect and Dagster, is a name in the workflow and a
  size set by whoever runs the machine.
- **An undefined pool means no limit.** Alternatives were an implicit size-1
  mutex (GitHub Actions, GitLab) or an error. No limit keeps the feature zero
  config and avoids Airflow's failure mode of a task waiting forever on a pool
  nobody created.
- **An invalid pool fails only where it's used.** A typed config key would make
  one bad value break every koto command on the machine; reading pools lazily
  confines the error to ticks that reach the pool and to `koto pool status`.
- **The pools directory sits under the koto root, next to the config that sizes
  it.** A fixed path under HOME would share pools across koto roots, but every
  other koto path follows `KOTO_HOME`, niwa workspaces share the default root
  unless the host sets one, and a separate root already brings its own config.
- **koto runs long steps itself as background actions.** Alternatives were a
  slot leased until the session leaves the state (held while idle, leaked by
  abandoned sessions) and a bare wrapper command for scripts (no template
  keyword). Only background actions hold the slot for the work's real length,
  and they remove the 30-second limit for long steps.
- **First come, first served, with a claim period and a lapse.** Plain retry
  lets a session that ticks often starve one that ticks slowly; a claim period
  keeps a freed slot from sitting idle behind a silent head waiter; a lapse
  keeps an abandoned waiter from holding the queue.
- **A short in-tick wait.** Returning at once would make every handoff between
  sessions cost a full retry interval. Waiting long would hold an agent's tool
  call: koto's poll gates already cap an in-tick hold at what a tool call can
  wait, and pools stay well under that, with a default of 10 seconds and a
  ceiling of 60.
- **Nested same-pool requests run on the outer slot.** The alternative, waiting
  like any other session, deadlocks a size-1 pool. This mirrors how child makes
  inherit jobserver tokens. Closes the BRIEF's third open question.
- **Per-workspace caps are deferred.** The BRIEF asked whether a workspace may
  cap its own share. No user has asked for a cap below the host size, and any
  rule for two consumers configuring one pool differently needs a precedence
  design. Closes the BRIEF's first open question as a known gap.
- **Cross-machine moves are refused while a run is in flight.** The alternative,
  detaching and re-attaching a run, has no safe meaning when the process lives
  on the old machine. Closes the BRIEF's second open question.

## Known Limitations

- Two koto roots on one machine (separate `KOTO_HOME` values) don't share
  pools.
- A pool counts steps, not load: a size-2 pool still lets two heavy suites run
  beside any amount of other work.
- A process that escapes a runner (on macOS, or a daemon left behind) keeps
  its run's slot until it exits, and koto never kills it; `koto pool status`
  shows the slot as held.
- The nesting marker is cooperative: a process of the same user that copies it
  can run on another session's slot.
- `docs/guides/cli-usage.md` states user config takes precedence over project
  config while the code applies the reverse; the pool keys are user-only either
  way, but the guide needs correcting.
