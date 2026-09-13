# Maintainer review: koto#236 state-log integrity

Diff reviewed: `git diff 5b8cfbf..HEAD -- . ':!wip'`. Checked against the code on
disk. `cargo test --test doc_names` passes (24/24); `cargo test --lib persistence`
passes (57/57).

Overall the change reads well. The why-comments at the four decision points
(no `create(true)`, header read before `store.add`, generic header type, the CLI
refusal) are present and, with the exceptions below, still true of the code
next to them. The `_in::<H>` naming follows an existing convention in the same
file (`append_header_in`-style "Header-type-generic form of ..." doc lines at
persistence.rs:84, :577, :671), so a reader will recognise the pattern.

## Blocking

### B1. A comment outside the diff now says `append_event` creates state files

`src/session/local.rs:282-283`:

```rust
// Match append_header/append_event which create state
// files with mode 0600.
```

After this change `append_event` never creates a file (persistence.rs:170-176,
and the whole point of the PR). The 0600 on the init tempfile is still right,
but the reason given names a behavior that no longer exists, and it contradicts
the new contract a reader just learned in persistence.rs. A developer reading
this could reasonably conclude append still has a create path and look for it.
One-line fix: "Match append_header, which creates state files with mode 0600."

## Advisory

### A1. The "only init creates a log" wording is narrower than the code

`src/cli/mod.rs:1422` ("Only `init` creates a session's log"),
`src/engine/persistence.rs:408` ("only init creates a log") and the user-facing
error at `persistence.rs:530` ("does not exist; only init creates one"). What
creates a log is `init_state_file`, reached from `koto init`, `koto session
start`, the batch scheduler's child spawn and retry respawn. In code comments
"init" is fine shorthand, but the error message at :530 reaches users, who will
read it as the `koto init` command and may try to run it on a batch child. Suggest
"only session initialization creates one" or naming the scheduler too.

### A2. The CLI refusal doesn't record why it is exit 2

`src/cli/mod.rs:1422-1434`, `:1484-1494`. The comment explains why the check
comes before the store, but not why it exits 2 (caller error) while a headerless
log from `handle_add` exits 3. The reason (match `koto status`, which uses exit 2
with the same message at mod.rs:5832-5840; a missing session is the caller's
mistake, a corrupt log is not) is only in the CHANGELOG. One clause in the
comment would stop someone "normalising" both paths to `EXIT_INFRASTRUCTURE`.

### A3. The request-store call site has no comment explaining `::<RequestHeader>`

`src/engine/request_store/mod.rs:1256`. This is the one caller that needs the
generic form, and nothing at the call site says so. Someone tidying imports back
to `append_event_idempotent` would get a header-parse refusal on every request
append (the request log's first line is a `RequestHeader`, not a
`StateFileHeader`). Tests would probably catch it, but a short comment ("the
request log has its own header type; the default form would refuse it") costs
nothing. Similarly the doc on `append_event_in` (persistence.rs:149-150) could
name the request store as the reason the generic form exists.

### A4. `append_event_idempotent_in` doc slightly overstates the refusal

`persistence.rs:307-308` says it "refuses a log whose first line isn't an `H`".
The hash scan (persistence.rs:336-368) runs before `next_seq_after_header`, so on
a headerless log whose later line carries a matching hash it returns
`Ok(Idempotent)` rather than refusing. Nothing is written either way, so
integrity holds, but the doc should say "never appends to" rather than "refuses".
Two smaller leftovers in the same function: `if path.exists()` at :336 is now
always true on unix (the flock open at :411-414 already failed if the file was
missing) and is dead; and the "Skip the header line" comment at :339 still skips
line 1 unseen, which is the exact pattern the new `next_seq_after_header` doc
(:512-518) calls out as the #236 bug. Harmless here because the scan only reads,
but worth a word so the next reader doesn't think it was missed.

### A5. Dead claim/respawn code keeps comments that assume append creates files

`src/engine/claim.rs:604-608` ("Fallback: write to
<session_dir>/<coord_id>.audit.jsonl so tests can still assert the call") and the
`create_dir_all` "ensure coord log parent" before `append_event` at claim.rs:620-626
and :671-677. With this change that fallback append fails, because the `.audit.jsonl`
file never exists (and `coord_state_file_for` looks for `<coord>.state.jsonl`
without the `koto-` prefix, so the fallback is the path it takes). The explore
research already established these functions have no production callers, so it's
not a live bug, but whoever revives them will hit a confusing "does not exist"
error that the comment says can't happen. A follow-up note or a one-line comment
update would do.

### A6. error-codes.md remedy: correct locally, silent on why ctx/ stays and on cloud

`docs/reference/error-codes.md:186`. I checked the local remedy against the code
and it holds: after moving the state file aside, `LocalBackend::exists` is false
(local.rs:81-83), the scheduler's half-init repair skips the child
(batch.rs:660-663), and `init_state_file` tolerates an existing session directory
(local.rs:229-240, exclusive rename on the file only). Two gaps:

- It says to leave `ctx/` in place but not why (so the context the agent already
  wrote survives the respawn; `koto session cleanup` works too but deletes it).
  Without the reason, a reader can't tell whether keeping `ctx/` is required or
  just allowed.
- With the cloud backend it doesn't work as written: `CloudBackend::exists` falls
  back to S3 (cloud.rs:686-692) and `read_header` pulls the S3 copy back down
  (cloud.rs:751-754, :150-158), so a headerless log already pushed to S3 comes
  back. The explore research listed this as an open question; the doc should at
  least say "local backend" or mention removing the remote copy.

### A7. cli-usage.md lists fewer no-log cases than the other docs

`docs/guides/cli-usage.md:459` gives "never initialized, or already finished and
cleaned up". command-reference.md and error-codes.md:224 also list a child moved
by a parent rewind and a `pending`/`blocked` batch child, which is the #236 case
itself. Not wrong, but the guide omits the case users actually hit.

### A8. Naming nits

- `next_seq_after_header` (persistence.rs:525) validates the header and computes
  the seq; the name reads as "the seq that follows the header line", which is only
  true for a header-only log. The doc comment covers it, so this is fine; something
  like `next_seq_checked` would be marginally clearer.
- `header_parse_failure` (persistence.rs:627) returns a message string, not an
  error; `describe_header_parse_failure` would say so. Its text ("The session was
  never initialized...", :634) is also produced for request-store logs through the
  shared `parse_header`, where "session" is the wrong noun. Rare path, low cost.
- `init_context_session` (tests/integration_test.rs) is clear, and its doc comment
  records why a bare directory no longer suffices.

## Verified accurate

- error-codes.md example message matches `header_parse_failure` output plus the
  `state file corrupted: ` prefix (errors.rs:19) exactly; the integration test
  asserts the same substrings via `koto status`.
- `workflow '<name>' not found` at exit 2 matches `koto status` (mod.rs:5832-5840),
  so the CHANGELOG's "the same as `koto status`" is true.
- `context.rs:23-26` and `:147-149` comments are true: the header read is what
  keeps a headerless-log refusal from storing content first, and the integration
  test `context_add_refuses_a_headerless_log_and_leaves_it_untouched` covers it.
- `acquire_state_flock` comment (persistence.rs:408-410) is true.
- `append_event` doc (persistence.rs:134-144) matches the code, including
  `sync_data()` after every write.
- CHANGELOG counts: 8 integration tests in tests/state_log_integrity_test.rs and
  5 unit tests in persistence.rs, as stated. The test-count sentence matches the
  style of neighbouring entries.
- batch-workflows.md outcome names (`pending`, `blocked`, `running`) match the
  table above it.
- No emojis or filler phrasing in the added prose or comments.
