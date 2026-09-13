# Explore Scope: state-log-integrity

## Visibility

Public

## Core Question

koto can leave a session's state log without its header line, after which every
command on the session fails with `state file corrupted: failed to parse header:
missing field workflow`. #236 has a clear mechanism (an append to a missing log
creates it headerless); #200 (a log truncated to one `context_added` line under
concurrent writers) does not. Which fix levels -- refuse appends to a missing
log, lock coverage plus atomic rewrites for every writer, and a tolerant reader
that recovers logs already on disk -- belong in one PR, and what evidence backs
each?

## Context

Dispatched from a task brief. koto is Rust (the generated `CLAUDE.local.md`
still describes the old Go code; ignore it). main is at 5b8cfbf. #236 and #200
are open with no comments and no PR. The brief's current facts, each to be
re-confirmed: `context add` creates the session directory and then appends
through `append_event`, which opens with `create(true).append(true)`;
`read_last_seq` skips line 1 as if it were the header, explaining the duplicate
`seq:1`; `lock_state_file` has one production caller in `koto next` on
batch-scoped states; `rewrite_header_identity` (session/local.rs) does a plain
`fs::write`; `rewrite_header_atomically` (engine/claim.rs) does
read-temp-rename with no lock; `rewind_relocate_children` renames live child
sessions, which could let a stale writer recreate a headerless log at the old
path (#200's shape). `session/recover.rs` already handles headerless logs in
tests. #171 asks whether the state-file lock should be blocking or fail-fast.

## In Scope

- #236: missing-log appends, including after a terminal state deletes the
  session directory (#234's cleanup)
- #200: a deliberate reproduction attempt and which mechanism it points to
- Every write path to a state log: append vs rewrite, lock taken or not
- The lock's coverage, and #171's blocking-vs-fail-fast question as far as
  coverage needs it
- Whether to recover headerless logs already on disk
- The cloud backend (`src/session/cloud.rs`) wrapping the local one
- Agent-facing surface changes (koto-skills, error codes, `doc_names`)

## Out of Scope

- #234 (terminal signal and retention), #190 (retention)
- #171 as a standalone fix
- The v0.12.3 release and CHANGELOG restructuring (only an `[Unreleased]` entry)
- Other open koto bugs

## Research Leads

1. **Does #236's mechanism hold exactly as described, and can a test show it?** (lead-236-mechanism)
   Write a failing integration test for `context add` against a session with no
   log (unstarted child, and a session deleted after a terminal state). Confirms
   the mechanism and gives the PR its regression test.

2. **What is every code path that writes a session's state log, and does each append or rewrite, and take the lock?** (lead-write-paths)
   The fix for concurrent writers depends on a complete inventory, including
   the cloud backend and any compaction/migration/rewind path.

3. **Can #200 be reproduced, and which mechanism does it point to?** (lead-200-repro)
   Concurrent writers (next, context add, session rebind, rewind of a parent
   while a child writer runs) in a tight loop, checking whether the header
   survives. Decides whether `Fixes #200` is honest.

4. **What does the lock actually protect, and what should its semantics be?** (lead-lock-semantics)
   flock on which file/inode, how a rename-replace interacts with a lock held on
   the old inode, blocking vs fail-fast (#171), and what the design docs say.

5. **Should koto recover headerless logs already on disk, and how?** (lead-recovery)
   What `recover.rs` already does, whether a header can be reconstructed from the
   session name and context, and what a tolerant reader would cost.

6. **What agent-facing surface would a fix change?** (lead-agent-surface)
   koto-skills text about `context add`, corruption warnings, error codes, and
   `doc_names` constraints, so a refusal or new error code lands with docs.
