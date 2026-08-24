# Evidence for the name-resolution check

Two records the check owes a reader: proof that it catches the defect it was
built for, and the classified list of everything it found on the branch that
introduced it.

This file lives under `tests/` rather than `docs/` deliberately. It quotes every
token the check reports, and `docs/testing/` is one of the nine checked
surfaces — so written there, each quotation would become a live finding with no
record behind it, and the file documenting the gate would turn the gate red.
`tests/` is outside the checked set.

## The acceptance run: v0.12.0

The bar this work was set was to catch the instance that motivated it, on the
tree as it actually stood — demonstrated, not inferred from the files being
unchanged since.

```bash
git archive v0.12.0 | tar -x -C /tmp/v0120
KOTO_DOC_NAMES_ROOT=/tmp/v0120 cargo test --test doc_names
```

The v0.12.0 tree carries no `tests/doc_names.allow`, so the allowlist is empty
and everything is reported. The check found 17 unresolved names. The one that
matters, in full:

```
FAIL: unresolved command `koto session rebind` (16 sites)
    docs/guides/default-action-authoring.md:515  koto session rebind demo --to <dir>
    docs/guides/default-action-authoring.md:574  koto session rebind <session> --to <dir>
    docs/guides/default-action-authoring.md:588  koto session rebind demo
    docs/reference/error-codes.md:94  koto session rebind my-workflow --to <dir>
    docs/reference/error-codes.md:102  koto session rebind my-workflow --to <dir>
    docs/reference/error-codes.md:107  koto session rebind
    plugins/koto-skills/skills/koto-user/SKILL.md:201  koto session rebind <session> --to <dir>
    plugins/koto-skills/skills/koto-user/SKILL.md:206  koto session rebind demo
    plugins/koto-skills/skills/koto-user/SKILL.md:542  koto session rebind
    plugins/koto-skills/skills/koto-user/references/command-reference.md:698  koto session rebind <session> --to <dir>
    plugins/koto-skills/skills/koto-user/references/error-handling.md:116  koto session rebind my-workflow --to <dir>
    plugins/koto-skills/skills/koto-user/references/error-handling.md:120  koto session rebind my-workflow --to <dir>
    plugins/koto-skills/skills/koto-user/references/error-handling.md:126  koto session rebind
    src/cli/mod.rs:3472  koto session rebind {} --to <dir>
    src/cli/mod.rs:3488  koto session rebind {} --to <dir>
    src/cli/next_types.rs:178  koto session rebind {}
```

The last three lines are the point. They are the string literals the binary
prints — `execution_anchor_unresolvable`, `execution_anchor_mismatch`, and the
anchor-adoption notice — and they are the reason a user whose checkout moved was
refused and then sent nowhere. Two of the three carry the phrase across a Rust
`\` continuation, so a scanner that reads physical lines misses them; the
extractor joins continuations first, which is what makes those three reachable
at all.

`docs/guides/default-action-authoring.md:574` is the other case worth naming: it
is inside a fence with no language tag. A rule keyed on the tag would have
missed it, and whether a finding exists would have depended on whether an author
happened to write ```` ```bash ````.

### What the root argument does and does not carry

`KOTO_DOC_NAMES_ROOT` supplies the **corpus**, not the verb set. The run above
resolves v0.12.0's prose against *today's* command surface, because the check is
compiled against this crate and cannot walk a foreign tree's clap definition
without building it.

That matters more than it first looks, and the run above is a snapshot rather
than something you can re-execute to the same output.

The run was executed while `koto session rebind` did not yet exist, so the verb
set it resolved against had no `rebind` in it and all sixteen sites were
reported. #218 has since landed the verb. Re-running the identical command
today resolves those sites against the current command surface and reports
**zero** findings for that token. That is the mechanism working correctly, not
a regression: the root argument carries the corpus and the compiled crate
carries the verb set, so an old corpus is always read against today's CLI.

The durable proof is therefore not this run but
`catches_the_v0_12_0_string_literals` in `tests/doc_names.rs`. That test holds
the three v0.12.0 literals verbatim and supplies the v0.12.0 verb set
explicitly, so it does not depend on the current clap tree and keeps asserting
the detection whatever ships next. It is what a reviewer should look at to
satisfy themselves the check catches the motivating defect; the archive run
above is the one-time demonstration that produced it.

## The pre-exception finding list

The complete set of findings against this branch with `tests/doc_names.allow`
emptied, classified. Reproduce it by deleting the records and re-running; the
`(file, token)` set must match.

Eleven genuine defects against six correct as written, at the time of capture.
Genuine defects outnumber, which is the bar — and not by a wide margin, so a
reviewer who reclassifies one or two path entries should check whether the
majority still holds.

### Genuine — fixed in this change

| Token | Sites | What was wrong |
|---|---|---|
| `koto query` | 4 | Named in two `///` doc comments in `src/engine/types.rs` and in two shipped `koto-user` skill files, one of them inside a bash fence presenting it as runnable. There is no `query` verb and never was; the event log is read from the session directory. |
| `koto session info` | 1 | `RESUME_CONTEXT_PROMPT` in `src/engine/respawn.rs`, the text handed to every respawning agent, told it to read prior state via a verb that does not exist. The same sentence named `koto session list --parent`, a flag that lives on `session start`. Pinned by a byte-equality snapshot in `tests/respawn.rs`, which made it harder to fix, not easier to notice. |
| the batch-child-spawning design, rooted at `docs/designs/` | 3 | Cited from `src/engine/batch_validation.rs`, `src/cli/batch_view.rs`, and `src/cli/task_spawn_error.rs`, each omitting the `current/` segment. |
| the native-workflows-render design, rooted at `docs/designs/` | 1 | Cited from `src/workflows_surface/contract.rs`. Same missing segment. |
| a request-store design that exists nowhere | 4 | Cited twice each from `docs/STABILITY.md` and `docs/workspace-layout.md`. No document by that name has ever existed; the live one is `docs/designs/current/DESIGN-request-store-converge.md`. |
| an example skill directory and five siblings under `plugins/koto-skills/` | 12 | `docs/guides/custom-skill-authoring.md` opened by calling a skill "the reference implementation" and built its eval section on a harness — `eval.sh`, `evals/<case>/prompt.txt`, `skill_path.txt`, `patterns.txt` — that no longer exists. The real format is one `evals.json` per skill, run by `scripts/run-evals.sh`. |
| `src/gate` | 1 | `CLAUDE.md`'s skill-maintenance table cited `src/gate/` as a directory. It is `src/gate.rs`, a file. |

Two further defects in `CLAUDE.md` are **not** in the list above, because the
check cannot see them: `cmd/koto/` and a bare `gate/`, both in the repository
structure diagram. Their leading segments do not name real top-level entries, so
the path anchor never treats them as candidates. They were fixed by hand in this
change, and `a_renamed_top_level_directory_is_the_accepted_false_negative`
asserts the blind spot in code so a future reader finds it there rather than
only in prose.

### Correct as written — recorded, not fixed

| Token | Category | Why it stays |
|---|---|---|
| `koto migrate` | intentional | `docs/STABILITY.md` commits koto to publishing a migration tool "under a similar discoverable subcommand". A forward commitment, not a claim. |
| `.github/workflows/check-templates.yml` | intentional | The guide tells the reader to add it to *their* repo; the sentence says "in your repo". |
| `docs/diagrams` | intentional | An output directory an example workflow creates. |
| `docs/my-workflow.html` | intentional | The `--output` argument of an example command. |

Four records against a budget of fifteen, four intentional against a budget of
five.

Note on how this file names dead paths: it describes them rather than writing
them as citations. `shirabe validate`'s FC20 check resolves repo-relative
references in any changed file, so an earlier draft of this table -- which wrote
the dead paths out in code font -- failed CI on exactly the defect class it was
documenting. Describing them is not squeamishness; it is the same rule the
check itself enforces.

### The promised record, and how it retired

The list above was captured while `koto session rebind` did not exist. It was
carried as a `promised` record against koto#215, sixteen sites under one entry,
with a reason naming the four passages that state the verb does not exist.

koto#215 landed as #218 while this branch was open, and v0.12.1 shipped the
verb. On the rebase, the check said so:

```
FAIL: stale record `session rebind` -- the command now resolves
  The record said: ... When it lands, this record goes stale AND four passages
  that state the verb does not exist become false: docs/reference/error-codes.md,
  plugins/koto-skills/skills/koto-user/SKILL.md, and that skill's
  error-handling.md and command-reference.md references. Correct all four in the
  same change that removes this record.
```

That is the whole mechanism exercised end to end, by accident, within a day of
being written. The finding is not "a name is missing" but "a record is obsolete",
and it carries forward the one thing the check cannot compute — which prose the
landing of that verb also falsified. #218 had already corrected all four
passages, so removing the record was the only action left.

Worth keeping in view: the reason text is *displayed*, not consumed. Nothing
verified that those four passages were fixed; the check only made it hard to
remove the record without reading what it said. That raises the odds the prose
is noticed. It does not ensure it.

## Runtime

The clap walk is 52 verb paths and runs in under a millisecond. The corpus scan
reads `src/`, the packaged skills, and seven documentation surfaces. The whole
test binary, including all 24 cases, reports `finished in 0.20s` on a warm
build — the cost is the incremental link, not the scan.
