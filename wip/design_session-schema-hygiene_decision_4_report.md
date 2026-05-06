# Decision 4: --rationale CLI surface

## Recommendation

Option A: Simple `--rationale <text>` flag

## Confidence

High

## Rationale

The codebase already uses `--rationale` as a simple `String` flag on `koto overrides record`, passing it directly to `handle_overrides_record` via clap. Adding the same flag to `koto next --to` and `koto rewind` follows the existing pattern exactly. The batch scheduler never invokes `koto next --to` (confirmed: batch.rs never creates `DirectedTransition` events, and `koto rewind` is entirely manual). Both commands are human-initiated overrides that agents can call naturally with an inline string flag. Stdin and file variants add complexity with no benefit for the typical short rationale string an agent or human would provide.

## Option Analysis

### Option A: Simple --rationale flag

**Pros:**

- Matches the existing `koto overrides record --rationale <text>` pattern in `overrides.rs`. No new conventions to learn.
- Clap handles it as `Option<String>` — absent means `rationale` is omitted from the event payload. Fully backward-compatible: existing scripts that call `koto next --to <state>` or `koto rewind <name>` continue to work without any changes.
- Agent-friendly: AI agents invoke CLI commands by constructing argument lists. A flag with an inline value is the simplest form; agents don't need to write files or manage stdin.
- Minimal implementation surface: add `#[arg(long)] rationale: Option<String>` to the `Next` and `Rewind` command variants, thread the value through `handle_next` and `handle_rewind`, and include it in the respective event payloads.
- The event types (`DirectedTransition` and `Rewound`) already carry only `from` and `to` today. Adding an optional `rationale` field with `#[serde(default, skip_serializing_if = "Option::is_none")]` keeps older state files round-trip clean, matching how other additive fields (e.g., `submitter_cwd` on `EvidenceSubmitted`) are handled.
- `koto overrides record` applies a 1 MB size cap to `--rationale`; the same cap is straightforward to apply here.

**Cons:**

- Long rationale text embedded in a shell argument may require quoting. This is a minor ergonomic issue; agents construct strings programmatically and humans rarely write more than a sentence of rationale inline.

### Option B: --rationale with stdin support

**Pros:**

- Supports arbitrarily long rationale text without quoting concerns.
- Allows `koto next --to <state> --rationale - < notes.txt` for file-based rationale.

**Cons:**

- Significantly more complex to implement: must detect the `-` sentinel, read and drain stdin, handle pipe-closed errors, and test both code paths.
- Agent usability is worse: agents invoking the CLI do not have a convenient stdin pipe. A flag value is the natural agent invocation form.
- Reads stdin in a CLI that otherwise never does I/O from stdin. Batch-scheduler safety is not a concern (it never invokes `--to`), but the behavior is surprising for interactive use.
- No existing precedent in the koto CLI. `resolve_with_data_source` handles `@file.json` for `--with-data`, but that reads a file path from the flag value — it does not read stdin. Adding stdin-reading would diverge from that pattern.

### Option C: --rationale-file

**Pros:**

- Separates long rationale text from the flag value entirely.
- No quoting issues for multi-line text.

**Cons:**

- Adds a second flag (`--rationale-file`) that agents and humans must know about alongside `--rationale`. That doubles the documentation surface for a feature whose primary value is a short audit string.
- Requires the caller to write a file before invoking the command, which is awkward for both agents and scripted use.
- Adds implementation complexity: mutual-exclusivity checks between `--rationale` and `--rationale-file`, or a combined resolver similar to `resolve_with_data_source`.
- No precedent for this pattern in koto today. The `@file.json` prefix on `--with-data` achieves the same goal with a single flag; if file-based rationale were ever needed, the same `@` prefix convention could be applied to `--rationale` without introducing a second flag. That said, there is no evidence that rationale text will ever be long enough to warrant this.

## Batch Mode Findings

The batch scheduler (`src/cli/batch.rs`) is invoked from `handle_next` after `advance_until_stop` returns, when the parent's final state carries a `materialize_children` hook. It spawns child workflows via `init_child_from_parent` and never calls `koto next --to` or appends `DirectedTransition` events. A search of `batch.rs` for `DirectedTransition` and `directed_transition` returned no results.

`koto rewind` is entirely manual: its handler (`handle_rewind` in `src/cli/mod.rs`) is only reached via `Command::Rewind { name }` from the CLI dispatcher. Neither the batch scheduler nor any internal code path invokes it programmatically.

Adding `--rationale` as an optional flag to both commands is therefore safe with respect to batch mode: the scheduler is unaffected because it never calls either command, and the flag's absence from existing call sites is backward-compatible because it is `Option<String>`.

## Key Assumptions

- Rationale text is short enough to pass inline. Agents write concise audit strings ("bypassing stale CI gate", "correcting misclassified state"). If rationale regularly exceeds a few hundred characters, the `@file.json` prefix pattern from `--with-data` could be applied to `--rationale` later without breaking existing callers.
- The `DirectedTransition` and `Rewound` event structs will gain an optional `rationale: Option<String>` field serialized with `#[serde(default, skip_serializing_if = "Option::is_none")]`, matching the additive-field convention used for `submitter_cwd` on `EvidenceSubmitted`.
- The 1 MB size cap from `koto overrides record` applies here too, preventing runaway payloads.
- `koto next --to` and `koto rewind` are the only two user-facing commands that need `--rationale`. Internal batch paths are confirmed to not invoke them.

## Rejected Options

**Option B** was ruled out because stdin reading adds implementation complexity and is awkward for agent invocation. No existing koto command reads from stdin, and the rationale use case does not justify introducing that pattern.

**Option C** was ruled out because it doubles the flag surface without corresponding benefit. If file-based rationale is ever needed, the `@` prefix convention from `--with-data` can extend `--rationale` to cover that case in a single flag, making a separate `--rationale-file` unnecessary.
