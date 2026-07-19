# Verifying native `/workflows` rendering

The initial render makes a koto session appear as a
native entry in Claude Code's `/workflows` screen; the phase-detail work enriches that entry
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
`ALL CHECKS PASSED: native /workflows rendering verified end-to-end.` The script
exercises everything koto owns:

- **The initial render** — single-session render with the current state, update on
  advance, done-on-completion, the opt-in no-write path (default path
  untouched), and the atomic `koto-<uuid>.json` filename.
- **The phase-detail enrichment** — the phases render in order with the active one marked, the
  active phase's directive is legible, a completed phase shows its gate outcome,
  and a session whose gate did not pass renders as `blocked` (not running, not
  done).

The `hello-koto` template's `awakening` state carries a command gate that fails
until a greeting file exists, so the first advance lands on a gate-blocked state
(the blocked case) and the second, after the greeting, completes —
exercising both statuses on one run.

The script does **not** confirm that Claude Code actually renders the file — CI
cannot drive the TUI — which is what the manual check below adds. The exact
enriched file shape is pinned by the golden fixture at
`tests/fixtures/native-workflows/enriched-shape.json` and its guard test
(`tests/native_workflows_shape.rs`); the future drift-guard adopts that fixture as the anchor
for its version/fixture guard over the undocumented surface.

## Manual (live Claude Code TUI)

This confirms the one thing the automated check cannot: that Claude Code renders
the emitted file.

1. **Enable rendering (once).** Opt in with a config flag:

   ```bash
   koto config set workflows.native true --user
   ```

   With this set, a koto session driven inside a Claude Code session
   self-discovers that session's workflows directory from the
   `CLAUDE_CODE_SESSION_ID` environment variable Claude Code sets in every
   subprocess — no plugin, hook, or manual export required. It stays off by
   default, so koto's normal path is untouched until you opt in.

   *Alternative (explicit handoff):* instead of the config flag, export the
   directory directly — useful for a headless host or a non-standard layout:

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
   does not pass, then reopen `/workflows` — the entry reads *blocked*, not a
   spinning *running* and not *done*.

6. **Complete and reopen.** Drive the session to its terminal state, then reopen
   `/workflows` — the entry reads *done* (completed), not a stuck *running*.

7. **Negative check.** With rendering disabled (`workflows.native` unset and no
   `KOTO_WORKFLOWS_DIR`), run a koto session and confirm `/workflows` is
   unaffected and no `koto-*.json` is written.

## Notes

- koto writes `koto-<uuid>.json`, namespaced so it never collides with Claude
  Code's own `wf_*.json` files in the same directory.
- The write is atomic (temp-then-rename), so a `/workflows` reopen never sees a
  half-written file.
- The `/workflows` file format is an undocumented, version-coupled Claude Code
  surface. The phase-detail enrichment pins the enriched shape with a committed golden fixture
  (`tests/fixtures/native-workflows/enriched-shape.json`) and a guard test, so a
  koto-side change to the emitted shape fails loudly. The guard that catches a
  *Claude Code*-side change to the surface (a version/fixture check plus a
  rendered smoke check) is scoped to a later slice, which adopts this fixture as its
  anchor; if the manual check stops rendering after a Claude Code update, that is
  the expected signal for the future drift-guard.
