<!-- decision:start id="koto-author-gate-doc-format" status="assumed" -->
### Decision: Gate documentation format in koto-author template-format.md

**Context**

The Layer 3 section of `koto-author`'s `references/template-format.md` already introduces gate types via a summary table (type, passes-when, key fields) and shows gate declaration syntax in YAML. What's missing is: (1) the `gates.<name>.<field>` path syntax used in transition `when`-blocks, (2) the output fields available per gate type (exit_code, exists, matches, error), and (3) `override_default` as an optional per-gate field with its three-tier resolution order.

Template authors writing `when`-blocks need to know exactly which field names to reference for each gate type. The existing Layer 3 content stops short of this — it documents what each gate *does* but not what fields the gate *emits*. An author following the current guide will have no basis for writing `when: gates.my_check.exit_code == 0` and will fall back to the legacy boolean pattern, which fails strict compilation.

The constraint is that all additions must fit within the existing Layer 3 section — no new reference files — and must cover all three gate types: command, context-exists, and context-matches.

**Assumptions**

- context-matches gate output fields (matches, error) are treated as provisional, consistent with the decision context labeling them that way. If their schema stabilizes before publication, the documentation should be updated to remove the provisional marker.
- The existing gate type overview table in Layer 3 remains in place; new content extends rather than replaces it.
- "Fits within Layer 3" means adding subsections to the existing Gates section, not relocating content to a new file.

**Chosen: Annotated YAML examples showing gate config and output side-by-side**

Each gate type gets a subsection containing: its output field table (name, type, description) and a complete annotated YAML block showing the gate declaration alongside a transition `when`-block that uses the emitted fields. The `override_default` field appears in the annotated YAML as an optional field with a comment explaining the three-tier resolution.

The structure per gate type:

```
### command gate output

| Field     | Type    | Description              |
|-----------|---------|--------------------------|
| exit_code | integer | Exit code of the command |
| error     | string  | Error message if failed  |

Example:

  gates:
    lint_check:
      type: command
      command: "cargo clippy --quiet"
      override_default:        # optional — pre-fills output when no evidence submitted
        exit_code: 0           # resolution order: --with-data > override_default > built-in default
  transitions:
    - target: done
      when: gates.lint_check.exit_code == 0
    - target: fix_lint
      when: gates.lint_check.exit_code != 0
```

The same pattern repeats for context-exists (exists, error) and context-matches (matches, error).

**Rationale**

Template authors need patterns to copy, not schemas to interpret. The primary task when writing a `when`-clause is: "what field name do I reference, and what value should I compare against?" An example answers both questions directly. A table alone requires the author to mentally compose the field name into the path syntax — an additional step that creates room for error. The annotated example eliminates that step.

Tables remain essential for completeness: they confirm all fields, not just those shown in the example, and they're faster to scan when an author already knows the pattern and just needs to confirm a field name. Embedding a compact table per gate type before the example gives both — the table as reference, the example as pattern.

Prose descriptions (Option C) add reading friction without adding information density. Template authors working quickly skip prose and look for code blocks. Prose also distributes information across vertical space without the visual anchor that a table or code block provides.

The hybrid of tables + examples satisfies all constraints: it fits within Layer 3 by extending the existing Gates subsection, it shows the `gates.<name>.<field>` syntax in a complete and correct when-block, it documents `override_default` inline with a comment explaining three-tier resolution, and it integrates naturally after the existing gate type overview table.

**Alternatives Considered**

- **Per-gate output tables only (Option A)**: Provides a clean field reference but forces authors to derive the `when`-block syntax from the table. The connection between "field name in table" and "path in when-block" is non-obvious for first-time readers, especially since `when`-blocks in Layer 2 use plain evidence field names, not `gates.<name>.<field>` paths. Rejected because it leaves the most important question unanswered without an example.

- **Prose descriptions with inline schema blocks (Option C)**: Hard to scan under time pressure. Template authors skip prose and look for code blocks. Prose also spreads information across more vertical space without the visual anchor that tables and annotated examples provide. Rejected because it optimizes for reading comprehension over task completion — template authors need to write, not just understand.

**Consequences**

The annotated-YAML approach does carry a drift risk: if field names change in the koto engine, the examples become incorrect before the tables do, because tables are often updated first when field schemas change. This risk is mitigated by the CLAUDE.md trigger list (skills must be audited after any gate behavior change) and the planned eval harness. The field tables also serve as a cross-check — the example and table must agree, which creates a small consistency pressure.

Adding per-gate subsections to Layer 3 increases the section length by roughly 40-60 lines. This is acceptable; the section is currently under-documented relative to the complexity of what it covers.
<!-- decision:end -->
