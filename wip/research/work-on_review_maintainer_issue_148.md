# Maintainer Review — Issue #148: Always wrap mermaid in fence

**Focus:** maintainer (can someone who didn't write this understand it and change it with confidence?)

---

## What the change does

`src/cli/mod.rs` lines 991–995. The `ExportFormat::Mermaid` arm now unconditionally produces:

```rust
format!("```mermaid\n{}```\n", raw).into_bytes()
```

The removed branch conditioned this on `args.output.is_some()` — when writing to a file, emit fenced mermaid; when writing to stdout (no `--output`), emit raw mermaid text. The removed comment read: "Raw mermaid text for stdout composability."

---

## Finding 1 — Missing explanation for the intentional design change (Advisory)

**Location:** `src/cli/mod.rs:991–995`

The next developer who reads this code will see a simple unconditional format call and have no idea that someone consciously decided to discard the "stdout composability" goal. There is no comment explaining why the two-path behavior was collapsed. That prior behavior was explicit enough to have its own comment; its removal deserves equal explanation.

The misread risk: the next developer wanting to add a `--raw` flag or fix a "piping doesn't work" bug will not know whether stdout composability was considered and rejected, was considered and deferred, or was simply overlooked. They may re-introduce the conditional or open a ticket for something that was deliberate.

**Recommendation:** Add a one-line comment at the `ExportFormat::Mermaid` arm:

```rust
// Fence is always emitted (stdout and file alike) so the output is
// directly embeddable in markdown without post-processing by callers.
```

This is advisory rather than blocking because the behavior itself is clear and the tests accurately reflect it — the risk is a confused next developer, not a wrong mental model that causes a bug.

---

## Finding 2 — No implicit contract broken; callers are consistent (Clean)

The `--output` path and the stdout path both now receive `output_bytes` built from the same unconditional `format!("```mermaid\n{}```\n", raw)`. The `--check` path reads `output_bytes` after the match, so it also compares against fenced content — consistent with what `--output` writes. No implicit ordering contract was broken.

There are no other callers of `to_mermaid()` in the CLI layer that expect raw output; the library function itself still returns raw mermaid (no fence), so library callers are unaffected.

---

## Finding 3 — Test communicates new behavior accurately, but the name undersells the intent (Advisory)

**Location:** `tests/integration_test.rs:4137–4165` (`export_cli_outputs_mermaid_to_stdout`)

The test asserts:

```rust
assert!(stdout.starts_with("```mermaid\n"), ...);
assert!(stdout.ends_with("```\n"), ...);
```

This correctly captures the new behavior. The test name "outputs mermaid to stdout" is accurate. However, it doesn't convey the specific policy change — that the fence is now unconditional (present on stdout, not just on file output). A future developer adding a `--raw` mode will read this test and see "mermaid goes to stdout" without understanding why the fence is there.

This is advisory: the test doesn't lie, and the assertion is correct. But renaming it to `export_cli_stdout_always_wraps_in_mermaid_fence` would make the policy legible as documentation.

The companion test `export_cli_writes_to_output_file` (lines 4167–4207) also asserts `starts_with("```mermaid\n")` on the file content, so both paths are tested and consistent. No finding there.

---

## Summary

| # | Severity | Location | Issue |
|---|----------|----------|-------|
| 1 | Advisory | `src/cli/mod.rs:991` | No comment explaining why "stdout composability" was intentionally removed; next developer won't know if this was a considered decision |
| 2 | Clean | `src/cli/mod.rs:991–1047` | No broken contracts; all paths use the same `output_bytes`; library callers unaffected |
| 3 | Advisory | `tests/integration_test.rs:4137` | Test name doesn't communicate that always-fenced-on-stdout is the intended policy, not an incidental behavior |

**Blocking:** 0
**Advisory:** 2
