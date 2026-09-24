---
schema: plan/v1
status: Active
execution_mode: coordinated
upstream: docs/designs/DESIGN-jev-decision-offload.md
milestone: "Decisions koto can settle without the agent"
issue_count: 14
split_rationale: |
  Hard Constraint. The work spans tsukumogami/koto and tsukumogami/shirabe,
  and shirabe's decider declarations must be validated by a published koto
  release that contains the feature, so the shirabe declarations PR is ordered
  after a koto-release gate. Within shirabe, the deterministic routing fixes
  form their own PR under Incremental Value: they remove silent misroutes and
  agent turns on any koto from v0.12.2, independent of the decider.
---

# PLAN: Decisions koto can settle without the agent

## Status

Active

Work items are local to this PLAN (no GitHub issues are filed). Issues are
numbered `#1`-`#14` here and in the Implementation Issues table.

## Scope Summary

Let koto consult an opt-in typed decider (Jev first) for template-declared
decisions, shadow-first with per-answer promotion backed by an append-only
ledger and `koto decider report`, and reshape shirabe's `/work-on` and
`/execute` so three decisions become eligible and their computable
"decisions" become gates.

## Decomposition Strategy

**Horizontal decomposition**, in the order the DESIGN's decisions force.

- **Declaration before everything (#1).** Decision 1 puts the whole
  declaration inside an `accepts` field, compiled to an `Option` that's skipped
  when unset. Every later koto issue reads that compiled shape, and the
  `template_hash` stability it guarantees is what lets the rest ship without
  touching templates that don't opt in.
- **Configuration and provider in parallel, then joined (#2, #3).** Decision 3
  keeps the provider synchronous and neutral and makes configuration the only
  place opt-in is decided. #2 owns that single predicate
  (`DeciderSettings::opted_in`, including the same-layer endpoint rule) so no
  later caller can define "opted in" differently. #3 builds the pure request and
  evaluation code plus the Jev client on the HTTPS client koto already links,
  and depends on both #1 (declarations) and #2 (settings).
- **Consultation as the single integration point (#4).** Decision 2 hooks the
  `NeedsEvidence` arm of the advance loop, keeps stickiness in the event log,
  and serializes with a dedicated `decider.lock`. Everything the acceptance
  criteria test about applying or not applying an answer lives here, so it
  depends on #1 and #3 and carries the event type the ledger mirrors.
- **Record, then report, then docs (#5, #6, #7).** Decision 4 writes the ledger
  at the moment each record happens (child and abandoned sessions never reach a
  terminal tick), so #5 follows #4's event. #6 joins the ledger and runs
  fixtures through #3's code, and #7 documents the finished surface (the
  `doc_names` check requires the verb to exist first).
- **Live validation before the release (#14).** Every other test runs
  against a stub written from our reading of the Jev docs, and those docs
  disagree with themselves (the quickstart's example response omits
  `probabilities`, which the API reference lists and Decision 3 depends on).
  #14 runs the client and an example workflow against the real API behind a
  feature flag and a repository secret, after #3, #4 and #6 exist, and the
  release gate requires it green on the tagged commit.
- **koto compatibility proof inside the koto PR (#8).** R27's old-binary
  guarantee is proven before any release, so the release gate that shirabe's
  declarations wait on already implies it.
- **shirabe deterministic moves stand alone (#9-#12).** Decision 5 found the
  drift facts must be computed before the rebase, question states must carry no
  gates (so R6 never blocks later promotion), and `batch_outcome` must route on
  `all_success` true/false. None of that needs the decider, so it ships as its
  own PR on koto v0.12.2, with #12's floor job proving it.
- **shirabe declarations last (#13).** Declarations are only safe once a koto
  that compiles and checks them is released, so #13 waits on the
  `koto-release` gate as well as #9, #11 and #12.

Delivery: three PRs (`pr-koto-default`: #1-#8 and #14; `pr-shirabe-deterministic`:
#9-#12; `pr-shirabe-declarations`: #13), plus the coordination PR (the koto
docs branch carrying the PRD, DESIGN and this PLAN), which merges last.

## Issue Outlines

### Issue 1: feat(template): declare decider blocks on accepts fields

**Goal**: Parse, lower, validate (`E-DECIDER-*`, including the floor), and hash field-level `decider` blocks on `accepts` enum and boolean fields, and add `description`/`value_descriptions` to `expects` for declared fields, without changing compiled output or `template_hash` for templates that don't use them.

**Repo**: `tsukumogami/koto` (PR group `default`); **Complexity**: critical

**Acceptance Criteria**:

Declaration and lowering:

- [ ] `SourceFieldSchema` stays non-strict and gains an optional `decider` key. `SourceDecider`, its answer, escape, and input structs are `deny_unknown_fields`, and a misspelled key inside the block (for example `thresold:`) fails `koto template compile` with output that names the misspelled key.
- [ ] A template with a valid enum declaration (per-value descriptions, an escape, one `context` input and one `var` input) compiles. A template with a valid boolean declaration (answers for `true` and `false`, no escape) compiles. Bare YAML keys `true:`/`false:` under `answers` are accepted.
- [ ] A `var` input may name a declared template variable or a `capture_stdout_as` name. Runtime names such as `SESSION_NAME` and `SESSION_DIR` are refused.
- [ ] A declaration whose `context` input is written by an earlier state's `default_action` and gated on there with `context-exists` compiles.
- [ ] Defaults are resolved at compile time: an answer with no `mode` compiles to `shadow`, one with no `threshold` compiles to `0.9`, and an input with no `max_bytes` compiles to `8192`. An explicit `max_bytes: 8192` and an omitted one produce the same compiled declaration and the same declaration hash.
- [ ] An unknown mode string fails with `E-DECIDER-MODE`. An input naming both or neither of `context`/`var` fails with `E-DECIDER-INPUT`.

Compile rules (R5). Each violation fails compilation, and the message carries its code and names the state, the field, and (where one applies) the value:

- [ ] `E-DECIDER-FIELD-TYPE`: a `decider` block on a `string`, `number`, or `tasks` field.
- [ ] `E-DECIDER-QUESTION`: a declared field with an empty `description`.
- [ ] `E-DECIDER-ANSWERS`: an `answers` key set that differs from `values` (enum) or from `{true, false}` (boolean). The message names the missing or extra key.
- [ ] `E-DECIDER-VALUE-DESCRIPTION`: an answer with an empty or missing `description`.
- [ ] `E-DECIDER-ESCAPE`: on an enum, a missing escape, an empty escape `value`, a missing escape `description`, or an escape value that's also in `values`. On a boolean, any `escape` key.
- [ ] `E-DECIDER-ESCAPE-ROUTED`: a transition whose `when` tests the field at the escape value. This code is reported instead of the generic unknown-value error from `validate_evidence_routing`.
- [ ] `E-DECIDER-THRESHOLD`: thresholds of 0.4 and 1.1 are each rejected with a message naming the value and the number. Thresholds of exactly 0.5 and 1.0 compile. NaN is rejected.
- [ ] `E-DECIDER-INPUT`: no inputs; an empty label; a duplicate label within the field; `max_bytes: 0`; a `var` that is neither a declared variable nor a capture; a `context` key that fails `unusable_context_key_reason` or has an undeclared `{{REF}}`; a `context` key that no `context-exists` or `context-matches` gate in the template names; and two declared fields on one state using the same label with a different source or budget.
- [ ] `E-DECIDER-SIBLING-REQUIRED`: a state with a declared field and another `required: true` field that has no `decider`. An optional sibling without a decider (such as a `rationale` string) is allowed.

The floor (R6):

- [ ] `E-DECIDER-FLOOR` rejects `auto` on a value whose transition targets a terminal state, on a value whose target state's `default_action` has `requires_confirmation: true`, and on a value whose `when` clause also tests any `gates.*` key. Each message names the target state and the rule broken, and the gate case also names the gate key.
- [ ] The same three templates compile with the offending value in `shadow` (and in `never` and `off`): only `auto` answers are floor-checked.
- [ ] `auto` on a boolean value whose transition targets a terminal state fails with `E-DECIDER-FLOOR`. A `when` clause that tests the boolean as the string `"true"` or `"false"` is also matched.
- [ ] The floor check is a pure public helper on `CompiledTemplate` that takes a state, field, and answer and returns every violating transition with its target and violation kind, with no I/O. `validate_deciders` calls it, and it's usable outside compilation.

Declaration hash (R23):

- [ ] `declaration_hash(&FieldDecider, question)` returns a hex SHA-256 computed on demand. Nothing hash-related is stored in the compiled JSON.
- [ ] Unit tests show the hash changes when the question, any value description, the escape value or description, an input's source, label, or `max_bytes`, or the value set changes, and doesn't change when a mode or threshold changes or when `values:` is reordered.
- [ ] The fingerprint is built by destructuring `FieldDecider`, `DeciderAnswer`, and `DeciderInput` with no `..` rest pattern, so adding a field to any of them fails to compile until the hash function is updated.

Agent-facing contract (R8):

- [ ] For a declared field, `derive_expects` emits `description` (the question) and `value_descriptions` (one key per value, `"true"`/`"false"` for booleans), so both `koto next` and `koto status` carry them. The escape value appears in neither key.
- [ ] For fields without a `decider` block, `ExpectsFieldSchema` serializes byte-identically to today: neither new key appears, even when the field has a `description`.
- [ ] Submitting the escape value as evidence is rejected with the same error as any value outside `values`, with no change to `validate_evidence`.

No change for templates that don't declare (R7):

- [ ] A unit test compiles a fixture template with no `decider` block and asserts its `template_hash` equals a constant captured on `main` before this change.
- [ ] A test compiles every template under `plugins/koto-skills/**/koto-templates/` and asserts the pretty JSON has no `"decider"` key and survives a deserialize/serialize round trip byte-identically.
- [ ] `tests/next_response_baseline.rs` passes with no fixture edits.

Docs and hygiene:

- [ ] `docs/reference/error-codes.md` documents every `E-DECIDER-*` code under the `template compile` section, following the `E-SKIP-*` entries' format.
- [ ] `plugins/koto-skills/skills/koto-author/references/template-format.md` documents the block's syntax and defaults, and `plugins/koto-skills/skills/koto-user/references/response-shapes.md` documents the two new `expects` keys. Skill guidance, opt-in, and eval runs stay with <<ISSUE:7>>.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` pass, including `tests/doc_names.rs`.
- [ ] Security review completed

Downstream deliverables:

- [ ] Must deliver: public `FieldDecider`, `DeciderAnswer`, `DeciderMode`, `DeciderEscape`, `DeciderInput`, and `DeciderInputSource` types in `src/template/decider.rs` with every default resolved, plus `declaration_hash(&FieldDecider, question)`, so the provider-neutral request builder and the Jev client can build requests from a compiled declaration (required by <<ISSUE:3>>).
- [ ] Must deliver: compiled declarations reachable through `TemplateState.accepts[field].decider`, and the pure floor helper callable at runtime on the transition the engine actually matches, so the consultation arm can recheck the floor where the compiler couldn't see it (required by <<ISSUE:4>>).
- [ ] Must deliver: a current koto that accepts a template with both an enum and a boolean `decider` declaration, with the escape absent from `values` and `expects`, so the v0.12.2 compatibility job can run the same fixtures on both binaries (required by <<ISSUE:8>>).

**Security Checklist**:

- [ ] The floor can't be relaxed by anything in the template: no key, flag, or mode other than moving the answer out of `auto` suppresses `E-DECIDER-FLOOR`, and the check runs whether or not `--allow-legacy-gates` is passed.
- [ ] Floor matching errs toward over-matching (string `"true"`/`"false"` on booleans counts), so a template can't dodge the floor by spelling a `when` value differently.
- [ ] The escape value never reaches `values`, `expects.values`, or `value_descriptions`, and it's refused as submitted evidence.
- [ ] Inputs can name only context-store keys and template variables or captures. The block offers no way to read an environment variable, a file path, or a runtime name such as `SESSION_DIR`.
- [ ] Context keys in inputs go through `unusable_context_key_reason`, so a traversal or otherwise unusable key fails at compile time.
- [ ] `max_bytes` is bounded (unsigned, greater than zero) and every threshold is finite and within [0.5, 1.0], so no declaration can make a later consultation unbounded or apply at confidence below one half.
- [ ] The declaration hash covers every input source, label, and budget, so widening what a declaration sends changes its identity and can't reuse evidence gathered under the old declaration.
- [ ] Nothing in this issue performs network I/O, reads configuration, or writes outside the existing compile cache.

**Dependencies**: None

**Type**: code

### Issue 2: feat(config): add opt-in decider configuration with hardened key handling

**Goal**: Add koto's opt-in `[decider]` configuration and the single opt-in predicate, `resolve_decider` -> `DeciderSettings::opted_in()`, that every later caller uses: the key, endpoint, and timeout come only from the user or the environment, a repository can only lower the mode, the key is only sent to an endpoint from its own layer, the key is never printed, and `cargo test` can't reach a real decider.

**Repo**: `tsukumogami/koto` (PR group `default`); **Complexity**: critical

**Acceptance Criteria**:

**Config table and parsing**

- [ ] `KotoConfig` gains a `decider` table with `mode`, `api_key`, `endpoint`, and `timeout_ms`, and a `project_mode` field that is never serialized.
- [ ] With no `[decider]` table in either file and no `KOTO_DECIDER*` env vars, `koto config list` and `koto config list --json` print exactly what they printed before this change (no `decider` key appears).
- [ ] A user or project config whose `[decider]` table has a wrong-typed value (for example `mode = 3` or `timeout_ms = "fast"`) or an unknown key still loads: `load_config()` returns `Ok`, the file's other tables (for example `session.backend`) still apply, and the bad value is treated as unset. Each problem produces a warning returned by `resolve_decider`, and `load_config()` itself writes nothing to stderr.
- [ ] `ALL_KEYS` includes `decider.mode`, `decider.api_key`, `decider.endpoint`, and `decider.timeout_ms`, and `get_value`, `set_value_in_toml`, and `unset_value_in_toml` handle all four.

**Layer-aware merge (project config can't supply credentials or raise the mode)**

- [ ] `merge_decider` exists in `src/config/resolve.rs`, and `merge_config` does not read or write `decider`.
- [ ] A project `.koto/config.toml` containing `decider.api_key`, `decider.endpoint`, and `decider.timeout_ms` has all three dropped at load: with a user config that sets none of them, the loaded config has no key, no endpoint, and no timeout. A unit test covers this, and an integration test shows `koto config get decider.api_key` exits 1 and `koto config list` doesn't show the project's values.
- [ ] With the user endpoint set to URL A and project config naming URL B, the resolved endpoint is A. With project config naming B and no user endpoint, the resolved endpoint is the default. B never appears in the resolved settings.
- [ ] A project `decider.mode` is stored only in `project_mode` and never overwrites the global mode.
- [ ] `PROJECT_ALLOWLIST` gains `decider.mode` only. `koto config set decider.api_key <v>` without `--user` fails with the existing credentials message. `koto config set decider.endpoint <v>` without `--user` fails with a message saying it can only be set in user config or `KOTO_DECIDER_ENDPOINT`. `koto config set decider.timeout_ms <v>` without `--user` fails with the generic "not allowed in project config" message.

**Env overrides and mode resolution**

- [ ] Non-empty `KOTO_DECIDER`, `KOTO_DECIDER_API_KEY`, and `KOTO_DECIDER_ENDPOINT` replace the global mode, key, and endpoint from user config. An empty value for any of them counts as unset.
- [ ] `resolve_decider` is a pure function over `DeciderConfig` (no env reads, no file reads, no printing), and its unit tests pass explicit inputs instead of mutating process env.
- [ ] The global mode defaults to `off` when neither `KOTO_DECIDER` nor user `decider.mode` is set, so a key alone never opts a user in.
- [ ] Mode parsing trims and lowercases (`" Shadow "` resolves to `shadow`). Unit tests show `KOTO_DECIDER=never` and `KOTO_DECIDER=bogus` each resolve to `off` with exactly one warning in the returned `Vec<String>` naming `KOTO_DECIDER` as the source, and nothing written to stderr by `resolve_decider`. The same two values in user `decider.mode` resolve to `off` with one warning naming user config. An unrecognized project mode also resolves to `off` with a warning, which can only lower the result.
- [ ] The effective mode is the minimum of global and project mode: user `shadow` + project `auto` gives `shadow`; user `auto` + project `shadow` gives `shadow`; user `auto` + project `off` gives `off`; user `auto` + project unset gives `auto`; user `off` + project `auto` gives `off`; `KOTO_DECIDER=auto` + project `shadow` gives `shadow`. Each case has a unit test.

**The opt-in predicate**

- [ ] `DeciderSettings::opted_in()` is the only function in `src/` that decides whether the user is opted in. It returns true only when the effective global mode (after the project minimum) is `shadow` or `auto`, a key is present, and the endpoint passed the scheme, userinfo, parse, and same-layer rules below.
- [ ] Unit tests hold every other input fixed and flip one condition at a time, showing `opted_in()` is false for each: effective mode `off` because the global mode is `off`; effective mode `off` because the project mode is `off` under a user `auto`; no key; unparseable endpoint; plain `http` to a non-loopback host; userinfo in the endpoint; env key with user-config endpoint; user-config key with env endpoint. A baseline case with all conditions met returns true.
- [ ] Mode `auto` with no key is not opted in and adds no warning (a warning on every tick would be noise).
- [ ] `opted_in()` reads only fields already on `DeciderSettings`. It does no env or file reads, so a caller can't get a different answer by calling it at a different time.
- [ ] `timeout_ms` defaults to 2000. A value above 10000 is capped at 10000 with a warning. Zero or an unparseable value falls back to 2000 with a warning. `koto config set --user decider.timeout_ms` accepts only integers in 1..=10000.

**Endpoint rules**

- [ ] Same layer: the key is used only with an endpoint from its own layer or the default. An env key works with an env or default endpoint. A user-config key works with a user-config or default endpoint. An env key with a user endpoint, or a user key with an env endpoint, leaves the user not opted in and adds a warning. A unit test covers all four combinations plus both default cases.
- [ ] The resolved settings carry the endpoint's origin as `default`, `user`, or `env`.
- [ ] Scheme: an `https` endpoint is accepted. Plain `http` is accepted only when the host is an IPv4 literal in 127.0.0.0/8, the IPv6 literal `::1`, or exactly `localhost`. `http://example.com/...`, `http://localhost.example.com/...`, `http://10.0.0.1/...`, and any other scheme leave the user not opted in with a warning. No name is resolved to decide loopback (no DNS call anywhere in `resolve_decider`).
- [ ] Userinfo: an endpoint containing userinfo (`https://user:pass@host/...` or `https://token@host/...`) is rejected, leaving the user not opted in. A test asserts the warning contains neither `user`, `pass`, nor `token` from the URL, and that endpoint warnings in general print only scheme, host, and path.
- [ ] An endpoint that doesn't parse as a URL leaves the user not opted in with a warning.
- [ ] `koto config set --user decider.endpoint <v>` applies the same scheme, loopback, and userinfo rules and refuses a bad value with an error.
- [ ] With no endpoint configured anywhere, the resolved endpoint is the Jev public endpoint, stored as a constant holding the full URL of the decision call (not a base URL).

**Key handling**

- [ ] `ApiKey` is a newtype in `src/decider/types.rs` with no `Display` impl and a `Debug` impl that prints `<redacted>`. A test asserts `format!("{:?}", key)` and `format!("{:?}", settings)` never contain the key string. The raw value is reachable only through one explicitly named accessor for the transport in <<ISSUE:3>>.
- [ ] `koto config get decider.api_key` prints `<set>` when a key is set in user config or `KOTO_DECIDER_API_KEY`, and exits 1 when none is set. The key string never appears in its stdout or stderr.
- [ ] `redact()` replaces `decider.api_key` with `<set>`, so `koto config list` and `koto config list --json` never print the key.
- [ ] No warning built by `resolve_decider` or by `koto config set` includes the key's value, and none includes endpoint userinfo.
- [ ] `koto config set --user decider.mode` accepts only `off`, `shadow`, and `auto`. `never` is refused with a message saying it's template-only.

**File permissions**

- [ ] `koto config set --user` creates `~/.koto/config.toml` with mode 0600 on Unix.
- [ ] `koto config set --user` and `koto config unset --user` tighten an existing `~/.koto/config.toml` with mode 0644 to 0600 on every write. A test covers both commands.
- [ ] Project config writes keep their current behavior.

**Test isolation**

- [ ] `.cargo/config.toml` exists with `[env] KOTO_DECIDER = { value = "off", force = true }`, and `.cargo/audit.toml` is unchanged.
- [ ] A guard test, `tests/decider_harness_guard.rs`, asserts `std::env::var("KOTO_DECIDER") == Ok("off")` inside the test process and fails with a message pointing at `.cargo/config.toml` if not.
- [ ] Running `KOTO_DECIDER=auto KOTO_DECIDER_API_KEY=dummy cargo test --test decider_harness_guard` still passes, which shows `force = true` overrides a developer's exported value.
- [ ] Integration tests that exercise the user-config layer call `cmd.env_remove("KOTO_DECIDER")` (or set it explicitly) so they test what they claim to test. Existing tests that point `HOME` at a temp dir keep doing so.
- [ ] The Go functional suite's command environment in `test/functional/steps_test.go` appends `KOTO_DECIDER=off` after `os.Environ()`, so a developer's exported value can't reach it.

**General**

- [ ] `cargo test` passes, and `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` are clean.
- [ ] No new crate appears in `Cargo.lock`. If endpoint parsing needs `url` as a direct dependency, it's declared at the version already locked (2.5.x) so no crate is added.
- [ ] Must deliver: an `ApiKey` type in `src/decider/types.rs` with a named raw-value accessor, and `DeciderSettings` from `resolve_decider` carrying the effective global mode, `Option<ApiKey>`, the endpoint URL, the endpoint origin (`default` / `user` / `env`), the timeout as a `Duration`, and the public `opted_in()` method. `build_decider(&DeciderSettings)` returns `None` exactly when `opted_in()` is false, and the Jev client reads the key, endpoint, and timeout from these fields. (required by <<ISSUE:3>>)
- [ ] Must deliver: `resolve_decider` returns its warnings as a `Vec<String>` without printing them, and `DeciderSettings` exposes the endpoint origin, so `koto next` can print the warnings to stderr once per invocation and record `endpoint_origin` on each consultation. (required by <<ISSUE:4>>)
- [ ] Must deliver: `load_config()` plus `resolve_decider()` plus `DeciderSettings::opted_in()` usable from a CLI verb outside `koto next`, so the `--fixtures` runner gates on the same predicate (same-layer rule included) and exits 2 when it's false, without a local mode-or-key check. (required by <<ISSUE:6>>)
- [ ] Security review completed

**Security Checklist**:

- [ ] The API key is read only from `KOTO_DECIDER_API_KEY` or user `decider.api_key`. No code path takes it from project config, and a test proves a project-config key is dropped at load, not just refused by `koto config set`.
- [ ] The endpoint is read only from `KOTO_DECIDER_ENDPOINT`, user config, or the built-in default, and project-config endpoints are dropped at load.
- [ ] The same-layer rule holds: a user-config key is never paired with an env endpoint, and an env key is never paired with a user-config endpoint. The injected `KOTO_DECIDER_ENDPOINT=<host> koto next` case is covered by a test.
- [ ] Project config can only lower the effective mode. There's no input combination where a project value raises it.
- [ ] Unknown, empty, or mistyped modes fail safe to `off`.
- [ ] `DeciderSettings::opted_in()` is the only opt-in check, and it includes the same-layer and endpoint rules, so no caller can build a client from a mode-and-key check alone.
- [ ] The key has no `Display` path, its `Debug` prints `<redacted>`, and grep finds no `format!`, `println!`, `eprintln!`, `warn`, or error construction that interpolates the raw accessor.
- [ ] `koto config get`, `koto config list`, `koto config list --json`, and every warning print `<set>` or nothing for the key, never its value.
- [ ] Warnings about an endpoint print only scheme, host, and path. Userinfo, query strings, and fragments are never echoed.
- [ ] Plain `http` is accepted only for loopback IP literals and exactly `localhost`, decided without DNS resolution.
- [ ] Endpoints with userinfo are rejected both at `koto config set` and at resolution.
- [ ] `~/.koto/config.toml` is 0600 after every `koto config set --user` and `koto config unset --user`, including when the file existed with looser permissions.
- [ ] `timeout_ms` is bounded (capped at 10000), so a config value can't make a `koto next` hang for an arbitrary time.
- [ ] A malformed `[decider]` table in a checked-in project config can't break koto for the user (no parse failure of the whole file).
- [ ] `cargo test` runs with `KOTO_DECIDER=off` forced, the guard test fails loudly if that stops being true, and the Go functional suite appends `KOTO_DECIDER=off`, so an exported developer key can't reach a provider during tests.
- [ ] No new crate is added to the dependency graph.

**Dependencies**: None

**Type**: code

### Issue 3: feat(decider): add provider-neutral decider types and the Jev client

**Goal**: Add the provider-neutral `src/decider/` module (the `Decider` trait and its request and response types, `build_request`, and `evaluate`), the Jev client, `build_decider` gated only on <<ISSUE:2>>'s `DeciderSettings::opted_in`, and a bounded HTTP transport (`post_json_with_deadline`), together with a `std::net` stub server for tests and a CI check that keeps async HTTP stacks out of `cargo tree`.

**Repo**: `tsukumogami/koto` (PR group `default`); **Complexity**: critical

**Acceptance Criteria**:

- [ ] HTTP 429 and HTTP 529 responses from the stub each give error class `http_status` with that status.
**Module layout and purity**

- [ ] `src/lib.rs` declares `pub mod decider;`, and `src/decider/` contains `mod.rs`, `types.rs`, `request.rs`, `evaluate.rs`, `jev.rs`, and `http.rs`. The existing `tests/lib_reexports.rs` passes unchanged.
- [ ] `types.rs`, `request.rs`, and `evaluate.rs` import nothing from `std::net`, `std::fs`, `std::thread`, `std::time::Instant`/`SystemTime`, or `attohttpc`, and none of the strings `noul`, `criteria`, or `"choice"` appears in them (checked by the validation script).
- [ ] `Decider` exposes `fn provider(&self) -> &str` and `fn decide(&self, req: &DecisionRequest) -> Result<DecisionResponse, DeciderError>`, and it's object-safe (`Box<dyn Decider>` compiles).
- [ ] `ErrorClass` serializes to exactly `timeout`, `connect`, `http_status`, `malformed`, and `mismatched`. A unit test pins all five strings.
- [ ] `DeciderError` carries `class`, `status` (set for `http_status` only), and `detail`. Its `detail` is at most 200 characters and is built only from fixed text plus the error kind.
- [ ] The API key is carried as <<ISSUE:2>>'s `ApiKey` type end to end, and no second key type is introduced anywhere under `src/decider/`.

**build_decider and the opt-in gate**

- [ ] `build_decider(&DeciderSettings) -> Option<Box<dyn Decider>>` returns `Some` if and only if <<ISSUE:2>>'s `DeciderSettings::opted_in()` returns true. Its body contains no mode comparison, key-presence check, or endpoint-origin check of its own (checked by the validation script), and nothing else under `src/decider/` builds a `JevDecider` for production use.
- [ ] A user-config key with an endpoint set only through `KOTO_DECIDER_ENDPOINT` (an env-only endpoint, pointed at the stub) yields `None` from `build_decider`, and the stub records zero requests. The settings for this test are produced by <<ISSUE:2>>'s `resolve_decider` from a user config and an env map, not hand-built with `opted_in` forced, so the test fails if the predicate and `build_decider` ever disagree.
- [ ] Settings with a user mode of `auto`, a project mode of `off`, and a key present also yield `None`, and settings with mode `shadow` or `auto`, a key, and a same-layer endpoint yield a client whose `provider()` is `jev`.
- [ ] The returned client uses the endpoint and timeout from the settings it was built from and nothing read from the environment afterwards: changing `KOTO_DECIDER_ENDPOINT` after `build_decider` returns doesn't change where the next `decide` sends its request (stub test).

**build_request**

- [ ] `build_request` takes a state's declared fields (name, field `description`, and <<ISSUE:1>>'s `FieldDecider`) plus assembled inputs keyed by label. It returns a `DecisionRequest` with one `Question` per declared field in declaration order, and with `inputs` ordered by the declaration's `inputs` list.
- [ ] An enum field becomes `QuestionKind::Choice`, with the field description as the question, every value with its answer description in `values` order, and the escape value with its description. A boolean field becomes `QuestionKind::Proposition`, with the field description as the proposition.
- [ ] A declared input label missing from the assembled inputs makes `build_request` return an error rather than a request with a gap. Budget enforcement and unset-key detection stay with the caller (<<ISSUE:4>>).
- [ ] The same declaration and inputs always produce an equal `DecisionRequest` (unit test).

**evaluate**

- [ ] `evaluate` takes the declared fields, the `DecisionResponse`, and a caller-supplied effective mode for every declared value (off, shadow, or auto). It doesn't read settings, doesn't take the configured mode, and doesn't compute a minimum of modes. No function named `effective_mode`, and no ordering of configured against template modes, exists anywhere under `src/decider/` (checked by the validation script).
- [ ] Enum winner: with probabilities `{proceed: 0.7, exit: 0.2, unclear: 0.1}` the winner is `proceed` with confidence 0.7. With `{proceed: 0.45, exit: 0.45, unclear: 0.1}` the result is the escape. With the escape highest, the outcome is `escape`.
- [ ] Confidence comes from the winner's probability, never from `provider_confidence`: an answer with winner probability 0.6, `provider_confidence` 0.99, and threshold 0.9 is `below_threshold`.
- [ ] Boolean with both thresholds at 0.9: P(true) 0.95 gives `true`, 0.05 gives `false`, and 0.5 gives the escape. A case where both qualify (both thresholds 0.5 with P(true) 0.5) is also the escape.
- [ ] Threshold boundary: confidence exactly equal to the threshold meets it (`qualified` when the supplied effective mode is `auto`). Confidence one ulp below is `below_threshold`. Each field result carries `at_threshold`, computed on the unrounded numbers.
- [ ] Outcome order holds for every precedence pair, with a unit test per pair: an escape winner above threshold on a field whose values are all template-`never` is `escape`; a below-threshold winner whose template mode is `never` is `below_threshold`; a confident winner whose template mode is `never` is `never` even when the supplied effective mode is `auto`; a confident winner with supplied effective mode `shadow` is `shadow`; a confident winner with supplied effective mode `auto` is `qualified`.
- [ ] The supplied mode decides `shadow` versus `qualified` on its own: the same confident answer on a template-`auto` value is `shadow` when the caller supplies `shadow` and `qualified` when it supplies `auto`, and a supplied `off` is never `qualified`.
- [ ] A value with no declared threshold is compared against 0.9. That default comes from <<ISSUE:1>>'s compiled `FieldDecider`, so `evaluate` hard-codes no threshold and no mode default.
- [ ] All-or-nothing: on a state with two declared fields where one is `qualified` and the other `below_threshold`, `evaluate` returns no candidate evidence, and the per-field results record `qualified` and `below_threshold`. With both `qualified`, the candidate holds both fields, with enum values as JSON strings and boolean values as JSON booleans.
- [ ] Defensive shape check: an answer missing a declared field, or whose kind doesn't match the question kind, returns `mismatched` or `malformed` rather than panicking. A missing effective mode for a declared value is treated as `shadow`, never as `auto`.

**Jev client (`jev.rs`)**

- [ ] Against the stub, the recorded request body has exactly the top-level keys `model` (equal to `jev-latest`), `state`, and `questions`. `state` holds exactly the declared labels in declaration order. An enum question is `{type: "choice", instructions, criteria}`, with criteria keys in value order followed by the escape. A boolean question is `{type: "noul", instructions}`. No other content appears.
- [ ] The same request serialized twice produces byte-identical bodies, because ordering doesn't depend on `serde_json`'s `preserve_order` feature.
- [ ] Recorded headers include `Authorization: Bearer <key>`, `Content-Type: application/json`, and `User-Agent: koto/<version>`.
- [ ] Response validation, one stub case each, producing the stated class: HTTP 500 gives `http_status` with status 500. HTTP 401 gives `http_status` with status 401 and prints one fixed stderr line naming the status. A body that isn't JSON gives `malformed`. A missing `answers` gives `malformed`. A missing answer for an asked field gives `mismatched`. An answer for an unasked field gives `mismatched`. A choice probability key outside values plus escape gives `mismatched`. A missing probability key gives `mismatched`. A probability of NaN, a negative number, or a value above 1 gives `malformed`. Choice probabilities summing to 1.02 give `malformed`, while 1.009 is accepted. A `noul` outside [0, 1] gives `malformed`. A type that doesn't match the question kind gives `malformed`.
- [ ] Model handling: a response without `model`, or with an empty or non-string `model`, records `unknown` and still succeeds. A `model` with surrounding whitespace, control characters, and 300 characters is recorded trimmed, stripped, and capped at 128 characters.
- [ ] A declaration that would exceed Jev's 2 to 255 choice keys, or has duplicate input labels, returns `malformed` without sending (the stub records zero requests).
- [ ] No `DeciderError` detail, and nothing printed to stderr, contains the API key, any response body text, or the endpoint's userinfo. The test uses a distinctive key string and stub bodies containing a marker string, then greps the error `Debug`/`Display` output and captured stderr for both.

**Transport (`http.rs`)**

- [ ] `post_json_with_deadline(url, bearer, body, budget) -> Result<(u16, Vec<u8>), DeciderError>` sets attohttpc's connect, read, and whole-request timeouts to `budget` and runs the request on a worker thread that the caller waits on with `recv_timeout(budget)`.
- [ ] With a 200 ms budget and a stub that delays its response 10 s, the call returns `timeout` in under 450 ms of wall-clock time.
- [ ] The watchdog is proven independently of the network: the deadline helper, given a worker that blocks for 5 s (standing in for a hung DNS lookup), returns `timeout` within budget plus 250 ms.
- [ ] A closed port (connection refused) returns `connect`.
- [ ] Redirects aren't followed: a stub answering `302` with `Location` pointing at a second stub produces `http_status` with status 302, and the second stub records zero requests.
- [ ] No proxy for loopback: with `HTTP_PROXY`, `HTTPS_PROXY`, and `ALL_PROXY` pointed at a closed port, a request to the stub at `127.0.0.1` and at `localhost` succeeds. This test runs in its own test binary so the env mutation can't race other tests. Loopback is decided with <<ISSUE:2>>'s loopback-by-literal helper (127.0.0.0/8, `::1`, or exactly `localhost`), never by resolution and never by a second copy of the rule.
- [ ] Body cap: a 1 MiB body is read in full, and a body of 1 MiB + 1 byte returns `malformed` without buffering beyond the cap.

**Stub server and dependency checks**

- [ ] `tests/support/decider_stub.rs` is a `std::net::TcpListener` server on an ephemeral loopback port, included by test files via `#[path]`. It serves one HTTP/1.1 request per connection. It counts requests, records bodies and headers, and replies from a per-request script: any status, any headers (including `Location`), any body, or a delay. It can also hold a request open until released, and it has a helper that yields a closed port.
- [ ] `tests/decider_client.rs` (plus the separate proxy test binary) exercises `build_decider`, the Jev client, and the transport against the stub, and all of it passes under `cargo test -- --test-threads=1`.
- [ ] `Cargo.toml` declares `attohttpc = { version = "0.30", default-features = false, features = ["tls-rustls-webpki-roots", "json"] }`. `Cargo.lock` still holds exactly one `attohttpc` package and gains no `[[package]]` entry. No dev-dependency is added.
- [ ] `.github/workflows/validate.yml` gains a step that fails when `cargo tree -e normal,build,dev` lists `tokio`, `hyper`, `reqwest`, or `ureq`, and when `cargo tree -d` lists more than one `attohttpc`. The step passes on this branch.
- [ ] Every test that reaches the stub sets `KOTO_DECIDER`, `KOTO_DECIDER_API_KEY`, and `KOTO_DECIDER_ENDPOINT` explicitly, either on its own `Command` or through settings resolved in-process from an explicit env map. None relies on a developer's exported environment. <<ISSUE:2>>'s `.cargo/config.toml` default of `off` stays in force.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` pass.

**Downstream deliverables**

- [ ] Must deliver: the `Decider` trait, `DecisionRequest`/`DecisionResponse`, `ErrorClass` with its serialized names, and `build_decider(&DeciderSettings) -> Option<Box<dyn Decider>>` gated solely on `DeciderSettings::opted_in` (required by <<ISSUE:4>>, whose `CliDeciderPort` constructs the provider and records `provider()`, `model`, and `error_class`).
- [ ] Must deliver: `build_request` from a compiled declaration plus labelled inputs, callable without any I/O (required by <<ISSUE:4>>'s port, and reused later by the fixture runner in <<ISSUE:6>>).
- [ ] Must deliver: `evaluate` taking a per-value effective-mode input and returning, per field, `winning`, `confidence`, `threshold`, `at_threshold`, and `outcome`, plus an optional candidate evidence map when every field qualifies. The input is shaped so <<ISSUE:4>> can fill it straight from its own `effective_mode` in `src/engine/decider.rs` (required by <<ISSUE:4>>, which appends these into `decider_consulted` and then runs `conditional_matches` on the candidate).
- [ ] Must deliver: a `#[cfg(test)]` scripted fake `Decider` in `src/decider/` that returns a queued answer or error, for engine unit tests (required by <<ISSUE:4>>).
- [ ] Must deliver: `tests/support/decider_stub.rs` with request counting, payload recording, scripted status, headers, body, and delay, hold-open-until-released, and a closed-port helper, usable from any integration test file (required by <<ISSUE:4>>'s stickiness, concurrency, cap, and failure-kind integration tests).
- [ ] Security review completed

**Security Checklist**:

- [ ] `build_decider` constructs a client only when <<ISSUE:2>>'s `DeciderSettings::opted_in` holds, and a user-config key with an env-only endpoint yields no client and no request.
- [ ] The API key travels only as `ApiKey` and appears only in the `Authorization` header. It never reaches a `DeciderError`, a `Debug` dump, stderr, or a log line, and the 401/403 message names the status only.
- [ ] Redirects are disabled, and a test proves a 3xx never sends the bearer token to the `Location` host.
- [ ] Proxy environment variables are ignored for loopback hosts, and loopback is decided by IP literal or the exact name `localhost` through <<ISSUE:2>>'s helper, never by DNS.
- [ ] Every request is bounded: connect, read, and whole-request timeouts plus the watchdog. Recorded wall-clock stays within budget plus 250 ms on the timeout test.
- [ ] The response body is capped at 1 MiB before parsing, and oversized bodies fail as `malformed` without unbounded allocation.
- [ ] Response validation rejects undeclared keys, missing keys, non-finite values, values outside [0, 1], and enum sums off by more than 0.01, so a provider can't name a value the template didn't declare.
- [ ] `evaluate` never returns `qualified` for a template-`never` value or a value whose supplied effective mode isn't `auto`, and a missing mode input falls to `shadow`.
- [ ] The recorded `model` string is trimmed, stripped of control characters, and capped at 128 characters, so a provider can't inject terminal escapes or oversized strings into events.
- [ ] The request payload contains only the question, the value and escape descriptions, field and value names, and the labelled inputs. The stub test asserts the exact key set.
- [ ] Error details are fixed text plus the error kind, and never include a response body, a header, or URL userinfo.
- [ ] No crate enters the dependency graph, and the CI `cargo tree` check prevents tokio, hyper, reqwest, ureq, or a second attohttpc from entering later.
- [ ] Every stub test sets the decider env explicitly, and no test can reach the real endpoint with a developer's exported key.

**Dependencies**: <<ISSUE:1>>, <<ISSUE:2>>

**Type**: code

### Issue 4: feat(engine): consult the decider when a state would stop for evidence

**Goal**: Make `koto next` consult an opted-in decider, once per state visit under a fail-fast lock, at the exact point a declared state would stop for evidence, apply its answer only when every declared field qualifies, exactly one conditional transition matches, and that transition clears the runtime floor, and print the decider configuration warnings to stderr once per invocation.

**Repo**: `tsukumogami/koto` (PR group `default`); **Complexity**: critical

**Acceptance Criteria**:

Unless stated otherwise, integration criteria run in CI against the `std::net` stub decider from <<ISSUE:3>>, reached through `KOTO_DECIDER_ENDPOINT` with the stub counting requests and recording payloads. This issue may extend the stub with a per-request hold and a "request received" signal the test can wait on, for the concurrency criterion. Engine criteria run as unit tests calling `advance_until_stop_with_decider` with a fake `DeciderPort` (a struct with a call counter and scripted replies), no network, and no lock.

**Engine structure**

- [ ] `advance_until_stop_with_decider(..., decider: Option<&mut dyn DeciderPort>)` exists in `src/engine/advance.rs`, and `advance_until_stop` is a one-line wrapper that passes `None`. The existing `advance_until_stop` test call sites are not edited and all pass.
- [ ] `conditional_matches(state, evidence, vars) -> Vec<String>` is extracted from `resolve_transition`, which now calls it. Existing `resolve_transition` tests pass unchanged, and a new unit test shows `conditional_matches` returns an empty list where `resolve_transition` returns `Resolved` for the unconditional fallback.
- [ ] The `Resolved` arm's bookkeeping (cycle check, `Transitioned { condition_type: "auto" }`, `visited`, `advanced`, `transition_count`, evidence reset) moves into a `take_transition` helper that both the `Resolved` arm and the decider path use.
- [ ] `src/engine/decider.rs` holds `DeciderPort` (`policy`, `consult`, a default no-op `recorded`), `ConsultRequest`, `ConsultReply` (`Skipped` | `Consulted`), `VisitGuard`, `effective_mode`, `visit_start_index`, `prior_consultation`, and `MAX_CONSULTATIONS_PER_CALL = 4`, with no file or network I/O.
- [ ] Unit tests for `visit_start_index` and `prior_consultation` cover workflow initialization (`transitioned` with `from: null`), a self-loop, a rewind, a directed transition, and a session that has left the state (returns `None`). A test asserts `visit_start_index` agrees with the `entry_slice(.., Boundary::AnyEntry)` boundary on the same logs.
- [ ] `effective_mode` returns the minimum of user, project, and template modes in the order `off` < `shadow` < `auto`, with template `never` counting as `shadow` for consulting and never applied. Unit tests cover each of the three inputs being the lowest, and `never` under a user and project `auto` (consulted, recorded as `never`, not applied).
- [ ] `fn effective_mode` is defined in `src/engine/decider.rs` and nowhere else in `src/`; `src/decider/`, `src/config/`, and `src/cli/` call it rather than re-deriving a per-value mode.

**Event and evidence marking**

- [ ] `EventPayload::DeciderConsulted(DeciderConsultation)` serializes with type name `decider_consulted`, is declared before `InstructionsDelivered`, and round-trips. The payload carries `state`, `visit_seq`, `provider`, `model`, `input_sha256` (omitted when inputs couldn't be assembled), `outcome` (`applied` | `not_applied` | `input_unavailable` | `error`), `error_class` (omitted when none), `latency_ms`, `directive_bytes`, `endpoint_origin` (`default` | `user` | `env`), and `fields`. A log line with an unknown future event type still falls through to `Unknown`.
- [ ] Each `fields` entry carries `declaration_hash`, `modes` (effective mode per value), `probabilities` rounded to 4 dp, `winning`, `confidence`, `threshold`, `at_threshold` (computed on unrounded numbers), and the per-field `outcome` (`qualified` | `shadow` | `never` | `below_threshold` | `escape`). A unit test with a probability of 0.89996 against a 0.9 threshold records `at_threshold: false` even though it rounds to 0.9000.
- [ ] `EvidenceSubmitted` gains `source: Option<String>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`. With `None` the serialized event has no `source` key (byte-identical to today); with `Some("decider")` it round-trips; a payload with an unrecognized `source` string still parses. Every existing construction site sets `source: None`, and an evidence event with `source: None` still doesn't shadow a real `submitter_cwd` in the batch `resolution_context`.
- [ ] `--with-data` can't set `source`: submitting `{"source": "decider", ...}` as evidence lands in `fields` (or is rejected by `accepts` validation) and never sets the event's `source` field.
- [ ] `docs/reference/session-feed.md` lists `source` on `evidence_submitted` as an optional, nullable string, and `koto template validate-feed` accepts a log containing a `source: "decider"` evidence event.

**Opt-in and warnings (R10, R11, R16, R17)**

- [ ] `handle_next` decides whether to build a port only by calling `DeciderSettings::opted_in()`; it contains no separate mode or key check.
- [ ] With the user mode `auto` and no API key, `handle_next` builds no port: a scripted run makes zero stub requests, records no `decider_consulted` event, and creates no `decider.lock` file in the session directory.
- [ ] With the user mode `off`, or `KOTO_DECIDER=off`, a template declaring `auto` makes zero stub requests and creates no `decider.lock` file.
- [ ] With a key set and the stub endpoint configured, `KOTO_DECIDER=never` and `KOTO_DECIDER=bogus` each make zero stub requests, record no `decider_consulted` event, and print a warning to stderr naming `KOTO_DECIDER`. The stdout JSON equals the same run with `KOTO_DECIDER=off`.
- [ ] The warning is printed once per `koto next` invocation: a single call that auto-advances through three declared states prints it exactly once, and two separate calls print it once each. A run with a valid configuration prints no decider warning.
- [ ] With the user mode `auto` and project config `shadow`, an `auto` template value above threshold is consulted but not applied; with project config `auto` and the user mode `shadow`, it's not applied either (project config can't raise the mode).
- [ ] `koto status` on a declared state makes no stub request.
- [ ] A `koto next` on a state with no declaration makes no stub request and appends no `decider_consulted` event.
- [ ] A state whose declared values all have effective mode `off` makes no stub request.
- [ ] A state whose gate failed makes no stub request and records nothing. After the gate passes on a later tick, the visit is consulted exactly once.
- [ ] When the visit's evidence already holds a declared field, no consultation happens and the agent's value stands (engine unit test with the fake port asserting zero `consult` calls).
- [ ] A declared state reached by auto-advance within one `koto next` is consulted, not only the state the call started on.

**Stickiness and concurrency (R12)**

- [ ] Three `koto next` calls within one visit make exactly one stub request, including when that request timed out and when the consultation stopped at `input_unavailable`. A rewind into the state makes one more, and a self-loop back into it makes one more.
- [ ] The lock fails fast. The stub is configured to hold the first request open for 3 s before answering (with `decider.timeout_ms` at 10000 so the first process doesn't time out). The test starts the first `koto next`, waits until the stub reports it has received that request, and only then starts a second `koto next` on the same visit. The second process exits within 1 s of starting, returns the opted-out `evidence_required` response, and the stub's request count is still 1 when it exits. After the first process finishes, the stub has received exactly one request and the log holds exactly one `decider_consulted` event for the visit. A blocking lock (the second waits out the hold) and a missing lock (the stub sees a second request) both fail this test.
- [ ] The lock loser appends nothing, isn't charged against its cap, and a later `koto next` on the same visit makes no stub request.
- [ ] A tick that starts on a batch-scoped state (one with `materialize_children`, so `handle_next` holds `_batch_lock`) and auto-advances into a declared state consults that state exactly once and returns without a `Locked`/`EWOULDBLOCK` error. A declared state that is itself batch-scoped is also consulted once.
- [ ] `CliDeciderPort` takes its lock on `<session_dir>/decider.lock` and never calls `lock_state_file`.
- [ ] The engine drops the `VisitGuard` only after appending `decider_consulted`, any decider-sourced `evidence_submitted`, and the `transitioned` event (engine unit test: a fake guard records the event count at drop time).
- [ ] Under the lock, the port reads through `SessionBackend::read_events_local`. `CloudBackend` overrides it to read only the local file; a unit test with an unreachable S3 endpoint shows `read_events_local` returns the local events without attempting a pull.
- [ ] The port re-checks for a prior consultation after the re-read and returns `Skipped` when one exists, or when the session has left the state.
- [ ] A log holding a `decider_consulted` event with outcome `applied` for the current visit but no following evidence (a crash between the two appends) is not consulted again and returns `evidence_required`.

**Evaluation and application (R14, R15, R9)**

- [ ] In shadow mode, with the stub returning each value in turn (including a confident wrong one), the `transitioned` events and every agent-visible `koto next` response are identical to a run with the user mode `off`.
- [ ] In auto mode, an answer at or above threshold for an `auto` value advances past the state without an `evidence_required` response; the response is the next stop with `advanced: true` and no new response field. The log shows, in order, one `decider_consulted` with outcome `applied`, one `evidence_submitted` with `source: "decider"` holding every declared field, then `transitioned` with `condition_type: "auto"`.
- [ ] An `auto` answer with confidence exactly equal to its threshold is applied.
- [ ] On a state with two declared fields where one qualifies and the other is below threshold, neither is applied; the event records `qualified` for the first field, `below_threshold` for the second, and `not_applied` overall; the response equals the opted-out response.
- [ ] For a boolean field with `true` at 0.9 and `false` at 0.9, P(true) 0.95 applies `true` in auto, 0.05 applies `false`, and 0.5 isn't applied and records `escape`.
- [ ] A tie for the top enum probability is recorded as `escape` and not applied.
- [ ] An `auto` answer whose evidence matches no conditional transition, or more than one, isn't applied, and the response equals the opted-out response.
- [ ] An `auto` answer whose single matching conditional transition targets a state already in `visited` for this call isn't applied.
- [ ] Runtime floor, gate-conditioned route: a fixture state declares `verdict` (`proceed` in `auto`, `exit` in `never`) with a passing gate `ci`, and its route for the decider's answer is `when: {evidence.verdict: present, gates.ci.exit_code: 0}` to a non-terminal state with no confirmation, next to a `when: {verdict: exit}` route. The test first asserts `koto template compile` accepts the fixture (no `E-DECIDER-FLOOR`, proving the route escaped compile-time analysis) and that `conditional_matches` returns exactly that one transition for `{verdict: proceed}` plus the gate output. With the stub answering `proceed` at 0.99 in auto, the answer is not applied: the event records outcome `not_applied`, no `source: "decider"` evidence and no `transitioned` event are appended, and the response equals the opted-out response.
- [ ] Runtime floor, terminal and confirmation-guarded targets: the same `evidence.verdict: present` route shape (without the gate key), once targeting a terminal state and once targeting a state whose `default_action` has `requires_confirmation: true`, compiles, matches exactly once, and is not applied, with the same observable results as the gate case.
- [ ] Control for the floor fixtures: the same template with the route's `gates.*` key removed and a non-terminal, unguarded target is applied, so the floor tests fail if the answer is never applied for some other reason.
- [ ] Each of these returns the opted-out response and records its outcome and, where it applies, its `error_class`: a timeout, a refused connection, an HTTP 500, an HTTP 401, a malformed body, a response naming an undeclared value, below threshold, the escape, a `never` value, an unset input context key, and an input over its byte budget. No stub request is made in the last two cases, which record `input_unavailable`.
- [ ] With no `decider.timeout_ms` set, a stub delaying 2500 ms produces outcome `error` with class `timeout`.
- [ ] With a 200 ms timeout and a stub that delays 10 s, `koto next` returns the opted-out response in under 2 s, the event records outcome `error` with class `timeout`, and its `latency_ms` is at most 450.

**The cap (R19)**

- [ ] A template chaining five consecutive auto-eligible states makes at most four stub requests in one `koto next`, and the fifth state returns `evidence_required`. An engine unit test shows `input_unavailable` and `error` replies count toward the four and `Skipped` replies don't.

**Input assembly and the payload (R13, R18)**

- [ ] The port resolves each declared input from the context store (`context`), template variables, or captured values (`var`), each against its byte budget. A context key written by an earlier state's `default_action` reaches the stub payload under its label, and a template variable and a captured value do the same.
- [ ] An input with no declared budget is treated as over budget at 8193 bytes and not at 8192.
- [ ] The payload the stub receives contains exactly the question, the value and escape descriptions, and the labelled inputs, and no other session content (no session name, other context keys, evidence, or directive text).
- [ ] `input_sha256` is the SHA-256 of the assembled inputs and is the same across two consultations with identical inputs.

**Recorded metadata**

- [ ] `directive_bytes` equals the byte length of the substituted `directive` plus `details` that the opted-out `koto next` response returns for the same state and visit.
- [ ] `endpoint_origin` is `env` when the endpoint came from `KOTO_DECIDER_ENDPOINT` and `user` when it came from user config; a unit test covers `default`.
- [ ] The `decider_consulted` event carries the provider name and the model string the stub returned, and `unknown` when the response has none.
- [ ] No `decider_consulted` event contains any input string from the payload, the API key, or response or error text.

**Regressions**

- [ ] A template with no `decider` block produces the same `koto next` and `koto status` JSON as before; `tests/next_response_baseline.rs` passes unchanged.
- [ ] A tick that doesn't consult makes no network call.
- [ ] Must deliver: `DeciderPort::recorded(&EventPayload)` called exactly once, right after each `decider_consulted` event is durably appended and while the `VisitGuard` is held, with the port retaining the session header's `session_id` (`None` for headers that predate it) from its locked re-read, so the ledger can write the `consulted` record from the hook (required by <<ISSUE:5>>).
- [ ] Must deliver: public `visit_start_index` and `prior_consultation` that take a `&[Event]` read by `handle_next` and return the current visit's seq and its `DeciderConsultation` (including `outcome`), so the `--with-data` path can decide whether to append an `answered` record and skip evidence whose `source` is `"decider"` (required by <<ISSUE:5>>).
- [ ] Must deliver: `DeciderConsultation` and `FieldConsultation` in `src/decider/record.rs` as `Serialize`/`Deserialize` structs usable outside the event, so the ledger's `consulted` record can flatten the same struct (required by <<ISSUE:5>>).
- [ ] Must deliver: a stub-driven integration test helper that produces a session log containing a `decider_consulted` event and a `source: "decider"` evidence event, whose on-disk shape is the one v0.12.2 must read as `Unknown` plus ordinary evidence (required by <<ISSUE:8>>).
- [ ] Security review completed

**Security Checklist**:

- [ ] No consultation, lock file, log read under lock, or network call happens unless `handle_next` built a port, and it builds one only when `DeciderSettings::opted_in()` is true, so the same-layer endpoint rule can't be bypassed by a local mode-or-key check.
- [ ] An unrecognized `KOTO_DECIDER` or `decider.mode` value resolves to `off` and is surfaced as a stderr warning, never silently ignored and never treated as opted in.
- [ ] The provider payload holds only the question, value and escape descriptions, and the declared inputs within their budgets; inputs resolve only from context-store keys, template variables, and captures, never from environment variables or arbitrary files.
- [ ] The `decider_consulted` event holds no input content, no API key, no response body, and no error text: only names, hashes, numbers, modes, and the error class. Cloud sync therefore carries nothing beyond those.
- [ ] An answer applies only when every declared field is `qualified`, none is the escape, exactly one conditional transition matches, its target isn't in `visited`, and the runtime floor holds for that transition (not terminal, not `requires_confirmation`, no `gates.*` in its `when`), including routes that reach the field only through `evidence.*` or `vars.*` keys.
- [ ] The escape value can never be written as evidence: the candidate is run through `validate_evidence`, which rejects values outside `values`.
- [ ] `source: "decider"` can only be set by the engine; agent-submitted evidence can't forge it.
- [ ] The lock is taken with `LOCK_NB`; a loser never waits, never consults, and records nothing, so a contended lock can't stall `koto next` or double-bill the key.
- [ ] `decider.lock` lives inside the session directory, holds no data, is removed with the session, and isn't uploaded by the cloud backend.
- [ ] Every failure path (lock error, read error, input error, timeout, transport, HTTP, malformed or mismatched response) returns the response an opted-out user would get and never a new error.
- [ ] The whole-consultation latency stays within the configured timeout plus 250 ms.

**Dependencies**: <<ISSUE:1>>, <<ISSUE:3>>

**Type**: code

### Issue 5: feat(decider): record consultations in a ledger that survives cleanup

**Goal**: Record every decider consultation, and every agent answer to a consulted visit, as append-only lines in `~/.koto/_decider_ledger.jsonl` that survive session cleanup and `koto workspace prune`, with the `decider_consulted` event registered in the session-feed contract and the ledger documented as authoritative state.

**Repo**: `tsukumogami/koto` (PR group `default`); **Complexity**: testable

**Acceptance Criteria**:

Shared append primitive:

- [ ] `src/engine/jsonl_append.rs` defines `append_bounded_line(dir: &Path, path: &Path, line: &str, max: usize) -> Result<()>`. It refuses a line over `max` bytes (newline included) before opening the file, creates `dir` if absent, opens with `create(true).append(true)`, performs exactly one `write_all`, then `sync_data`, and never seeks or calls `write_at`.
- [ ] Files it creates have mode 0600 on unix; a pre-existing file's mode is left unchanged. A unit test covers both.
- [ ] `append_terminal_index_entry` in `src/engine/terminal_index.rs` serializes its entry and calls `append_bounded_line` with `MAX_INDEX_LINE_BYTES`; it no longer opens the file itself. `MAX_INDEX_LINE_BYTES` stays in `terminal_index.rs`.
- [ ] The existing terminal-index tests pass unchanged, including `race_n_writers_produce_n_parseable_lines` and everything in `tests/terminal_index.rs` and `tests/terminal_index_compaction.rs`.

Ledger records:

- [ ] `src/decider/ledger.rs` defines `ledger_path(koto_root: &Path) -> PathBuf` returning `<koto_root>/_decider_ledger.jsonl`, `MAX_LEDGER_LINE_BYTES = 4096`, and `Serialize + Deserialize` record types for both kinds, each with `kind`, `v: 1`, `at` (RFC 3339 UTC), `session`, and `session_id: Option<String>`.
- [ ] A `consulted` record is the envelope plus every field of `DeciderConsultation` flattened in: `state`, `visit_seq`, `provider`, `model`, `input_sha256` (when present), `outcome`, `error_class` (when present), `latency_ms`, `directive_bytes`, `endpoint_origin`, and `fields` (each with `declaration_hash`, `modes`, `probabilities`, `winning`, `confidence`, `threshold`, `at_threshold`, `outcome`). A round-trip test serializes a record, parses it back, and compares.
- [ ] `CliDeciderPort::recorded` appends the `consulted` record after the `decider_consulted` event append and before the `VisitGuard` is dropped. Every consultation outcome (`applied`, `not_applied`, `input_unavailable`, `error`) produces exactly one `consulted` line. (The `recorded` hook is delivered by <<ISSUE:4>>; if it landed without one, add the hook to `DeciderPort` with a default no-op as a sub-task of this issue.)
- [ ] In `handle_next`'s `--with-data` path, after `EvidenceSubmitted` is appended, an `answered` record is appended when the current visit (found with <<ISSUE:4>>'s `visit_start_index`) holds a `decider_consulted` event whose outcome isn't `applied`. `values` contains only fields that carry a `decider` block. No record is written when the visit has no consultation, when the consultation was `applied`, or when none of the submitted fields is declared.
- [ ] Two submissions in the same visit (the first matching no transition, the second corrected) each write an `answered` record with the same `visit_seq`.
- [ ] The `answered` record is written even when the decider mode was switched to `off` between the consultation and the submission.
- [ ] When the session header's `session_id` is empty, both record kinds serialize `"session_id": null`, and the consultation and the response are otherwise unchanged.

Size, content, and failure:

- [ ] A `consulted` line whose serialized length exceeds 4096 bytes is rewritten without any `probabilities` map and with `"trimmed": true`; `winning`, `confidence`, `at_threshold`, and `outcome` remain. A unit test with an oversized fixture checks the trimmed line fits and parses.
- [ ] A line still over 4096 bytes after trimming is not written, and stderr gets `warning: decider ledger write failed (...)` naming the ledger path. The session-log `decider_consulted` event for that consultation still carries full probabilities.
- [ ] No event or ledger line contains any input string from the stub's received payload: an integration test with distinctive input text greps the session log and the ledger for it and finds nothing. Every ledger line in that test is at most 4096 bytes.
- [ ] The API key string appears nowhere in the ledger in a run that includes an HTTP 401 from the stub.
- [ ] With `_decider_ledger.jsonl` made read-only, a consulting `koto next` exits 0, prints the same stdout JSON as a run with a writable ledger, and prints one ledger warning line on stderr.
- [ ] With `HOME` pointing at a path where `.koto` can't be created, a consulting `koto next` still exits 0 with an unchanged response and a warning.

Survival and pairing:

- [ ] In a single non-child session with the stub in shadow mode, a consultation followed by an agent `--with-data` answer leaves one `consulted` and one `answered` record in `$HOME/.koto/_decider_ledger.jsonl` with the same `session_id` and `visit_seq`. The `consulted` record carries `declaration_hash`, `input_sha256`, `directive_bytes`, `latency_ms`, `endpoint_origin`, and effective `modes`. The session header's `schema_version` is unchanged.
- [ ] A parent run with two decider-consulting children, taken to terminal with default cleanup, leaves one `consulted` record per child consultation and one `answered` record per agent-answered child visit, after the child session directories are gone.
- [ ] `tests/cli_workspace_prune.rs` gains a case that plants a ledger file, prunes a session tree, and asserts the ledger's bytes are unchanged. No change to the prune code is needed or made.
- [ ] A concurrency test mirroring `race_n_writers_produce_n_parseable_lines` appends N ledger lines from N threads through `append_bounded_line` and reads back N parseable lines.

Session-feed contract:

- [ ] `docs/reference/session-feed.md` frontmatter declares `decider_consulted` at tier 2 with `state` (string), `visit_seq` (integer), `provider` (string), `model` (string), `input_sha256` (string, optional, nullable), `outcome` (string, enum `applied`/`not_applied`/`input_unavailable`/`error`), `error_class` (string, optional, nullable, no enum), `latency_ms` (integer), `directive_bytes` (integer), and `fields` (object), in the file's existing block style. Whether `endpoint_origin` is listed follows whatever the <<ISSUE:4>> event serializes; if the event carries it, it's declared as an optional string with no enum.
- [ ] A catalogue entry for `decider_consulted` is added under "Tier 2: Optional Display", stating that it carries no input content or credentials and that an `applied` outcome is followed by a decider-sourced `evidence_submitted` and a `transitioned`.
- [ ] `src/cli/validate_feed.rs` tests: `shipped_spec_declares_the_new_events_rather_than_skipping_them` (and the shipped-spec accept test) gain `decider_consulted` rows; a payload missing `visit_seq` fails; a payload with `outcome: "bogus"` fails.
- [ ] `koto template validate-feed` accepts a session log produced by a consulting run.

Workspace layout:

- [ ] `docs/workspace-layout.md` lists `_decider_ledger.jsonl` in the directory tree as authoritative state, adds a section saying it can't be rebuilt, nothing in koto deletes it, it isn't compacted in v1, and it's created 0600, and updates the "two trees are authoritative" and "four derived files" wording to stay accurate. It doesn't go on `docs/STABILITY.md`'s rollback delete list.
- [ ] Opted-in integration tests that exercise the ledger set `HOME` to a temp directory; none writes to the developer's real `~/.koto`.

Downstream deliverables:

- [ ] Must deliver: `ledger_path()`, the `consulted`/`answered` record types (deserializable, with `session_id: Option<String>`, `endpoint_origin`, per-field `declaration_hash`, `at_threshold`, `winning`, and `outcome`), and the documented line format (`kind`, `v: 1`, `trimmed` flag) so the report can join records on `(session_id, visit_seq)`, skip null-id records from pairing, and exclude non-default endpoints (required by <<ISSUE:6>>).
- [ ] Must deliver: a ledger populated by real runs in the integration tests, so <<ISSUE:6>> can build its known-pairs fixture ledger from the same writer rather than hand-guessing the format (required by <<ISSUE:6>>).

**Dependencies**: <<ISSUE:4>>

**Type**: code

### Issue 6: feat(cli): add koto decider report with promotion eligibility and fixtures

**Goal**: Add `koto decider report`, which joins the decider ledger into per-question, per-value metrics and success measures, and its `--fixtures` runner, which gates on the shared opt-in predicate from <<ISSUE:2>>, compiles the named template and pushes golden cases through the runtime's own request and evaluation code to mark each value promotion-eligible under R22.

**Repo**: `tsukumogami/koto` (PR group `default`); **Complexity**: testable

**Acceptance Criteria**:

**Verb and read-only behavior**

- [ ] `koto decider report --help` lists `--ledger`, `--state`, `--json`, `--include-custom-endpoints`, `--fixtures`, `--template`, and `--field`. clap rejects `--fixtures` without `--template` and `--state`, and rejects `--template` or `--field` without `--fixtures`.
- [ ] The ledger reader (parsing `consulted` and `answered` lines into typed records, with skip-and-count for bad lines) lives in `src/decider/report.rs` and is added by this issue; <<ISSUE:5>>'s ledger module keeps only the writers and `ledger_path()`.
- [ ] With no `--ledger`, the report reads `_decider_ledger.jsonl` under the koto home directory resolved from `HOME`. A missing ledger file produces an empty report and exit 0, not an error.
- [ ] `--state <s>` limits the questions in the output (table and JSON) to those on state `<s>`.
- [ ] Running the report, with and without `--fixtures`, leaves the bytes of the named template, the user `config.toml`, the project `.koto/config.toml`, and the ledger unchanged, and creates no compile-cache entry and no session directory. A test compares bytes before and after.
- [ ] A fixture run appends no line to the ledger and writes no session event.

**Ledger join and metrics**

- [ ] Given a hand-written fixture ledger with known pairs across at least two questions (two declaration hashes for the same field count as two questions), `koto decider report --json` reports the exact expected values for: per-value paired counts, the confusion matrix including the `below_threshold`, `escape`, and `no_answer` columns, per-value recall, per-value and question coverage, disagreements with their visit ids, fallback rate, error rate by `error_class`, latency p50 and p95 (nearest-rank), agent stops removed, and directive bytes not delivered.
- [ ] The human table shows the same numbers as `--json` for that ledger, and prints coverage over well-formed answers alongside coverage over all consultations.
- [ ] Coverage uses only `applied` and `not_applied` consultations as its denominator; a ledger that adds one `input_unavailable` and one `error` consultation to an otherwise fixed set leaves per-value coverage unchanged and raises the fallback and error rates.
- [ ] Latency percentiles exclude `input_unavailable` consultations.
- [ ] Consultations with a null `session_id` are counted as consultations but never paired, even when an `answered` record carries a matching `visit_seq`.
- [ ] Two `answered` records for the same visit and field resolve to the later value. A duplicate `consulted` line for the same key is counted as a duplicate and not double-counted. An `answered` with no matching consultation is counted as orphaned.
- [ ] A ledger containing a malformed line (bad JSON, a missing required key, or a trailing line with no newline) produces a full report that skips it and prints the skipped count in the header, and exits 0. A line with an unknown `kind` is counted separately from malformed lines.
- [ ] The low-coverage flag appears for a question with 30 consultations where some value is `auto` and coverage over those is below 30%, and doesn't appear at 29 such consultations or at exactly 30% coverage.
- [ ] Changing a value description, the question, the escape description, or an input's `max_bytes` in a template changes the declaration hash the report shows for fixtures run against it; changing a threshold or a mode doesn't. (Pins R23 end to end through the report; the hash function itself is from <<ISSUE:1>>.)

**Custom-endpoint exclusion**

- [ ] For a ledger whose pairs all carry `endpoint_origin: "env"` and that otherwise meets every R22 condition, the value is ineligible without `--include-custom-endpoints` (the reason names the custom-endpoint exclusion) and eligible with it.
- [ ] A fixture run against a non-default endpoint (the loopback stub) can't mark any value eligible unless `--include-custom-endpoints` is passed, and the output says why.
- [ ] Metric tables still include custom-endpoint consultations, and the header prints how many were excluded from eligibility.

**Fixture runner**

- [ ] The fixture runner decides opt-in only by calling `DeciderSettings::opted_in()` from <<ISSUE:2>> (via `resolve_decider` and `build_decider()`); `src/cli/decider.rs` and `src/decider/report.rs` contain no separate check of the global mode or key presence. Each refusal case below prints a message on stderr saying fixture runs need an opted-in decider, prints nothing on stdout, makes zero requests to the loopback stub (its request counter reads 0), and exits 2:
  - no opt-in at all (`KOTO_DECIDER` unset or `off`, and no user `decider.mode`);
  - mode `shadow` but no API key in the user config or the environment;
  - a user-config `decider.api_key` and user `decider.mode = "auto"`, with the stub's URL supplied only through `KOTO_DECIDER_ENDPOINT` (no user-config endpoint): the same-layer rule fails, so the user-config key is never sent to the env endpoint;
  - user `decider.mode = "auto"` with a valid key and same-layer endpoint, run from a directory whose `.koto/config.toml` sets `decider.mode = "off"` (the minimum of the two modes is `off`);
  - a project `.koto/config.toml` that sets `decider.mode = "shadow"` while the user and env modes are unset or `off`.
- [ ] With an env key and env endpoint (`KOTO_DECIDER_API_KEY` and `KOTO_DECIDER_ENDPOINT` both pointing at the stub) and `KOTO_DECIDER=shadow`, the same fixture run does reach the stub, which is the positive control for the refusal cases above.
- [ ] A fixture line with an unknown key, an `expected` value that isn't declared (or a string for a boolean field), a missing or extra input label, or an input over its `max_bytes` aborts with that line number and exit 2 before any stub request.
- [ ] `--template`/`--state` naming a state without a decider field, or a state with two declared fields and no `--field`, exits 2 with a message naming the state.
- [ ] The stub's recorded request payloads for a fixture run are byte-identical to the payloads the runtime sends for a consultation with the same declaration and input texts (the runner goes through `build_request`, the provider client, and `evaluate`, not a separate client). If <<ISSUE:3>> or <<ISSUE:4>> left input assembly and request building in one function, this issue splits them at an `AssembledInputs` seam as a sub-task, with no behavior change to the runtime path.
- [ ] A stub that refuses connections, or returns 401, aborts the fixture run with exit 2 and a message that fixture runs need network access to the configured endpoint. A stub that times out or returns a malformed answer for one case records that case as `no_answer`, and the run completes with exit 0.

**Promotion eligibility boundaries**

- [ ] Starting from a fixture set and ledger that meet every R22 condition (the value is reported `eligible`), each single change below makes that value ineligible and the output names the failing condition. Each is its own test case:
  - 9 cases labelled with the value (40 in total still met);
  - 39 cases in total (every value still at 10 or more);
  - 29 paired observations under the current declaration hash;
  - 2 disagreements where the decider picked the value;
  - one fixture labelled otherwise that the stub answers as the value at or above threshold;
  - macro recall exactly equal to the majority baseline.
- [ ] One fixture case ending in `no_answer` makes every value ineligible for that run.
- [ ] Paired observations recorded under an older declaration hash for the same field don't count toward the 30-pair condition.
- [ ] Escape-labelled cases count toward the 40-case total but the escape itself needs no 10-case minimum, and the majority baseline is 0 when the escape is the most frequent label.
- [ ] A boolean declaration requires 10 cases each for `true` and `false`, and its eligibility is reported for both values.
- [ ] A value with template mode `never` is evaluated, reported with a `(template: never)` note, and can be `eligible`.
- [ ] A completed fixture run exits 0 whether or not any value is eligible.

**Docs and doc_names**

- [ ] `docs/guides/cli-usage.md` gains a section for `koto decider report` covering every flag, the ledger default, the eligibility conditions, and the exit codes. (Anchor check: the file exists with per-command `###` sections such as `### config`; no `decider` section exists yet.)
- [ ] `plugins/koto-skills/skills/koto-user/references/command-reference.md` gains a `## koto decider report` entry. (Anchor check: the file exists with `## koto <verb>` headings such as `## koto decisions list`.)
- [ ] `cargo test --test doc_names` passes, and `tests/doc_names.allow` gains no record mentioning `decider`.
- [ ] Must deliver: a stable `koto decider report` surface (flag names, the eligibility condition wording, and the JSON field names `questions`, `values`, `confusion`, `rates`, `latency_ms`, `success_measures`, `flags`, `fixtures`) plus the two reference entries above, so the guide and skill guidance can document the promotion workflow against a verb that exists (required by <<ISSUE:7>>).

**Tests and hygiene**

- [ ] New integration tests live in `tests/decider_report.rs`, run against the `std::net` loopback stub from <<ISSUE:3>>, set `HOME` to a temp dir so no test touches the developer's real ledger, and opt in on their own `Command` only.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` pass.

**Dependencies**: <<ISSUE:5>>

**Type**: code

### Issue 7: docs(koto-skills): document decider declarations, opt-in, and the report

**Goal**: Document decider declarations, per-value modes, opt-in, and promotion through `koto decider report` in the `koto-author`, `koto-user`, and `koto-adhoc` skills and a new `docs/guides` page, then run the skill evals and record the results.

**Repo**: `tsukumogami/koto` (PR group `default`); **Complexity**: simple

**Acceptance Criteria**:

- [ ] `plugins/koto-skills/skills/koto-author/references/template-format.md` gains a decider section placed after `### The accepts block` that covers: the full `decider` block syntax for enum and boolean fields, every default, the four modes and what each does (`off`, `shadow`, `auto`, `never`), the escape (required on enums, refused on booleans, never listed in `values` or tested in a `when`), input sources and byte budgets, the one-question-per-state rule, and the floor rule with the `E-DECIDER-FLOOR` code and a worked example of a rejected `auto`.
- [ ] The koto-author material states which parts of a declaration feed the declaration hash (question, values, value and escape descriptions, escape, inputs) and which don't (modes, thresholds), and why that matters: promoting a value keeps its evidence, but rewording a description starts the evidence over.
- [ ] The koto-author material explains promotion: every value ships in `shadow` or `never`, a maintainer runs `koto decider report` (with `--fixtures`, `--template`, `--state`) against a golden fixture file, and moves a value to `auto` by editing its `mode` only when the report marks it promotion-eligible. It lists the eligibility bar in plain terms (at least 10 fixture cases per value and 40 in total, no false positives for that value, macro recall above the majority baseline, at least 30 paired ledger observations with at most one disagreement) and says the report never changes a mode.
- [ ] The koto-author material carries the authoring guidance from the design's Security Considerations: `auto` only on answers whose wrong outcome costs a reversible step; inputs carrying externally authored text are weaker promotion candidates; declare the narrowest context key that answers the question, since opted-in users send declared inputs to a third-party provider.
- [ ] `plugins/koto-skills/skills/koto-author/SKILL.md` points to the new material from its `## Reference material` section, and mentions that koto v0.12.2 ignores the block so declared templates keep working on older koto.
- [ ] `plugins/koto-skills/skills/koto-user/references/response-shapes.md` shows a declared field's `expects` entry with `description` and `value_descriptions`, states that fields without a declaration are unchanged, and states that a decider-settled state isn't returned (the next stop arrives with `advanced: true`).
- [ ] `plugins/koto-skills/skills/koto-user/SKILL.md` under `## Handling \`evidence_required\`` tells the agent to read `value_descriptions` when choosing a value, that the escape value is not a valid submission, and that the decider's answer is never shown and never overrides evidence the agent submits.
- [ ] `plugins/koto-skills/skills/koto-user/references/command-reference.md` documents `koto decider report` (every flag the shipped command accepts, the `--fixtures` opt-in requirement and non-zero exit without it) and adds the `[decider]` keys to its `## koto config` section, marking `api_key` as user-config-only and shown as `<set>`.
- [ ] `plugins/koto-skills/skills/koto-adhoc/SKILL.md` says ad hoc templates shouldn't declare `decider` blocks and points to koto-author for reused workflows.
- [ ] A new guide in `docs/guides/` (for example `docs/guides/decider-authoring.md`) covers, for a human reader: what a decider declaration is and when a decision qualifies (one closed-set value judged from stored inputs), a complete enum and a complete boolean example, modes and how the effective mode is the minimum of global, project, and template modes, opting in (config keys and env overrides, project config can only lower the mode, no key means not opted in, the same-layer key and endpoint rule, https-only endpoints except loopback), what leaves the machine when opted in, what's recorded (the `decider_consulted` event and the ledger path, no input content), and the promotion workflow through `koto decider report`.
- [ ] `docs/guides/cli-usage.md` `#### Config keys reference` table lists `decider.mode`, `decider.api_key`, `decider.endpoint`, and `decider.timeout_ms` with defaults and whether project config accepts each (only `mode`), and links to the new guide.
- [ ] Every `koto <verb>` and repo-relative path written in code font in the changed skills and guides resolves: `cargo test --test doc_names` passes without new entries in `tests/doc_names.allow`.
- [ ] koto-author and koto-user each gain at least one eval in their `evals/evals.json` exercising the new material (for example: author a template with a declared enum field that compiles and respects the floor; read a declared field's `value_descriptions` and submit a valid non-escape value). `scripts/check-evals-exist.sh` passes.
- [ ] Baseline pass rates are captured by running `scripts/run-evals.sh --all` against the skill content before these edits, then `scripts/run-evals.sh --all` is run after. For each of the three skills, the post-change pass rate on the evals that existed before the change is at or above the baseline, and the new evals pass with the skill.
- [ ] The koto pull request description includes an `## Eval Results` table in the format from koto's `CLAUDE.md` for all three skills, with baseline and post-change numbers.
- [ ] Tests pass (run project's test command)
- [ ] CI green

**Dependencies**: <<ISSUE:6>>

**Type**: docs

### Issue 8: ci(koto): verify decider templates and logs against koto v0.12.2

**Goal**: Add a `decider-compat-v0-12-2` job to koto's `validate.yml` that installs koto v0.12.2 through a pinned installer with a mandatory checksum, proves that enum and boolean `decider` declarations route identically on v0.12.2 and on the PR's build (with the escape value never surfacing), and proves that v0.12.2 can run `koto status` and `koto next` on a session log containing `decider_consulted`.

**Repo**: `tsukumogami/koto` (PR group `default`); **Complexity**: testable

**Acceptance Criteria**:

Job wiring and installation:

- [ ] `.github/workflows/validate.yml` has a job with id `decider-compat-v0-12-2` that runs on `ubuntu-latest`, and the job is listed in the `validate` job's `needs:` with a matching `failure` check in its "Check results" step.
- [ ] The job downloads `install.sh` from `https://raw.githubusercontent.com/tsukumogami/koto/<40-hex-sha>/install.sh` into a file, never from a branch name such as `main` and never through `curl ... | bash`. It checks the downloaded file's SHA-256 against a value recorded in the workflow before running it.
- [ ] Before running the installer, the job fails with an `::error::` message if `sha256sum` isn't on PATH. It doesn't rely on `install.sh`'s warning-only fallback.
- [ ] The installer runs with `--version=v0.12.2 --no-modify-path` and `KOTO_INSTALL_DIR` set to a directory under `$RUNNER_TEMP`. The job asserts that `"$KOTO_FLOOR_BIN" version` reports 0.12.2, and every v0.12.2 call uses that absolute path, so no other koto on PATH can stand in.
- [ ] The job builds the PR's koto (`cargo build --release`) and passes both binaries to `test/compat/decider-v0_12_2.sh` by absolute path (`KOTO_FLOOR_BIN` and `KOTO_NEW_BIN`).

Declaration compatibility (enum and boolean):

- [ ] `test/compat/fixtures/` holds a fixture template with one state declaring an enum field with a `decider` block (per-answer descriptions, an escape value that's a distinctive token not used anywhere else in the file, and at least one input), and one state declaring a boolean field with a `decider` block (answers `true` and `false`, no escape). The template compiles on the PR's build.
- [ ] The script makes a stripped copy with mikefarah `yq` v4 (`yq --front-matter=process 'del(.states[].accepts[]?.decider)'`), checks the `yq` version first and fails if it isn't v4, and asserts that the stripped copy differs from the original, so a no-op strip can't make the comparison pass.
- [ ] v0.12.2 compiles the declared and stripped templates successfully, and both print the same cache path.
- [ ] A scripted session drives both declared fields to a terminal state. It covers each enum value except the escape, and both `true` and `false` on the boolean, across one or more runs. For every tick it records `state`, `action`, and `advanced` from the `koto next` JSON via `jq`, then the final `koto status` state. The script runs this scenario three ways: v0.12.2 on the declared template, v0.12.2 on the stripped template, and the PR's build on the declared template with `KOTO_DECIDER=off`. Each run gets its own `HOME` and working directory. All three transcripts must be byte-identical, and the script fails if any transcript is empty.
- [ ] No raw `koto next` or `koto status` response from any of the three runs contains the escape token (checked by grepping the full JSON output, not only `expects.values`).
- [ ] Submitting the escape value as evidence fails on v0.12.2 and on the PR's build with the same non-zero exit code a value outside `values` gets.

Event-log forward compatibility:

- [ ] The script runs a local stub decider on loopback and has the PR's build produce a session whose log holds at least one `decider_consulted` event (for example a `shadow` consultation that falls back to `evidence_required`). The script asserts the event is present before handing the session to v0.12.2, so the check can't pass against a log that never held one.
- [ ] The script also produces a session in which the decider's answer was applied: the fixture's enum declares one value `auto`, its transition targets a non-terminal, gate-free state so the compile-time floor accepts it, and the stub returns that value above threshold. The script asserts the log holds an `evidence_submitted` event with `"source":"decider"`.
- [ ] Against each of those two sessions, with no decider environment variables set, v0.12.2 runs `koto status` and then `koto next`. Both exit 0, and neither output contains a parse, corruption, or template-hash error. On the applied session, v0.12.2's `koto status` reports the state the decider routed to.
- [ ] The stub makes no outbound connection and uses only a fixed test key. No repository secret is referenced by the job, so it runs on pull requests from forks.

Harness quality:

- [ ] `test/compat/decider-v0_12_2.sh` is executable, starts with `set -euo pipefail`, uses `mktemp -d` for every scratch directory and removes them on exit, and exits non-zero with a message naming the failed check. A local run with both binary paths set works on Linux and macOS.
- [ ] Deliberately breaking the fixture in the script's self-test mode, or by hand once during review (for example putting the escape token into the enum's `values`, or changing one `when` target in the stripped copy only), makes the script fail. The PR description records which mutation was tried and the failure it produced.

Downstream:

- [ ] Must deliver: a passing `decider-compat-v0-12-2` job on koto `main` before the koto release that shirabe's declarations wait on, proving that v0.12.2 ignores field-level enum and boolean `decider` blocks without changing transitions and reads logs holding `decider_consulted` and `source: "decider"` evidence (required by <<ISSUE:13>> in tsukumogami/shirabe, via the koto-release gate).

**Dependencies**: <<ISSUE:1>>, <<ISSUE:4>>

**Type**: code

### Issue 9: feat(execute): compute upstream drift facts before the rebase

**Goal**: Compute `/execute`'s upstream drift facts in a script before the rebase, so a run with no drift reaches `spawn_and_await` without asking the agent anything, and the drift question that remains is a gate-free, two-value state.

**Repo**: `tsukumogami/shirabe` (PR group `deterministic`); **Complexity**: testable

**Acceptance Criteria**:

Script:

- [ ] `skills/execute/scripts/drift-facts.sh` exists, is executable, runs under bash 3.2 without associative arrays, and implements steps 1-7 above, including the exit codes 0, 64, 65, 66, and 67.
- [ ] The script fetches `origin` itself and computes the base as `merge-base(<last commit touching the PLAN>, origin/main)`, or `merge-base(HEAD, origin/main)` for an untracked PLAN, all before any rebase.
- [ ] `drift_facts.json` is compact JSON whose first key is `route` (`none` or `judge`) and whose `schema` is `drift-facts/v1`. It carries `reasons`, `plan_doc`, `base`, `main_head`, `main_advanced`, `commits_since_base`, `referenced_paths`, `overlap` (each entry with `path`, `status`, `added`, `removed`), `deleted_referenced_paths`, and `truncated`.
- [ ] `drift_facts.json` and `plan_intent.md` are each at most 8192 bytes. `truncated: true` forces `route: judge`.
- [ ] `drift_facts.json` contains no commit subjects and no diff text.
- [ ] `plan_intent.md` is written before `drift_facts.json`, so if the facts key exists, the intent key does too. The script writes nothing to stdout.
- [ ] The route is `judge` when the resolved reference set holds nothing beyond the PLAN and its upstreams, so a PLAN that names no code is always asked about.
- [ ] The script's header comment says that `koto rewind` into `drift_facts` after `worktree_sync` has run computes against the already rebased branch and reads as "main has not advanced".

Template (`skills/execute/koto-templates/execute.md`):

- [ ] A new `drift_facts` state sits between `settled_branch_record` and `worktree_sync`, with the action, gates, accepts, and three transitions described above.
- [ ] `worktree_sync`'s `default_action` command is exactly `git rebase origin/main`, with no fetch. Its fallback text and details section no longer say it fetches.
- [ ] `worktree_sync` has a `drift_clear` gate, and exactly one of its transitions targets `spawn_and_await`: the one keyed on `gates.rebased_on_main.exit_code: 0` and `gates.drift_clear.matches: true`.
- [ ] `worktree_discipline_check` declares no gates. `impact.values` is exactly `[informational, intent-changing]`, and `impact` has a field `description`. The state holds no `decider` block, since that lands with the declarations.
- [ ] The `impact_classified` gate and every mention of `wip/work-on_{{PLAN_SLUG}}_impact.json` are gone from `execute.md` and `phase-2.5-worktree-discipline.md`.
- [ ] The `PLAN_SLUG` variable description, and the matching paragraph in `skills/execute/SKILL.md` (it currently says the `worktree_discipline_check` gate interpolates it), now name `settled_branch_record` and `drift_facts` as the states that use the variable.
- [ ] The `settled_branch_record` details text that says the passing path "advances to `worktree_discipline_check`" names the state that actually follows it.
- [ ] `skills/execute/koto-templates/execute.mermaid.md` is regenerated with `koto template export ... --format mermaid`, and the koto freshness job in `check-templates.yml` passes.
- [ ] The template compiles on koto v0.12.2, and `scripts/check-template-interpolation.sh` passes.

Behavior (end to end, on a fixture with a bare `origin`):

- [ ] When `origin/main` advanced only in paths the PLAN doesn't reference, one `status: override` tick at `orchestrator_setup` reaches `spawn_and_await` with no agent answer about drift, and `drift_facts.json` starts with `{"route":"none",`.
- [ ] When `origin/main` modified a referenced file, the run stops at `worktree_discipline_check`, and `drift_facts.json`'s `overlap` lists that path. Submitting `impact: informational` then reaches `spawn_and_await`.
- [ ] When `origin/main` deleted a referenced file, the run stops at `worktree_discipline_check`, and `deleted_referenced_paths` lists that path. Submitting `impact: intent-changing` with a `rationale` ends at `done_blocked`.
- [ ] After `drift_facts` and `worktree_sync` pass, `origin/main` is an ancestor of HEAD, which shows the fetch in `drift_facts` fed the rebase.

Tests and CI:

- [ ] `skills/execute/scripts/drift-facts_test.sh` covers: no advance, an advance outside the referenced paths, overlap, deletion, a rename, a prospective path mapped to its nearest existing ancestor, the root-ancestor drop, the no-references rule, truncation, `wip/` exclusion, rejected token shapes (`{{`, `$`, `*`, `://`), cross-repo `upstream:` skipping, a branch-only PLAN whose fork point sits behind `origin/main`, exit codes 64-67, and `route` coming first so the gate patterns match.
- [ ] `.github/workflows/check-execute-scripts.yml` runs `drift-facts_test.sh` on the Linux leg, and `scripts/check-bash-floor.sh` lists it in the `execute` suite so the macOS bash 3.2 leg runs it too.
- [ ] `settled-branch-record_test.sh` and `terminal-retention_test.sh` still pass. `terminal-retention_test.sh`'s `PAUSE_PATH` includes `drift_facts` so its walk still follows real edges.

Collateral:

- [ ] `skills/work-on/references/phases/phase-2.5-worktree-discipline.md` is rewritten to match: no fetch-and-rebase step, no "write the impact artifact" step, two classes instead of three, and the agent reads `drift_facts.json` and `plan_intent.md`. Its stale "per-child" wording becomes "once per run".
- [ ] `/execute` evals 5 and 6 in `skills/execute/evals/evals.json` (`parity-drift-gate-intent-changing-halts` and `parity-drift-gate-absorbs-non-intent-changing`) describe the new flow: the agent doesn't fetch or rebase, writes no impact file, and isn't asked at all when main didn't touch the PLAN's paths. `check-evals.yml` passes.
- [ ] No file under `skills/` still references `impact_classified` or a `wip/work-on_*_impact.json` artifact.

Downstream deliverables:

- [ ] Must deliver: the reshaped graph `settled_branch_record → drift_facts → worktree_sync → spawn_and_await | worktree_discipline_check`, drivable on koto v0.12.2 from a fixture repo with a bare `origin` and a branch-only PLAN, with stable exit codes, so the floor job's `execute` scenarios can script it: no drift, overlap answered `informational`, and a deleted reference answered `intent-changing` (required by <<ISSUE:12>>).
- [ ] Must deliver: a gate-free `worktree_discipline_check` whose `impact` field has exactly the values `informational` and `intent-changing` plus a field `description`, and the context keys `drift_facts.json` and `plan_intent.md`, both written before that state is reached and each at most 8192 bytes, in the `drift-facts/v1` shape above. The declaration's inputs and its golden fixtures are built from these (required by <<ISSUE:13>>).

**Dependencies**: None

**Type**: code

### Issue 10: fix(execute): route batch_outcome on the children-complete gate

**Goal**: Route `/execute`'s `spawn_and_await` state on the `batch_done` gate's `all_complete` and `all_success` fields (true to `pr_finalization`, false plus `needs_attention` to `escalate`) and stop asking the agent for `batch_outcome`.

**Repo**: `tsukumogami/shirabe` (PR group `deterministic`); **Complexity**: testable

**Acceptance Criteria**:

- [ ] `spawn_and_await` in `skills/execute/koto-templates/execute.md` no longer declares `batch_outcome` in `accepts`; `tasks` stays as the only accepted field, still `required: true`.
- [ ] `spawn_and_await` has exactly two transitions: `pr_finalization` when `gates.batch_done.all_complete: true` and `gates.batch_done.all_success: true`, and `escalate` when `gates.batch_done.all_complete: true`, `gates.batch_done.all_success: false`, and `gates.batch_done.needs_attention: true`. No `when` clause in the template mentions `batch_outcome`.
- [ ] `koto template compile skills/execute/koto-templates/execute.md` succeeds on koto v0.12.2 and no longer prints the W4 warning for `spawn_and_await`.
- [ ] The tick-2 directive in the `## spawn_and_await` body no longer tells the agent to inspect children or choose an outcome. The `OUTCOME=` assignment, the `batch_outcome` key in the submitted JSON, and the "Set `batch_outcome` to:" list are gone. Tick 2 is a bare `koto next {{SESSION_NAME}} --no-cleanup` with no `--with-data`, and the prose says the gate routes the batch.
- [ ] Tick 1's block is unchanged in shape: it still starts with `TMP=$(mktemp)`, injects `SHARED_BRANCH` from `{{SETTLED_BRANCH}}` and `PLUGIN_ROOT`, and ends with `rm -f "$TMP"`, so `skills/execute/scripts/settled-branch-record_test.sh` (Case 11 extracts that block) still passes.
- [ ] Every `koto next` line in `execute.md` still carries `--no-cleanup`, and `skills/execute/scripts/terminal-retention_test.sh` passes.
- [ ] `skills/execute/koto-templates/execute.mermaid.md` is regenerated so its two `spawn_and_await` edges show the gate conditions, and `scripts/validate-template-mermaid.sh` passes.
- [ ] `skills/execute/SKILL.md` (the "Including the two ticks in `spawn_and_await`" paragraph, around line 247) explains the chain in terms of the gate's attention route rather than `batch_outcome: needs_attention`.
- [ ] The execute evals that assert the agent submits `batch_outcome` are rewritten to assert gate routing: `parity-failed-issue-skips-dependents` (id 8), `execute-plan-all-success` (id 11), and `execute-plan-needs-attention` (id 12) in `skills/execute/evals/evals.json`. The two evals that assert "Both spawn_and_await ticks inject ..." (`single-pr-adopts-scoping-branch-routes-children-to-settled-branch`, id 26, and `single-pr-fresh-run-r7-parity-lands-on-impl-slug`, id 27) are updated to say tick 1 injects the branch, since tick 2 no longer submits `tasks`.
- [ ] The stubbed `spawn_and_await` directive in `skills/execute/evals/fixtures/scenarios/e2e-resume-plan/koto-next-work-on.json` no longer tells the agent to determine and submit `batch_outcome`. The same fixture under `skills/work-on/evals/fixtures/scenarios/e2e-resume-plan/` is updated to match.
- [ ] `git grep -n batch_outcome` in shirabe returns no hits outside `docs/designs/current/DESIGN-work-on-efficiency.md`, which is a historical design and stays as written.
- [ ] Must deliver: a `spawn_and_await` whose two routes are decided entirely by the `batch_done` gate, so a scripted run with every child reaching a success terminal reaches `pr_finalization`, and a run with one child in `done_blocked` (and its dependents skipped) reaches `escalate` and then `done_blocked`, in both cases with no evidence submitted at tick 2 (required by <<ISSUE:12>>, whose floor job scripts both batch scenarios on koto v0.12.2 and diffs their transitions).

**Dependencies**: None

**Type**: code

### Issue 11: feat(work-on): record changed paths and route issue type in one state

**Goal**: Ask `/work-on`'s issue-type question exactly once, in a new gate-free single-field `issue_type_routing` state that sits after a mechanical `changed_paths_record` state, and do it in a way that works on koto v0.12.2.

**Repo**: `tsukumogami/shirabe` (PR group `deterministic`); **Complexity**: testable

**Acceptance Criteria**:

Template shape (checked against the compiled JSON from `koto template compile skills/work-on/koto-templates/work-on.md`):

- [ ] `analysis.accepts` has no `issue_type` key, and `analysis` has a `default_action` whose command invokes `skills/work-on/scripts/record-changed-paths.sh --base` with `{{SESSION_NAME}}` as the session argument.
- [ ] `implementation.accepts` has no `issue_type` key, `implementation.gates` has no `has_commits` key, and exactly one `implementation` transition tests `implementation_status: complete`. That transition targets `changed_paths_record`, and its `when` is exactly `{implementation_status: complete, gates.on_feature_branch_impl.exit_code: 0}`.
- [ ] A new `changed_paths_record` state exists with a `default_action` invoking `record-changed-paths.sh --write` with `{{SESSION_NAME}}`, a `changed_paths_recorded` gate of type `context-exists` on key `changed_paths.txt`, `accepts.paths_status` with values exactly `[override, blocked]`, and three transitions: gate `exists: true` to `issue_type_routing`, `exists: false` with `paths_status: override` to `issue_type_routing`, and `exists: false` with `paths_status: blocked` to `done_blocked` with a `failure_reason` context assignment.
- [ ] A new `issue_type_routing` state exists whose `accepts` has exactly one key, `issue_type`: an enum with values exactly `[code, docs, task]`, `required: true`, and a non-empty `description`.
- [ ] `issue_type_routing` has exactly three transitions: `{issue_type: code}` to `scrutiny` with no `gates.*` key; `{issue_type: docs, gates.has_commits.exit_code: 0}` to `verification`; `{issue_type: task}` to `verification` with no `gates.*` key. `issue_type_routing` declares the `has_commits` gate with the same command string `implementation` carried before this change.
- [ ] `scrutiny` declares the `has_commits` gate (identical command), and its `passed` transition to `review` tests `gates.has_commits.exit_code: 0` in addition to its existing `scrutiny_outcome: passed` and `gates.scrutiny_results.exists: true`. Its `blocking_retry` and `blocking_escalate` transitions are unchanged.
- [ ] `research.accepts` has no `context_gathered` key, `research`'s single unconditional transition to `post_research_validation` is unchanged, and the `## research` directive no longer asks for `context_gathered`.
- [ ] The only `accepts` block in the template that contains `issue_type` is `issue_type_routing`'s, and no `accepts` block contains `context_gathered`.
- [ ] The `## issue_type_routing` directive explains `code`, `docs`, and `task`, and it still names the `{{ISSUE_TYPE}}` hint. The `## analysis` directive no longer has the "Issue type classification" paragraph, and the `## implementation` directive no longer tells the agent to submit `issue_type`. The "stalls with no error" warning is gone.
- [ ] `koto template compile` succeeds on koto v0.12.2 with no `warning:` lines. `bash scripts/check-template-directives.sh`, `bash scripts/check-template-interpolation.sh`, and `bash scripts/validate-template-mermaid.sh` pass. `work-on.mermaid.md` is regenerated and shows the new states and edges.
- [ ] The `unguarded-evidence` record for `research` in `scripts/check-template-directives.allow` still cites `tsukumogami/shirabe#333`, and its reason text no longer names `context_gathered`.

`record-changed-paths.sh`:

- [ ] `skills/work-on/scripts/record-changed-paths.sh` exists, is executable, and runs under bash 3.2.
- [ ] `--base <session>` writes context key `impl_base` holding `git rev-parse HEAD` when the key is absent, and leaves an existing `impl_base` untouched: running it again after a new commit leaves the stored SHA unchanged.
- [ ] `--write <session>` uses `impl_base` as the base when it's set. When it's unset, it uses `git merge-base HEAD origin/<default-branch>`, and falls back to local `main` when no origin default resolves. It writes `changed_paths.txt` with a `base: <sha>` line, a `commits: <n>` line, then one `git diff --name-status -M <base> HEAD` line per path. A rename appears as a single `R…` line, not a delete plus an add.
- [ ] Output is capped at 200 path lines and 8192 bytes total. When either cap cuts it, the last line is `... N more paths` with N the exact number omitted. The script never emits a classification such as `code` or `docs`.
- [ ] Exit codes: 0 when written; 64 when no base resolves; 67 when an argument is missing. On 64 or 67 it writes no `changed_paths.txt`.
- [ ] `skills/work-on/scripts/record-changed-paths_test.sh` covers: `impl_base` staying fixed across re-entry (second `--base` after a new commit); a `SHARED_BRANCH`-style history where sibling commits made before `--base` do not appear in `changed_paths.txt`; the merge-base fallback with `impl_base` unset; rename detection; truncation at both caps with the exact `... N more paths` count; and exit codes 64 and 67. Every run isolates `HOME` and builds its own git fixture.
- [ ] The new test runs in `.github/workflows/check-work-on-scripts.yml` on both legs and is listed in `scripts/check-bash-floor.sh`'s `work-on` suite, so the macOS bash 3.2 leg exercises it.

Engine-driven routing on koto v0.12.2 (in `record-changed-paths_test.sh` or an existing engine-backed suite, skipping only when koto is absent the way the other suites do):

- [ ] A run submitting `implementation_status: complete` with no `issue_type` advances through `changed_paths_record` with no agent evidence (gate passes) and stops at `issue_type_routing` asking for `issue_type`, with `changed_paths.txt` present in context.
- [ ] From `issue_type_routing`, `code` reaches `scrutiny` even when the branch has no commits over `main`; `docs` with commits reaches `verification`; `task` with no commits reaches `verification`; `docs` with no commits does not advance.
- [ ] At `scrutiny`, `scrutiny_outcome: passed` with `scrutiny_results.json` present but no commits over `main` does not reach `review`. With commits it does.
- [ ] With `changed_paths.txt` absent (script made to exit non-zero), `changed_paths_record` stops for evidence; `paths_status: override` reaches `issue_type_routing` and `paths_status: blocked` reaches `done_blocked`.
- [ ] Across a full plan-backed or issue-backed run, `issue_type` is requested exactly once.

Collateral:

- [ ] `skills/work-on/scripts/retry-clearing_test.sh` no longer submits `issue_type` on `analysis` or `implementation`, drives completions through the new states where it previously relied on the `implementation` edges, and passes.
- [ ] The work-on evals (`evals.json` around lines 350-371 and the `docs-routing`, `task-routing`, and `already-complete` scenario fixtures) describe `issue_type` being submitted at `issue_type_routing`, not with `implementation_status`, and `scripts/check-evals-exist.sh` still passes.
- [ ] `skills/work-on/SKILL.md`, `references/phases/phase-3-analysis.md`, `references/agent-instructions/phase-3-analysis.md`, and the `ISSUE_TYPE` variable description no longer tell the agent to submit `issue_type` during `analysis` or `implementation`. They name `issue_type_routing` as the one place it's asked.
- [ ] No `decider` key appears anywhere in `work-on.md` (declarations belong to <<ISSUE:13>>).

Downstream deliverables:

- [ ] Must deliver: the final state and edge shapes above (`changed_paths_record`, `issue_type_routing`, the `has_commits` move to `scrutiny`, and `research` without `context_gathered`), drivable on koto v0.12.2 with `koto next`/`--to`, so the floor job's `work-on` scenarios can walk `plan_validation: proceed` through `implementation: complete`, `changed_paths_record`, and `issue_type_routing` with each of `code`, `docs`, and `task`, plus a free-form run through `research` (required by <<ISSUE:12>>).
- [ ] Must deliver: `record-changed-paths.sh` with stable flags (`--base`, `--write`) and exit codes (0, 64, 67), which the floor job's git fixtures call through the template's `default_action` (required by <<ISSUE:12>>).
- [ ] Must deliver: `issue_type_routing` as a single-field state whose `issue_type` field carries a `description` and whose `code` and `task` routes test no gate. Also, the `changed_paths.txt` key is named by a `context-exists` gate (`changed_paths_recorded`) and `context.md` by the existing `context_artifact` gates, so a field-level `decider` block over `context.md` and `changed_paths.txt` resolves its inputs under R5 and clears the R6 floor for `code: shadow` (required by <<ISSUE:13>>).

**Dependencies**: None

**Type**: code

### Issue 12: ci(shirabe): check changed templates against the koto v0.12.2 floor

**Goal**: Add a shirabe CI job, `check-koto-floor.yml`, that installs koto v0.12.2 through koto's checksummed installer fetched from a pinned koto commit, strips `decider` blocks with `yq`, requires identical compiled output, and diffs per-tick transitions of scripted `/work-on` and `/execute` runs, and state v0.12.2 as shirabe's minimum koto version in the README.

**Repo**: `tsukumogami/shirabe` (PR group `deterministic`); **Complexity**: testable

**Acceptance Criteria**:

Workflow and installer

- [ ] `.github/workflows/check-koto-floor.yml` exists, runs on `ubuntu-latest` only, and triggers on `pull_request` for `skills/*/koto-templates/**`, `skills/*/scripts/**`, `scripts/check-koto-floor*.sh`, `scripts/koto-floor/**`, and `.github/workflows/check-koto-floor.yml`.
- [ ] The job runs `scripts/check-koto-floor.sh`, the same command a developer runs locally, and the workflow holds no scenario logic of its own.
- [ ] koto's `install.sh` is fetched from `https://raw.githubusercontent.com/tsukumogami/koto/<sha>/install.sh`, where `<sha>` is a full 40-character koto commit SHA held in one named variable. Neither the workflow nor the script fetches the installer from `main` or any other branch name.
- [ ] Before running the installer, the script fails with a non-zero exit and a message naming the missing tool when neither `sha256sum` nor `shasum` is on PATH. It also fails if the installer's output contains `Could not verify checksum`.
- [ ] The installer runs with `KOTO_INSTALL_DIR` set to a directory under `$RUNNER_TEMP` (or a `mktemp -d` directory locally), `--no-modify-path`, and `--version=v0.12.2`. The script then asserts that `"$KOTO_BIN" version` reports `0.12.2` and fails otherwise.
- [ ] Every koto invocation in `scripts/check-koto-floor.sh` and the scenario scripts goes through `$KOTO_BIN` by absolute path, never a bare `koto`, so a tsuku-installed `latest` on PATH can't stand in for the floor.
- [ ] The script checks for mikefarah `yq` v4 (`yq --version` output) and `jq`, and fails with a message naming the tool if either is missing or `yq` is a different major version or implementation.

Strip and compiled identity

- [ ] The script copies `skills/` into a temp tree with relative paths preserved, so `execute.md`'s `default_template: ../../work-on/koto-templates/work-on.md` still resolves in the copy.
- [ ] In the copy it runs `yq --front-matter=process -i 'del(.states[].accepts[]?.decider)'` on every `skills/*/koto-templates/*.md` except `*.mermaid.md`.
- [ ] For every template whose original contains a `decider` key, the script asserts the stripped copy differs from the original and contains no `decider` key. For a template with no `decider` key it records that the strip was a no-op, and every other check still runs on it.
- [ ] The script compiles each original and its stripped copy with `"$KOTO_BIN" template compile` and fails unless both succeed and print the same compiled cache path (the same compiled form and `template_hash`).
- [ ] The script collects every escape value (`.states[].accepts[].decider.escape.value`) from the original templates and fails if any of them appears among the `expects` field values in any `koto next` response recorded during the scripted runs. With no declarations the collected set is empty and the check still runs.

Scripted per-tick transition diffs

- [ ] Each scenario under `scripts/koto-floor/scenarios/` runs twice, once against the original tree and once against the stripped tree. Every run gets its own `HOME` and a fresh git fixture (a bare `origin` plus a clone), and no run touches the invoking user's `~/.koto`.
- [ ] Each run writes a transcript with one line per tick, `state<TAB>action<TAB>advanced` taken from the `koto next` JSON with `jq`, followed by the final `koto status` state. The script fails unless the two transcripts for a scenario are byte-identical, and on failure it prints a `diff` of the two.
- [ ] The `execute` scenarios cover: no drift, reaching `spawn_and_await` without stopping at `worktree_discipline_check`; overlap answered `informational`, reaching `spawn_and_await`; a deleted referenced path answered `intent-changing`, ending at `done_blocked` through `escalate_upstream_drift`; an all-success batch reaching `pr_finalization`; and a batch with a failing child reaching `escalate` and then `done_blocked`.
- [ ] The batch scenarios use per-task `template:` overrides that point at two trivial child templates in `scripts/koto-floor/fixtures/` (one success terminal, one `failure: true` terminal).
- [ ] The `work-on` scenarios cover: a plan-backed run through `plan_validation` answered `proceed`, then `implementation` answered `complete`, `changed_paths_record`, and `issue_type_routing` answered each of `code`, `docs`, and `task` (three runs); and a free-form run through `research` that submits no `context_gathered`.
- [ ] Each scenario asserts its expected final state, not only transcript equality, so a scenario that silently stops early fails even when both runs stop at the same place.
- [ ] States that need GitHub or network access are crossed with `koto next --to`, and both runs of a scenario use the same script, so the crossing doesn't affect the comparison.
- [ ] The scenarios make no network call: no `gh` invocation, no push to a remote other than the local bare `origin`, and no fetch from GitHub.

Self-test

- [ ] `scripts/koto-floor/fixtures/` holds one small template whose `accepts` field carries a `decider` block with an `escape` value. The script's strip, compile-identity, and escape-collection steps run against it, so that code is exercised while no shipped template declares anything.
- [ ] `scripts/check-koto-floor_test.sh` covers the helper logic without a network: the strip removes a `decider` block and leaves the rest of the front matter intact; the escape collector returns the fixture's escape value; a pair of differing transcripts makes the comparison fail with a diff; and a missing checksum tool, a missing `yq`, or a wrong `koto version` string each make the script fail. The workflow runs this test before the main script.
- [ ] The test and the script run under bash 3.2. `scripts/check-koto-floor_test.sh` is either added to a suite in the suite registry of `scripts/check-bash-floor.sh` (the `SUITES` list and `suite_scripts`) or named in that registry's comment among the suites it deliberately doesn't cover, with the reason.

README floor

- [ ] `README.md`'s prerequisites line for koto reads that `/work-on` and `/execute` need koto v0.12.2 or later and names `check-koto-floor.yml` as the job that checks that floor.
- [ ] The README paragraph that currently says no skill states a version floor is reworded so it no longer contradicts the new line: `requires.tsv` and the preflight still carry no version, and the one stated floor is the README's koto minimum, which CI rechecks.
- [ ] `skills/work-on/requires.tsv`, `skills/execute/requires.tsv`, `scripts/skill-preflight.sh`, and `references/tool-declaration-policy.md` are unchanged by this issue.

Integration

- [ ] The job passes on the PR that carries <<ISSUE:9>>, <<ISSUE:10>> and <<ISSUE:11>>, on the templates as those issues leave them.
- [ ] `validate-templates.yml`, `check-templates.yml` and `check-bash-floor.yml` keep passing.

Downstream

- [ ] Must deliver: the template list the strip and compile-identity legs cover and the escape collector's input, both derived from `skills/*/koto-templates/*.md` rather than a hardcoded list, so declarations added to `plan_validation`, `issue_type_routing` and `worktree_discipline_check` are checked with no change to the discovery code (required by <<ISSUE:13>>).
- [ ] Must deliver: the koto binary under test selected through one variable (`KOTO_BIN`, or the version passed to the install step), with the install step factored so a second binary can be installed beside the floor one, which <<ISSUE:13>> uses for its compile with the new koto release (required by <<ISSUE:13>>).
- [ ] Must deliver: scenario scripts that reach `plan_validation`, `issue_type_routing` and `worktree_discipline_check` and submit each declared value, so the transcript diff and escape-leak check cover those states once they carry `decider` blocks (required by <<ISSUE:13>>).

**Dependencies**: <<ISSUE:9>>, <<ISSUE:10>>, <<ISSUE:11>>

**Type**: code

### Issue 13: feat(shirabe): declare three decider-eligible decisions with golden fixtures

**Goal**: Declare shirabe's three decider-eligible decisions (`plan_validation.verdict`, `issue_type_routing.issue_type`, `worktree_discipline_check.impact`) with the PRD's modes, ship maintainer-labelled golden fixtures that meet the promotion minimums, add an offline `check-decider-declarations.sh`, and turn on the koto floor job's strip leg plus a compile against the koto release that validates the declarations.

**Repo**: `tsukumogami/shirabe` (PR group `declarations`); **Complexity**: testable

**Acceptance Criteria**:

Declarations:

- [ ] `plan_validation.verdict` in `skills/work-on/koto-templates/work-on.md` gains the field `description` and the `decider` block shown in Context, verbatim apart from whitespace: `proceed` with no mode (so `shadow`) and no threshold, `exit` with `mode: never`, escape `unclear`, inputs `context.md` as `outline_item` with `max_bytes: 12000` and var `PLAN_DOC` as `plan_path`. Its `values`, `required`, the optional `rationale` field, and both transitions are unchanged. (Anchor check at plan time: `plan_validation` exists at shirabe `main` 8a81481 with `verdict: [proceed, exit]` and `PLAN_DOC` is a declared variable; `context.md` is gated by `plan_context_injection.gates.context_artifact`.)
- [ ] `issue_type_routing.issue_type` in `work-on.md` gains a `decider` block: `code` shadow, `docs` never, `task` never, escape `unclear` with a description, inputs `context.md` labelled `issue_context` and `changed_paths.txt` labelled `changed_paths` (default budgets). Only the field gets new keys; the state's gates and transitions from <<ISSUE:11>> are unchanged, and the `code` transition still tests no `gates.*` key. (Anchor check at plan time: `issue_type_routing` and `changed_paths_record` are absent from shirabe `main` 8a81481 because <<ISSUE:11>> adds them. If either is still missing when this issue starts, the minimal state definition from the design's Decision 5 is included here as a sub-task rather than declaring against a state that doesn't exist.)
- [ ] `worktree_discipline_check.impact` in `skills/execute/koto-templates/execute.md` gains a `decider` block with `informational` and `intent-changing` both `never`, escape `unclear` with a description, and inputs `drift_facts.json` labelled `upstream_facts` and `plan_intent.md` labelled `plan_intent` (default budgets). The field's `values` are exactly `[informational, intent-changing]`, and the state carries no `gates` block. (Anchor check at plan time: on shirabe `main` 8a81481 the state still has the `impact_classified` gate and `values: [none, informational, intent-changing]`, and there's no `drift_facts` state; <<ISSUE:9>> reshapes it. If that reshape isn't present when this issue starts, the minimal reshape is included here as a sub-task.)
- [ ] No escape value (`unclear`) appears in any field's `values` or in any `when` clause of either template.
- [ ] No declared value in either template has `mode: auto`.
- [ ] `work-on.mermaid.md` and `execute.mermaid.md` are unchanged (declarations add no transitions), and `scripts/validate-template-mermaid.sh` passes.

Fixtures:

- [ ] The three fixture files exist at the paths in Context, and every line parses as a JSON object whose only keys are `id` (optional), `inputs`, and `expected`.
- [ ] `plan_validation.verdict` fixtures: at least 10 `proceed`, at least 10 `exit`, at least 40 lines in total. Every line's `inputs` has exactly the keys `outline_item` and `plan_path`, and every `outline_item` is at most 12000 bytes.
- [ ] `issue_type_routing.issue_type` fixtures: at least 10 each of `code`, `docs`, and `task`, at least 40 in total. Every line's `inputs` has exactly `issue_context` and `changed_paths`, each at most 8192 bytes. The set includes cases where every changed path ends in `.md` but the change is a koto template or skill edit labelled `code`.
- [ ] `worktree_discipline_check.impact` fixtures: at least 10 each of `informational` and `intent-changing`, at least 40 in total. Every line's `inputs` has exactly `upstream_facts` and `plan_intent`, each at most 8192 bytes. Every `upstream_facts` value is output of `drift-facts.sh` (it starts with `{"route":"judge",` and carries `"schema":"drift-facts/v1"`), never hand-typed JSON of another shape.
- [ ] Every `expected` is one of the field's declared values or its escape. Every `id`, where present, is unique within its file.
- [ ] Fixture content comes only from public sources or is synthetic. The PR description says which sources each file drew from and confirms that a maintainer assigned the labels.

Check script:

- [ ] `scripts/decider-declarations.tsv` lists one row per declared value (template path, state, field, value, mode), seven rows in total, matching the modes table in Context.
- [ ] `scripts/check-decider-declarations.sh` finds every `accepts` field with a `decider` block in `skills/*/koto-templates/*.md` (excluding `*.mermaid.md`) and fails, naming the template, state, field, and reason, when: the fixture file under the naming rule is missing; a line isn't valid JSON or has a key other than `id`, `inputs`, `expected`; an `expected` is neither a declared value nor the escape; any declared value has fewer than 10 cases or the file has fewer than 40; a line's input labels don't exactly equal the declared labels; an input exceeds its declared `max_bytes` (8192 when unset); a value's effective mode (`shadow` when unset) differs from its TSV row; a TSV row has no matching declared value, or a declared value has no TSV row.
- [ ] `scripts/check-decider-declarations_test.sh` covers each failure above with a temp-tree case, plus a passing case, and the passing case at exactly 10 per value and 40 total while 9 per value and 39 total each fail.
- [ ] The script makes no network call: it doesn't invoke `curl`, `wget`, `gh`, or any `koto` subcommand (in particular not `koto decider report`), and the test runs it with `KOTO_DECIDER_API_KEY` unset.
- [ ] The script and its test run under bash 3.2 (no associative arrays, no `mapfile`), and both are listed in the `templates` suite of `scripts/check-bash-floor.sh` beside the existing `check-*` scripts, with `yq` added to the floor image if the script needs it.
- [ ] `.github/workflows/check-templates.yml` gains a job that runs the test and then the script against the repo, with a pinned, version-checked mikefarah `yq` v4.

Compatibility and compile:

- [ ] `check-koto-floor.yml`'s strip leg (added as a no-op by <<ISSUE:12>>) is turned on. For each template containing a `decider` key, the `yq`-stripped copy differs from the original, and koto v0.12.2 compiles original and stripped copy to the same cache path.
- [ ] The floor job's scripted scenarios still produce byte-identical per-tick transcripts on both trees, now covering the declared states (`plan_validation: proceed`, `issue_type_routing` with each of `code`, `docs`, `task`, and the drift overlap and deletion cases), and no escape value appears in any `expects` values in any response.
- [ ] CI also installs the published koto release that contains the decider feature, pinned by version through koto's installer with the checksum verified, and compiles both declared templates with it successfully.
- [ ] That same leg proves the new koto enforces the rules: in a temp copy of `skills/`, setting `plan_validation.verdict`'s `exit` to `mode: auto` fails compilation with `E-DECIDER-FLOOR` (its route targets the `validation_exit` terminal), and the unmodified copy compiles.
- [ ] `validate-templates.yml` passes on the PR with tsuku's `latest` koto (the published release).
- [ ] All existing shirabe suites pass unchanged (`check-templates.yml`, `check-template-consistency.yml`, `check-work-on-scripts.yml`, `check-execute-scripts.yml`, `check-bash-floor.yml`).

**Dependencies**: <<ISSUE:9>>, <<ISSUE:11>>, <<ISSUE:12>> (and the `koto-release` gate: a published koto release whose tag contains the koto PR's merge commit)

**Type**: code

### Issue 14: test(decider): verify the Jev client and a decider workflow against the live API

**Goal**: Prove the koto decider works against the real Jev API, not just the stub: a feature-gated live test suite exercises the client and a small example workflow end to end, and a CI job with a repository secret runs it on trusted triggers so a koto release is never cut on a client that only agrees with the stub.

**Repo**: `tsukumogami/koto` (PR group `default`); **Complexity**: testable

**Acceptance Criteria**:

**Prerequisite (outside the code)**

- [ ] A `KOTO_DECIDER_API_KEY` GitHub Actions secret exists on `tsukumogami/koto`, holding a key created at `console.typesafe.ai`, and the maintainer has confirmed it answers a one-question request at `https://api.typesafe.ai/v1/systemone`. The PR description states the secret's owner and where it's rotated.

**Example workflow**

- [ ] A template at `test/fixtures/decider-live/decider-live.md` declares, in one state, one enum field (three values plus an escape; one value `auto` at threshold 0.8 on a non-terminal, gate-free route; the others `shadow` or `never`), and in a separate later state one boolean field with both values `auto`. Each field reads a context key the template gates on. It compiles with the new koto and passes the R5/R6 rules from <<ISSUE:1>>.
- [ ] Input fixtures beside it cover: an input whose answer is unambiguous for the `auto` value, an input that should land on another value, and an input designed to be ambiguous.

**Live test suite**

- [ ] The live tests sit behind a cargo feature (`decider-live-tests`), so a plain `cargo test` never compiles or runs them. With the feature on but no `KOTO_DECIDER_API_KEY` they fail with a message naming the missing variable rather than passing silently. `validate.yml`'s coverage job stops using `--all-features` and lists its features explicitly, leaving `decider-live-tests` out, and no job triggered by `pull_request` enables that feature.
- [ ] Client-level: one request with an enum question and a boolean question to the real endpoint returns answers that `src/decider/jev.rs` parses without a `malformed` or `mismatched` error. Every choice answer carries `probabilities` over all declared values plus the escape, summing to within 0.01 of 1. The recorded model string is non-empty and not `unknown`.
- [ ] If the live API's response shape differs from what <<ISSUE:3>> implemented (for example, `probabilities` missing or nested differently), the test fails with the observed shape, and this issue's PR updates `src/decider/jev.rs`, its stub fixtures, and the DESIGN's Decision 3 text to match the real API before merging.
- [ ] An invalid key produces `http_status` 401 handling: `koto next` returns the opted-out response, prints the fixed 401 warning once, and the key string never appears in stdout, stderr, the session log, or the ledger.
- [ ] Workflow end to end, with `HOME` set to a temp dir, and `KOTO_DECIDER=auto` and the secret key set explicitly on each spawned `koto` command (overriding the forced `off` from `.cargo/config.toml`): for every consultation, the run is applied if and only if every field's recorded outcome is `qualified`. When applied, the log shows `decider_consulted` with outcome `applied`, then `source: "decider"` evidence, then `transitioned`, with no `evidence_required` response. When not applied, the agent-facing response equals the `KOTO_DECIDER=off` response. Every consultation leaves exactly one `consulted` ledger record. The test does not assert which value the model picks, and fails, printing each field's recorded probabilities and confidence, if no consultation in the run was applied.
- [ ] If any consultation records `error_class` `timeout`, `connect`, or `http_status` with 429, 529 or another 5xx, the test fails with a message saying the provider was unavailable and naming the status, instead of failing a behavioral assertion.
- [ ] The same end-to-end run with `KOTO_DECIDER=shadow` never applies an answer and produces responses identical to `KOTO_DECIDER=off`.
- [ ] `koto decider report --fixtures` over the example's fixture file (a small set, below eligibility size) completes against the live API with exit 0 and prints per-value results, and reports the set as too small for eligibility rather than marking anything eligible.
- [ ] The suite counts its provider requests (`decider_consulted` events plus direct client calls plus fixture cases) and fails if a run exceeds 20. The example's fixture file has at most 6 cases.

**CI job**

- [ ] A workflow (for example `.github/workflows/decider-live.yml`) runs the live suite on `push` to `main`, on a nightly `schedule`, and on `workflow_dispatch`, and never on `pull_request` or `pull_request_target`, so fork PRs can't reach the secret.
- [ ] The job reads the key only from `secrets.KOTO_DECIDER_API_KEY`, passes it to the test process alone, and fails (not skips) when the secret is empty on its trusted triggers.
- [ ] `release.yml` gains a job that runs the live suite on the pushed tag with `secrets.KOTO_DECIDER_API_KEY` (tag pushes aren't reachable from forks), and the job that publishes the GitHub release declares `needs:` on it, so a tag whose live run fails produces no release.

**Dependencies**: <<ISSUE:3>>, <<ISSUE:4>>, <<ISSUE:6>>

**Type**: code

## Implementation Issues

### Milestone: Decisions koto can settle without the agent

| Issue | Dependencies | Complexity |
|-------|--------------|------------|
| [#1: feat(template): declare decider blocks on accepts fields](#issue-1-feattemplate-declare-decider-blocks-on-accepts-fields) | None | critical |
| ^_Repo: tsukumogami/koto \| Group: default_ | | |
| _Adds the field-level `decider` block to `src/template/`, the E-DECIDER-* compile rules and floor, the declaration hash, and `description`/`value_descriptions` in `expects`, leaving templates without a block byte-identical._ | | |
| [#2: feat(config): add opt-in decider configuration with hardened key handling](#issue-2-featconfig-add-opt-in-decider-configuration-with-hardened-key-handling) | None | critical |
| ^_Repo: tsukumogami/koto \| Group: default_ | | |
| _Adds the `[decider]` config table and env overrides, drops key/endpoint/timeout from project config at load, and defines `DeciderSettings::opted_in` as the only opt-in check every later caller uses._ | | |
| [#3: feat(decider): add provider-neutral decider types and the Jev client](#issue-3-featdecider-add-provider-neutral-decider-types-and-the-jev-client) | [#1](#issue-1-feattemplate-declare-decider-blocks-on-accepts-fields), [#2](#issue-2-featconfig-add-opt-in-decider-configuration-with-hardened-key-handling) | critical |
| ^_Repo: tsukumogami/koto \| Group: default_ | | |
| _Adds the pure `src/decider/` types, `build_request`, and `evaluate`, the Jev client over the already-linked attohttpc with a watchdog, and the `std::net` test stub, building clients only through issue 2's predicate._ | | |
| [#4: feat(engine): consult the decider when a state would stop for evidence](#issue-4-featengine-consult-the-decider-when-a-state-would-stop-for-evidence) | [#1](#issue-1-feattemplate-declare-decider-blocks-on-accepts-fields), [#3](#issue-3-featdecider-add-provider-neutral-decider-types-and-the-jev-client) | critical |
| ^_Repo: tsukumogami/koto \| Group: default_ | | |
| _Wires issue 1 and issue 3 into the advance loop: consults in the `NeedsEvidence` arm once per visit under `decider.lock`, applies answers only when all fields qualify and one clean route matches, and records `decider_consulted`._ | | |
| [#5: feat(decider): record consultations in a ledger that survives cleanup](#issue-5-featdecider-record-consultations-in-a-ledger-that-survives-cleanup) | [#4](#issue-4-featengine-consult-the-decider-when-a-state-would-stop-for-evidence) | testable |
| ^_Repo: tsukumogami/koto \| Group: default_ | | |
| _Mirrors issue 4's event into `~/.koto/_decider_ledger.jsonl` as `consulted` and `answered` records that survive session cleanup, and registers the event in the session-feed contract._ | | |
| [#6: feat(cli): add koto decider report with promotion eligibility and fixtures](#issue-6-featcli-add-koto-decider-report-with-promotion-eligibility-and-fixtures) | [#5](#issue-5-featdecider-record-consultations-in-a-ledger-that-survives-cleanup) | testable |
| ^_Repo: tsukumogami/koto \| Group: default_ | | |
| _Adds `koto decider report`, which joins issue 5's ledger into per-answer metrics and promotion eligibility and runs golden fixtures through issue 3's code behind issue 2's opt-in predicate._ | | |
| [#7: docs(koto-skills): document decider declarations, opt-in, and the report](#issue-7-docskoto-skills-document-decider-declarations-opt-in-and-the-report) | [#6](#issue-6-featcli-add-koto-decider-report-with-promotion-eligibility-and-fixtures) | simple |
| ^_Repo: tsukumogami/koto \| Group: default_ | | |
| _Documents declarations, modes, opt-in, and promotion in the three koto skills and a `docs/guides` page, with skill evals recorded._ | | |
| [#8: ci(koto): verify decider templates and logs against koto v0.12.2](#issue-8-cikoto-verify-decider-templates-and-logs-against-koto-v0122) | [#1](#issue-1-feattemplate-declare-decider-blocks-on-accepts-fields), [#4](#issue-4-featengine-consult-the-decider-when-a-state-would-stop-for-evidence) | testable |
| ^_Repo: tsukumogami/koto \| Group: default_ | | |
| _Proves in koto's CI that koto v0.12.2 compiles and routes declared templates identically and reads a log containing `decider_consulted`, before any release._ | | |
| [#9: feat(execute): compute upstream drift facts before the rebase](#issue-9-featexecute-compute-upstream-drift-facts-before-the-rebase) | None | testable |
| ^_Repo: tsukumogami/shirabe \| Group: deterministic_ | | |
| _Moves `/execute`'s drift judgment behind a `drift-facts.sh` state that runs before the rebase and routes no-overlap drift without asking, leaving a gate-free two-value question._ | | |
| [#10: fix(execute): route batch_outcome on the children-complete gate](#issue-10-fixexecute-route-batch_outcome-on-the-children-complete-gate) | None | testable |
| ^_Repo: tsukumogami/shirabe \| Group: deterministic_ | | |
| _Routes `/execute`'s `spawn_and_await` on the children-complete gate's `all_success`/`needs_attention` fields instead of an agent-typed `batch_outcome`._ | | |
| [#11: feat(work-on): record changed paths and route issue type in one state](#issue-11-featwork-on-record-changed-paths-and-route-issue-type-in-one-state) | None | testable |
| ^_Repo: tsukumogami/shirabe \| Group: deterministic_ | | |
| _Records `/work-on`'s changed paths in a mechanical state and routes issue type once in a new single-field state, dropping the double `issue_type` submission and `context_gathered`._ | | |
| [#12: ci(shirabe): check changed templates against the koto v0.12.2 floor](#issue-12-cishirabe-check-changed-templates-against-the-koto-v0122-floor) | [#9](#issue-9-featexecute-compute-upstream-drift-facts-before-the-rebase), [#10](#issue-10-fixexecute-route-batch_outcome-on-the-children-complete-gate), [#11](#issue-11-featwork-on-record-changed-paths-and-route-issue-type-in-one-state) | testable |
| ^_Repo: tsukumogami/shirabe \| Group: deterministic_ | | |
| _Adds shirabe's koto-floor CI job, which installs koto v0.12.2 through a pinned installer and diffs scripted transitions for the templates issue 9-issue 11 changed._ | | |
| [#13: feat(shirabe): declare three decider-eligible decisions with golden fixtures](#issue-13-featshirabe-declare-three-decider-eligible-decisions-with-golden-fixtures) | [#9](#issue-9-featexecute-compute-upstream-drift-facts-before-the-rebase), [#11](#issue-11-featwork-on-record-changed-paths-and-route-issue-type-in-one-state), [#12](#issue-12-cishirabe-check-changed-templates-against-the-koto-v0122-floor) | testable |
| ^_Repo: tsukumogami/shirabe \| Group: declarations_ | | |
| _Declares `plan_validation`, `issue_type_routing`, and `worktree_discipline_check` in shadow or never with golden fixtures, once a koto release containing issue 1-issue 8 exists._ | | |
| [#14: test(decider): verify the Jev client and a decider workflow against the live API](#issue-14-testdecider-verify-the-jev-client-and-a-decider-workflow-against-the-live-api) | [#3](#issue-3-featdecider-add-provider-neutral-decider-types-and-the-jev-client), [#4](#issue-4-featengine-consult-the-decider-when-a-state-would-stop-for-evidence), [#6](#issue-6-featcli-add-koto-decider-report-with-promotion-eligibility-and-fixtures) | testable |
| ^_Repo: tsukumogami/koto \| Group: default_ | | |
| _Runs issue 3's client and an example declared workflow against the real Jev API behind a cargo feature and a repository secret, on trusted CI triggers only, and makes a green run on the tagged commit a release requirement._ | | |
| ^_Gate: koto-release \| After: pr-koto-default \| Before: pr-shirabe-declarations_ | | |

## Dependency Graph

```mermaid
graph TD
    subgraph koto["tsukumogami/koto (pr-koto-default)"]
        I1["#1: declare decider blocks"]
        I2["#2: opt-in decider configuration"]
        I3["#3: decider types and Jev client"]
        I4["#4: consult the decider"]
        I5["#5: ledger"]
        I6["#6: koto decider report"]
        I7["#7: koto-skills docs"]
        I8["#8: v0.12.2 compatibility CI"]
        I14["#14: live Jev validation"]
    end
    G1{{"koto-release gate"}}
    subgraph shirabeA["tsukumogami/shirabe (pr-shirabe-deterministic)"]
        I9["#9: drift facts before rebase"]
        I10["#10: batch_outcome on gate"]
        I11["#11: changed paths + issue type"]
        I12["#12: v0.12.2 floor CI"]
    end
    subgraph shirabeB["tsukumogami/shirabe (pr-shirabe-declarations)"]
        I13["#13: three declarations + fixtures"]
    end

    I1 --> I3
    I2 --> I3
    I1 --> I4
    I3 --> I4
    I4 --> I5
    I5 --> I6
    I6 --> I7
    I1 --> I8
    I4 --> I8
    I8 --> G1
    I3 --> I14
    I4 --> I14
    I6 --> I14
    I14 --> G1
    I7 --> G1
    I9 --> I12
    I10 --> I12
    I11 --> I12
    I9 --> I13
    I11 --> I13
    I12 --> I13
    G1 --> I13

    classDef done fill:#c8e6c9
    classDef ready fill:#bbdefb
    classDef blocked fill:#fff9c4
    classDef needsDesign fill:#e1bee7
    classDef needsPrd fill:#b3e5fc
    classDef needsSpike fill:#ffcdd2
    classDef needsDecision fill:#d1c4e9
    classDef tracksDesign fill:#FFE0B2,stroke:#F57C00,color:#000
    classDef tracksPlan fill:#FFE0B2,stroke:#F57C00,color:#000

    class I1,I2,I9,I10,I11 ready
    class I3,I4,I5,I6,I7,I8,I12,I13,I14 blocked
```

**Legend**: Green = done, Blue = ready, Yellow = blocked

The hexagon node is the non-PR `koto-release` gate.

## Implementation Sequence

**Critical path:** #1 -> #3 -> #4 -> #5 -> #6 -> #7 -> koto release -> #13
(seven issues plus the release gate). #2 joins at #3.

**Parallelization:**

- Start immediately: #1 and #2 (koto), and #9, #10, #11 (shirabe).
- After #1 and #2: #3. After #3: #4. After #4: #5 and #8 in parallel.
- After #6: #14 (live validation), in parallel with #7.
- `pr-shirabe-deterministic` (#9-#12) proceeds entirely in parallel with the
  koto PR and can merge first.

**Merge order:** `pr-koto-default` and `pr-shirabe-deterministic` in either
order; then the `koto-release` gate (a published koto release whose tag
contains the koto PR's merge commit and has a green `decider-live` run); then `pr-shirabe-declarations`; the
coordination PR merges last.
