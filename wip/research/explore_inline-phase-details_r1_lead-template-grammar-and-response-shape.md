# Lead: koto's real template grammar and `koto next` response shape, and where a "details" payload could attach

## Findings

### 1. The template grammar is already what koto#90 is asking for — it shipped in PR #109

The headline finding: koto#90 is not proposing new territory. A `details` field with
first-visit-only inclusion and a `--full` escape hatch **already exists**, introduced in
commit `517ee83` "feat(cli): redesign koto next output contract (#109)". The issue's
proposed YAML shape (`analysis: {directive: ..., details: ...}`) doesn't exist and was
never how it was built — but the underlying capability the issue asks for is already
implemented, in a different shape, and (per the second finding below) inconsistently.

### 2. Markdown template grammar

Source templates are Markdown files with YAML front-matter (`src/template/compile.rs`).

- **Front-matter** (`SourceFrontmatter`, `compile.rs:16-29`): `name`, `version`,
  `description`, `initial_state`, `variables: {KEY: {description, required, default}}`,
  `states: {name: SourceState}`.
- **`SourceState`** (`compile.rs:47-70`, `#[serde(deny_unknown_fields)]` — typos in
  source YAML are compile errors): `transitions` (list of `{target, when}` — `when` is a
  `field: value` map used for evidence-routing), `terminal: bool`, `gates:
  {name: SourceGate}`, `accepts: {field_name: {type, required, values, description}}`,
  `integration: Option<String>`, `default_action: {command, working_dir,
  requires_confirmation, polling}`, `materialize_children: {from_field,
  default_template, failure_policy}` (batch child-spawning hook), `failure: bool`,
  `skipped_marker: bool`, `skip_if: {field: value}` (auto-advance predicate).
- **Body**: states are delimited by `## <state-name>` H2 headings
  (`extract_directives`, `compile.rs:637-671`). The front-matter `states` map is the
  authority for what counts as a state boundary — an H2 heading that doesn't match a
  declared state name is just directive prose, not a new state. Content between two
  state headings belongs to the first.
- **Directive vs. details split**: within one state's body content, a line consisting
  of exactly `<!-- details -->` (HTML comment, `DETAILS_MARKER`, `compile.rs:623`)
  splits the content: everything before becomes `directive`, everything after becomes
  `details`. Only the *first* occurrence of the marker counts as the split point — a
  second `<!-- details -->` later in the body stays inside `details` as literal text
  (`split_directive_details`, `compile.rs:673-688`, confirmed by test
  `multiple_details_markers_only_first_splits`). No marker present → whole body is
  `directive`, `details` is empty.
- Every declared state **must** have a non-empty `directive` (a heading with only a
  details marker and nothing before it fails compilation — `directive.is_empty()`
  check, `compile.rs:188-193`).
- `{{VAR_NAME}}` references (uppercase, digits, underscore — `VAR_REF_PATTERN`,
  `types.rs:9`) are valid inside `directive`, gate `command`/`working_dir`, and
  `default_action` fields, and are validated at compile time against the declared
  `variables` block plus two runtime-injected names (`SESSION_DIR`, `SESSION_NAME`).
  I did not find variable-reference validation applied to the `details` body text
  itself — worth confirming before assuming `{{VAR}}` in details gets the same
  compile-time guarantee as in directive.

Real fixtures under `test/functional/fixtures/templates` and
`plugins/koto-skills/skills/koto-author/koto-templates` exist but I focused on the
in-source `compile.rs` unit tests (which are extensive and exercise every grammar
rule with copy-pasteable examples) rather than walking every fixture file; the grammar
above is drawn from the parser code plus 15+ passing/failing compile tests, not
speculation.

### 3. Compiled representation (`koto template compile` JSON, FormatVersion=1)

`CompiledTemplate` (`src/template/types.rs:22-32`): `format_version` (currently always
`1`), `name`, `version`, `description` (omitted if empty), `initial_state`, `variables`
(`BTreeMap`, omitted if empty), `states: BTreeMap<String, TemplateState>`.

`TemplateState` (`types.rs:53-92`) — every field a state body may carry:
`directive: String` (required, never omitted), `details: String` (omitted if empty —
`#[serde(default, skip_serializing_if = "String::is_empty")]`), `transitions: Vec<Transition>`,
`terminal: bool`, `gates: BTreeMap<String, Gate>`, `accepts: Option<BTreeMap<String,
FieldSchema>>`, `integration: Option<String>`, `default_action: Option<ActionDecl>`,
`materialize_children: Option<MaterializeChildrenSpec>`, `failure: bool`,
`skipped_marker: bool`, `skip_if: Option<BTreeMap<String, Value>>`.

Two deliberately different serde policies, and the reason matters for the hash/lock
question: `SourceState` (source YAML) uses `deny_unknown_fields` so template authors
get compile-time typo errors; `TemplateState` (compiled JSON / cache artifact)
explicitly does **not**, with a comment at `types.rs:47-52` explaining that the compile
cache may be read by an older binary than the one that wrote it, so new fields must
stay additive/non-breaking. A dedicated test,
`compiled_template_not_deny_unknown_fields` (`compile.rs:1881-1905`), locks this
invariant by constructing JSON with a fabricated future field and asserting it still
deserializes.

### 4. `koto next` response type — all six variants

`NextResponse` (`src/cli/next_types.rs:63-127`), custom-serialized so each variant
gets its own `action` string plus common fields. Confirmed action strings from the
`Serialize` impl (`next_types.rs:374-527`) and matching tests:

| Variant | `action` | Fields |
|---|---|---|
| `EvidenceRequired` | `evidence_required` | state, directive, details?, advanced, expects, blocking_conditions, unassigned_children |
| `GateBlocked` | `gate_blocked` | state, directive, details?, advanced, blocking_conditions, unassigned_children |
| `Integration` | `integration` | state, directive, details?, advanced, expects?, integration, unassigned_children |
| `IntegrationUnavailable` | `integration_unavailable` | state, directive, details?, advanced, expects?, integration, unassigned_children |
| `Terminal` | `done` | state, advanced, unassigned_children (no directive/details — terminal states have nothing left to instruct) |
| `ActionRequiresConfirmation` | `confirm` | state, directive, details?, advanced, action_output, expects?, unassigned_children |
| `Error` | `error` | state, advanced, error (typed `NextError`), batch?, blocking_conditions, unassigned_children |

Every non-terminal, non-error variant already carries `details: Option<String>` as a
first-class field, serialized only `Some` (test `serialize_evidence_required_no_options`
confirms `json["details"] == "Extra context here."` when present, and
`serialize_next_error_no_details`-style tests elsewhere confirm omission when `None`).
`unassigned_children` (discovery-scan populated) rides on every variant including
errors.

Dispatch order in the pure classifier `dispatch_next` (`src/cli/next.rs:32-124`):
terminal → gate-blocked (unless an `accepts` block exists, in which case it falls
through to evidence-required so agents can submit override/recovery evidence) →
integration-unavailable → evidence-required (has `accepts`) → evidence-required
fallback with empty `expects` (auto-advance candidate). `dispatch_next` itself always
includes `details` when `template_state.details` is non-empty (`next.rs:50-54`) — it
has no visit-count awareness; that gating is applied by its caller (next finding).

### 5. First-visit-only gating already exists — but is applied in exactly one of two call sites

The real "first visit only" logic lives in the `koto next` command handler
(`src/cli/mod.rs:3998-4012`), not in `dispatch_next` itself:

```rust
let details = if final_template_state.details.is_empty() {
    None
} else {
    let post_events = backend.read_events(&name).map(|(_, evts)| evts).unwrap_or_default();
    let visit_counts = derive_visit_counts(&post_events);
    let count = visit_counts.get(final_state.as_str()).copied().unwrap_or(0);
    if full || count <= 1 {
        Some(final_template_state.details.clone())
    } else {
        None
    }
};
```

`derive_visit_counts` (`src/engine/persistence.rs:981`) walks the event log and counts
state-entry events (transitioned/directed/rewind targets) per state name. `count <= 1`
means: include details on the state's very first entry (count 1, or 0 defensively);
omit on every re-visit (count ≥ 2). `--full` (a `bool` CLI flag on `Next`,
`mod.rs:143-146`, doc comment: *"Always include the details field in the response,
regardless of visit count"*) is the escape hatch — but it is **`koto next <name>
--full`**, not a separate `koto phase-info` subcommand as sketched in koto#90. There is
no `phase-info` command anywhere in the codebase (`grep` for `phase-info`/`phase_info`
returned nothing).

**Gap**: this gating only runs on the normal advance path in `handle_next`. The
directed-transition path (`koto next <name> --to <state>`, `mod.rs:3260-3390`) calls
`dispatch_next` directly at `mod.rs:3355` with no visit-count check at all — so a
directed transition into a state always re-includes `details` if non-empty, regardless
of how many times that state has been visited. This is a real inconsistency in the
current implementation, not a hypothetical one: two code paths construct the same
response shape and only one of them applies the "first visit only" rule.

### 6. Template hash / integrity: what happens when a template gains a `details` field

The `template_hash` used to lock a session (`src/session/mod.rs:138` header field) is
**not** a hash of the source `.md` file — it's `SHA256` of the serialized compiled JSON
(`compile_cached_into`, `src/cache.rs:41-87`, doc comment at `cache.rs:31`: *"The cache
key is SHA256 of the compiled JSON"*). The artifact is written content-addressed as
`<sha256>.json`, either to the global `~/.cache/koto/` dir or, for `--from-stdin`
sessions, into the session directory itself (`cache.rs:34-40`). At `koto next` time,
`mod.rs:3202-3230` re-reads the exact bytes at `machine_state.template_path` and
re-hashes them, erroring (`TemplateError`) only if those specific bytes were tampered
with or evicted — it does **not** re-compile the source template and compare.

Consequence for "what happens to an existing session if a template gains a new field":
nothing happens to existing sessions. Each session pins its own content-addressed
compiled artifact at init time (`init_child.rs`); a template author editing the source
`.md` (e.g., adding a `<!-- details -->` marker to a state) produces a *new* hash and a
*new* cache file on next compile, but old sessions keep referencing their original
`<sha256>.json` unaffected. This is enabled by two independent forward-compat design
choices already in place: (a) `TemplateState`/`CompiledTemplate` are not
`deny_unknown_fields`, so a compiled-JSON reader from an older binary tolerates unknown
future fields; (b) the hash only pins bytes-identity of one artifact per session, it isn't
a "does the template match latest" check. Adding a `details` field (which already
happened) is safe by construction — old sessions simply never had it, new sessions
compiled after the source changed do.

### 7. Where a "details"-shaped payload could attach — enumerated candidates

Since `details` already exists as a top-level `TemplateState` string field split via an
HTML-comment marker in the same H2 body, this sub-question is less hypothetical and
more "what are the actual design points a future change to this mechanism would touch":

- **Current shape (implemented)**: `<!-- details -->` marker inside the `## state`
  body, single string field on `TemplateState`, no internal structure. Parser change
  needed for *this* shape: none — it's done.
- **A nested `###` subheading** (e.g. `### Details` under `## state`) instead of an
  HTML comment: would require `extract_directives`/`parse_h2_heading` to also parse H3
  markers and change `split_directive_details` from a marker-line scan to a
  heading-level scan. More visible in rendered Markdown than an HTML comment, but a
  bigger parser change than what shipped.
- **A bolded marker line** (`**Details**` on its own line, mirroring how the design
  notes elsewhere describe `**Transitions**`-style prose markers) — note: I did not
  find `**Transitions**` or any bolded-marker convention actually implemented anywhere
  in `compile.rs`; transitions are pure front-matter YAML (`transitions: [...]` under
  the state's YAML block), never markdown prose. Bolded markers are not part of the
  real grammar today. Introducing one would mean a new regex/line-scan alongside the
  existing HTML-comment scan — plausible but not free (ambiguity with agent-authored
  markdown inside the directive body).
- **A new front-matter map** (e.g. `details:` alongside `directive` implicitly derived
  from body) is not how the grammar works today — `directive`/`details` both come from
  *body* content per state, not front-matter, because front-matter is
  `deny_unknown_fields` YAML meant for structure/config, while prose lives in the
  markdown body. Moving details into front-matter would cross that boundary and fight
  the existing design split.
- **A fenced code block** or **separate file reference** (`details_file: path.md`):
  neither exists; either would need new `SourceState` fields (front-matter) plus
  either inline fence parsing or a file-read step in `compile()`. Not indicated by
  anything in the current code as an anticipated direction.

### 8. Other template fields already carrying prose beyond the directive

Besides `details`, the closest things to "extra prose" are: `description` (template
frontmatter, `CompiledTemplate.description` — one string describing the whole
template, not per-state), and `FieldSchema.description` /
`VariableDecl.description` (per-`accepts`-field and per-variable one-line descriptions,
both plain strings, both optional). None of these are state-body prose the way
`directive`/`details` are — they're all front-matter-declared, short, structural
descriptions rather than long-form instructional content.

## Implications

- The exploration should not treat koto#90 as "does this capability exist" — it does,
  shipped in PR #109, and its shape (HTML-comment marker splitting one Markdown body
  into `directive`/`details`, visit-count gating, `--full` flag) is a *specific design
  already made*, not open design space. The real question in front of this exploration
  is narrower than the issue title suggests: is the *existing* mechanism correctly and
  consistently applied, and does it need extension (e.g., a dedicated `phase-info`
  command instead of `--full`, or closing the directed-transition gap)?
- The directed-transition (`--to`) gap (finding 5) is a concrete, small, well-scoped
  bug/inconsistency this exploration could recommend fixing regardless of what else
  gets decided — `dispatch_next` at `next.rs:32` has no visit-count parameter, so the
  caller at `mod.rs:3355` can't apply the same `full || count <= 1` gate the normal
  path does at `mod.rs:4010` without either passing visit count into `dispatch_next` or
  duplicating the gating logic at the call site.
- If the issue's actual ask is a *separate escape-hatch command* (`koto phase-info`)
  rather than the existing `--full` flag on `koto next`, that's still new work — but
  it should be scoped as "add a read-only variant of the existing details-gating
  mechanism," not "invent a details payload," since the payload, its storage, its
  hashing/integrity story, and its first-visit gating already exist and are exercised
  by tests.
- The template-hash/session-lock story means schema evolution here is already solved
  safely — no migration concern for extending or restructuring `details` further, as
  long as new fields stay additive on `TemplateState` (matching the existing
  non-`deny_unknown_fields` policy) and don't change the *meaning* of bytes an existing
  session already pinned.

## Surprises

- The single biggest surprise: this scope assumption in the brief — *"none of which
  match the sketch in the issue body"* — is only half right. The issue's proposed YAML
  shape doesn't match, but the underlying behavior (details field, first-visit
  inclusion, later-visit omission, escape hatch) is not a gap to be designed — it's
  already implemented and tested, under a different name/shape (`<!-- details -->`
  marker + `--full` flag rather than `analysis: {directive, details}` YAML + `koto
  phase-info` command).
- The escape hatch already exists but isn't a separate command — it's a flag on the
  same `koto next` invocation. If the issue's motivating scenario is genuinely "context
  compression drops the details and the agent needs to re-fetch them without knowing
  the current visit count or wanting to also risk re-triggering advance semantics,"
  `--full` on `koto next` might not be a clean fit (it's bundled with the full next
  semantics, including gate re-evaluation and `advanced` flag side effects) — a
  dedicated read-only `phase-info` lookup could still be a real, separate ask even
  though the payload itself isn't new.
- The directed-transition path bypassing visit-count gating entirely (finding 5) was
  not something I expected going in — it means "first visit only" is not actually a
  reliable guarantee today for every way a state can be entered.

## Open Questions

- Is `{{VAR}}` substitution validated/applied inside `details` the same way it is for
  `directive`? `NextResponse::with_substituted_directive` does map over `details`
  (confirmed, `next_types.rs:159-251`), so runtime substitution happens — but I did not
  confirm whether `CompiledTemplate::validate()`'s compile-time "variable reference
  must be declared" check (`types.rs:781-791`) also scans `details` text, or only
  `directive`. Worth a quick follow-up grep of `extract_refs` call sites before relying
  on this.
- Does the issue's motivating overhead problem (large phase instructions repeated on
  every `koto next` tick) actually still occur anywhere given `details` already exists?
  i.e., is there a category of *directive* text (not `details`) that's long and
  repeats every tick, which the existing mechanism doesn't address because template
  authors haven't adopted the `<!-- details -->` marker in the relevant templates?
  That would reframe #90 as an authoring/adoption gap rather than an engine gap.
- Should the directed-transition inconsistency (finding 5) be filed as its own fix
  regardless of what this exploration concludes about #90's broader scope?
- Real fixture templates under `test/functional/fixtures/templates` and the
  `koto-author` skill's `koto-templates` weren't walked file-by-file — worth a
  follow-up pass to see whether any production templates actually use
  `<!-- details -->` today, which would show whether the existing mechanism is used in
  practice or sitting unused since #109 landed.

## Summary

koto already has almost exactly what issue #90 asks for: a per-state `details` field
(split from `directive` in the Markdown body via an `<!-- details -->` HTML-comment
marker), included on a state's first visit and omitted on repeat visits via
`derive_visit_counts`, with a `--full` flag on `koto next` as the escape hatch — all
shipped in PR #109, not the YAML shape the issue proposes. The main real gaps are that
the escape hatch is a flag on `koto next` rather than a separate read-only
`phase-info`-style lookup, and that the directed-transition path (`koto next --to`)
calls the classifier directly and skips the visit-count gate entirely, so repeat
visits via `--to` always re-show details. The biggest open question is whether #90's
actual pain point is a leftover authoring/adoption gap (templates not using the
marker) rather than a missing engine capability.
