# Decision 3 (CRITICAL): Result storage location + dereference, index stays lean

Executed INLINE.

## Question
Where does the full result live, and how does the converge point dereference
it, keeping the terminal-index scan path lean (PRD R9, AC9)?

## Options
- **3A — Result on the child session event log; bounded flag in the index.**
  The `request_store.result` event (Decision 1/2) is appended to the CHILD's
  own session log. The terminal-index entry gains one additive boolean field
  `has_result` (and optionally a short result-event seq pointer). The full
  result is dereferenced at converge time by reading the child's recorded
  result event (NOT replaying its working log).
- **3B — Result embedded in the index line.** The full envelope is serialized
  into the `TerminalIndexEntry` JSONL line.
- **3C — Per-result sidecar file.** Each result is written to a dedicated
  `<session>/result.json` sidecar; the index carries a pointer to the file.

## Chosen: 3A — result on child session log, bounded has_result flag in index

The terminal index is the hot path: the discovery scan walks it line-by-line
on every parent poll, and each line is bounded to `MAX_INDEX_LINE_BYTES`
(4096, within Linux `PIPE_BUF`) so concurrent `O_APPEND` writes never
interleave. The index must therefore stay a done-bit + pointer; the result
lives elsewhere.

The child's own session event log is the right home: it is append-only NDJSON
with the same atomic-append discipline, it is where `EvidenceSubmitted` and the
terminal evidence already live, and the result is produced on the child's
terminal tick. A new closed-enum variant `EventPayload::RequestStoreResult`
(wire `type: "request_store.result"`, in the reserved `request_store.*`
namespace) carries the `WorkflowResult` envelope. Older koto builds fall
through the existing `Unknown` arm (PRD R10 / AC10).

The terminal-index entry gains one additive field:

```rust
pub struct TerminalIndexEntry {
    // ... existing four fields ...
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_result: bool,   // bounded: a single bool, ~16 bytes on the wire
}
```

`has_result` is the done-bit: it tells the parent's converge gate, without
opening any child, that a result exists to dereference. It is bounded by
construction so the line stays within 4096 bytes regardless of result payload
size (AC9). Older readers tolerate the extra key (the index reader already
accepts unknown keys — see `reader_accepts_extra_unknown_keys_forward_compat`).

**Cleanup interaction (the decisive constraint).** koto auto-cleans a child's
session directory on its terminal tick (`backend.cleanup(child)` in
`handle_next`), which is why `ChildCompleted` is appended to the PARENT's log
as a fallback. A result stored ONLY on the child log would vanish on cleanup
before the parent converges. The design resolves this the same way koto
already resolves the outcome: the result envelope is ALSO carried on the
`ChildCompleted` event appended to the parent's log (an additive
`result: Option<WorkflowResult>` field on `ChildCompleted`). The parent's
converge gate therefore dereferences the result from its OWN log when the
child is cleaned up, or from the child's `request_store.result` event when the
child session still exists — never by replaying a transcript (PRD R8 / AC8).
The child-log copy is the durable record for `koto query` / status; the
parent-log copy is the converge-read source. The index `has_result` flag is
the cheap signal that gates the dereference.

## Rejected: 3B — embed result in index line
Directly violates R9 / AC9: an arbitrary-size payload blows past the 4096-byte
`PIPE_BUF` bound, and `append_terminal_index_entry` already hard-errors on
overlength lines. Embedding would force truncation or lose the atomic-append
guarantee that protects the N-concurrent-writers AC.

## Rejected: 3C — per-result sidecar file
Adds a third write target and a new file-lifecycle to manage (creation,
cleanup, atomic-rename, stale-orphan recovery) beside the log and the index —
exactly the kind of parallel surface PRD R7 / D4 push against. The session
event log already gives append-only durability and atomic writes for free; a
sidecar re-implements that machinery for no gain. It also reintroduces the
cleanup race (the sidecar would be removed with the session directory) without
solving it as cleanly as the parent-log copy does.

## Confidence: high on 3A's index+child-log split; medium-high on carrying the
result on the parent's `ChildCompleted` to survive cleanup — this is the
load-bearing detail and is grounded in the existing `ChildCompleted` fallback
mechanism (Issue #134). Flagged for cross-validation against Decisions 1 and 4.
