# Lead: What agent-facing surface would a fix change?

The likely fix makes `koto context add` (and possibly `koto context remove`,
which has the same shape) refuse a session whose state log does not exist.
This lead maps what agents currently read about that verb, what error they
would get, which tests encode the current behaviour, and where the docs have
to change.

## Findings

### 1. The code path today

- `src/cli/context.rs:16-48` (`handle_add`): reads content, calls
  `store.add(session, key, &content)`, then `backend.append_event(session,
  ContextAdded, ..)`. It never checks that the session exists.
- `src/session/local.rs:493-499` (`ContextStore::add`): `validate_context_key`
  first, then `fs::create_dir_all(ctx_dir)`. That creates the session
  directory as a side effect.
- `src/session/local.rs:189-202` -> `src/engine/persistence.rs:139-183`
  (`append_event`): `if path.exists() { read_last_seq + 1 } else { 1 }`, then
  `OpenOptions::create(true).append(true)`. The doc comment at
  `persistence.rs:137` says outright "Creates the file with mode 0600 on unix
  if it doesn't exist." That's #236's mechanism.
- `src/cli/context.rs:137-150` (`handle_remove`) makes the same unconditional
  `append_event` call, so `koto context remove` on a session with no log also
  creates a headerless log (a `context_removed` line at `seq:1`).
- `src/session/cloud.rs:729-738` (cloud `append_event`) delegates to local
  and then pushes. `cloud.rs:869-895` (cloud `add`) delegates to local `add`
  first. The cloud backend adds no existence check, and it would push the
  headerless log to S3.
- `src/cli/mod.rs:1417-1432`: every `context add` error exits
  `EXIT_INFRASTRUCTURE` (3) with the flat body
  `{"error": e.to_string(), "command": "context add"}`. There's no per-error
  exit mapping. `context remove` (`mod.rs:1470-1481`) works the same way.

### 2. How other verbs report a missing session today

`backend.exists(id)` checks the state *file*, not the directory
(`src/session/local.rs:81-83`: `base_dir/id/koto-<id>.state.jsonl`
`.exists()`; the cloud version falls back to S3 at `cloud.rs:686-692`). So
it's already the right predicate for "no log", and it answers false for a
directory-only session. The verbs that use it don't agree on exit code:

| Verb | Location | Body | Exit |
|------|----------|------|------|
| `next` | `src/cli/mod.rs:3485-3493` | structured `{"error":{"code":"workflow_not_initialized","message":"workflow 'X' not found",...}}` | 2 |
| `status` | `src/cli/mod.rs:5807-5816` | flat `{"error":"workflow 'X' not found","command":"status"}` | 2 |
| `rewind` | `src/cli/mod.rs:2128-2133` | flat, same wording | 1 (`exit_with_error` defaults to 1, `mod.rs:741-743`) |
| `cancel` | `src/cli/mod.rs:6293-6299` | flat, same wording | 1 |
| `decisions record` | `src/cli/mod.rs:5533-5538` | flat, same wording | 1 |

`docs/reference/error-codes.md:210-212` documents the rewind body but gives
it no exit code.

### 3. The error-code catalog

- `NextErrorCode` (`src/cli/next_types.rs:713-751`) has `WorkflowNotInitialized`
  and `PersistenceError`, but only `koto next` emits it. There's no
  `session_not_found` code anywhere in `src/` (grep for `session_not_found` /
  `SessionNotFound` finds nothing).
- `tests/error_envelope_schema_test.rs:381-400,438-460` round-trips every
  `NextErrorCode` into the structured envelope. It pins the `next`/batch
  envelope and doesn't cover flat-shape commands like `context add`.
- `docs/reference/error-codes.md:1-11` says every non-`next`/batch/`request`
  command uses the flat `{error, command}` shape with no `code`.
  `error-codes.md:13-232` has per-command sections for `init`, `next`,
  `rewind` and `context exists`. There's **no `context add` or
  `context remove` section**.
- `docs/reference/error-codes.md:407-416` defines the four exit classes. Class
  2 is "Caller error: change something ... pick a different target", which
  fits "session not started".
- Lock codes that already exist: `concurrent_access`
  (`error-codes.md:64`, `next` only), `concurrent_tick`
  (`error-codes.md:309`, batch parent flock, carries `holder_pid`), and
  `lock_contention` (`error-codes.md:361`, request store only, 5-second
  deadline). koto-user documents them at
  `references/error-handling.md:66` (`concurrent_access`),
  `references/batch-workflows.md:13` (`concurrent_tick`) and
  `references/error-handling.md:247` (`lock_contention`). None of them
  applies to `context add` today. If the fix puts `context add` under the
  state-file lock, a fail-fast lock would need a flat-shape equivalent
  (`context add` can't emit a structured code without changing its
  envelope). A blocking lock would add no new agent-facing surface.

**The natural refusal:** the flat shape with the wording every other verb
already uses, `{"error":"workflow '<name>' not found","command":"context
add"}`, at exit **2**, matching `status` and `next`'s
`workflow_not_initialized` class. No new error code is needed, because flat
commands carry none. Exit 3 (today's catch-all for `context add`) would be
wrong: `docs/reference/error-codes.md:414` says 3 means "inspect the
workspace", while the right move is "wait for the child to start" or "run
`koto init` first". Refusing is a behaviour change: the command currently
exits 0 in this case.

### 4. What the docs currently say about `context add` on a missing session

- `docs/guides/cli-usage.md:452`: "Exits non-zero if the session doesn't
  exist or the input can't be read." **This is false today**: #236 is a
  success exit on a session with no log. The fix makes this sentence true,
  so it needs no change beyond the exit code.
- `docs/guides/cli-usage.md:486` (`context get`): "Exits non-zero if the
  session or key doesn't exist."
- `plugins/koto-skills/skills/koto-user/references/command-reference.md:771-785`
  (`koto context add`): "No stdout on success. Exit 3 on infrastructure
  errors." It says nothing about a missing session. If the refusal exits 2,
  this line has to gain a sentence like "Exit 2 if the session has no state
  log yet (not initialized, or a batch child still `blocked`/`pending`)".
- `command-reference.md:836-863` (`koto context remove`): "Idempotent.
  Removing a key that is not there succeeds." ... "Errors (an unwritable
  store) exit 3". If `remove` also refuses a session with no log, the
  idempotency claim needs scoping ("a key that is not there", not "a
  session that is not there").
- `command-reference.md:31`: `koto context add` is listed as "Runner --
  primary".
- `docs/guides/cli-usage.md:775` and `docs/guides/cloud-sync-setup.md:61`:
  `context add` syncs to the cloud automatically. That's unaffected, but the
  cloud backend needs the same check before it pushes.
- `docs/reference/session-feed.md:668-669`: `context_added` is "Emitted by
  `koto context add` after a context artifact is stored." It doesn't say the
  event can't be first in the log, and a header-first invariant would sit
  naturally here or at `session-feed.md:384` (seq-gap corruption rule).
- `plugins/koto-skills/skills/koto-user/references/error-handling.md:78`
  (`persistence_error`: "State file I/O failure or corruption ... Report to
  user") and `docs/reference/error-codes.md:172-178` ("Corrupt state file
  (exit code 3) ... empty files, invalid JSON, and sequence number gaps").
  Neither mentions a missing header, which is exactly what #236 and #200
  produce (`state file corrupted: failed to parse header: missing field
  workflow`). `error-codes.md:178` says "The first line should be a header
  with `schema_version`". That's the only place the header-first rule is
  stated to agents.

### 5. The "skipping ... corrupted" warnings

The warning quoted in #236 is
`info: wake_candidates_pass found no readable coord log at ... (state file corrupted: ...); skipping`,
from `src/engine/wake.rs:477`. Its siblings are `src/engine/wake.rs:278`
("warning: skipping wake candidate {} -- header read failed") and
`src/engine/discovery.rs:410` ("warning: skipping {} during discovery").
**None of these stderr lines is documented** in koto-skills or docs (a grep
for "skipping" or "corrupt" in `plugins/koto-skills` and `docs/guides`
finds only unrelated hits). `docs/guides/cli-usage.md:372` says `session
list` skips "Directories without a valid state file", silently. So a
tolerant reader or recovery path changes no documented warning text.

### 6. `koto session recover`

Documented at `docs/guides/cli-usage.md:375-400`,
`plugins/koto-skills/skills/koto-user/references/command-reference.md:24,659-...`,
`docs/workspace-layout.md:47` and `CHANGELOG.md:30-...`. All of these
describe the **migration-quarantine** recovery (moving
`.migration-conflicts/<repo-id>/<name>/` back). The only overlap with this
work is its `header_rewritten` field (`cli-usage.md:400`). If header
reconstruction for #236 logs gets folded into `session recover`, that's a
scope change to a documented verb whose docs frame it as "Operator --
one-time migration recovery" (`command-reference.md:24`).

### 7. Does anything rely on `context add` before `koto init`?

**Documented workflows: no.** Every documented or templated `context add`
runs against an initialized session:
- `docs/guides/custom-skill-authoring.md:51,253,385-401,618`: after `koto init`.
- `docs/guides/default-action-authoring.md:197,209,244` and
  `plugins/koto-skills/skills/koto-author/references/template-format.md:317`:
  `koto context add {{SESSION_NAME}} ...` inside a state's command, which
  runs under a tick, so the session exists.
- `test/functional/features/skip-if.feature:27-29`: `koto init` first.
- `tests/cloud_integration_test.rs:200-208,256-263,301-311,375-389`: `init` first.
- `tests/integration_test.rs:3453-3461,3499-3505`: use `init_workflow` first.
- The template-driven uses in `tests/nested_next_test.rs`,
  `tests/gate_field_substitution_test.rs`, `tests/next_response_baseline.rs`
  and `tests/instructions_delivery_test.rs` run the command inside a tick.

**Tests: yes, a whole block.** `tests/integration_test.rs:3065-3070`
defines `create_session_dir`, commented "create a session directory (without
a full workflow init) so context commands have somewhere to store files".
These tests run `context add` or `context remove` against it and would fail
under a refusal:

| Test | Line | Why it breaks |
|------|------|---------------|
| `context_add_from_stdin_and_get_to_stdout` | 3073 | add on dir-only session |
| `context_add_from_file_and_get_to_file` | 3099 | add, asserts success |
| `context_exists_returns_exit_0_when_present` | 3147 | add at 3153 |
| `context_exists_tells_an_unusable_key_from_an_absent_one` | 3186 | add at 3191 |
| `context_remove_deletes_a_present_key` | 3270 | add at 3275 |
| `context_remove_drops_the_key_from_list` | 3303 | add at 3309 |
| `context_remove_is_idempotent_on_a_missing_key` | 3334 | remove only (breaks only if `remove` also refuses) |
| `context_remove_appends_a_context_removed_event` | 3348 | add + remove, then **reads the state log** |
| `context_list_returns_json_array` | 3378 | add at 3394/3399 |
| `context_list_with_prefix_filter` | 3415 | add at 3420-3430 |
| `context_add_rejects_invalid_key` | 3562 | asserts exit 3 for `../escape.md`; still passes only if key validation runs before the existence check |
| `context_add_overwrites_existing_key` | 3579 | add twice |
| `context_hierarchical_keys_work` | 3603 | add |

`context_exists_returns_exit_1_when_missing` (3254) and
`context_get_missing_key_returns_error` (3546) don't write, so they're
unaffected unless reads also start refusing.

The fix is mechanical: switch `create_session_dir` to `init_workflow(dir,
name, minimal_template())`, the helper the two `state-wf` tests at 3453/3499
already use. Tests that assert *on the log* after `init` will see a header
line first. The line counts in `context_add_appends_context_added_event_without_state_transition`
are relative, so they still hold.

Tests calling `persistence::append_event` directly
(`tests/idempotency.rs:453-463`, `tests/dashboard_test.rs:178-205`,
`tests/wake_recovery.rs:118,411,...`) all append to a file that already has
a header (`write_session_file` or a prior `koto init`). None relies on the
create-on-missing branch, so moving the check down into `append_event` would
not break them.

### 8. Batch children: how context is meant to reach a child

- `plugins/koto-skills/skills/koto-user/references/batch-workflows.md:17`:
  "**Do not manually initialize batch children.** The scheduler owns the
  child lifecycle ..."
- `batch-workflows.md:64-76`: `ready_to_drive` requires `outcome: running`
  (state file exists). `pending` means "no state file has been written yet".
  `blocked` means "No state file; at least one `waits_on` dependency is
  non-terminal". Both are "always `false` -- no child file for a worker to
  drive".
- `batch-workflows.md:32-34` and `SKILL.md`: the supported per-child input
  channel is the task entry's `vars` (e.g.
  `{"name":"task-2","vars":{"ISSUE_NUMBER":"102"},"waits_on":["task-1"]}`),
  set at submission and materialized at spawn.
- `plugins/koto-skills/skills/koto-user/SKILL.md:318-321` and
  `template-format.md:879` document only the reverse direction: the parent
  *reads* a child's results with `koto context get <child> <key>` after the
  child has run.
- Nothing in koto-skills or docs documents seeding a child's context before
  it spawns, or `context add` creating a session implicitly. #236's repro
  (step 3: `context add <parent>.<blocked-child>`) is an undocumented
  pattern, and the documented rule already implies it's unsupported.
  Supported alternatives: pass small inputs through task `vars`; put shared
  artifacts in the **parent's** context store and have the child read them
  with `koto context get <parent> <key>` (context reads cross sessions
  freely); or wait until the child shows `ready_to_drive: true`, then add
  context. koto docs don't reference a shirabe usage pattern for this. The
  shirabe mentions under `docs/` are PRD/brief background only
  (`docs/prds/PRD-koto-runs-commands.md:35,491`).
- `--needs-agent` children (`docs/guides/cli-usage.md:945-955`) are a
  separate case. They get a state file at `koto session start` time, so
  `context add` on them isn't affected by a missing-log refusal.

### 9. CHANGELOG `[Unreleased]` format

`CHANGELOG.md:9-45`: `## [Unreleased]` with `### Added` and `### Fixed`
subsections (Keep a Changelog, `CHANGELOG.md:3-7`). Each entry is a single
`- **Bold one-sentence headline stating the new behaviour.**` followed by
several wrapped prose paragraphs, indented two spaces. The paragraphs cover
what was broken and how it showed up, what now happens (naming the error
code and exit class), what deliberately stays the same, and they close with
a test count (e.g. `CHANGELOG.md:84-85`: "Seven integration tests, four of
which fail against the previous release."). Issues are cited in prose
(`koto#221`), not as trailing links.

### 10. `doc_names` constraints

`tests/doc_names.rs:996-1015` resolves backticked `koto <verb>` tokens
against the live clap tree (`context add`, `context remove`, `init`,
`session recover` are all real). `CLAUDE.md:51-63`: any new remedy text in
an error message or doc that names a verb (e.g. "run `koto init`", "wait
until the child is `ready_to_drive`") must name a real verb. A remedy
pointing at a verb that doesn't exist yet (say a hypothetical
`koto session repair`) would fail CI unless it's recorded in
`tests/doc_names.allow` with an issue. Error text in `src/` is scanned too.

### 11. Evals

`scripts/run-evals.sh` runs `plugins/koto-skills/skills/*/evals/evals.json`.
None of the three eval files exercises `context add`. The only context
mention is `koto-user/evals/evals.json:65` (`koto context get` for checking
child progress). No eval would break, and none covers the new refusal
unless one is added (e.g. "agent wants to hand context to a blocked child").

## Implications

- A refusal needs **no new error code**. `context add` is a flat-envelope
  command, and the existing wording `workflow '<name>' not found` plus exit 2
  matches `status` and `next`. Use `backend.exists(name)` (which checks the
  state file) before `store.add`, so a refused call leaves no `ctx/`
  directory or content behind. Do the key validation first, or the exit code
  asserted by `context_add_rejects_invalid_key` flips. Apply the same check
  in `context remove`, and ideally in `persistence::append_event` itself
  (fail rather than `create(true)`), so no other writer can re-open #236.
- The message should carry a remedy an agent can act on: `koto init` for a
  plain session, or "wait until the child is `ready_to_drive`, or store the
  context on the parent" for a batch child. Otherwise #236's reporter just
  hits a new dead end. It must pass `doc_names`.
- Docs to touch in the same PR: `command-reference.md:771-785` (exit 2 on a
  missing session; remove "Exit 3" as the only failure),
  `command-reference.md:836-863` (scope remove's idempotency if it refuses
  too), a new `### context add` section in `docs/reference/error-codes.md`,
  `docs/guides/cli-usage.md:452` (now true; add the exit code),
  `koto-user/references/batch-workflows.md:64-76` or `:17` (one sentence:
  don't `context add` to a `blocked`/`pending` child, and where context goes
  instead), and optionally `error-codes.md:172-178` to add "missing header"
  to the corrupt-state list if a recovery path is added.
- Tests: rewrite the 13 `create_session_dir` tests to init first. Two of
  them currently **encode #236's bug**, and
  `context_remove_appends_a_context_removed_event` (3348) reads a headerless
  log as its passing condition. Add the regression test as a new
  "context add on an uninitialized session is refused and writes nothing"
  case.
- If the fix puts `context add` under the state-file lock and the lock
  fails fast (#171), a flat-shape contention message and exit 1 need a doc
  entry. A blocking lock has no agent-facing surface. That argues for
  blocking, as far as this lead can see.
- CHANGELOG: one `### Fixed` entry under `[Unreleased]` in the house style
  (bold headline, mechanism paragraph, new behaviour with exit class,
  test count).

## Surprises

- `docs/guides/cli-usage.md:452` has promised a non-zero exit for a missing
  session all along. The code never implemented it, so #236 is a doc/code
  contradiction as well as a data-loss bug.
- The test suite deliberately runs context commands on sessions with no log
  (`tests/integration_test.rs:3065` helper), and one test asserts on the
  headerless log as its success condition. The bug is enshrined in tests, not
  just untested.
- `context remove` has the identical bug and isn't named in #236.
- "Workflow not found" exits 2 on `status`/`next` and 1 on
  `rewind`/`cancel`/`decisions record` (`exit_with_error` defaults to 1). The
  exit-class doc (`error-codes.md:407-416`) says the classes are "the same
  everywhere", which isn't true for this condition.
- koto-user's `references/error-handling.md:50-55` says flat errors go to
  **stderr**, but `exit_with_error_code` (`src/cli/mod.rs:746-749`) prints
  them with `println!` to stdout, which `command-reference.md` itself says
  for `context remove`. This is a pre-existing drift the new refusal will
  inherit.
- The "skipping" line in #236 is an `info:` from `wake.rs:477`, not from any
  session-listing code, and nothing documents it.

## Open Questions

- Should the refusal be exit 2 (caller error, matches `status`/`next`) or
  should `context add` join a harmonization of the 1-vs-2 split for "not
  found"? This lead recommends 2 and leaving the others alone in this PR.
- Should `context get`/`exists`/`list` also refuse a session with no log?
  They don't corrupt anything, and `exists` has a strict 0/1/2 contract
  (`command-reference.md:797-818`) that a refusal would disturb. That argues
  for leaving reads alone.
- Should the check live only at the CLI boundary or in
  `persistence::append_event` too? The latter closes every writer but changes
  a documented function contract (`persistence.rs:137`).
- Should the cloud backend consult S3 (`CloudBackend::exists` does) before
  refusing? A session that exists remotely but not locally should be pulled,
  not refused.
- Is there an unwritten shirabe or other-consumer pattern that seeds child
  context before spawn? koto's docs don't show one, but #236's reporter
  was doing it.

## Summary

A refusal needs no new error code: `context add` uses the flat envelope, so the natural fix returns `workflow '<name>' not found` at exit 2 (matching `status` and `next`), checked with the existing `backend.exists` state-file predicate after key validation. That also makes `docs/guides/cli-usage.md:452`'s long-standing claim true, and `context remove` has the same headerless-append bug and should get the same check. No documented workflow relies on `context add` before init (batch docs route per-child input through task `vars` and mark blocked or pending children as having no state file), but 13 tests in `tests/integration_test.rs` (3073-3603) run context writes on a bare `create_session_dir` session, one of them asserts on the headerless log, and they all need to init first, while the docs to update are koto-user's `command-reference.md:771-785`, a new `context add` section in `docs/reference/error-codes.md`, a line in `batch-workflows.md`, and one `### Fixed` CHANGELOG entry.
