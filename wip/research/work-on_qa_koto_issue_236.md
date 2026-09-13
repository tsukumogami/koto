# QA validation: koto#236 / #200

Driven through the real CLI (`target/debug/koto` at 9f08117), not the test
harness. Every session lived under a temp `KOTO_SESSIONS_BASE` with `HOME`
redirected, so the user's `~/.koto` was untouched. Script:
`/home/dangazineu/.claude/jobs/d49ea37c/tmp/qa-236.sh`.

Result: 25 checks, 25 passed, 0 failed.

## S1: the happy path still works

- `context add` on a started session succeeds.
- `context get` returns what was stored.
- `status` reads the session.

## S2: #236 as reported

A batch parent with `b waits_on a`. After the tick, `b` is blocked with no
state log.

- `context add orch.b` exits 2.
- The error is `workflow 'orch.b' not found`.
- No session directory is created for `orch.b`.

## S2b: the refusal does not strand the batch

This is the part that matters for #236's impact: the parent's batch used to
never settle.

- `context add` on the spawned sibling still works.
- Driving `a` to terminal and ticking the parent spawns `b` (`outcome:
  running`).
- `status orch.b` reads it.
- `context exists orch.b context.md` reports absent, so the refused write left
  nothing for the child to inherit.

## S3: after terminal cleanup

- A workflow driven to its terminal state has its directory removed.
- A late `context add` exits 2 and does not resurrect the session.

## S4: after a parent rewind moved the child

- `rewind` relocates `rw.t` to `rw~1.t`.
- `context add rw.t` (the old name) exits 2 and does not recreate the old name.
- The relocated child still has its header.

## S5: a log that already has no header

The shape older releases left on disk.

- `context add` exits 3.
- The error says `state log has no header`.
- The log is left byte-for-byte unchanged.
- `status` names the same condition.

## S6: context remove

- On a session that is not there, exits 2 with `workflow 'nosuch' not found`.
