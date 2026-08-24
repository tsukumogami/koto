# Authoring a state's `default_action`

A template state can declare a command the engine runs itself, on entering the state, before
that state's gates are evaluated. That's `default_action`. It's how a workflow does the
mechanical step -- read the branch name, create the directory, run the formatter -- instead of
writing prose asking an agent to do it and then gating on whether it did.

This guide answers two questions, and it's the one place both are answered:

1. **May the engine run this command?** The rule is below, and it's worth reading before the
   schema. It's the question authors get wrong, and getting it wrong is not recoverable by
   anything koto ships later.
2. **How do I write the action?** The field, how the command is invoked, where it runs, what
   happens to its output, what happens when it fails, and how a failing action interacts with
   the state's gates.

## Part 1: which commands the engine may run

### The rule

**Does the command's risk live in a bad success, or only in a bad failure?**

Keep `default_action` off any command whose *successful* exit is itself the irreversible,
externally visible event: creating, publishing, or closing a pull request, posting a comment,
marking a draft ready for review. Allow it for a command whose only irreversibility is bounded
and repairable after a successful run.

The joint is whether **success itself** creates the harm, or only a **bad run** does.

### Why the two sides are treated differently

Risk in a bad failure is answered by koto's failure path. A command that fails stops the tick at
the state that ran it, hands the agent the command's own exit status, stdout, and stderr, says
which kind of failure it was, and delivers the author's `fallback` prose alongside. Nothing
advances, and the agent finds out in the same tick. A bad failure is a diagnosis problem, and
diagnosis is what Part 2 describes.

Risk in a bad success is answered by nothing, and not because nobody has written it yet. No
signal that arrives after a command succeeds can un-fire an event that already happened. A
review request already landed in someone's inbox. A pull request number was already consumed.
A webhook already fired and a CI run already started somewhere else. That's why the first
category is permanent rather than pending a future koto capability: there is no capability to
wait for.

### Reversibility has two axes, and only one of them governs

The mistake is asking "can this be undone?" and stopping at the local artifact. Two different
questions hide under that phrasing:

- **Can the local artifact be reset?** Usually yes. A branch can be deleted, a file rewritten, a
  pull request closed.
- **Did the command fire an externally visible event that cannot be un-fired?** This is the one
  that decides.

Closing a pull request undoes its *state*. It does not undo the "opened" notification every
watcher already received, the number the repository already spent, or the CI run that already
started. The artifact is reversible; the event is not. When the two answers disagree, the second
one wins.

### Worked example: `gh pr create` is permanently agent-run

An author wants the engine to open the pull request, so the workflow doesn't have to ask the
agent to do it and then gate on whether it did. Apply the rule: where is the risk?

A *failed* `gh pr create` is harmless and well handled -- no pull request exists, the tick stops,
the agent reads the error and the fallback prose. That's the good case.

A *successful* `gh pr create` is the event. Reviewers are notified, the number is allocated, and
subscribed automation reacts. If the workflow opened it against the wrong base, or with a body
assembled from a state the run should never have reached, no later signal recalls it. Closing the
pull request afterwards leaves every notification already delivered.

So `gh pr create` stays with the agent -- permanently, not until some future release. The same
applies to `gh pr comment`, `gh pr ready`, `gh pr close`, and anything else whose success is the
announcement. Write the state's directive telling the agent to run it, and gate on the result if
you need to.

### Worked example: `git rev-parse --abbrev-ref HEAD` is engine-runnable

Now the other side. A workflow needs the current branch name in the instructions it hands the
agent two states later.

A successful `git rev-parse --abbrev-ref HEAD` reads a ref and prints a string. It notifies
nobody, allocates nothing, and triggers no external system. Success creates no event to recall.
A *failed* run -- not a git repository, git not installed, a detached HEAD -- is a diagnosis
problem, and the failure path handles it: the tick stops at that state, and the agent gets exit
code 128, git's own stderr, and the author's fallback prose in one response. Part 3 shows the
full template and both responses, run for real.

Read-only commands are the easy case. The rule also clears commands that write, as long as the
write is local and repairable: `mkdir -p build/reports`, `cargo fmt`, a write to koto's own
context store. The context store lives under `~/.koto/sessions/<id>/ctx/`, is local to the
machine unless the operator opted into cloud sync, and koto already writes it autonomously
during batch finalization. The worst a bad run leaves behind is a stale local value. That's
repairable, so it doesn't push the command onto the permanent side. (One asymmetry worth
knowing: `koto rewind` repoints the state pointer and does not unwind context writes, so a
rewind doesn't undo one.)

### A command must also be safe to re-run

Engine-runnable isn't the same as run-once. A state's action executes every time the advance
loop enters that state without evidence -- including each retry tick while the state's gates are
still failing, and each lap of a self-loop. Write the command so a second run is harmless:
`mkdir -p`, not `mkdir`.

### When the classification depends on a claim you haven't checked

Sometimes the answer turns on a factual question about external visibility that you don't
actually know. The rule for that case is fixed: **a command whose classification depends on an
unverified claim about external visibility stays with the agent until the claim is checked.**

koto's own worked instance of this is `git push` to an already-open pull request. The case for
calling it engine-runnable rested on one checkable claim -- that GitHub's `synchronize` event
notifies subscribers meaningfully more quietly than `opened` or `ready_for_review`. GitHub's
published notification documentation doesn't settle it. There's no push or synchronize entry in
the documented notification-reason taxonomy, and the one directional signal available
(`review_requested` fires when review is requested, not on plain pushes) supports an inference,
not a citation. So the classification sits where the burden of proof puts it, and pushing to an
open pull request stays with the agent.

The point isn't the specific answer. It's that "probably quiet enough" is not the same as
"checked", and the rule resolves the difference in one direction rather than leaving it to the
author's confidence on the day.

### Summary

| Command | Side | Because |
|---|---|---|
| `git rev-parse --abbrev-ref HEAD` | Engine-runnable | Success reads a ref; failure is a diagnosis problem the failure path handles |
| `cargo fmt`, `mkdir -p dist` | Engine-runnable | The write is local and repairable |
| A koto context-store write | Engine-runnable | Local to the machine; worst case is a stale value |
| `gh pr create`, `gh pr comment`, `gh pr ready` | Permanently agent-run | Success *is* the unrecallable, externally visible event |
| `git push` to an open pull request | Agent-run for now | Classification depends on a claim about notification volume that isn't checked |

## Part 2: writing the action

### The field

`default_action` is declared on a state, alongside `gates`, `accepts`, and `transitions`:

```yaml
states:
  detect:
    default_action:
      command: git rev-parse --abbrev-ref HEAD
      capture_stdout_as: BRANCH
      fallback: "Read the branch name yourself and carry on with it."
    transitions:
      - target: write_up
```

| Field | Required | Type | Meaning |
|---|---|---|---|
| `command` | Yes | string | The command line, passed to `sh -c` as a single string |
| `capture_stdout_as` | No | string | A name this command's trimmed stdout is delivered under, readable by later states. See Part 3 |
| `fallback` | No | string | Prose the agent reads when the action fails. Written as literal text -- it is spliced after variable substitution and is never expanded |
| `working_dir` | No | string | A **relative** path under the session's execution anchor. Absolute values are rejected |
| `requires_confirmation` | No | bool | After a *successful* run, stop and ask the operator to confirm before transitioning |
| `polling` | No | map | `interval_secs` and `timeout_secs`. Re-runs the command on an interval, re-evaluating the state's gates between runs, until they pass or `timeout_secs` expires |

### How the command is invoked

The string is handed to `sh -c` as one argument. Shell syntax works -- pipes, `&&`, redirection --
because a shell is genuinely doing the parsing. The child is placed in its own process group, so
a timeout kills the whole group rather than leaving orphans behind. It inherits the environment
of the `koto next` process.

**Every single run gets 30 seconds**, and that isn't configurable. On expiry the process group is
killed and the failure reports `timed_out`. `polling` doesn't change this: `timeout_secs` bounds
how long koto keeps *retrying* the command, while each individual attempt still gets its 30
seconds. So `polling` is for a condition that needs waiting out -- re-run this until the state's
gates pass -- not for one long command. A single command that can't finish in 30 seconds isn't
something the engine can run; leave it with the agent.

`{{VARIABLE}}` references in `command` are substituted before the shell sees the string, in the
shell-safe form: values pass an allowlist (letters, digits, `. _ - / : @` and spaces) that
excludes every character able to start a command, expansion, or redirection. The allowlist blocks
injection, not word splitting -- quote the reference when a value must stay a single argument:

```yaml
command: mytool --calendar "{{CALENDAR}}"
```

The two runtime names resolve here too, but the allowlist above does not apply to them. It is
enforced on a declared variable when its value is bound at `koto init`; the runtime names are
never bound that way, and are replaced by a separate pass that validates nothing.

What makes them safe is validation at creation instead. `{{SESSION_NAME}}` is the session's own
name, and a session name is checked before the session can hold anything: a letter, then letters,
digits, and `. _ -` (an internally generated epoch-branch name may also carry a `~`). None of
those are shell-special mid-word, and a name cannot begin with the one that would be, so it is
safe unquoted. `{{SESSION_DIR}}` is the path to the session's
directory: the sessions base with that validated name on the end. Nothing constrains the base --
it is the operator's home directory, and can contain a space. Quote it.

```yaml
command: koto context add {{SESSION_NAME}} findings.md --from-file "{{SESSION_DIR}}/findings.md"
```

`working_dir` resolves both names as well, but `{{SESSION_DIR}}` is not usable there: it resolves
to an absolute path, and a `working_dir` must be relative so it can be resolved against the
execution anchor. koto refuses the action and says so.

A gate on the same state reads the same references, which is what lets an action and the gate
beside it agree about a context key scoped to the session:

```yaml
default_action:
  command: 'koto context add {{SESSION_NAME}} {{SESSION_NAME}}-note --from-file note.txt'
gates:
  has_note:
    type: context-exists
    key: "{{SESSION_NAME}}-note"
```

A gate's `key` takes the plain form rather than the shell-safe one -- it is a store key, not a
shell word -- so an empty value renders as nothing and `key: "{{PREFIX}}note"` with an empty
prefix asks for `note`. A `context-matches` `pattern` escapes each value it substitutes, so the
value matches itself and the regex you wrote around it is the only regex in play.

### One command the engine refuses: `koto next`

Automating a workflow's own bookkeeping by having a state tick itself looks like the obvious
move, and it is the one thing a command may not do. A command runs inside a tick. A `koto next`
started from inside it performs a real transition -- in the case that prompted this rule it drove
the session all the way to its terminal state -- while the tick that spawned it finished against
the snapshot it started with and answered `advanced: false` on the state the session had already
left. The caller's view was wrong, not absent, so nothing surfaced an error.

koto now refuses the nested call: it exports `KOTO_TICK_SESSION` before running anything, and a
`koto next` that sees it fails with the `nested_invocation` code and exit 2, naming the session
the marker came from. The refusal is scoped to the process tree rather than to one session name,
so ticking a *different* workflow from a command is refused too -- a chain that ticks back into
the outer session through a second one lands on the same defect.

The marker is inherited and has no liveness behind it, which matters if you write a command that
detaches. koto kills a timed-out command by its process group; a command that called `setsid` or
backgrounded itself is no longer in that group, so it survives the kill and keeps the marker for
as long as it runs. A `koto next` it issues after the tick exits is refused by a tick that is
already gone. The refusal message names the way out (`KOTO_TICK_SESSION= koto next <name>`), but
the cleaner answer is not to leave processes behind a command: koto stops watching at the
timeout, so anything still running past it is outside the engine's model.

Every other `koto` subcommand still works from a command. `koto context add` and `koto context
remove` in particular are a supported pattern -- clearing a key on a loop-back edge is authored
exactly that way.

### When it runs, and when it doesn't

The action runs when the advance loop enters the state on a tick that carries no evidence for
it. Two consequences:

- A tick that submits evidence into that state (`koto next <name> --with-data ...`, or a gate
  override) **skips** the action rather than re-running it. This is what makes
  `requires_confirmation` work: confirming re-enters the state with evidence, so the command
  doesn't run twice.
- Every other tick that reaches the state runs it again -- gate-blocked retries and self-loops
  included. See "A command must also be safe to re-run" above.

### What directory it runs in

Every gate and action of an accepted tick runs at the session's **execution anchor**: the
directory the session is bound to. Not the directory you happened to type `koto next` in. Part 4
covers anchoring, including what it does not promise.

`working_dir` moves an individual action to a subdirectory of the anchor. It must be relative,
and the resolution happens in a fixed order that matters:

1. An absolute value is refused **before** anything is joined. A literal absolute path is a
   compile error:

   ```
   validation error: state 'detect': default_action working_dir '/etc' is an absolute path;
   working_dir must be relative, and is resolved against the session's execution directory
   ```

   A value that becomes absolute only *after* substitution -- because it came from a variable --
   is refused at run time, as an action failure naming the field.
2. Only then is the value joined to the anchor.
3. The result is canonicalized and refused if it escaped the anchor through `..`.

The order is the whole point. `Path::join` with an absolute argument returns the argument and
discards the base, so a beneath-the-anchor check placed after the join would have nothing left to
catch.

### What happens to its output

Every run appends a `default_action_executed` event to the session log carrying the command, the
exit code, stdout, stderr, and a `truncated` flag. Each stream is bounded at **64KB**; past that
it's cut at a UTF-8 boundary and marked. The command keeps running either way -- both pipes are
drained on their own threads, so a chatty command doesn't deadlock against the kernel's pipe
buffer.

On a **successful** run with no `capture_stdout_as`, that log entry is where the output ends. The
agent never sees it, and no state can read it. If the point of running the command was its
output, you want Part 3.

On a **failed** run, stdout and stderr both reach the agent on the response.

### What happens when it fails

The tick stops at the state that ran the command. No transition occurs, and no later state's
action runs. The response is an ordinary blocked response -- not an error envelope, so `koto
next` still exits 0 -- carrying a blocking condition under the reserved name `__action__`:

```json
{"action":"gate_blocked","advanced":false,
 "blocking_conditions":[{"name":"__action__","type":"action","status":"failed",
   "agent_actionable":false,"category":"corrective",
   "output":{"command":"git rev-parse --abbrev-ref HEAD","exit_code":128,
             "failure_kind":"nonzero_exit","state":"detect",
             "stdout":"","stderr":"fatal: not a git repository (or any of the parent directories): .git\n",
             "truncated":false}}],
 "directive":"koto could not read the branch name. Run `git rev-parse --abbrev-ref HEAD` yourself, ...\n\nReading the current branch.",
 "state":"detect"}
```

Route on `failure_kind`, never on the wording of a message:

| `failure_kind` | Meaning |
|---|---|
| `nonzero_exit` | The command ran to completion and exited non-zero. The only kind carrying a real `exit_code` |
| `spawn_failed` | The child could not be started -- the tool isn't installed, the path doesn't resolve, `working_dir` was rejected |
| `timed_out` | The command exceeded its timeout and its process group was killed |
| `wait_failed` | Waiting on the child failed, so no exit status was ever obtained |
| `capture_failed` | The command exited zero, but its output could not be delivered under the declared name. See Part 3 |

Every kind other than `nonzero_exit` omits `exit_code` rather than reporting a synthetic `-1`,
which is exactly why the discriminator exists: those outcomes used to share `-1` and had to be
told apart by searching stderr for "timed out".

The state's `fallback` prose is spliced onto the front of `directive`, not into `details`.
`details` can be withheld from a response that has already delivered it, and a fallback the agent
might not receive isn't a fallback. Write it as plain prose telling the agent how to do the step
by hand; it's delivered verbatim, with no `{{...}}` expansion.

All of this arrives in the tick that ran the command. The agent never needs a second `koto next`
to find out why the workflow stopped.

### How a failing action interacts with the state's gates

**The state's gates do not evaluate.** The tick returns before gate evaluation is reached.

The reason matters, because the alternative looks reasonable until you follow it through: a
state's gates judge the work its action did, and if the action didn't happen there's nothing for
them to judge. Were the gates to run anyway, a passing gate could carry the workflow past a
command that failed -- exactly the silent advance the failure path exists to prevent.

This holds for a state with no gates at all, which is the case an author is most likely to write
first, and the case that would otherwise detect nothing whatsoever. It also holds in the other
direction: because the failure returns ahead of the gate block, an action failure is never
*detected by* a gate. There's one path, and it reports once.

The ordering against `requires_confirmation` is the same shape. Failure is classified first, so a
failing action stops as a failure whether or not the flag is set, and the confirm stop is reached
only after a successful run.

## Part 3: capturing output into a name

### Declaring and reading a name

`capture_stdout_as: NAME` delivers the command's trimmed stdout under `NAME`, for states entered
after the command ran -- in a later tick, and in the same tick when the engine auto-advances from
the producing state through to the reading one.

The name is its own declaration. It deliberately does **not** go in the template's `variables:`
block, because every declared variable is materialized by `koto init` -- so a run that never
entered the producing state would render the reference as an empty string, which is the outcome
this design exists to prevent.

A delivered name can be read anywhere a variable can: a later state's directive or details text,
a `vars.NAME: {is_set: true}` when-clause, a gate command string, a later state's action command.
Against a capture, `is_set` answers "has the producing command run yet".

Know the constraints before you pick the command, because they're what decides whether the
command is usable here at all. The trimmed output must be non-empty, at most **4096 bytes**, and
made only of characters the value allowlist accepts: `^[a-zA-Z0-9._/:@ \-]*$`. That forbids
newlines, so **a multi-line capture is not representable** -- pipe the command through something
that yields one line. Output that breaks any of the three is a failure, not a warning, and the
next two sections give the numbers and the failure shapes in full.

The compiler validates every `{{KEY}}` reference against the union of the variables block, every
state's capture name, and the runtime names, and rejects a capture name that collides with a
declared variable, a reserved runtime name (`SESSION_NAME`, `SESSION_DIR`), or another state's
capture. Two states delivering the same name is a compile error, so "which one wins" never
arises.

### A worked example that runs

The whole template. It reads the branch name in one state and renders it in the next:

```markdown
---
name: branch-report
version: "1.0"
description: Read the current branch, then have the agent write it up
initial_state: detect

states:
  detect:
    default_action:
      command: git rev-parse --abbrev-ref HEAD
      capture_stdout_as: BRANCH
      fallback: "koto could not read the branch name. Run `git rev-parse --abbrev-ref HEAD` yourself, read the output above for why koto's run failed, and carry on with the name it prints."
    transitions:
      - target: write_up
  write_up:
    accepts:
      filed:
        type: enum
        required: true
        values: [ok]
    transitions:
      - target: done
        when:
          filed: ok
  done:
    terminal: true
---

## detect

Reading the current branch.

## write_up

Summarize the work in progress on branch {{BRANCH}}, then submit `filed: ok`.

## done

Reported.
```

Run it from a git checkout on a branch called `feature/anchor`:

```
$ koto init demo --template branch-report.md
{"name":"demo","state":"detect"}

$ koto next demo
{"action":"evidence_required","advanced":true,"state":"write_up",
 "directive":"Summarize the work in progress on branch feature/anchor, then submit `filed: ok`.",
 ...}
```

One tick ran the command, delivered the value, advanced through to the reading state, and
rendered the branch name into the instructions the agent receives. The session log holds both
halves:

```json
{"seq":4,"type":"default_action_executed","payload":{"state":"detect","command":"git rev-parse --abbrev-ref HEAD","exit_code":0,"stdout":"feature/anchor\n","stderr":"","truncated":false}}
{"seq":5,"type":"variable_captured","payload":{"key":"BRANCH","value":"feature/anchor"}}
```

Run the same template outside a git repository and you get the failure response quoted in Part 2:
`failure_kind: "nonzero_exit"`, `exit_code: 128`, git's own stderr, and the fallback prose on the
front of the directive.

### The two bounds, and why they differ

Two size limits apply, they're different numbers, and they do different jobs:

- **64KB per stream** bounds what the *response and the event log* carry, for actions and for
  gates alike. Past it, output is cut at a boundary and marked truncated.
- **4096 bytes** bounds what a *capture* may deliver, measured on the trimmed value.

The capture bound is deliberately far below the response bound. A captured value is a token that
lands in prose and possibly in a shell word, not a transcript. The allowlist already rules out
newlines, so anything approaching 4096 bytes is a template mistake -- the bound is there to say
so early rather than to ration anything.

### The three ways delivery fails

After the command exits zero, koto trims the output and then checks it, in this order:

1. **Empty.** Nothing, or nothing but whitespace.
2. **Too large.** The trimmed value exceeds 4096 bytes.
3. **A character the allowlist forbids.** The same allowlist every declared variable passes:
   `^[a-zA-Z0-9._/:@ \-]*$`. It forbids newlines, which is why a multi-line capture isn't
   representable and why trimming is mandatory rather than a courtesy.

All three are action failures, not skips -- the same stop, the same `__action__` condition, the
same `fallback` prose -- with `failure_kind: "capture_failed"` and a `capture_error` object
naming the case:

```json
{"command":"true","failure_kind":"capture_failed","state":"detect",
 "stdout":"","stderr":"","truncated":false,
 "capture_error":{"key":"BRANCH","case":"empty"}}
```

```json
{"command":"git log -1 --format=%s","failure_kind":"capture_failed","state":"detect",
 "stdout":"fix(x): drop the ; from parsing\n","stderr":"","truncated":false,
 "capture_error":{"key":"BRANCH","case":"disallowed_character","position":3,"character":"("}}
```

The `too_large` case carries `bytes` and `limit` instead of `position` and `character`.

Skipping the delivery was the obvious alternative and it's wrong three times over. A skip is a
silent drop. It doesn't make the problem go away, it defers it to the reading state, where the
message names a variable instead of the command that failed to produce it. And it would mean two
failure models, so an author's `fallback` prose would be delivered for some failures and not
others.

### Reading a name that was never delivered

A state that reads a capture the run never produced stops the tick with the `capture_unset`
error code (exit 3), naming both the value and the state that would have delivered it:

```json
{"error":{"code":"capture_unset","message":"state 'report' reads {{BRANCH}}, which state 'detect' delivers with capture_stdout_as; this run has not entered that state, so the value is unset","details":[]}}
```

It's a stop rather than a rendered empty string or a raw `{{BRANCH}}` token, either of which
would put a placeholder into an agent's instructions. The check runs on each string the response
actually substitutes, so prose that's never rendered is never refused. A name no state declares
at all is caught earlier, when the template compiles. The fix is usually in the template: route
through the producing state, or move the reference to a state that always follows it.

The same stop covers the state's own action. A `{{NAME}}` in `command` or in `working_dir`
is checked before either is substituted, so the token never reaches `sh -c`, nothing is
spawned, and no `default_action_executed` event is written. That message names the field too:

```json
{"error":{"code":"capture_unset","message":"state 'report' has a default_action whose command reads {{BRANCH}}, which state 'detect' delivers with capture_stdout_as; this run has not entered that state, so the value is unset and the command did not run","details":[]}}
```

It's a stop and not an action failure, so your `fallback` prose is not delivered for it — that
prose describes a command that ran and didn't succeed, and here nothing ran. A capture that
*has* been delivered resolves normally, whether the producing state ran earlier in this same
tick or on an earlier one; only a name that is unbound right now is refused.

### Lifetime of a delivered value

| Situation | Behavior |
|---|---|
| The producing state is entered a second time | The command runs again and the later value wins. Captures fold in event order |
| Two states declare the same name | Compile error. It can't happen at run time |
| `koto rewind` past the producing state | **The value stays.** A rewind appends an event and truncates nothing, so nothing removes it |
| A captured value contains `{{...}}` | Not re-expanded. Captures resolve in the final substitution layer, and the allowlist forbids braces anyway |
| The action stopped for confirmation | The capture is still delivered. Confirming re-enters the state with evidence, which skips the action, so capturing only on the unconfirmed path would mean a confirmed action never delivered its value |

The rewind row is the one that surprises people, and it's deliberate rather than accidental: a
rewind moves the state pointer, and it does not unwind what a command already did on disk. The
captured value describes something that really happened.

## Part 4: execution anchoring, and what it does not promise

### What it guarantees

A session records the directory it was created in, and **every tick is checked against it**. The
check runs before the template is read and before any gate or action closure exists, so a refusal
means nothing ran, nothing was evaluated, and nothing moved.

Standing in a subdirectory of the anchor is fine -- being beneath the anchor satisfies the check,
because working from a subdirectory is ordinary and isn't the hazard. The hazard is ticking a session
from a *different* tree. Every gate and action of an accepted tick runs at the anchor itself, so
a command means the same thing no matter where in the tree you typed `koto next`.

Ticking from a different tree is refused with `execution_anchor_mismatch` and exit 2, naming the
directory the session is bound to:

```json
{"error":{"code":"execution_anchor_mismatch","message":"workflow 'demo' is bound to /home/dev/repo; `koto next` must run from that directory or one beneath it, not /tmp/elsewhere. Run `koto session rebind demo --to <dir>` if the checkout moved","details":[]}}
```

An anchor that no longer resolves on this machine is a different code, `execution_anchor_unresolvable`
(exit 3), because the repair differs: change directory for the first, rebind for the second.

### What it does not do

**Anchoring is not containment, sandboxing, or isolation, and it does not bound what a command
can touch.** It binds where a session's commands *start*. Once a command is running it can name
absolute paths, change directory, or reach anything the invoking user can reach, and nothing here
stops it. Don't read the guarantee as more than it is, and don't describe it to anyone else as
more than it is.

That's a deliberate boundary, not an unfinished one. Loading a workflow is itself the grant:
invoking a koto-backed workflow authorizes the commands that workflow bakes in. That relocation
of consent -- from prompting per command to the decision to run the workflow -- is what lets koto
carry mechanical work at all. Which is also why Part 1's rule carries the weight it does: the
rule is what decides which commands get that grant, and there's no second line of defense behind
it.

### How the anchor is compared

Both paths are compared in canonical form. That resolves `.`, `..`, and symlinks, and strips
trailing slashes:

| Path variant | Satisfies the anchor? |
|---|---|
| A symlink to the anchor, or a symlinked ancestor | Yes -- canonicalization resolves it to the same directory |
| A trailing-slash variant (`/home/dev/repo/`) | Yes -- canonicalization strips it |
| A path differing only in case (`/home/dev/Repo` against an anchor of `/home/dev/repo`) | **No** |

**Comparison does not case-fold, on any platform.** Where the filesystem treats two casings as
two directories, koto treats them as two directories too, and the mismatch is refused -- there's
no platform on which the comparison quietly becomes case-insensitive. Where the filesystem itself
is case-insensitive, the two spellings name the same directory, so there's no case-differing path
to refuse in the first place.

"Beneath" is compared component-wise, so `/home/dev/repo-2` is not beneath `/home/dev/repo`. A
working directory that can't be canonicalized at all is compared as given, which fails closed:
the tick is refused rather than accepted on a path koto can't vouch for.

### Which directory a session is anchored to

| Session | Anchored to |
|---|---|
| Created by `koto init` | The directory `koto init` ran in |
| Created by another session (`koto session start --parent`, batch spawning) | **The parent's recorded anchor**, copied explicitly -- not the working directory of whatever process did the spawning. Only if the parent records none does the child fall back to the spawning process's directory |
| Either, with `--execution-dir` | The directory that flag names, which wins over both |

The child rule matters because a child session isn't created from a directory a developer is
standing in. Inheriting the parent's anchor is what keeps a whole workflow tree pointed at one
checkout, regardless of where the spawning agent happened to be.

### Rebinding, and sessions that predate anchoring

A developer whose checkout genuinely moved rebinds the session with one deliberate command:

```
koto session rebind <session> --to <dir>
```

`--to` is optional and defaults to the directory you run the command in, which is the common
case: stand in the checkout you moved to and rebind. The target is canonicalized before it's
recorded, and a directory that doesn't resolve is refused there and then rather than written and
refused on every tick afterwards.

This is the only verb that changes an anchor, and the move lands in the log as an
`execution_anchor_rebound` event carrying the directory the session left and the one it landed
on, so a rebind is auditable rather than silent. Rebinding to the directory the session is
already bound to writes nothing and reports `"rebound": false`. It works on any session,
including one created by another session, and rebinding a child doesn't move its parent.

Both refusals name it, but the repairs still differ: the mismatch refusal can also be repaired by
just running from the anchor, while the unresolvable one leaves rebinding as the only way out
short of restoring the tree. Route on the error code rather than the message text.

A session created before anchoring existed has no recorded directory, so it adopts the one it's
first ticked from, records an `execution_anchor_adopted` event, and says so once on the
directive:

```
[koto] Session 'demo' had no recorded directory; it is now bound to /home/dev/repo. Later ticks must run there or below it -- `koto session rebind demo` moves it.
```

It doesn't refuse, and it doesn't adopt silently. The next tick finds the anchor recorded and
takes the ordinary path. The notice is there because the adopted directory is whatever tree
happened to be current, which may not be the right one.

## Related

- [Error codes and failure kinds](../reference/error-codes.md) -- the machine-readable
  vocabulary: error codes, exit codes, `failure_kind` values, and the anchoring refusals
- [Session feed data contract](../reference/session-feed.md) -- `default_action_executed`,
  `variable_captured`, `execution_anchor_adopted`, and `execution_anchor_rebound` payloads
- [Template format](../../plugins/koto-skills/skills/koto-author/references/template-format.md) --
  the full template surface this action sits inside
