# Response: Blocked gate on `security-audit`

## Short answer: no, you cannot override this gate

The `koto next` response tells you two things that matter here:

1. `"action": "gate_blocked"` — the state has no `accepts` block, so there is no evidence submission path. You can't use `koto next --with-data` to push past it.
2. `"agent_actionable": false` on the `secrets_scan` condition — this gate has no `override_default` value and no built-in default that koto can substitute. The `koto overrides record` command would fail because there's no override value to apply.

In other words, both escape hatches are closed. The gate is externally controlled by the actual scan result.

## What you should do

The gate failed because the secrets scanner found 3 potential secrets in `src/config.rs`. The only way to unblock is to fix that underlying issue:

1. Open `src/config.rs` and locate the flagged values. The scanner's output (`"3 potential secrets found in src/config.rs"`) tells you the file but not the line numbers — run the scanner directly to get exact locations.
2. Remove or rotate the secrets. Common fixes:
   - Replace hardcoded credentials with environment variable reads.
   - Move secrets to a `.env` file that is gitignored.
   - If the values are false positives, annotate them according to your scanner's ignore syntax (e.g., a `# nosec` comment for some tools), then confirm with your security policy whether that's acceptable.
3. Re-run `koto next security-audit` after fixing the file. The gate will re-evaluate automatically on the next call — you don't need to do anything special in koto to trigger it.

## Why `agent_actionable: false` blocks overrides

`koto overrides record` requires a value to substitute as the gate's output. For `command` gates the built-in default is `{"exit_code": 0, "error": ""}`. However, when the template author sets `agent_actionable: false` (typically by omitting or explicitly disabling `override_default`), koto refuses to record an override because the gate is intentionally not overridable. A secrets gate in a security audit workflow is a reasonable place for this restriction — bypassing it would undermine the purpose of the workflow.

## Summary of your options

| Option | Available? | Why |
|---|---|---|
| `koto next --with-data ...` to submit evidence | No | `action` is `gate_blocked`, not `evidence_required`; no `accepts` block |
| `koto overrides record ... --gate secrets_scan` | No | `agent_actionable: false` |
| Fix `src/config.rs` and re-run `koto next` | Yes | The correct path forward |
| `koto next --to <state>` to force a transition | Technically possible, but not advised | `--to` bypasses all gate checks; use only when explicitly authorized to skip gates |

The `--to` flag exists for emergency use but should not be used here unless someone with authority over this workflow explicitly approves skipping the secrets check. Using it would leave the secrets in place and defeat the audit.
