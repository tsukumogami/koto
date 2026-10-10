# Lead: What can the context store and session backend already answer about a session's contents?

## Findings

### ContextStore trait (`src/session/context.rs:53-104`; key grammar at 33-52)

- `add`, `add_with_writer`, `meta` (default `None`), `get` (returns
  `Vec<u8>`), `ctx_exists` (bool), `remove`, `list_keys(session, prefix)`.
- No bulk read, no stat-and-size call, no text/binary notion.
- `Backend` (enum over Local and Cloud) implements it by delegation at
  `src/session/mod.rs:749-780`. `ContextLogStore` (`context_log.rs:313-336`)
  wraps it.

### KeyMeta and manifest (`context.rs:6-21`, `local.rs:479-504`)

- `KeyMeta`: `created_at` (ISO string), `size: u64`, `hash` (sha256 hex),
  `writer: Option<String>` (open vocabulary: agent, transition, koto, sync;
  absent on keys written before writers were recorded or via plain `add`).
- Manifest `ctx/manifest.json`, shaped `{"keys": {key: KeyMeta}}` (BTreeMap).
- Missing manifest reads as empty; corrupt manifest makes `list_keys` return
  `Err` but `meta` quietly returns `None`.

### Local semantics

- `list_keys` (`local.rs:694`) is manifest-driven and sorted; `ctx_exists`
  (`local.rs:648`) checks the file on disk — the two can disagree.
- `meta` returns manifest data only; size/hash never re-checked against the
  file. `get` is a plain `fs::read`.

### `koto context list` / `get` today (`src/cli/context.rs:94-258`, dispatch `src/cli/mod.rs:1595-1720`)

- `list` prints a JSON array of key names only; no size, hash or writer.
- `get` writes raw bytes to stdout or `--to-file`, and appends a
  `context_read` event (reader `cli`) to the session log, best-effort;
  `exists` does the same. Before `get`, `restore_assigned` reconciles
  transition-assigned keys from the log, which can write to the store.
- Errors are JSON `{error, command}` with exit 3; exit 2 for a migrated
  session. `context list`/`exists` don't check the session exists.

### Session header (`src/engine/types.rs:292-`)

- Identity: `workflow`, `session_id`, `created_at`. Template: `template_hash`,
  `template_name`, `template_source_file`, `template_source_dir`. Intent and
  lineage: `intent`, `parent_workflow`, `parent_session_id`,
  `root_session_id`. Execution: `execution_dir` (canonical; the anchor),
  `origin` (`SessionOrigin{anchor, store:{kind: "local"|"cloud", base}}`,
  lines 225-242), `command_environment`. Plus request-store fields.
- Optional on older sessions: `execution_dir`, `origin`, `intent`,
  `template_name`.
- **No git remote is stored anywhere.** "Remote" in the view must be derived
  (or means the cloud store, `origin.store.kind`).

### Current state and directive

- `derive_state_from_log(events)` (`engine/persistence.rs:902`) gives current
  state; `derive_machine_state(header, events, session_dir)`
  (`persistence.rs:1147`) gives `{current_state, template_path,
  template_hash}`, `None` on a corrupt log.
- The directive comes only from `handle_status` (`src/cli/mod.rs:7037-7230`):
  reads the compiled template, checks sha256 (reports `template_hash_mismatch`
  rather than failing), substitutes `SESSION_DIR`/`SESSION_NAME`/log
  variables into `directive` and `details`, adds `expects`, `result` on
  terminal, batch info. Read-only, no lock.
- `handle_status` is NOT reusable as a library call: it calls
  `exit_with_error_code` on every error path. The directive logic would need
  extracting.
- The dashboard (`dashboard_data.rs:655 read_detail(path, id)`) works on a
  state-file `Path`, not through the backend.

### Cloud reads (`cloud.rs:1136-1275`, `sync.rs`)

- `list_keys`: local manifest merged with remote manifest (one manifest GET,
  cached 5 seconds in a single-session `ManifestCache`); no per-key request.
- `meta`: local manifest, else remote manifest (no content GET).
- `ctx_exists`: local file check, else remote manifest; fetch errors become
  `false`.
- `get` calls `pull_context_if_newer`: fetches remote manifest (cached),
  compares hashes, GETs `ctx/<key>` when stale/missing, writes it into the
  local store with writer `sync` and appends a `context_added` event. **A
  "read-only" view calling `get` on cloud mutates local files and the log.**
  Pull failures warn and fall back to the stale local copy.
- Reading all keys on cloud: one manifest GET plus one object GET per
  stale/missing key, no batching. Locally: one manifest read plus N
  `fs::read`.
- `read_events`/`read_header` on cloud call `check_not_migrated` then
  `sync_pull_state` (a network round trip). `read_events_local` does no pull.

### Migration refusal (`cloud.rs:136-176`, `session/mod.rs:104-127`)

- `check_not_migrated`: one bucket listing for `migrated.json`, cached per
  process; failed listing warns and proceeds; unparseable marker still
  refuses. Returns `SessionMigrated{name, target, workspace}` with text
  `session_migrated: session 'X' was migrated to 'Y' in <path>; continue it
  there`.
- Per path: `read_events`/`read_header`/`list_keys`/`get`/`add`/`remove` give
  `Err(SessionMigrated)` (downcastable); `ctx_exists` gives `false`; `meta`
  gives `None`; `koto context *` pre-check exits 2; `status` exits 2; `next`
  emits `NextErrorCode::SessionMigrated`, exit 2.
- A view should call `check_not_migrated` (reachable via
  `Backend::check_not_migrated`, `session/mod.rs:585`) once up front, then
  name `target` and `workspace` and stop, instead of seeing absent keys.
- The local backend never reports migration. The dashboard and the
  `/workflows` surface read state files by path, bypassing the backend, so
  they would not see the refusal on a cloud session whose local copy is stale.

### Text vs binary detection

None for the context store or viewing. Only UTF-8 handling is lossy decoding
of command output (`redact.rs:420`), of a context value for `result`
(`engine/terminal_result.rs:154`), and of an input body (`init_entry.rs:930`).
No NUL check, no size cap, no truncation helper for context values.
`KeyMeta.size` is available without reading content.

### Anything reading all keys' content today

None found. `list_keys` is called only from `cli/context.rs:253`. `meta` is
used in `context_log.rs` and `workflows_surface/materialize.rs`. `ctx_exists`
in `gate.rs` and `workflows_surface/discover.rs`. `get` is used by gates,
`context_assign` and `terminal_result`, always for a single key.

## Implications

- The view's data layer is a thin composition: header, derived state,
  `list_keys`, `meta` per key, `get` per key — plus two new pieces: a
  size-capped text-or-binary classifier, and a shared "session snapshot"
  function with the directive logic extracted from `handle_status`.
- `meta.size` and `hash` let the view say "binary, 4.2 KB, sha…" without
  fetching content; fetch only to classify and preview, checking `size` first.
- On cloud, `get` is not a pure read. The view should either accept the pull
  side effects or read local content only and use `meta` for "present
  remotely, not pulled".
- "Absent" and "unreadable" verdicts need a different source than the trait's
  bools: call `check_not_migrated` first; treat a manifest-listed key whose
  `get` errs as "unreadable".
- Cloud costs are bounded for metadata (~one manifest GET plus one migration
  listing); content costs one GET per stale key.

## Surprises

- Reading a context key through the CLI writes to the session log
  (`context_read` events). A viewer should call `store.get` directly or
  accept the events.
- A cloud `get` can write locally.
- `ctx_exists` and `list_keys` use different sources locally (disk vs
  manifest).
- No git remote is stored in the header, contrary to the scope's "anchor and
  remote" assumption.
- The dashboard reads state files by path and bypasses the backend, so it has
  no built-in migration or cloud awareness.

## Open Questions

- Does "remote" mean the git remote of the anchor, the cloud store, or
  `origin.store.kind`? A git remote would have to be computed at view time.
- Should the view pull remote-only keys (with side effects), or show "remote
  only" from `meta`?
- What are the size cap and the text/binary rule, and how is a large value
  shown (head, tail, or size and hash only)?
- Should the directive logic be extracted from `handle_status` into a shared
  function, and should the dashboard path see `SessionMigrated`?
- Should reads in the view be logged as `context_read`?

## Summary

The store already answers presence, key list, and per-key size, hash, writer
and time cheaply via the manifest, and a migrated session is reliably
detectable up front with `check_not_migrated` — though `ctx_exists` and `meta`
hide that refusal as `false`/`None`. Missing: a text/binary classifier, a
library-callable state-and-directive derivation (it lives inside
`handle_status`, which exits the process), a recorded git remote, and a
side-effect-free cloud content read. The biggest open question is whether the
view may pull remote-only content, with those side effects, or must show
remote-only keys from metadata alone.
