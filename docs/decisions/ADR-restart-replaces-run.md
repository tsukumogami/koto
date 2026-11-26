---
status: Proposed
decision: |
  `koto init NEW --replaces OLD` closes the run it replaces. It runs every
  check that creating NEW, or closing OLD, can refuse on, takes OLD's
  `tick.lock` exclusive with a bounded wait, stops OLD's runner, creates NEW,
  and then closes OLD with cancel's usual events, a `workflow_cancelled` naming
  NEW and the existing `session_superseded` fence marked `replaced: true`, in
  one write. `koto session replace OLD --by NEW` runs the same close for a NEW
  that already exists.
rationale: |
  The fence is the only refusal every build in the field already enforces, so
  it is the only option where a stale agent on an older binary can't rewind or
  supersede the replaced run back to life. The cancel is what every existing
  reader already treats as a stop, so the old directory is released on every
  build. Creating NEW before closing OLD means a bad template can't seal OLD
  with no successor.
---

# ADR: restart replaces run

## Status

Proposed

## Context

Sometimes work restarts as a fresh run under a new name, `<name>-r4` after `<name>-r3`, instead of a supersede. Nothing closes the old run today. It stays wherever it stopped, non-terminal and anchored in its directory, and `koto session needed` holds that directory for as long as the run exists. The only write that closes a run on another's behalf is `koto init --supersede`'s `session_superseded` fence, and every reader ignores the fence. A `workflow_cancelled`, by contrast, is read as a stop on every build by inventory, `koto session hierarchy`, `koto session list --state` and `needed`. But `koto rewind` never checks for a cancel. A stale agent can therefore turn a cancelled run back into one that holds its directory, which `koto next` refuses and no ordinary verb closes again.

A fenced log refuses every append except `context_read` on every build since the fence shipped, and that includes rewind, cancel, rebind and supersede. `koto init --supersede` checks its predecessor only for a fence, never for a cancel. `koto status` reads a kept cancelled session as `is_terminal: false` with no `status` field. Two limits are fixed. Supersede stays unchanged, and on-disk changes stay additive, so older builds keep reading newer logs correctly.

The question is how a restarted run names the run it replaces, what closes the old run, and what readers report after.

## Assumptions

- Every build since the supersede fence shipped refuses appends to a fenced log. A build older than that would still let a stale agent rewind a replaced run.
- No koto path re-serializes existing event lines when it copies or pushes a log, so additive fields survive on older builds.
- In the takeover case the new run is anchored in a different directory from the old one.
- The sidecar `tick.lock` from the rebind tick exclusion ADR ships first, or together with this change.
- A flock is released when its process exits, and a gate's child processes don't inherit `tick.lock`. Nobody has confirmed this by a run.

## Decision

**`koto init NEW --replaces OLD` closes OLD with cancel's events plus the existing fence, under OLD's tick lock.**

1. **Checks.** `koto init NEW --template <t> --replaces OLD` starts by running every check that creating NEW can refuse on: the template compile, variables, the name and the directory. It checks OLD too: OLD exists, isn't NEW, and isn't already fenced by another session. It refuses up front when `KOTO_TICK_SESSION` or `KOTO_BACKGROUND_OF` names OLD, since a tick or a background command of OLD would wait on its own lock or stop its own runner. That guard applies under `--abandon-tick` too.
2. **The lock.** It takes OLD's `tick.lock` exclusive, waiting up to `--wait <secs>` (10 seconds by default; `--wait 0` refuses at once). Past the bound it refuses `session_busy` with exit 1 and writes nothing. The message says a tick is in flight on OLD and that stopping its `koto next` frees the lock.
3. **The runner.** Holding the lock, it stops OLD's local background runner, as `koto cancel` does. A tick that was in flight before the lock was taken may have started a runner and exited, so the stop has to come after the lock, not before.
4. **`--abandon-tick`.** It closes OLD without the lock, for a predecessor the caller can't stop: one on another host, a detached gate command, a process that won't die. The output reports `tick_in_flight: true`. Only the fence holds OLD's later writes off, and the runner is stopped once more after the close, in case the in-flight tick started one.
5. **Order.** Holding the lock, the command creates NEW with a `koto/replaces` key written as part of NEW's creation. NEW records no `continues` relation and carries nothing over. It closes OLD last, then releases the lock. A re-run that finds `koto/replaces` on NEW finishes the close. If a NEW exists without the key, `koto session replace` repairs it. A re-run reports `unchanged` only when OLD's fence is `replaced: true`, names NEW, and OLD reads cancelled or terminal. A plain supersede fence naming NEW is refused, not reported `unchanged`.
6. **The write.** A dedicated close writer appends to OLD's log in one buffer and one sync, under OLD's manifest lock and then its append lock, with bounded waits. In order, it writes:
   - the events `koto cancel` would write first: resolving a self-attached request leg to failed, and leaving pool queues with `pool_released`;
   - `workflow_cancelled { state, reason: "replaced by NEW", replaced_by, replaced_by_session_id }`;
   - `session_superseded { by, by_session_id, actor, at, replaced: true }`.
   The other events have to land before the fence, because the fence refuses them after it. The new fields are additive and optional. There is no new event kind and no new enum value.
7. **A torn write.** Any unparseable last line blocks every later append, and the fence refuses a cancel. So recovery is the close writer's job, not an ordinary re-run's. Under the append lock it trims its own torn tail, keeping a backup as the existing merged-line repair does. It then appends whatever is missing. That includes appending the cancel past a fence, but only when the fence is `replaced: true` and names NEW.
8. **Cloud.** Every line goes in the one conditional PUT the supersede fence already uses, and in the local adopt of a diverged copy. Both use the same writer, so no host's copy ends up fenced but not cancelled. Today `push_fence` and `adopt_fenced_copy` write the fence alone, and both change.
9. **The awkward cases.**
   - An already-terminal or already-cancelled OLD gets the fence only. A fence changes nothing any build reports today, but it still blocks a rewind, which `koto rewind` allows even on a finished run.
   - An OLD that was cancelled and then rewound gets a fresh cancel plus the fence, written by the close writer, not through `koto cancel`'s "already cancelled" refusal. That cancel becomes the last standing event, so every build reads OLD as cancelled.
   - OLD's children are reported in the output, as `koto cancel` reports them, and are neither refused nor cancelled. A child of OLD that finishes later can't append `ChildCompleted` to OLD's fenced log, so its cleanup is put off until someone cleans the child up directly. The output says so.
10. **`koto session replace OLD --by NEW`** runs the same close for a NEW that already exists. That covers restarts made before this flag, runs made by `koto session start` or import, and repair. It reads NEW's session id from NEW's header.
11. **Readers.**
    - On every build, inventory, hierarchy, `list --state` and `needed` read OLD as cancelled (or terminal) and stop holding its directory.
    - On new builds, `koto status` for a cancelled OLD carrying a `replaced: true` fence reports `is_terminal: true`, `status: "cancelled"` and `replaced_by`. An OLD that was already terminal keeps its terminal shape and takes `replaced_by` from the fence's `by`. No existing session reads differently, because no existing log carries the field.
    - New builds also learn four refusals or skips. The decision chain stops at a `replaced: true` fence, so NEW isn't read as an unconfirmed supersede successor. Supersede's re-run check refuses a `replaced: true` fence. `koto init OLD --replace-terminal` refuses a log with a replaced fence. The wake ring, and stale-wake recovery, don't follow a replaced fence.
    - The supersede and `--replace-terminal` refusals cover an on-disk state no existing log contains, so behavior on every existing input is unchanged.
    - The fence is checked in `append_under_lock` next to the rebind anchor fence, and the closed-run refusal comes first, since it names where to continue. The CLI pre-checks that print "superseded ... continue in" today learn the same wording for a replaced fence.

## Rationale

The rule that a stale agent can't revive the replaced run by an ordinary verb has no build qualifier. The stale agent is also the one most likely to be running an older binary. Of the options weighed, only the existing fence is refused at the log by every build already in the field. It refuses rewind, rebind, next, cancel and supersede alike, without any change to supersede's code. A cancel with a `replaced_by` field and no fence is read correctly by every build. But on an older build it leaves `koto rewind` and `koto init --supersede` landing on the replaced run, and the supersede case forks the work.

Pairing the fence with a cancel is what makes every existing reader see a stop; the fence alone is invisible to them. Writing both at once, with every refusable check run first and NEW created before OLD is closed, removes the case where a bad template seals OLD with no successor. One command at create time is harder to forget than a second verb, and the verb stays for pairs that already exist.

Holding OLD's tick lock means NEW exists only after OLD's last same-host tick has landed. The fence stops later log events. It doesn't stop a gate or action already running, such as a push or a file write that could race NEW. A live polling `koto next` in OLD's directory is also exactly what makes releasing that directory unsafe. The wait was the one point the review left split. Refusing by default turns a silent race into a visible `session_busy`. A caller who stops the predecessor frees the lock without any flag, and `--abandon-tick` keeps the stuck-gate restart possible as a choice the caller can see.

## Options Considered

### Cancel with a `replaced_by` field, no fence

One line every build reads as a stop, plus a guard on new builds that refuses transitions and rewinds after a replace-cancel. Rejected because it meets the no-revival rule only on new builds: on older builds `koto rewind` and `koto init --supersede` still land on the replaced run.

### A new terminal event kind `workflow_replaced`

Rejected because older builds read an unknown event kind as nothing. They keep reporting the run as running and keep letting it advance, which breaks the rule that older readers stay correct. A later variant, a cancel plus the new event, has the same gap on older builds as the option above and adds a torn-write window.

### A separate verb only, run after NEW exists

Kept as the close for existing pairs and for repair, but not as the main path. If the second command is never run, OLD stays open. Once the close is ordered after NEW's creation, a single command loses nothing.

### Reusing lineage: a `continues` relation only, or `--supersede --fresh`

Not taken further. A relation alone leaves OLD drivable and reported as running by status and hierarchy, unless readers change for existing superseded sessions too. Reusing supersede writes its marker, makes NEW a confirmed successor of OLD and changes supersede's surface, which has to stay unchanged.

### Closing without waiting on OLD's tick

A probe, or a bounded wait, that reports an in-flight tick and closes anyway. One reviewer held to this to the end. Its reason was that a predecessor stuck in a polling gate is the most common reason to restart. A close that refuses `session_busy` there fails in the usual case, and the fence already refuses whatever that tick tries to append afterwards. Rejected as the default because the fence doesn't reach a gate or action already running, which would then race NEW with nothing ordering it. It survives as `--abandon-tick`, which is the same behavior asked for by name.

## Consequences

A restart closes its predecessor in one command. `needed` releases the old directory on every build, hierarchy and `list --state` show the old run as stopped, and new builds' `koto status` names the replacement.

What stays open:

- **Writes the fence doesn't cover.** Header rewrites and raw context writes still get past it, but none of them can revive OLD.
- **Cross-host ticks on the cloud backend.** They're ordered against the close only by the conditional PUT, since the tick lock is host-local.
- **Ticks from older builds.** They take no `tick.lock`, so the close can't wait them out; only the fence refuses their later writes. This is the same residual the rebind tick exclusion ADR carries.
- **Builds older than the supersede fence.** They don't refuse anything.

Older builds misread the new log shape in five ways, all of which go in the release notes:

- **The decision chain.** They read the replaced fence as an unconfirmed supersede hop. Where the original run's chain is broken, a reversal from NEW of one of OLD's decisions is accepted as `asserted` today and refused there. With a complete chain, it's refused either way and only the reason code changes.
- **The wake ring.** It rings NEW for requests that name OLD, which costs one stray re-poll.
- **Stale-wake recovery.** It reads NEW's log time for requests that name OLD. That's harmless.
- **A stale supersede naming NEW.** A stale `koto init NEW --supersede OLD` on an older build is taken for an interrupted supersede. It turns the restart into a confirmed supersede, though OLD still isn't revived.
- **`--replace-terminal` on OLD's name.** `koto init OLD --replace-terminal` on an older build treats the cancel as finished and replaces the session without checking the fence. It doesn't revive the old run's log, but it creates a fresh, unfenced run under OLD's name, beside NEW. Today a run that is only superseded refuses this with `session_live`, so the cancel is what opens this route. Of the five, it's the only one that fails open on older builds.

New builds don't have any of these.

Older and newer builds also disagree about `koto status` on a replaced run: older builds show the existing kept-cancelled shape. This change ships after the rebind tick lock, or together with it.

Creating NEW first has a cost. If another session fences OLD between the checks and the close, the command refuses with NEW already created. The output says so and points at `koto cancel NEW --cleanup`.

The name `--replaces` sits next to `--replace-terminal` and `replaced_result`, which mean removal of a same-name session. Help text has to keep the two apart.
