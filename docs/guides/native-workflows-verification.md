# Verifying native `/workflows` rendering

The initial render makes a koto session appear as a
native entry in Claude Code's `/workflows` screen; the phase-detail work enriches that entry
from a bare status into the session's real structure (ordered phases with the
active one marked, the active directive, per-phase evidence/gate outcomes, and a gate-blocked
session that reads as blocked). This guide covers the automated check, the
guard that checks a Claude Code release against the conventions koto relies
on, and the manual live-TUI check.

The entry file is at contract version 5. Its top-level `status` uses only the
words Claude Code's screen knows (`running`, `completed`, `failed`); it never
reads `blocked`. koto's own classification (`running`, `blocked`, `completed`
or `failed`) is in `koto.status`. A gate-blocked session is written `running`,
with its active step as an `error` step carrying `blocked: true` and
`error: "gate <name>: FAIL"`, and a `summary` ending `· blocked on gate <name>`.
The manual steps below describe what Claude Code 2.1.314 draws for that file.

## Automated (CI / CLI)

`scripts/verify-native-workflows.sh` drives a real koto session with a real
template through the full publish → advance → render path and asserts both
features' "Verified when" criteria against the emitted `koto-<uuid>.json`:

```bash
cargo build
scripts/verify-native-workflows.sh
```

Expected output ends with
`ALL CHECKS PASSED: native /workflows rendering verified end-to-end.` The script
exercises everything koto owns:

- **The initial render** — single-session render with the current state, update on
  advance, done-on-completion, the no-write path when no target resolves (default
  path untouched), and the atomic `koto-<uuid>.json` filename.
- **The phase-detail enrichment** — the phases render in order with the active one marked, the
  active phase's directive is legible, a completed phase shows its gate outcome,
  and a session whose gate did not pass writes top-level `running`,
  `koto.status` `blocked`, a blocked `error` step reading
  `gate spirit_greeting: FAIL`, and a summary ending
  `· blocked on gate spirit_greeting`. A finished session writes every
  `workflow_agent` step `done`, with none left in `progress`.

The script pins koto's local session backend before its first koto call: it
exports `KOTO_SESSION_BACKEND=local` and unsets every cloud session setting,
for the main run and the no-publish run alike. Its sessions land only in its
own scratch directories, never in a shared store, so it can run twice in a row
on a host whose environment selects the cloud backend.

The `hello-koto` template's `awakening` state carries a command gate that fails
until a greeting file exists, so the first advance lands on a gate-blocked state
(the blocked case) and the second, after the greeting, completes —
exercising both statuses on one run.

The script does **not** confirm that Claude Code actually renders the file — CI
cannot drive the TUI — which is what the manual check below adds. The exact
enriched file shape is pinned by the golden fixture at
`tests/fixtures/native-workflows/enriched-shape.json` and koto's own shape test
(`tests/native_workflows_shape.rs`). Both sides of that comparison are koto's,
so a change on Claude Code's side is the convention guard's to catch.

## The Claude Code convention guard

`scripts/check-claude-code-conventions.sh` checks a Claude Code release for the
four conventions koto's entry depends on: the file shape, the directory
convention, the `CLAUDE_CODE_SESSION_ID` session variable and the SessionStart
hook input. It fetches the release's Linux x64 npm package, never runs it, and
searches the executable's bytes for the probes in
`tests/fixtures/native-workflows/claude-code-conventions.json`. A probe is a
stable literal (the anchor) and a name that has to appear within a number of
bytes of it (the token and window). The release it checks by default is the one
recorded in `tests/fixtures/native-workflows/claude-code-release`.

```bash
scripts/check-claude-code-conventions.sh                       # the recorded release
scripts/check-claude-code-conventions.sh --version 2.1.320     # a named release
scripts/check-claude-code-conventions.sh --evidence <executable>  # a local executable, no download
scripts/check-claude-code-conventions.sh --self-test           # the guard itself, no network
```

A run prints `checking Claude Code <version>`, a `FAIL` line for each probe
that failed, naming the convention and the item, and then one line per
convention reading `ok` or `FAIL`. Every convention is checked on every run, so
one break never hides another. The exit status says what happened: 0 every
convention holds, 1 at least one failed, 2 the release could not be obtained
(`could not obtain Claude Code <version>`, naming no convention), 3 the
recorded-release or expectations file is missing or malformed (the message
names the file), 64 a usage error.

`--self-test` builds synthetic evidence from the expectations file alone,
checks every convention passes on it, then checks each convention's
demonstration mutation makes that convention fail. It exits 1 when a probe or
a mutation stops catching anything.

`--evidence` takes a Claude Code executable you already have, for iterating on
probes offline. To get one for a release, fetch the package with install
scripts off, outside the repository, and extract the one file; don't run it:

```bash
cd "$(mktemp -d)"
npm pack --ignore-scripts @anthropic-ai/claude-code-linux-x64@2.1.316
tar -xzf anthropic-ai-claude-code-linux-x64-2.1.316.tgz package/claude
```

### Showing it fail on each convention

`--alter <slug>` renames one item in an in-memory copy of the evidence before
the check (the file and the download are never changed), so you can see the
guard fail on that convention by name. Each command exits 1:

```bash
scripts/check-claude-code-conventions.sh --alter file-shape        # FAIL file shape: workflowName ...
scripts/check-claude-code-conventions.sh --alter directory         # FAIL directory convention: "workflows" directory ...
scripts/check-claude-code-conventions.sh --alter session-variable  # FAIL session variable: CLAUDE_CODE_SESSION_ID not found
scripts/check-claude-code-conventions.sh --alter hook-input        # FAIL SessionStart hook input: transcript_path ...
```

`--alter` combines with `--version` and `--evidence`.

### When koto's file shape changes

`tests/native_workflows_conventions.rs` walks the shape fixture and fails naming
any field outside the `koto` block, or any node `type` value, that has no
file-shape probe. A field Claude Code doesn't read is listed under the
file-shape convention's `unread`, with the reason; `id`, `name` and a phase's
`detail` are there today. To add a field:

1. Add it to the contract in `src/workflows_surface/contract.rs`, regenerate
   the fixture with
   `cargo test --test native_workflows_shape -- --ignored --nocapture`, and
   commit the printed JSON.
2. Run `CARGO_BUILD_JOBS=2 cargo test --test native_workflows_conventions` and
   see it fail naming the new field.
3. Find where Claude Code's `/workflows` reader uses the field, in an executable
   extracted as above. The snapshot reader sits next to the literal
   `Failed to parse workflow snapshot`; the progress-node schema sits next to
   the `state` values `["start","progress","done","error"]`.
4. Add a probe to the file-shape convention: `item` is the field name, `anchor`
   a string literal or object key that stays put between releases (never a
   minified identifier, which changes every release), `token` the field as it
   appears there, and `window` the distance you measured plus some headroom. If
   Claude Code doesn't read the field, put it in the `koto` block or list it
   under `unread` with the reason instead.
5. Run the coverage test again, `scripts/check-claude-code-conventions.sh` and
   `--self-test`, and see each pass.
6. Finish with `scripts/check-claude-code-conventions.sh --alter file-shape` and
   see it still exit 1 naming `file shape`.

## Manual (live Claude Code TUI)

This confirms the one thing the automated check cannot: that Claude Code renders
the emitted file.

1. **Rendering is on by default.** A koto session driven inside a Claude Code
   session self-discovers that session's workflows directory from the
   `CLAUDE_CODE_SESSION_ID` environment variable Claude Code sets in every
   subprocess — no plugin, hook, or manual export required. A fully headless run
   (no Claude Code environment) still renders nothing.

   To opt out, set the flag to false:

   ```bash
   koto config set workflows.native false --user
   ```

   *Alternative (explicit handoff):* to point koto at a specific directory —
   useful for a non-standard layout — export it directly:

   ```bash
   export KOTO_WORKFLOWS_DIR="<projectDir>/<sessionId>/workflows"
   ```

2. **Drive a koto session.** In the same Claude Code session, run a workflow:

   ```bash
   koto init demo --template <template> --intent "demo"
   koto next demo
   ```

3. **Open `/workflows`.** The koto session appears as an entry named for the
   session, showing its phases in order with the active one marked, the active
   phase's directive, and — for a multi-phase workflow — each completed phase's
   evidence or gate outcome.

4. **Advance and reopen.** Run `koto next demo` again, then reopen
   `/workflows` — the active-phase marker moves to the new phase, the previously
   active phase shows its outcome, and the directive updates (refresh-on-open).

5. **Block on a gate and reopen.** Drive the session into a state whose gate
   does not pass, then reopen `/workflows`. On Claude Code 2.1.314 the entry is
   counted under "running" in the list's summary line, not under "completed".
   In its detail, the active step reads Blocked (a cross in the permission
   color) with `gate <name>: FAIL` as its Outcome, naming the gate that failed,
   and the line under the title ends `· blocked on gate <name>`. No step reads
   Stopped.

6. **Complete and reopen.** Drive the session to its terminal state, then reopen
   `/workflows`. On Claude Code 2.1.314 the header reads done, every phase
   reads done (the terminal phase `1/1`), and no step reads Stopped.

7. **Negative check.** With rendering opted out (`koto config set
   workflows.native false`) and no `KOTO_WORKFLOWS_DIR`, run a koto session and
   confirm `/workflows` is unaffected and no `koto-*.json` is written.

## Notes

- koto writes `koto-<uuid>.json`, namespaced so it never collides with Claude
  Code's own `wf_*.json` files in the same directory.
- The write is atomic (temp-then-rename), so a `/workflows` reopen never sees a
  half-written file.
- The `/workflows` file format is an undocumented, version-coupled Claude Code
  surface. The phase-detail enrichment pins the enriched shape with a committed golden fixture
  (`tests/fixtures/native-workflows/enriched-shape.json`) and a shape test, so a
  koto-side change to the emitted shape fails loudly. A *Claude Code*-side
  change is what the convention guard above checks for, by reading the
  release. It checks that names are present, not what the screen does with
  them, so the manual check is still the one that shows the entry renders.
