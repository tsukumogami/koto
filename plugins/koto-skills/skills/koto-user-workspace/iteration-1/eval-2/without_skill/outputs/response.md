Run this command to submit the smoke test result and advance the workflow:

```
koto next deploy-staging --with-data '{"smoke_test_result":"passed"}'
```

The `--with-data` flag accepts a JSON object whose keys match the fields listed in the `expects.fields` section of the previous `koto next` response. Here the only required field is `smoke_test_result`, which is an enum that accepts `"passed"`, `"failed"`, or `"partial"`. Passing `"passed"` satisfies the schema, koto appends an `evidence_submitted` event, and the advancement loop runs to move the workflow to the next state.
