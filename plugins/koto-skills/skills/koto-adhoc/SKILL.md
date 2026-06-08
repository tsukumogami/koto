---
name: koto-adhoc
description: |
  Decompose a novel, one-off complex task into a koto workflow and run it immediately via `koto init --from-stdin`, with no template file to author or commit. Use when you (or the person directing you) hit a multi-phase task that needs ordered, recoverable, auditable execution but has no matching template — for example "break this migration into steps and run it under koto" or "I want koto to govern this investigation." Self-activate when you're about to drive a complex task that would benefit from koto's state machine and no template exists yet. For a workflow you'll run again and again, author a durable template with koto-author instead.
---

# koto-adhoc

Some tasks are complex enough to want koto's guarantees — ordered phases, recoverable state, an audit trail — but they're one-offs. There's no template, and writing a durable, committed one would be overkill. This skill closes that gap: you decompose the task in front of you into a workflow, pipe it straight into koto, and run it. No scratch file, no separate compile step.

The path is `koto init <name> --from-stdin`: you author a definition, pipe it on standard input, and koto strict-compiles it into the session directory and starts the session in one invocation. From there, running it is identical to any other koto workflow.

## When this applies (and when it doesn't)

Use koto-adhoc when **all** of these hold:

- The task has multiple phases that must run in order, or branches on a decision you'll make partway through.
- You want resumability and an audit trail (the session records every transition).
- No existing template matches the task.
- It's a one-off. You don't expect to run this same shape of workflow repeatedly.

Don't reach for it when:

- The task is a single linear pass with no decision points or verification boundaries — plain step-by-step execution is simpler, and koto adds overhead.
- You'll run this workflow shape again. That's a durable template; see [Repeated authoring](#repeated-authoring-switch-to-koto-author) below.
- You already have a template for the task — just run it with koto-user.

## Prerequisites

- koto >= 0.10.0 must be installed and on PATH (`koto version` to verify).

If koto is not installed or the version is too old, install the latest release:

```bash
# Detect platform
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m); [ "$ARCH" = "x86_64" ] && ARCH="amd64"; [ "$ARCH" = "aarch64" ] && ARCH="arm64"

# Download and install
gh release download -R tsukumogami/koto -p "koto-${OS}-${ARCH}" -D /tmp
chmod +x "/tmp/koto-${OS}-${ARCH}"
mv "/tmp/koto-${OS}-${ARCH}" ~/.local/bin/koto
```

## The shape of the work

1. **Decompose** the task into states, transitions, and gates against the quality bar below.
2. **Write** the definition in the koto template format. This skill does not re-teach the grammar — read the [template format guide](../koto-author/references/template-format.md), the same reference koto-author uses. You'll usually need Layer 1 (structure) and Layer 2 (evidence routing); reach for Layer 3 (gates, self-loops) when the task has a real verification boundary.
3. **Run** it through `koto init --from-stdin`, then drive the workflow with the standard run loop.

### Authoring and running in one step

Pipe the definition to `koto init` with `--from-stdin`:

```bash
koto init my-task --from-stdin <<'EOF'
---
name: my-task
version: "1.0"
initial_state: first
states:
  first:
    transitions:
      - target: done
  done:
    terminal: true
---

## first

Do the first thing.

## done

Complete.
EOF
```

On success koto prints `{"name": "my-task", "state": "first"}` and the session is live. The full flag contract — mutual exclusion with `--template`, the strict-only rule, error shapes — is in the [koto-user command reference](../koto-user/references/command-reference.md#koto-init). Read it once if you haven't.

Two properties of this path matter while you author:

- **It's strict.** The definition must meet koto's current template standard: every gate that a state declares must be routed with a `gates.<name>.<field>` `when` clause. Legacy boolean-only gates are rejected (no `--allow-legacy-gates` on this path). If compilation fails, koto exits non-zero and names the failing element — fix it and re-pipe.
- **The source is persisted for audit.** koto writes your definition into the session directory. Don't embed secrets (tokens, keys) in `command` gate strings or variable defaults; reference `$VAR` or files read at gate-evaluation time instead.

### Running the workflow

Once the session starts, the run loop is exactly the same as for any koto workflow — this skill does not re-teach it. Follow **koto-user**: call `koto next <name>` in a loop, dispatch on the `action` field, submit evidence with `--with-data`, and stop at `action: "done"`. `koto rewind <name>` walks back a state if you need to redo one.

## Decomposition quality bar

A workflow that compiles isn't automatically a good workflow. Run your decomposition through this checklist before you pipe it.

**State granularity**

- One state per distinct phase of *agent work* — a unit you'd hand off, checkpoint, or resume at. Not one state per shell command.
- No monolithic state that hides the whole task behind a single directive. If a state's directive is "do everything," you haven't decomposed.
- No filler states that exist only to pass through. If a state has one unconditional transition and no gate or evidence, fold it into its neighbor.
- Aim for a handful of states (roughly 3–8 for a typical one-off). Far more usually means you're encoding shell steps; far fewer usually means a state is doing too much.

**When to introduce a gate**

- Add a gate at a **real verification boundary** — a point where the workflow must not proceed until something is objectively true (tests pass, a file exists, a service is reachable). The gate is the machine checking, not you asserting.
- Don't gate on things only you can judge ("the code looks clean"). That's an `accepts` evidence field with a `when` route, not a gate.
- Every gate needs `gates.<name>.<field>` routing on the same state's transitions. A gate with no routing is rejected on the strict path.
- Prefer `context-exists` / `context-matches` gates over `command` gates when checking a path or file that comes from a variable — they don't invoke a shell and avoid injection.

**Branching**

- Introduce a branch only where you'll genuinely make a different decision and the downstream work differs. Branches that reconverge immediately with identical work are noise.
- Branch on submitted evidence (an `accepts` field + `when` clauses), and keep the conditions mutually exclusive — the compiler rejects overlapping routes.

## Worked examples

Both definitions below compile under the strict `--from-stdin` path and start a session.

### Example A: branching

A flaky-test triage. The agent reproduces the failure, classifies the root cause, and routes to the matching fix — three branches that do genuinely different work, each re-gating on a verified fix or looping back to re-classify. This is a good fit for koto-adhoc: real decision points, a clear "verified" boundary on each branch, and no template worth committing for a one-off triage.

```yaml
---
name: flaky-test-triage
version: "1.0"
description: Reproduce a flaky test, classify the cause, and route to the right fix
initial_state: reproduce

states:
  reproduce:
    accepts:
      reproduced:
        type: boolean
        required: true
    transitions:
      - target: classify
        when:
          reproduced: true
      - target: cannot_reproduce
        when:
          reproduced: false
  classify:
    accepts:
      cause:
        type: enum
        values: [race_condition, ordering_dependency, external_service]
        required: true
    transitions:
      - target: fix_race
        when:
          cause: race_condition
      - target: fix_ordering
        when:
          cause: ordering_dependency
      - target: fix_external
        when:
          cause: external_service
  fix_race:
    accepts:
      verified:
        type: boolean
        required: true
    transitions:
      - target: done
        when:
          verified: true
      - target: classify
        when:
          verified: false
  fix_ordering:
    accepts:
      verified:
        type: boolean
        required: true
    transitions:
      - target: done
        when:
          verified: true
      - target: classify
        when:
          verified: false
  fix_external:
    accepts:
      verified:
        type: boolean
        required: true
    transitions:
      - target: done
        when:
          verified: true
      - target: classify
        when:
          verified: false
  cannot_reproduce:
    terminal: true
  done:
    terminal: true
---

## reproduce

Run the failing test in a loop until it fails at least once. Submit `{"reproduced": true}` once you have a confirmed failure, or `{"reproduced": false}` if it passes consistently across many runs.

## classify

Inspect the failure. Decide the root cause: `race_condition` (shared state without synchronization), `ordering_dependency` (the test relies on another test running first), or `external_service` (a network or service dependency). Submit `{"cause": "..."}`.

## fix_race

Add the missing synchronization, then re-run the test in a loop. Submit `{"verified": true}` if it now passes consistently, or `{"verified": false}` to re-classify.

## fix_ordering

Make the test self-contained so it no longer depends on execution order, then re-run. Submit `{"verified": true}` if stable, or `{"verified": false}` to re-classify.

## fix_external

Stub or mock the external dependency, then re-run. Submit `{"verified": true}` if stable, or `{"verified": false}` to re-classify.

## cannot_reproduce

The test could not be reproduced as failing. Document the runs attempted and close the investigation.

## done

The flake is fixed and verified stable.
```

Why it passes the bar: each state is a distinct phase; the three causes are mutually exclusive enum routes; the `verified` self-loop lets a failed fix re-classify instead of dead-ending; and `cannot_reproduce` is an honest terminal for the no-repro case.

### Example B: linear with a gate

A dependency bump with a verification gate. The work is mostly linear — edit the manifest, run the suite, record the bump — but it must not reach the changelog until the test suite is objectively green. That's a real verification boundary, so it's a `command` gate routed on exit code, with a self-correcting loop back through `fix_breakage`.

```yaml
---
name: dependency-bump
version: "1.0"
description: Bump a dependency, then gate the merge on the test suite passing
initial_state: edit_manifest

states:
  edit_manifest:
    accepts:
      bumped:
        type: boolean
        required: true
    transitions:
      - target: run_tests
        when:
          bumped: true
  run_tests:
    gates:
      suite:
        type: command
        command: "test -f {{SESSION_DIR}}/tests_passed"
    transitions:
      - target: update_changelog
        when:
          gates.suite.exit_code: 0
      - target: fix_breakage
        when:
          gates.suite.exit_code: 1
  fix_breakage:
    accepts:
      fixed:
        type: boolean
        required: true
    transitions:
      - target: run_tests
        when:
          fixed: true
  update_changelog:
    accepts:
      recorded:
        type: boolean
        required: true
    transitions:
      - target: done
        when:
          recorded: true
  done:
    terminal: true
---

## edit_manifest

Update the dependency version in the manifest and lockfile. Submit `{"bumped": true}` once the manifest reflects the new version.

## run_tests

Run the full test suite. The `suite` gate passes when the suite is green: write a marker file with `touch {{SESSION_DIR}}/tests_passed` after a clean run so this gate can verify it. If the gate fails, you'll route to `fix_breakage`.

## fix_breakage

The suite failed against the new version. Diagnose the breakage, apply the fix, re-run the suite, and refresh the marker. Submit `{"fixed": true}` to re-gate.

## update_changelog

The suite is green on the new version. Record the bump in the changelog with the old and new versions. Submit `{"recorded": true}`.

## done

The dependency is bumped, verified against the test suite, and recorded.
```

Why it passes the bar: the gate sits at the real boundary (the suite must be green before the changelog), it routes on `gates.suite.exit_code` so the engine resolves it without an agent assertion, and the `fix_breakage` loop re-runs the gate rather than letting the agent declare success.

## Repeated authoring: switch to koto-author

The `--from-stdin` path is for *one-off* tasks. It is deliberately not a shortcut for building durable workflows — keep the ephemeral path ephemeral.

If you notice you're authoring the **same workflow shape more than once** — re-piping a near-identical definition for tasks that recur, or copying a previous ad-hoc definition as a starting point — stop and author a durable template with **koto-author** instead. koto-author produces a committed template plus a paired SKILL.md, which is the right home for a workflow you'll run repeatedly: it lives in version control, gets reviewed, and is run with koto-user every time. Using `--from-stdin` as a standing substitute leaves no reusable artifact and re-pays the authoring cost on every run.

Rule of thumb: first time, ad-hoc; second time you reach for the same shape, make it durable.

## Troubleshooting

**"koto: command not found"** — koto isn't on PATH. Install it or add its directory to PATH.

**Compilation fails on `--from-stdin`** — koto exits non-zero and names the failing element (a state, transition, or gate). Common causes: a state declared in the frontmatter with no `## state` body section; a gate with no `gates.<name>.<field>` routing on its transitions (rejected on the strict path); overlapping `when` conditions on two transitions from the same state. Fix the named element and re-pipe.

**"session already exists"** — a session with this name is already active. Resume it with `koto next <name>`, or cancel and re-init: `koto cancel <name> --cleanup` then re-pipe.

**The workflow runs but a state does nothing useful** — that's a decomposition problem, not a koto error. Re-read the [quality bar](#decomposition-quality-bar): a state with one unconditional transition and no gate or evidence is filler and should be folded into its neighbor.
