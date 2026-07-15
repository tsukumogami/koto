# Verifying native `/workflows` rendering

Feature 1 of the koto agent-surface work makes a koto session appear as a
native entry in Claude Code's `/workflows` screen. This guide covers both the
automated check and the manual live-TUI check.

## Automated (CI / CLI)

`scripts/verify-native-workflows.sh` drives a real koto session with a real
template through the full publish → advance → render path and asserts Feature
1's four "Verified when" criteria against the emitted `koto-<uuid>.json`:

```bash
cargo build
scripts/verify-native-workflows.sh
```

Expected output ends with `ALL CHECKS PASSED: Feature 1 verified end-to-end.`
The script exercises everything koto owns (materialization on commit, the
context-store publish/discover, the file shape, terminal inference, and the
opt-in no-write path). It does **not** confirm that Claude Code actually renders
the file — CI cannot drive the TUI — which is what the manual check below adds.

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
   session, showing its current state, with a running status.

5. **Advance and reopen.** Run `koto next demo` again, then reopen
   `/workflows` — the entry shows the new state (refresh-on-open).

6. **Complete and reopen.** Drive the session to its terminal state, then reopen
   `/workflows` — the entry reads *done* (completed), not a stuck *running*.

7. **Negative check.** In a plain terminal (no `KOTO_WORKFLOWS_DIR`, no hook),
   run a koto session and confirm `/workflows` is unaffected and no
   `koto-*.json` is written.

## Notes

- koto writes `koto-<uuid>.json`, namespaced so it never collides with Claude
  Code's own `wf_*.json` files in the same directory.
- The write is atomic (temp-then-rename), so a `/workflows` reopen never sees a
  half-written file.
- The `/workflows` file format is an undocumented, version-coupled Claude Code
  surface. A guard that fails loudly when it drifts is scoped to a later slice;
  if the manual check stops rendering after a Claude Code update, that is the
  expected signal to add it.
