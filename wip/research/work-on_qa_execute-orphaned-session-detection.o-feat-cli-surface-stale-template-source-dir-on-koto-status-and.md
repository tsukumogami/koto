# QA: Issue 4 -- feat(cli): surface stale template_source_dir on koto status and koto session list

Commit reviewed: b53d61a.

## Method

Ran the automated test suites (`cargo test`, full suite: 1289+ lib tests plus every integration binary,
zero failures; targeted `cargo test --test stale_template_source_dir_cli_test`: 6/6 pass), then manually
exercised the built `./target/debug/koto` binary end to end against each acceptance criterion as a real
user would invoke it, independent of the automated test harness, to validate from a user perspective
rather than only re-running what the coder's own tests already assert.

## Scenarios exercised manually

### Scenario 1: `koto status` omits the key when `template_source_dir` still exists

```
$ koto init sess1 --template <tmp>/srctpl/simple.md
{"name":"sess1","state":"start"}
$ koto status sess1
{"current_state":"start","is_terminal":false,"name":"sess1","template_hash":"...","template_path":"..."}
```

**Pass.** No `stale_template_source_dir` key in the response.

### Scenario 2: `koto status` surfaces the key with direct wording for a LocalBackend session after the source directory is deleted

```
$ koto init sess2 --template <tmp>/srctpl/simple.md
$ rm -rf <tmp>/srctpl
$ koto status sess2
{"...","stale_template_source_dir":{"machine_id":"...","note":"template source directory no longer exists","path":"<tmp>/srctpl"},...}
```

**Pass.** Key present, `note` matches the direct (non-cloud) wording exactly, `path` matches the deleted
directory.

### Scenario 3: `koto session list` surfaces `template_source_status` with the same distinction, per row

```
$ koto session list
[
  {"id":"sess1", ..., "template_source_status":{"exists":false,"machine_id":"...","note":"template source directory no longer exists","path":"<tmp>/srctpl"}},
  {"id":"sess2", ..., "template_source_status":{"exists":false,"machine_id":"...","note":"template source directory no longer exists","path":"<tmp>/srctpl"}}
]
```

**Pass.** Both rows include the `note` field (both sessions pointed at the same now-deleted directory,
which was deleted after both were initialized -- an expected side effect of the manual scenario setup,
not a bug).

### Scenario 4: `koto status` surfaces softened wording for a CloudBackend session

Configured a live `koto` process against `Backend::Cloud` via `koto config set session.backend cloud` +
an RFC 5737 non-routable endpoint (`http://192.0.2.1:19000`), then repeated scenario 2 against that
configuration:

```
$ koto status sess3
{"...","stale_template_source_dir":{"machine_id":"...","note":"template source directory not found (if this session was synced from another machine, this may be expected)","path":"<tmp>/srctpl"},...}
```

stderr showed the expected non-fatal S3 warnings (`warning: cloud sync pull failed: ...`,
`warning: cloud sync: failed to list sessions: ...`) -- these are pre-existing `CloudBackend` behavior
(swallowed, logged to stderr, do not affect the command's exit code or stdout JSON), not new defects
introduced by this issue.

**Pass.** Key present, `note` is the softened cloud wording, distinct from the direct wording in
Scenario 2.

### Scenario 5: `koto session list` surfaces softened wording for a CloudBackend session

```
$ koto session list
[{"id":"sess3", ..., "template_source_status":{"exists":false,"machine_id":"...","note":"template source directory not found (if this session was synced from another machine, this may be expected)","path":"<tmp>/srctpl"}}]
```

**Pass.** Same softened wording as Scenario 4, confirming `handle_list`'s backend gating.

## Build/lint gates

- `cargo build`: clean.
- `cargo test`: 1289+ passed, 0 failed, across every test binary including the new
  `stale_template_source_dir_cli_test.rs`.
- `cargo fmt --check`: clean.
- `cargo clippy --lib -- -D warnings`: clean.

## Result

```json
{"scenarios_run": 5, "scenarios_passed": 5, "scenarios_failed": 0}
```

All five manually-exercised scenarios pass, matching the automated test suite's results. No defects
found.
