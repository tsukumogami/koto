# Verifying native `/workflows` rendering

Feature 1 of the koto agent-surface work makes a koto session appear as a
native entry in Claude Code's `/workflows` screen; Feature 2 enriches that entry
from a bare status into the session's real structure (ordered phases with the
active one marked, the active directive, per-phase evidence/gate outcomes, and a
gate-blocked → `blocked` status). This guide covers both the automated check and
the manual live-TUI check.

## Automated (CI / CLI)

`scripts/verify-native-workflows.sh` drives a real koto session with a real
template through the full publish → advance → render path and asserts both
features' "Verified when" criteria against the emitted `koto-<uuid>.json`:

```bash
cargo build
scripts/verify-native-workflows.sh
```

Expected output ends with
`ALL CHECKS PASSED: Feature 1 + Feature 2 verified end-to-end.` The script
exercises everything koto owns:

- **Feature 1** — single-session render with the current state, update on
  advance, done-on-completion, the opt-in no-write path (default path
  untouched), and the atomic `koto-<uuid>.json` filename.
- **Feature 2** — the phases render in order with the active one marked, the
  active phase's directive is legible, a completed phase shows its gate outcome,
  and a session whose gate did not pass renders as `blocked` (not running, not
  done).

The `hello-koto` template's `awakening` state carries a command gate that fails
until a greeting file exists, so the first advance lands on a gate-blocked state
(Feature 2's blocked case) and the second, after the greeting, completes —
exercising both statuses on one run.

The script does **not** confirm that Claude Code actually renders the file — CI
cannot drive the TUI — which is what the manual check below adds. The exact
enriched file shape is pinned by the golden fixture at
`tests/fixtures/native-workflows/enriched-shape.json` and its guard test
(`tests/native_workflows_shape.rs`); Feature 4 adopts that fixture as the anchor
for its version/fixture guard over the undocumented surface.

## Manual (live Claude Code TUI)

This confirms the one thing the automated check cannot: that Claude Code renders
the emitted file.

1. **Enable the hook.** Ensure the `koto-skills` plugin is installed so its
   `SessionStart` hook runs. On session start it announces the workflows
   directory for the current Claude Code session.

2. **Point koto at the directory.** Export the announced path so koto processes
   inherit it:

   ```bash
   export KOTO_WORKFLOWS_DIR="<projectDir>/<sessionId>/workflows"
   ```

   (The hook prints the exact `export` line. `<projectDir>` is the directory of
   the session's transcript; `<sessionId>` is the Claude Code session id.)

3. **Drive a koto session.** In the same Claude Code session, run a workflow:

   ```bash
   koto init demo --template <template> --intent "demo"
   koto next demo
   ```

4. **Open `/workflows`.** The koto session appears as an entry named for the
   session, showing its phases in order with the active one marked, the active
   phase's directive, and — for a multi-phase workflow — each completed phase's
   evidence or gate outcome.

5. **Advance and reopen.** Run `koto next demo` again, then reopen
   `/workflows` — the active-phase marker moves to the new phase, the previously
   active phase shows its outcome, and the directive updates (refresh-on-open).

6. **Block on a gate and reopen.** Drive the session into a state whose gate
   does not pass, then reopen `/workflows` — the entry reads *blocked*, not a
   spinning *running* and not *done*.

7. **Complete and reopen.** Drive the session to its terminal state, then reopen
   `/workflows` — the entry reads *done* (completed), not a stuck *running*.

8. **Negative check.** In a plain terminal (no `KOTO_WORKFLOWS_DIR`, no hook),
   run a koto session and confirm `/workflows` is unaffected and no
   `koto-*.json` is written.

## Notes

- koto writes `koto-<uuid>.json`, namespaced so it never collides with Claude
  Code's own `wf_*.json` files in the same directory.
- The write is atomic (temp-then-rename), so a `/workflows` reopen never sees a
  half-written file.
- The `/workflows` file format is an undocumented, version-coupled Claude Code
  surface. Feature 2 pins the enriched shape with a committed golden fixture
  (`tests/fixtures/native-workflows/enriched-shape.json`) and a guard test, so a
  koto-side change to the emitted shape fails loudly. The guard that catches a
  *Claude Code*-side change to the surface (a version/fixture check plus a
  rendered smoke check) is scoped to Feature 4, which adopts this fixture as its
  anchor; if the manual check stops rendering after a Claude Code update, that is
  the expected signal for Feature 4's guard.
