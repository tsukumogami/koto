<!-- decision:start id="gate-legacy-mode-declaration" status="assumed" -->
### Decision: Gate legacy mode declaration and compiler enforcement

**Context**

Features 1–3 of the gate-transition contract introduced structured gate output,
the override mechanism, and compiler validation (D2: override_default, D3:
when-clause field references, D4: reachability + unreferenced-field warnings).
D4 emits a non-fatal `eprintln!` warning for gate fields never referenced in any
when clause.

The only known template using legacy gate behavior is the shirabe work-on
template: ~10 gate-bearing states, none with `gates.*` in any when clause. Gates
act as pure pass/fail blockers; routing happens entirely on agent evidence via
accepts blocks. This template must keep compiling until it migrates.

The problem is that the compiler currently has no way to distinguish "intentionally
legacy" from "accidentally omitted gates.* routing." Without a declaration, the D4
unreferenced-field warning fires for every gate in every state of the work-on
template — noise that makes the warning meaningless for structured-mode templates.
And new templates with the same shape (gates but no when-clause references) would
compile silently, with no signal that they're on an unsupported path.

From reading the code: both `koto template compile` and `koto init` call the same
`compile_cached()` → `compile()` → `template.validate()` chain. There is no
divergence point at which the two commands could behave differently without code
changes. The D4 unreferenced-field warning (lines 591–607 of `types.rs`) fires
inside `validate_gate_reachability()` unconditionally for all gates — even for
states with no pure-gate transitions (the reachability check exits early for those
states, but the warning loop runs first).

**Assumptions**

- The shirabe work-on template is the only template that needs legacy mode at this
  time. If additional templates exist outside this repo, they would also need to
  add the frontmatter field.
- `koto init` will remain a thin wrapper around `compile_cached()` — no separate
  compile path is planned.
- The legacy frontmatter field name `legacy_gates: true` is assumed; the exact
  name can be refined during implementation.
- "Easily removable and hard to access" (user constraint) means the field should
  not appear in any autocomplete, guide, or generated template — just exist in
  code for the one template that needs it.

**Chosen: Per-template frontmatter field `legacy_gates: true`**

A top-level frontmatter boolean field `legacy_gates: true` opts a template into
legacy mode. When present and true:

1. The compiler (D3 check) suppresses the error it would otherwise emit for
   gates used without any `gates.*` when-clause references.
2. The D4 unreferenced-field warning is suppressed entirely for the template.
3. `koto init` logs a warning to stderr ("template uses legacy gate mode; consider
   migrating to structured routing") but does not fail.
4. `koto template compile` succeeds without error.

For templates without `legacy_gates: true`: gates present without any `gates.*`
when-clause references are a compile error (not a warning). This is the new strict
default.

The compiled `CompiledTemplate` struct gains a `legacy_gates` boolean field
(default false, `skip_serializing_if = "is_false"`). The `SourceFrontmatter`
struct in `compile.rs` gains a corresponding `#[serde(default)] legacy_gates:
bool` field. The compiler passes this flag into `validate()` (or into the
`CompiledTemplate` before calling `validate()`), and all D3/D4 checks that fire
for missing `gates.*` references are gated on `!self.legacy_gates`.

Migration path: remove `legacy_gates: true` from frontmatter, add `gates.*` when
clauses and accepts blocks. The migration PR diff is exactly one frontmatter line
removed plus transition updates.

**Rationale**

The frontmatter field wins on all three user constraints:

- *Self-documenting*: the template file itself declares its own compat status. A
  reviewer reading the template sees the field and immediately knows this is a
  migration target.
- *Co-located*: no out-of-band CLI flag or config needed. The declaration travels
  with the template across repos, forks, and CI environments.
- *Migration PR is obvious*: removing `legacy_gates: true` is the commit that
  marks migration complete. A CLI flag would require the CI script to be updated
  separately from the template.

The frontmatter field is also "hard to access" in practice: it's not documented,
not generated, and not taught in the authoring guide. It exists for one template.

For `koto init`: the init command must accept any template without failing (the
constraint says "koto init must work + only log a warning; no flag needed for
init"). Since init calls `compile_cached()` and compile succeeds for legacy
templates, init already passes. The only required change is adding an `eprintln!`
warning after successful init when `compiled.legacy_gates` is true.

D4's unreferenced-field warning suppression is straightforward: the warning loop
in `validate_gate_reachability()` is currently unconditional. A single
`if !self.legacy_gates` guard around the warning block silences it for declared
legacy templates. For structured-mode templates, the warning is meaningful and
should remain.

**Alternatives Considered**

- **CLI flag `--allow-legacy-gates` on `koto template compile`**: Rejected. A flag
  is not self-documenting — the template file gives no indication of its compat
  status without consulting the compile invocation. CI scripts that call
  `koto template compile` would carry the flag indefinitely; migration would require
  updating both the script and the template. Violates the "co-located" requirement.
  The user explicitly preferred the frontmatter approach for these reasons.

- **Per-state annotation**: Rejected. The work-on template has ~10 legacy states.
  Per-state annotations multiply the migration surface and make "remove one line"
  impossible. A per-template declaration is simpler, and the whole template migrates
  as a unit anyway.

- **Separate compile path (`koto template compile-legacy`)**: Rejected. Adds a new
  subcommand with no benefit over a flag. Harder to remove than a frontmatter field.
  The "easily removable" constraint argues against any mechanism in the CLI.

**Consequences**

What changes:
- `SourceFrontmatter` in `compile.rs` gains `legacy_gates: bool` (default false).
- `CompiledTemplate` in `types.rs` gains `legacy_gates: bool` (default false,
  skip_serializing if false).
- `validate()` gains a guard: gates present without `gates.*` when-clause
  references are a compile error when `!self.legacy_gates`.
- D4 warning loop is suppressed when `self.legacy_gates`.
- `handle_init()` in `cli/mod.rs` emits a stderr warning when
  `compiled.legacy_gates` is true.
- The shirabe work-on template gains `legacy_gates: true` in its frontmatter.

What becomes easier:
- New templates can't accidentally use legacy gate behavior — they'll get an error.
- Migration is a one-line diff in the template frontmatter.
- D4's unreferenced-field warning is meaningful again for structured templates.
- The legacy code path is completely self-contained: removing `legacy_gates` support
  is a grep-and-delete of the flag and its guards.

What becomes harder:
- Nothing, for structured-mode templates. Legacy authors must add the field.

<!-- decision:end -->
