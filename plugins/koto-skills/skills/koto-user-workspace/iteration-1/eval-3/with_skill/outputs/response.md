# Can you override the gate and continue?

**No, you cannot override this gate yourself.** The response shows `"agent_actionable": false` on the `secrets_scan` blocking condition, which means the gate has no override mechanism available to agents.

## What the response tells you

The action is `gate_blocked` with a single blocking condition:

```json
{
  "name": "secrets_scan",
  "type": "command",
  "status": "failed",
  "agent_actionable": false,
  "output": {
    "exit_code": 1,
    "error": "3 potential secrets found in src/config.rs"
  }
}
```

`agent_actionable: false` means calling `koto overrides record security-audit --gate secrets_scan --rationale "..."` will fail. Don't attempt it.

`expects` is `null` (implicit in a `gate_blocked` response with no accepts block), so there's no evidence submission path either.

## What you should do

This gate requires you to resolve the underlying condition — remove the potential secrets from `src/config.rs` — before the workflow can advance.

**Step 1** — Fix the issue in the source file. The scanner found 3 potential secrets in `src/config.rs`. Open that file and remove or replace the secrets (use environment variables, a secrets manager, or a config injection pattern instead of hardcoded values).

**Step 2** — Once the file is clean, call `koto next` again:

```bash
koto next security-audit
```

The gate re-runs on each `koto next` call. If `src/config.rs` no longer contains secrets, the gate will pass and the workflow will advance.

**Step 3** — If you can't resolve this yourself (for example, the secrets belong to a system you don't control), escalate to the user. The directive confirms the intent: "The secrets scanner must complete without findings before this step can proceed." This is an intentional hard block — the workflow author chose not to allow overrides for this gate.

## Summary

| Question | Answer |
|---|---|
| Can I call `koto overrides record` to bypass this? | No — `agent_actionable` is `false` |
| Can I submit evidence to bypass this? | No — `expects` is `null`, no accepts block |
| What unblocks the workflow? | Fix `src/config.rs` so the scanner exits 0 |
| What if I can't fix it? | Escalate to the user |
