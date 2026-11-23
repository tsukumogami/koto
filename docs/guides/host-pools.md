# Host resource pools and background actions

A machine running several koto sessions at once can run several heavy commands at once: three
workspaces each reach their test-suite state in the same minute, and the host swaps. A host
resource pool caps how many sessions under one koto root run a named kind of action at the same
time. The template says which pool an action belongs to; the person who runs the machine says how
big the pool is. A session that finds the pool full isn't failed. Its tick stops and reports it
queued, and a later tick runs the action once a slot is free.

A pooled action that runs longer than koto's 30-second action limit, which a test suite usually
does, is declared `background: true`. koto then starts the command through a detached runner and
returns from the tick at once, and the slot stays held for as long as the command runs.

This guide covers both, for template authors and for operators. Everything here works on Linux and
macOS; [Platforms](#platforms) says what happens elsewhere.

## Naming a pool in a template

`pool` goes on a state's `default_action`:

```yaml
states:
  test:
    default_action:
      command: "cargo test --locked"
      pool: test-suite
    transitions:
      - target: review
```

A pool name is 1 to 63 characters of lowercase letters, digits and hyphens, starting with a letter
or a digit: `a`, `test-suite` and `gpu-0` are names, `Test`, `-a` and `suite/x` aren't. It must be
written literally. `pool: {{POOL}}` is refused, because the name becomes a directory under the koto
root and a template's caller must not pick it.

The template never sizes a pool. `size`, `claim_secs`, `lapse_secs` and `wait_secs` written under a
state or its `default_action` are compile errors.

**A pool nobody defined is no limit.** On a machine whose config has no `[pools.test-suite]`, the
action above runs exactly as it would without the `pool` line: nothing is created on disk, nothing
is recorded, and the tick costs at most 5 ms more, which the repository's tick-cost check
enforces (in practice the two medians are within a millisecond of each other). So naming a pool in a shared template is safe: it only takes effect where an
operator defined it.

## Sizing a pool

Pools are defined in the user config only, `~/.koto/config.toml` (or `$KOTO_HOME/config.toml` when
`KOTO_HOME` is set):

```toml
[pools.test-suite]
size = 2
wait_secs = 5
```

| Key | Meaning | Default | Range |
|-----|---------|---------|-------|
| `pools.<name>.size` | How many sessions may run the pool's actions at once | none, required | 1 to 1024 |
| `pools.<name>.claim_secs` | How long a freed slot is kept for the first waiter before it passes to the next | 120 | 1 to 86400 |
| `pools.<name>.lapse_secs` | How long a waiter may go without ticking before it loses its place | 600 | 1 to 86400 |
| `pools.<name>.wait_secs` | How long a tick waits for a slot before it reports the session queued | 10 | 0 to 60 |

A project's `.koto/config.toml` can't define or resize a pool, trusted or not: `koto config list`
lists any `pools.*` key it sets among the dropped keys, and nothing else changes. Pools are read
from the config each time a pooled action is reached, so a resize applies on the next tick.
Shrinking a pool keeps its current holders and admits nobody until fewer than `size` are held.
Removing a pool lets its waiters run unthrottled on their next tick.

A pool whose table is invalid (a `size` of 0, a value out of range, a value that isn't a whole
number, or a config file that doesn't parse) fails only the ticks that reach that pool: they stop
with error code `pool_config_invalid`, naming the config file and the key, and run nothing.
`koto status`, `koto config list` and ticks on other states are unaffected.

## What a tick does at a pooled action

Before the action's command and before any of the state's gates, koto takes a slot of the pool.
When none is free it joins the pool's queue and waits inside the tick for up to `wait_secs`. If a
slot frees in that time the action runs; if not, the tick stops with exit 0, `action:
"gate_blocked"` and one reserved `__pool__` condition:

```json
{
  "name": "__pool__",
  "type": "pool",
  "status": "queued",
  "category": "temporal",
  "agent_actionable": false,
  "output": {
    "status": "queued",
    "pool": "test-suite",
    "size": 1,
    "position": 1,
    "waiting": 1,
    "waited_secs": 0,
    "retry_after_secs": 15,
    "holders": [
      {"session": "a", "workspace": "ws-a", "state": "test", "held_secs": 42}
    ]
  }
}
```

Nothing the agent does moves the queue, so the condition is temporal and not agent-actionable:
tick again after about `retry_after_secs` seconds. The state's gates were not evaluated, and the
run hasn't failed. `__pool__` is reserved, so no template can declare a gate by that name.

Waiters are served in queue order, not by who ticks first. A session that arrives while others wait
queues behind them even if a slot is free. A freed slot is kept for the head waiter for
`claim_secs`; if that waiter doesn't tick in time the slot passes to the next one, and the skipped
waiter keeps first place for the following slot. A waiter that doesn't tick for `lapse_secs` loses
its place and queues at the back on its next tick. A session that leaves the state while queued
(by a transition, `koto rewind` or completion) leaves the queue.

An ordinary pooled action holds its slot while its command runs inside the tick. The slot's file
descriptor is inherited by the command, so anything the command leaves running keeps the slot held
until it exits too.

## Background actions

```yaml
states:
  test:
    default_action:
      command: "cargo test --locked"
      pool: test-suite
      background: true
      timeout_secs: 3600
    transitions:
      - target: review
```

`background: true` needs `timeout_secs`, a whole number of seconds from 1 to 86400, and
`timeout_secs` is refused without `background: true` (an ordinary action runs under koto's
30-second limit). `background` can't be combined with `polling` or `requires_confirmation`. A
background action without `pool` runs detached without taking a slot.

The first tick that reaches the state starts the command through a runner and returns at once.
Every later tick in that visit reports the run instead of starting another copy, as an `__action__`
condition with `type: "action"`, `category: "temporal"` and the run's status:

| Status | Meaning | What to do |
|--------|---------|------------|
| `running` | The command is running. `output` carries `run_id`, `elapsed_secs`, `timeout_secs`, `retry_after_secs`, and `pool` when it holds a slot. | Tick again after `retry_after_secs`. |
| `orphaned` | The runner died but processes of the run are still alive. They keep the slot, and koto never signals them. `output` adds `runner_pid`. | Wait; once they exit, the next tick reports the run lost. |

The first tick after the command ends delivers its result as an ordinary action's would:
`capture_stdout_as` is stored, the gates are evaluated and the state advances. A failed run is an
action failure with a `failure_kind`:

- `nonzero_exit`, with the command's exit code;
- `timed_out`, when the command outlived `timeout_secs` and was killed;
- `lost`, when the runner is gone and left no result: it was killed, or the run was started on
  another machine. There is no exit code.

After any of them the next tick starts a new run.

When the command exits, times out or is stopped, the runner sends TERM to the command's process
group and to every descendant it can find (on Linux, including one that left the group with
`setsid`), and KILL two seconds later. The slot is free only once every process of the run has
exited.

`koto cancel` and removing a session stop a live run and record its slot's release, and a session
that leaves the state stops the visit's run before anything else. A session with a run in flight
can't be moved: `koto session rebind`, `koto session import` and a relocation refuse with
`background_run_in_flight` until the run ends or is cancelled.

### Operator settings for background actions

```toml
[background]
enabled = true
max_timeout_secs = 7200
```

| Key | Meaning | Default | Range |
|-----|---------|---------|-------|
| `background.enabled` | Whether background actions run on this machine | `true` | `true` or `false` |
| `background.max_timeout_secs` | The longest a background run may last, whatever the template asks | 7200 | 1 to 86400 |

Like pools, these are read from the user config only. With `enabled = false`, a tick reaching a
background action stops with an `__action__` error, `error_code: "background_disabled"`, before it
queues for any pool. A template `timeout_secs` above `max_timeout_secs` is cut to it, and the
`running` report carries `timeout_capped: true`, so the agent knows the run will be stopped sooner
than the template said.

## Nested koto calls: the marker

A pooled command often runs koto itself, and that inner koto may reach an action on the same pool.
If it queued, it would wait behind the slot its own parent holds. So a pooled command runs with
`KOTO_POOL_HELD` (`<pool>/<slot>/<holder id>`) and `KOTO_POOL_ROOT` set, and a koto that reaches an
ordinary action on the same pool, under the same koto root, runs on that slot without queueing and
records no second acquisition. koto checks the marker against the slot's holder record, so a stale
marker left in a daemon grants nothing once its run ends. Another pool, another root, or a slot held
by someone else gets no exemption, and a background action never borrows a slot, since its runner
would outlive the slot it borrowed.

A background command also carries `KOTO_BACKGROUND_OF` naming its session, in place of the
`KOTO_TICK_SESSION` an in-tick command gets.

The marker is cooperative: a process of the same user that copies it can run on another session's
slot. A pool is a scheduling aid between one user's sessions, not an isolation boundary.

## Seeing who holds a pool: `koto pool status`

```
$ koto pool status
pool test-suite: size 1, held 1, free 0, waiting 1
  holder: session a, workspace ws-a, state test, held 42s
  waiter 1: session b, workspace ws-b, waited 3s
```

`koto pool status --json` gives the same as a `pools` array, each entry with `name`, `size`,
`held`, `free`, `waiting`, `holders` (`session`, `workspace`, `state`, `held_secs`) and `waiters`
in queue order (`session`, `workspace`, `waited_secs`). It lists every pool the user config defines
and only those: an undefined pool that templates name isn't listed. It reads and never writes,
takes no lock, and answers while a holder runs. With no pools configured it says so and exits 0; a
pool whose entry is invalid is reported with `pool_config_invalid` and the command exits 1.

## What gets recorded

Each pooled tick appends events to the session's log, naming the pool and the state:
`pool_queued` once when the session joins a queue, `pool_acquired` when it takes a slot, and
`pool_released` with a reason when it gives one up or leaves a queue:

| Reason | When |
|--------|------|
| `done`, `failed`, `timed_out`, `lost` | The action or its run ended that way |
| `cancelled` | `koto cancel` or session removal stopped the run |
| `lapsed` | The waiter didn't tick for `lapse_secs` |
| `left_state` | The session left the state while it waited |
| `pool_removed` | The pool was removed from the config while the session waited |

A background run adds `background_started` and `background_finished`, each with its `run_id`.

## Compile errors

| Written | Error names |
|---------|-------------|
| `pool:` with an invalid name, or with `{{VAR}}` | the state and `pool` |
| `size`, `claim_secs`, `lapse_secs` or `wait_secs` under a state or its action | the state and the key |
| `timeout_secs` without `background: true` | the state and `timeout_secs` |
| `background: true` without `timeout_secs` | the state and `background` |
| `timeout_secs` of 0, a negative or fractional number, or above 86400 | the state and `timeout_secs` |
| `background` with `polling` or `requires_confirmation` | the state and both fields |
| `background: true` on a platform other than Linux and macOS | the state and `background` |

## Platforms

Pools and background actions run on Linux and macOS. On another Unix platform (a BSD, say) a
pooled action on a defined pool stops with an error `__pool__` condition, `error_code: "pool_unsupported_platform"`,
and runs nothing, and the compiler refuses `background: true`. An undefined pool imposes nothing
there either, so a template that names a pool still runs where pools don't. (koto doesn't run
actions at all on non-Unix platforms, so the compile-time refusal is the only part that applies
there.)

On macOS there is no child subreaper, so a process that escapes the runner (a daemon a command
leaves behind) is not killed when the run ends. It keeps the run's slot until it exits, and
`koto pool status` shows the slot held.

## Older koto releases

A template that uses none of `pool`, `background` or `timeout_secs` compiles to the same bytes and
the same template hash as before the feature existed. A koto release from before pools (v0.38.0
and earlier) ignores the three fields: it runs a pooled action unthrottled, and it reads a session
log holding the pool and background events without error. `test/compat/pool-v0_38_0.sh` checks
both against the released binary.

## Error codes

| Code | Where | Meaning |
|------|-------|---------|
| `pool_config_invalid` | `__pool__` or `__action__` condition; `koto pool status` | A `[pools.<name>]` or `[background]` value is missing, out of range or unparseable |
| `pool_dir_unavailable` | `__pool__` condition | The pool's directory under the koto root can't be created or is unsafe |
| `pool_unsupported_platform` | `__pool__` condition | The platform isn't Linux or macOS |
| `background_disabled` | `__action__` condition | `[background] enabled = false` on this machine |
| `background_run_in_flight` | moving a session | A background run of the session is still in flight |
