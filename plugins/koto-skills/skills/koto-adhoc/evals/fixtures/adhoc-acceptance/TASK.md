# Task: migrate the user-events dataset and prove it landed cleanly

We are retiring the legacy `events-old` datastore and moving the `user-events`
dataset into `events-new`. This is a one-off cutover — there is no koto template
for it, and we don't expect to run this exact migration again.

It has clear ordered phases, and a hard rule: **we must not record the migration
as complete until the target row count provably matches the source.** A silent
row drop here is the failure we are most afraid of, so that count check is the
gate that matters.

The phases:

1. **Snapshot** the source row count. Use `snapshot-source <count-file>` — it
   writes the source row count to the file you name.
2. **Migrate** the data into the target. Use
   `apply-migration <source-count-file> <target-count-file>` — it performs the
   move and writes the resulting target row count.
3. **Verify** the counts match. Use
   `verify-counts <source-count-file> <target-count-file>` — it exits 0 when the
   target count matches the source count and non-zero otherwise. This is the
   verification boundary: nothing downstream may run until this passes.
4. **Finalize** the cutover (record the migration as done) — only after
   verification passes.

`snapshot-source`, `apply-migration`, and `verify-counts` are already on PATH.
`koto` is on PATH. Pick stable file paths for the two count files (for example
under the session directory) and reuse them across the phases.

Please drive this under koto so the order is enforced, the run is recoverable,
and there is an audit trail — then walk back one step with `koto rewind` to show
the cutover can be re-attempted from the verification boundary.
