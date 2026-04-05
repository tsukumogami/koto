Run this command to submit the smoke test result and advance the workflow:

```bash
koto next deploy-staging --with-data '{"smoke_test_result": "passed"}'
```

**Why this command:**

The response has `action: "evidence_required"` with `blocking_conditions: []` (empty) and a non-empty `expects.fields`. This is Sub-case A — no gates are blocking, and the state is waiting for evidence. You submit it with `--with-data`.

The `expects.fields` schema defines one required field:

```json
"smoke_test_result": {
  "type": "enum",
  "required": true,
  "values": ["passed", "failed", "partial"]
}
```

Since all smoke tests passed, the correct value is `"passed"`. The key name comes directly from `expects.fields` — use it exactly as shown (snake_case).
