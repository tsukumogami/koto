---
schema: design/v1
status: Current
problem: |
  koto writes its `/workflows` entry against four Claude Code conventions that
  Claude Code doesn't publish: the workflows file shape, the per-session
  directory layout, the `CLAUDE_CODE_SESSION_ID` variable and the SessionStart
  hook's input fields. Nothing in koto's CI reads a Claude Code release, so a
  release that changes one of them passes CI and reaches operators as an empty
  `/workflows`. The hook script that derives koto's directory is untested and
  emits invalid JSON on hosts without `jq` (koto#243).
decision: |
  A guard script fetches the Linux platform package of a Claude Code release
  from npm, never runs it, and checks its embedded JavaScript against a
  committed expectations file: per convention, a list of probes, each a stable
  anchor literal and a token that must sit near it. A failing probe prints the
  convention and the item. A one-line file records the release last
  confirmed. A dedicated workflow runs the guard on path-filtered pull
  requests against that release, weekly against the newest release, and on
  demand against a named one; a cargo test ties every screen-read field of the
  shape fixture to a probe. A maintainer-run tmux harness opens a real
  `/workflows` and reports rendered, not rendered or unavailable. The hook's
  no-`jq` path escapes its output and parses escaped input, and a payload
  matrix test runs in the plugin CI job.
rationale: |
  The release's own bundle is the only evidence that can be read without
  credentials, on every pull request, and that names what changed; a version
  pin fails on every release, and a live capture needs a paid key and still
  can't see the file reader. Rendering needs a terminal and a login, so it
  belongs on the maintainer's machine, where it costs no model call. The
  schedule closes the gap a pull-request trigger leaves when Claude Code ships
  with no koto commit. Fixing the hook in place keeps it dependency-free.
upstream: docs/prds/PRD-native-workflows-ci-guard.md
decision_provenance: inline-resolved
---

# DESIGN: a CI guard over the Claude Code conventions behind koto's `/workflows` rendering

## Status

Current

## Context and Problem Statement

koto's native `/workflows` rendering (src/workflows_surface/) writes
`koto-<session-uuid>.json` atomically into a Claude Code session's workflows
directory. Four conventions decide whether that file ever reaches the screen,
and all four belong to Claude Code:

1. **File shape.** The screen's reader lists `*.json` under the session's
   workflows directory, parses each as plain JSON, drops a file it can't parse
   with only a debug log line (`Failed to parse workflow snapshot`), and reads
   `status`, `startTime`, `workflowName`, `summary`, `phases` and
   `workflowProgress`, whose nodes are typed `workflow_phase` or
   `workflow_agent` and carry `index`, `label`, `phaseIndex`, `phaseTitle`,
   `state`, `promptPreview`, `resultPreview`, `blocked` and `error`. koto's
   contract (`CONTRACT_VERSION` 5 in contract.rs) writes exactly these, plus a
   `koto` block Claude Code ignores.
2. **Directory convention.** A transcript at
   `~/.claude/projects/<slug>/<session-id>.jsonl` with the workflows directory
   at `<slug>/<session-id>/workflows/`. materialize.rs `resolve_target_dir`
   finds it by scanning for the transcript when neither `KOTO_WORKFLOWS_DIR`
   nor a published location applies.
3. **Session variable.** `CLAUDE_CODE_SESSION_ID`, which Claude Code sets for
   the processes it starts and which koto reads to know its session.
4. **SessionStart hook input.** `session_id`, `transcript_path` and `cwd` on
   the hook's stdin, read by plugins/koto-skills/hooks/session-start-workflows.sh.

The technical gap is that no check in koto reads a Claude Code release. The
shape test (tests/native_workflows_shape.rs) compares koto's serialized output
with a fixture koto generated, so it can't see Claude Code move. The directory
and variable conventions are exercised only against synthetic trees. The hook
script runs in no CI job, and its no-`jq` path is broken twice over: its
`printf` interpolates a context string containing unescaped quotes, and its
grep extractor stops at the first quote, so a path containing `"` is cut short
(koto#243). What koto knows about the conventions was read by hand from Claude
Code 2.1.209, 2.1.296 and 2.1.314 and lives only in design prose; no file
records the release it was last confirmed against.

The PRD this design answers asks for the following, and each requirement is
restated where the design meets it:

- Every convention check reads evidence from a published Claude Code release,
  not only koto's own output (R1), against a release recorded in a file as
  `MAJOR.MINOR.PATCH`, with a named failure when that file is missing or
  malformed (R2).
- One check per convention, all four reporting on every run (R3), each failure
  printing the convention's name and the item that no longer matches, with a
  non-zero exit (R4).
- The recorded release by default, any named release on request, the checked
  release named in the output, and a distinct could-not-obtain failure (R5); a
  weekly scheduled run against the newest release (R6); a documented command
  per convention that demonstrates a failure against an altered input (R7).
- Every screen-read field of the shape fixture covered by the file-shape check
  (R8); the existing shape test kept as koto's own regression check (R9).
- A rendered smoke check against a real Claude Code with three outcomes,
  rendered, not rendered and unavailable, each with its own exit status (R10),
  documented with one real run (R11).
- The hook run in CI with and without `jq`, producing valid JSON that names the
  derived directory exactly, even for paths with quotes, backslashes or
  spaces, and printing nothing for input without `session_id` or that isn't
  JSON; this closes koto#243 (R12).
- Maintainer procedures for extending the guard and moving the recorded
  release (R13), and an advisory release step (R14).
- No credentialed job triggered by pull requests and a named failure when a
  credential is missing (R15); pull-request jobs with no secrets that finish
  within five minutes each (R16); no niwa (R17); one cargo process at a time
  and koto isolated from the maintainer's sessions when run locally (R18).

## Decision Drivers

- **Evidence from Claude Code, not from koto** (R1). The failure this work
  exists to catch is invisible to any check whose inputs koto wrote.
- **Name the convention** (R3, R4). A maintainer reading a red run should know
  which of the four broke and which item, without bisecting.
- **No credentials where pull requests run** (R15, R16). Fork pull requests
  must not spend a key, and the pull-request path must stay fast.
- **Notice a release with no koto commit** (R6). Claude Code ships weekly or
  faster; a trigger tied to koto changes alone sees a break late.
- **The screen is a TUI** (R10). Claude Code 2.1.316 has no non-interactive way
  to list workflows, so "renders" can only be observed in a terminal.
- **Untrusted download.** Any Claude Code package fetched by CI is someone
  else's code; the guard must read it, never run it.
- **No new runtime dependency for koto users.** The hook runs on operators'
  hosts; its fix can't require anything they don't already have.
- **Host limits** (R18). Local runs use one cargo process at a time and keep
  koto away from the maintainer's real session store.
- **Tool independence** (R17). Nothing here may call or read niwa.

## Considered Options

### Decision 1: The guard's form

Resolved inline.

The guard has to compare koto's expectations with something a Claude Code
release provides, name the convention that broke, and run without credentials.
Research on 2.1.316 found that `@anthropic-ai/claude-code` on npm is a 29 KB
wrapper whose real code ships in per-platform packages
(`@anthropic-ai/claude-code-linux-x64` and siblings, about 330 MB unpacked),
fetchable anonymously. Each holds a Bun single-file executable whose embedded
JavaScript is minified but keeps string literals and object keys. On the
installed 2.1.316 binary, the literals koto depends on are all present: the
`Failed to parse workflow snapshot` log line beside the snapshot reader,
`workflow_phase` and `workflow_agent` beside the `workflowProgress` merge code,
`CLAUDE_CODE_SESSION_ID` in the environment registry, and the SessionStart
input schema `hook_event_name:C("SessionStart"),source:U(["startup","resume","clear","compact","fork"])`
alongside a base schema with `session_id`, `transcript_path` and `cwd`.

Key assumptions:

- The bundle stays readable text inside the executable, as it has since at
  least 2.1.296.
- The Linux x64 package carries the same bundle as the macOS one inspected.
- `python3` is available on CI runners and maintainer hosts (the repository's
  verification script already depends on it).

#### Chosen: release-evidence probes over the shipped bundle

A committed expectations file,
`tests/fixtures/native-workflows/claude-code-conventions.json`, lists the four
conventions. Each convention has a name (`file shape`, `directory convention`,
`session variable`, `SessionStart hook input`), a slug for the command line
(`file-shape`, `directory`, `session-variable`, `hook-input`), a list of
probes, and one demonstration mutation. A probe is
`{item, anchor, token, window}`: the guard finds every occurrence of `anchor`
in the release bytes and passes the probe when `token` appears within `window`
bytes on either side of at least one of them. An `anchor` of `null` means the token must
appear anywhere, which the guard allows only for tokens distinctive enough to
mean something alone, such as `CLAUDE_CODE_SESSION_ID`. Anchors are string
literals or stable object keys, never minified identifiers, which change every
release.

A probe fails with a line naming the convention and the item, for example
`FAIL file shape: workflowName not found near "Failed to parse workflow snapshot"`;
a missing anchor fails every probe that uses it with
`FAIL file shape: anchor "Failed to parse workflow snapshot" not found`. All
four conventions are evaluated on every run and a summary line lists each
convention as `ok` or `FAIL`, so one break never hides another.

The release is recorded in `tests/fixtures/native-workflows/claude-code-release`,
one line holding `MAJOR.MINOR.PATCH`. The guard reads it by default; a
missing, empty or malformed file stops the guard with a message naming the
file and exit status 3.

R8 is met offline: a cargo test, `tests/native_workflows_conventions.rs`,
walks `tests/fixtures/native-workflows/enriched-shape.json`, collects every key
outside the `koto` block (the `type` key included) and every `type` value,
and fails naming any that no file-shape probe lists as its `item`. Keys inside `koto` are skipped because
Claude Code doesn't read them. The same test checks the recorded-release file
parses as a version.

#### Alternatives Considered

**Version pin only**: record a Claude Code version and fail whenever npm
publishes a newer one. Rejected because it fails on every release whether or
not anything koto relies on changed, never names a convention (R4), and reads
no evidence from the release (R1).

**Live capture with `claude -p`**: start Claude Code headlessly in CI with a
recorder SessionStart hook, then check the hook's stdin, the environment and
the transcript layout. Rejected as the guard because it needs a paid API key
and a model call, so it can't run on pull requests (R15, R16), and it still
can't observe the file reader, the convention most likely to drift.

**Diff normalized code windows**: snapshot the source text around each anchor
and diff it against the next release. Rejected because the minifier renames
identifiers on every release, so the diff fails on noise rather than on a
change koto cares about.

### Decision 2: Where and when the guard runs

Resolved inline.

The guard downloads a large package, so running it on every pull request
wastes time on changes that can't affect the conventions. It also has to see a
new Claude Code release when koto itself hasn't changed.

Key assumptions:

- Fetching one platform package (cached by version) and scanning it fits well
  inside five minutes on a GitHub-hosted runner.
- A failed scheduled run, which GitHub reports in the Actions tab and by
  notification, is the signal R6 asks for.

#### Chosen: one dedicated workflow with four triggers

`.github/workflows/claude-code-conventions.yml` runs on:

- `pull_request` and `push` to `main`, path-filtered to the files that can
  change the answer: `src/workflows_surface/**`, `src/host_env.rs`,
  `tests/fixtures/native-workflows/**`, the guard script and its helper,
  `plugins/koto-skills/hooks/session-start-workflows.sh`, and the workflow
  file. These runs check the recorded release.
- `schedule`, weekly (Mondays 06:00 UTC), checking the newest release npm
  reports for `@anthropic-ai/claude-code`.
- `workflow_dispatch` with a `version` input (default empty, which the job
  maps to omitting `--version` and so checks the recorded release; `latest`
  meaning the newest). The input reaches the script only as `--version`,
  through an environment variable; `--evidence` and `--alter` are never fed
  from a dispatch input.

The job declares `permissions: contents: read`, references no secret, pins
its actions the way the repository's other workflows do, and caches the
guard's `--cache-dir` (which holds only downloaded tarballs, never extracted
trees) under a key naming the resolved version. The guard checks a cached
tarball against the registry's integrity value before extracting it. The job
runs
`--self-test` before the real check so a broken guard can't pass silently. The
offline coverage test needs no new job: validate.yml's unit-test job already
runs every cargo test.

#### Alternatives Considered

**Guard in validate.yml on every pull request**: always run against the
recorded release. Rejected because it downloads the package on pull requests
that touch nothing the conventions depend on, slowing the main CI path for no
signal.

**Schedule only**: a weekly run and nothing else. Rejected because a pull
request that edits the expectations, the fixture or the hook would merge
unchecked, and a koto-side mistake would surface days later.

**Release-time only**: run inside the release workflow. Rejected because the
break would be found at the last moment instead of before a release is
planned, which is the outcome the PRD's goals name.

### Decision 3: The rendered smoke check

Resolved inline.

Only a real Claude Code can say whether an entry renders. `/workflows` is a
`local-jsx` command drawn in the terminal; 2.1.316 offers no subcommand or
flag that lists workflows, and the command can be disabled by setting,
environment variable (`CLAUDE_CODE_DISABLE_WORKFLOWS`) or account policy.

Key assumptions:

- Opening `/workflows` sends no prompt, so the check makes no model call.
- `claude --session-id <uuid>` and `--settings <file>` exist (listed by
  2.1.316's help).
- The folder-trust prompt for a new directory can be answered by the harness.

#### Chosen: a maintainer-run tmux harness

`scripts/smoke-native-workflows-render.sh` drives a real interactive `claude`
inside a detached tmux session, on the maintainer's machine with their own
login:

1. Check prerequisites: `claude`, `tmux`, `python3` and a built koto. A
   missing one reports *unavailable* and names it.
2. Make a scratch project directory under `$TMPDIR`, a fresh session UUID,
   and a settings file registering `session-start-workflows.sh` as the
   SessionStart hook through a wrapper that also saves its stdin and output
   in the scratch directory.
3. Start `claude --session-id <uuid> --settings <file>` in tmux with the
   scratch directory as its working directory; accept the folder-trust
   prompt; wait up to 30 seconds for the hook's saved output. A login screen,
   any other prompt, or no hook output reports *unavailable* and prints the
   captured pane.
4. Read the derived directory from the hook output, then run koto through
   `scripts/without-session-env.sh` with `KOTO_HOME` and `KOTO_SESSIONS_BASE`
   in the scratch directory and the local backend pinned, as
   scripts/verify-native-workflows.sh does: `koto init` on the `hello-koto`
   test template, `koto workflows publish --dir <derived dir>`, `koto next`.
   The entry's `workflowName` is read back from the written file. With
   `--no-entry`, this step is skipped.
5. Send `/workflows` and Enter, wait for the pane to settle, capture it.
6. Report and exit: **rendered** (status 0) when the captured pane shows the
   entry's `workflowName`; **not rendered** (status 1) when the workflows list
   or its empty state is shown without it; **unavailable** (status 3) when the
   command is reported unknown or disabled, or step 1 or 3 failed. Status 2 is
   a usage error. Every report names the Claude Code release from
   `claude --version`.
7. Clean up: kill the tmux session, remove the scratch directory, and remove
   only the `<uuid>.jsonl` file and `<uuid>/` directory Claude Code created
   under `~/.claude/projects/`, plus that project directory when it is left
   empty. Nothing else under `~/.claude` is touched.

The guide records one real run's output (R11), and the release step asks for
the outcome against the release being shipped (R14).

#### Alternatives Considered

**A credentialed CI job**: the same harness on a `workflow_dispatch` runner
with an API key secret. Rejected for now because whether `/workflows` is
enabled for an API-key organization is outside koto's control, a fresh runner
meets first-run prompts that make the harness brittle, and it adds a paid
secret for a check a maintainer can run in a minute. The script doesn't
preclude it later.

**Headless `claude -p` with a debug log**: write the file, run a print-mode
session with `--debug-file`, and treat the absence of `Failed to parse workflow
snapshot` as success. Rejected because it only shows the file parsed, not that
the screen drew it, and print mode may not load workflows at all.

**A written manual checklist**: extend the guide's existing manual steps.
Rejected because it has no exit status and can't distinguish the three
outcomes R10 requires.

### Decision 4: Fixing and testing the session-start hook

Resolved inline.

The hook runs on operators' hosts, some without `jq`. Its no-`jq` path must
produce valid JSON and read `transcript_path` correctly even when the path
contains a quote, a backslash or a space.

Key assumptions:

- `bash`, `cat`, `dirname`, `grep -E`, `sed` and `head` are on every host the
  plugin supports.

#### Chosen: escape on output, honour escapes on input, test both paths

The script gains a `json_escape` function, covering backslash, double quote
and every control character below U+0020, applied to the context before the
`printf` fallback interpolates it. The no-`jq` `read_field` matches a JSON
string that may contain escapes, `"<name>"[[:space:]]*:[[:space:]]*"(([^"\\]|\\.)*)"`, and
decodes `\"`, `\\` and `\/` in one pass; a value holding any other escape
(`\n`, `\t`, `\u....`) is refused, and a refused `session_id`,
`transcript_path` or `cwd` makes the hook print nothing rather than fall back
to another field, since a fallback would name a directory Claude Code doesn't
use. The
derivation rule doesn't change: the directory is
`<dirname of transcript_path>/<session_id>/workflows`, or
`<cwd>/<session_id>/workflows` when `transcript_path` is absent or empty, and
no `session_id` (including stdin that isn't JSON) prints nothing and exits 0.
Three input rules are new: a `session_id` that doesn't match `[A-Za-z0-9_-]+`
counts as absent, so it can't add path components; a control character in the
derived directory (below U+0020, or DEL) makes the hook print nothing; and the `export` line the
context asks the agent to run quotes the directory for the shell (single
quotes, with an embedded `'` written `'\''`), because JSON escaping alone would
let a directory named with `$(...)` or a backtick run a command. The context's
plain sentence still names the directory unquoted, so the directory appears in
it byte for byte.

A sibling test, `plugins/koto-skills/hooks/test/session-start-workflows.test.sh`,
runs a fixed payload matrix twice: once with the normal `PATH` (and asserts
`jq` is on it), once with a `PATH` built from symlinks to only the commands the
script needs. The payloads: a plain `session_id` and `transcript_path`; a
`transcript_path` whose directory contains `"`, `\` and a space; one whose
directory contains `$(`, a backtick and `'`; an empty `transcript_path` with
`cwd`; no `session_id`; a `session_id` containing `/`; a directory holding a
DEL character; and stdin that isn't JSON. For the first four it parses the output with `jq` from the test's own
`PATH`, checks `hookSpecificOutput.additionalContext` contains the derived
directory byte for byte, and checks that evaluating the export line in a
subshell sets `KOTO_WORKFLOWS_DIR` to exactly that directory without running
anything else; for the last four it checks the output is empty and the status
is 0. validate-plugins.yml's `hook-smoke-test` job runs it as a new step.

#### Alternatives Considered

**Drop the no-`jq` path**: print nothing when `jq` is absent. Rejected because
it silently disables the feature on exactly the hosts the fallback exists for.

**A `python3` fallback**: parse and emit with Python when `jq` is missing.
Rejected because it swaps one optional dependency for another on operators'
hosts.

**A koto subcommand that reads hook stdin**: let koto's Rust JSON parser
derive the directory. Rejected because it adds a koto command surface, which
the PRD puts out of scope, for a script fix of a few lines.

## Decision Outcome

**Chosen: 1 release-evidence probes + 2 dedicated workflow + 3 maintainer tmux harness + 4 in-place hook fix**

### Summary

koto gains a guard script, `scripts/check-claude-code-conventions.sh`, that
reads a Claude Code release without running it. Given the recorded release
(from `tests/fixtures/native-workflows/claude-code-release`), a version passed
with `--version`, or `--version latest`, it fetches
`@anthropic-ai/claude-code-linux-x64@<version>` with `npm pack` into a fresh
directory, extracts the tarball, and hands the executable's bytes to a Python
helper, `scripts/claude_code_conventions.py`, run with `python3 -I`. The helper
loads `tests/fixtures/native-workflows/claude-code-conventions.json`, evaluates
every probe of every convention, prints one line per failing probe and a
summary per convention, and exits 0 when all pass or 1 when any fail. A
registry or network failure, or an unpublished version, exits 2 with
`could not obtain Claude Code <version>` and names no convention. A bad
recorded-release file or expectations file exits 3 naming the file. The first
output line names the release checked.

Two flags make the guard demonstrable. `--self-test` builds a synthetic
evidence blob from the expectations alone (each anchor followed by its
tokens), checks it passes, then applies each convention's demonstration
mutation and checks the run fails naming that convention; it needs no
network. `--alter <slug>` runs the real check after applying that
convention's demonstration mutation to a copy of the downloaded bytes: a
same-length rename of `workflowName`, the quoted `"workflows"` literal,
`CLAUDE_CODE_SESSION_ID` or `transcript_path`. The four `--alter` commands are
R7's documented commands, and their output goes in the implementing pull
request.

A dedicated workflow runs the guard on pull requests and pushes that touch the
surface (recorded release), weekly (newest release) and on demand (named
release), with no secrets and a version-keyed cache. A cargo test keeps the
shape fixture and the file-shape probes in step, so a field added to koto's
file without a probe fails CI by name. The existing shape test stays as the
check that koto writes the shape it means to, and its header stops calling it
the drift guard. The hook's no-`jq` path is fixed in place and covered by a
payload matrix in the plugin CI job, which closes koto#243. A maintainer-run
tmux harness answers whether an entry actually renders, with distinct exit
statuses for rendered, not rendered and unavailable. The verification guide
gains the procedures for running all of this, extending the probes and moving
the recorded release, and the release declarations gain an advisory
before-release step.

### Rationale

The four decisions split the problem along the line the evidence draws.
What the release ships can be read cheaply and without credentials, so that
half runs in CI on every relevant change and on a schedule. What the screen
draws can only be seen in a terminal with a login, so that half runs on a
maintainer's machine at release time. The probes and the smoke check overlap
on purpose: the probes catch a rename the day it ships and name it, while the
smoke check catches what probes can't see, such as a reader that still
mentions a field but stops drawing it. The hook fix sits under both, since the
smoke check uses the hook to find the directory and the guard checks the hook
input fields the script reads.

## Solution Architecture

### Overview

```
                 tests/fixtures/native-workflows/
                 ├── claude-code-release          (recorded version)
                 ├── claude-code-conventions.json (probes per convention)
                 └── enriched-shape.json          (koto's shape fixture)
                          │                │
        cargo test        │                │  guard
  native_workflows_conventions.rs          │
   (fixture keys ⊆ file-shape probes)      ▼
                          scripts/check-claude-code-conventions.sh
                            npm pack claude-code-linux-x64@<v> → extract
                            python3 -I scripts/claude_code_conventions.py
                            → per-probe FAIL lines, per-convention summary

  .github/workflows/claude-code-conventions.yml
    pull_request / push (paths)  → recorded release
    schedule (weekly)            → latest
    workflow_dispatch(version)   → named release

  plugins/koto-skills/hooks/session-start-workflows.sh (fixed)
    └── test/session-start-workflows.test.sh  ← validate-plugins.yml hook-smoke-test

  scripts/smoke-native-workflows-render.sh (maintainer machine)
    tmux → claude --session-id --settings(hook) → koto writes entry → /workflows
    → rendered (0) | not rendered (1) | unavailable (3)
```

### Components

- **Recorded release** — `tests/fixtures/native-workflows/claude-code-release`,
  one line, `2.1.316` at landing time (the release the probes are calibrated
  against).
- **Expectations** — `tests/fixtures/native-workflows/claude-code-conventions.json`:

  ```json
  {
    "schema": 1,
    "package": "@anthropic-ai/claude-code-linux-x64",
    "member": "<path of the executable inside the tarball, set in Phase 2>",
    "conventions": [
      {
        "name": "file shape",
        "slug": "file-shape",
        "demo": {"from": "workflowName", "to": "workflowNamX"},
        "probes": [
          {"item": "workflowName", "anchor": "Failed to parse workflow snapshot",
           "token": "workflowName", "window": 6000}
        ]
      }
    ]
  }
  ```

  The file-shape convention carries one probe per screen-read field and per
  node type; the directory convention probes the `"workflows"` path join and
  the `.json` filter near the snapshot reader; the session variable probes
  `CLAUDE_CODE_SESSION_ID` with no anchor; the hook input probes `session_id`,
  `transcript_path` and `cwd` near the hook input schema and
  `hook_event_name:C("SessionStart")`. Anchors and windows are calibrated
  against the recorded release when the file is written, and `member` names
  the one tarball entry the guard extracts.
- **Guard script** — `scripts/check-claude-code-conventions.sh`, Bash. Parses
  `--version <v|latest>`, `--evidence <file>` (scan a local executable instead
  of downloading, for offline work; it replaces `--version`), `--cache-dir
  <dir>` (where downloaded tarballs are kept, default a fresh temporary
  directory), `--alter <slug>` (combines with either source) and
  `--self-test` (takes no other flag); resolves the version; reads the
  registry's `dist.integrity` for it with `npm view`; reuses a tarball in the
  cache directory only when its sha512 matches, else fetches one with `npm
  pack --ignore-scripts`; extracts `member` with a size cap enforced while
  streaming; calls the helper; maps its result to the exit statuses below.
- **Probe helper** — `scripts/claude_code_conventions.py`, run as
  `python3 -I`, reads the evidence with `mmap` and only ever searches it.
- **Coverage test** — `tests/native_workflows_conventions.rs`, offline, run by
  `cargo test`.
- **Workflow** — `.github/workflows/claude-code-conventions.yml`.
- **Hook fix and test** — `plugins/koto-skills/hooks/session-start-workflows.sh`
  and `plugins/koto-skills/hooks/test/session-start-workflows.test.sh`, plus a
  step in validate-plugins.yml's `hook-smoke-test` job.
- **Smoke harness** — `scripts/smoke-native-workflows-render.sh`.
- **Documentation** — new sections in
  `docs/guides/native-workflows-verification.md`, a `## Before release`
  section in `.claude/shirabe-extensions/release.md`, and updated comments in
  `tests/native_workflows_shape.rs` and `src/workflows_surface/contract.rs`.

### Key Interfaces

Guard command line and exit statuses:

| Invocation | Checks | Exit |
|---|---|---|
| `check-claude-code-conventions.sh` | recorded release | 0 pass, 1 convention failure |
| `--version 2.1.320` / `--version latest` | named / newest release | same |
| `--alter file-shape` (or `directory`, `session-variable`, `hook-input`) | recorded release with that convention's demo mutation | 1, naming the convention |
| `--evidence <file>` (with or without `--alter`) | a local executable, no network | as for a release |
| `--self-test` | synthetic evidence, no network | 0 when the guard catches every mutation, 1 when it misses one |
| unpublished version, registry or network failure | — | 2, `could not obtain` |
| missing or malformed release or expectations file | — | 3, naming the file |

Smoke harness: `smoke-native-workflows-render.sh [--no-entry] [--koto <path>]`,
exit 0 rendered, 1 not rendered, 2 usage, 3 unavailable. Each outcome prints
one line, `<outcome>: Claude Code <version>`, followed by the captured pane on
anything but *rendered*.

Hook: unchanged contract. Stdin is the SessionStart payload; stdout is either
nothing or one JSON object,
`{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"..."}}`,
whose context names the derived directory; exit is always 0.

### Data Flow

On a pull request touching the surface, the workflow restores the cached
package for the recorded release (or fetches it), runs `--self-test`, then the
real check. The helper prints `checking Claude Code 2.1.316`, one line per
failing probe, and a summary such as
`file shape: ok · directory convention: ok · session variable: ok · SessionStart hook input: FAIL`.
The weekly run asks npm for the newest version, fetches it, and runs the same
check; a red run is the signal. When a maintainer confirms a newer release,
they run the guard with `--version <new>`, update koto or the probes if
needed, write the new version into the recorded-release file, and open a pull
request, which the path filter checks against the new recorded release.

## Implementation Approach

### Phase 1: Fix and test the session-start hook

Independent of everything else and the one change operators feel. Deliverables:
`json_escape` and the escape-aware `read_field` in
`plugins/koto-skills/hooks/session-start-workflows.sh`; the payload-matrix test
`plugins/koto-skills/hooks/test/session-start-workflows.test.sh`; a new step in
validate-plugins.yml's `hook-smoke-test` job. The pull request shows the test
failing on the unfixed script's no-`jq` run of the quote-and-backslash payload
and closes koto#243.

### Phase 2: The convention guard and its expectations

The first step is to confirm that the Linux x64 package's bundle carries the
same literals as the macOS build this design inspected; if it doesn't, the
package name in the expectations file changes before any probe is written.
Deliverables: `tests/fixtures/native-workflows/claude-code-release`,
`tests/fixtures/native-workflows/claude-code-conventions.json` calibrated
against 2.1.316, `scripts/check-claude-code-conventions.sh`,
`scripts/claude_code_conventions.py`, and `tests/native_workflows_conventions.rs`.
The pull request includes the output of the four `--alter` runs, a
`--version 0.0.0` run, and the commands to reproduce them.

### Phase 3: The CI workflow

Depends on Phase 2; can run alongside Phase 4. Deliverable: `.github/workflows/claude-code-conventions.yml`
with the four triggers, the version-keyed cache, `--self-test` before the real
check, and `permissions: contents: read`.

### Phase 4: The rendered smoke check

Depends on Phase 1 (the harness uses the fixed hook). Deliverable:
`scripts/smoke-native-workflows-render.sh`, with one real run's output.

### Phase 5: Maintainer documentation and the release step

Depends on Phases 2 to 4. Deliverables: sections in
`docs/guides/native-workflows-verification.md` for running the guard, the four
`--alter` commands, extending the probes when koto's file shape changes (add
the field to the fixture, watch the coverage test name it, add a probe,
confirm against the recorded release, finish with `--alter file-shape`),
moving the recorded release, and the smoke check with its exit statuses and
recorded run; a `## Before release` section in
`.claude/shirabe-extensions/release.md` that asks the releaser to confirm the
newest scheduled guard run passed and to run the smoke check, putting
`Checked against Claude Code <version>: /workflows entry renders` (or the
outcome it got) in the release notes; and the updated comments in
`tests/native_workflows_shape.rs` and `src/workflows_surface/contract.rs`.

### How a later file-shape change extends the guard

A contributor adding a field koto expects the screen to read adds it to
`enriched-shape.json` through the existing regeneration command. The coverage
test then fails naming the field. The contributor finds where the Claude Code
reader uses that field, adds a probe with a stable anchor to
`claude-code-conventions.json`, runs the guard against the recorded release,
and finishes with `--alter file-shape` to confirm the guard still fails when
the evidence changes. A field Claude Code doesn't read belongs in the `koto`
block, which the coverage test skips.

## Security Considerations

### What the guard downloads and how it treats it

The guard fetches a platform package of a Claude Code release from the npm
registry: third-party code of about 330 MB that the guard never runs, imports
or decodes. The Python helper, started with `python3 -I`, memory-maps the
executable and runs plain substring searches, so a malformed or hostile bundle
can at worst make a probe pass or fail wrongly. The guard's claim is bounded
the same way: names are present near anchors, nothing more, which is why the
smoke check exists.

Inputs that reach the package manager and the file system are validated
first. The package name comes from the committed expectations file and must
start with `@anthropic-ai/claude-code-`. A version must match
`MAJOR.MINOR.PATCH`, or be `latest`, which is resolved to a concrete version
and validated again. In the workflow, the dispatch input reaches the script
through an environment variable, never interpolated into a command line.
`npm pack` runs in a fresh directory outside the checkout, with install
scripts disabled and no credentials. The guard extracts only the expected
executable, by exact member name, refuses absolute paths, `..` components and
link members, and caps the extracted size while streaming, so a compressed bomb stops at
the cap rather than at the disk. The job also has a timeout.

### Workflow permissions and cache integrity

The workflow declares `permissions: contents: read`, triggers on
`pull_request` (never `pull_request_target`) and references no secret, so a
fork's pull request that edits the guard or the workflow runs with a read-only
token and nothing to take. The cache holds the downloaded tarball under a key
naming the resolved version, with no restore-key prefix. Every run checks the
tarball against the sha512 integrity value the registry reports for that
version and discards the cache on a mismatch. GitHub scopes cache writes to
the writing ref, so a pull request can't plant an entry the default branch
reads; the integrity check covers entries written by default-branch and
dispatch runs. The value comes from the same registry, so it detects a
tampered cache, not a compromised registry, and because the package never
runs, a compromised registry could only skew the verdict. Provenance
verification can be added if Anthropic publishes attestations for the
package. A pull request that changes the expectations file's `package` field
deserves a reviewer's attention.

### The session-start hook and hostile input

The hook reads JSON from Claude Code, but the values include directory names
a user, a cloned repository or a worktree can shape. A `session_id` outside
`[A-Za-z0-9_-]+` is treated as absent, so it can't add path components. A
control character in the derived directory makes the hook print nothing.
Because the context asks the agent to run an `export` line, the directory is
shell-quoted there as well as JSON-escaped in the envelope; JSON escaping alone
would let a directory named with `$(...)` or a backtick run a command. The
no-`jq` reader decodes the escapes it supports in one pass and refuses any
other. The test matrix covers `$(`, backtick and single-quote paths besides
quote, backslash and space, and a directory holding a DEL character, and
checks both the envelope and the export line. One residual stays: the
directory also appears in the context's plain sentence, so printable text in a
directory name reaches the agent as text. The sentence labels it as a path,
and the risk is low because the same agent already sees the working
directory.

### The rendered smoke check

The harness runs a real Claude Code with the maintainer's login and
privileges, from an empty scratch directory made by `mktemp -d`, so accepting
the folder-trust prompt loads no project settings. It sends keystrokes with
`tmux send-keys -l` and answers the trust prompt only when the pane shows it.
Cleanup is narrow: it deletes the transcript file and session directory named
by the harness's own UUID, validated against a UUID pattern, with every
variable checked non-empty, no globs and no symlinks followed, and removes the
project directory with `rmdir`, so it goes only when empty. koto runs through
`scripts/without-session-env.sh` with its own home and sessions base, so it
never writes into the maintainer's real sessions.

### Data exposure

The guard adds only anonymous registry requests and prints release numbers,
probe names and anchors, all public; it never echoes the environment or npm
configuration, and doesn't run `npm` with debug logging. The smoke harness's
captured pane and saved hook payload can hold the maintainer's email,
organization, home path and username. They stay in a mode-700 scratch
directory removed on exit, and the pane is printed to the terminal only. The
run recorded in the verification guide is reduced to the outcome line, the
release and the entry name, or redacted, before it is committed, because the
repository is public.

## Consequences

### Positive

- A Claude Code release that renames a field, moves the directory, renames the
  session variable or changes the hook input fails koto's CI within a week of
  shipping, with the convention and item named.
- koto-side drift (a new field without a probe, a broken hook) fails on the
  pull request that introduces it.
- Operators on hosts without `jq` get a working hook, and koto#243 closes.
- Releases carry a statement of the Claude Code version they were checked
  against.

### Negative

- Probes check that names are present near an anchor, not what the reader
  does with them. A change in rendering semantics with the same names passes
  the guard.
- Probe anchors and windows can break on a bundle reorganization that changes
  nothing koto relies on, producing a red run that needs a maintainer to
  recalibrate.
- Each guard run downloads a package of a few hundred megabytes unpacked.
- The smoke check depends on a maintainer's machine and login, so it runs only
  as often as maintainers run it.
- GitHub disables scheduled workflows in a repository with no activity for 60
  days, which would silence the weekly run.

### Mitigations

- The smoke check covers rendering semantics at each release, and the guide
  tells a maintainer to run it after any probe recalibration.
- A failing probe prints its anchor and window, so recalibration starts from
  the exact probe; `--evidence` lets a maintainer iterate on a downloaded
  package offline.
- The download is cached by version and happens only on the four triggers.
- The release step makes the smoke check part of every release.
- koto's commit cadence keeps the repository well inside the 60-day window;
  the guide notes the limit.
