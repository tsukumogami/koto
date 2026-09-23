---
schema: design/v1
status: Accepted
problem: |
  koto hands every unresolved branch to the agent as `evidence_required`, even
  when the decision is one closed-set value over inputs koto already stores.
  There is no way to declare such a decision, consult a typed decider without
  risking the run, record how the decider did against agents, or let it act
  once that record justifies it, and shirabe's templates bundle work with
  verdicts so none of their decisions are eligible as written.
decision: |
  A field-level `decider` block on `accepts` enum and boolean fields declares
  the question, per-answer descriptions, modes and thresholds, an escape, and
  the context or variable inputs. koto consults a provider-neutral decider
  (Jev first, over the HTTPS client koto already links) in the `NeedsEvidence`
  arm of the advance loop, once per state visit under a non-blocking lock,
  and applies an answer only when every declared field qualifies and exactly
  one conditional transition matches. Each consultation writes a
  `decider_consulted` event and an append-only ledger line that
  `koto decider report` joins with agent answers. shirabe moves its
  computable decisions into gates and declares three decisions in shadow.
rationale: |
  Answering the state's own evidence field keeps one routing surface and
  makes today's response the fallback for every failure, so an opted-out or
  unsure run is unchanged. Field-level keys are the only placement older koto
  ignores instead of rejecting. Deriving per-visit stickiness from the event
  log survives process exits without a second store. A synchronous client on
  an already-linked crate adds no dependency. Shadow-first with per-answer
  promotion, driven by a ledger that outlives session cleanup, answers the
  calibration risk with this project's own data rather than vendor claims.
upstream: docs/prds/PRD-jev-decision-offload.md
user_visible_surface: true
---

# DESIGN: Decisions koto can settle without the agent

## Status

Accepted

## Context and Problem Statement

A koto workflow stops at a state and returns `evidence_required` whenever
`resolve_transition` in `src/engine/advance.rs` returns `NeedsEvidence` on a
state with an `accepts` block. That covers three situations: no conditional
transition matched and there's no fallback; a gate failed; or the loop reached
the state by auto-advance with no fresh evidence. The agent then submits
evidence with `koto next --with-data`, and `when` clauses route on it by exact
equality. That's the only point where koto asks the agent to choose a branch.

Many of those choices are narrow. shirabe's `/work-on` asks on every
plan-backed issue whether the outline item is clear enough to code against
(`plan_validation.verdict`). It asks whether the issue is code, docs, or task
work (`issue_type`, submitted twice today). `/execute` asks whether upstream
drift changed the plan's intent (`worktree_discipline_check.impact`). Each is
one value out of two or three, judged from text koto already stores or could
store. Typed decision models answer that shape in a few hundred milliseconds
with calibrated probabilities, and the source PRD
(`docs/prds/PRD-jev-decision-offload.md`) asks koto to use one without
changing anything for users who haven't opted in.

The PRD's requirements fall into six groups, and the decisions below cite
them where they constrain a choice:

- declaration and compile rules (R1-R7), including the floor that refuses
  `auto` on terminal, confirmation-guarded, or gate-conditioned routes;
- the agent-facing contract (R8-R9);
- when and how often koto consults, and what it applies (R10-R15, R19);
- configuration, data egress, and the provider boundary (R16-R18, R20);
- the event, ledger, report, and promotion bar (R21-R23);
- shirabe's changes and compatibility (R24-R27), plus the non-functional
  bounds (R28-R31).

Three technical facts shape every decision. First, older koto binaries treat
new keys differently depending on where they sit: koto v0.12.2 rejects an
unknown key at state level with a generic parse error, but silently drops an
unknown key inside an `accepts` field. shirabe users run whatever koto they
have installed, so anything a template declares has to live inside the field,
and nothing an old koto might show the agent can be added to `values`.

Second, koto has no process that outlives a tick. Every `koto next` is a new
process, so anything "sticky" about a visit has to be recoverable from disk.
Third, koto's routing surface is exact-equality `when` clauses over evidence,
gate output, and variables. There's no threshold comparison and no counter, so
anything numeric has to be settled before routing.

## Decision Drivers

- **No change for users who haven't opted in.** The same responses, the same
  `template_hash`, and no network use. This rules out anything that alters
  compiled output or the response shape for undeclared fields.
- **Graceful degradation on old koto.** koto v0.12.2 silently drops unknown
  field-level keys and rejects unknown state-level keys. The whole
  declaration must live inside an `accepts` field.
- **One routing surface.** The agent path and the decider path must route
  through the same `when` clauses, so the fallback is today's behavior by
  construction.
- **Stickiness that survives processes.** Every `koto next` is a new process,
  and R12 needs the visit's result reused across them.
- **koto's existing precedents.** `default_action` (invisible success,
  fallback through existing response shapes, injected closures for tests) and
  cloud sync (synchronous rustls HTTP, credentials only from env or user
  config).
- **Stability contract.** Additive event variants and optional fields don't
  bump the schema version. `condition_type` values are fixed by the
  session-feed contract.
- **Testability without a network.** Every behavior above has to be provable
  in CI against a local stub, with no tokio or hyper pulled into the tree.
- **Evidence before trust.** Organic traffic produces one or two observations
  per question per week, almost all of them the common answer. Promotion has
  to rest on per-answer recall from fixtures plus paired shadow observations.

## Considered Options

### Decision 1: How a decision is declared and compiled

The declaration has to sit where koto v0.12.2 ignores it, carry per-answer
data for the agent and the decider alike, compile without disturbing
templates that don't use it, and give the compiler enough to enforce R5 and
R6 with messages naming the state, field, value, and rule.

Key assumptions:

- Boolean fields carry descriptions for `true` and `false` too, so
  `value_descriptions` has the same shape for both field types.
- A declaration needs at least one input.
- A context input resolves only against keys that a `context-exists` or
  `context-matches` gate in the template names. The compiler can't see what a
  `default_action` script writes (`ActionDecl` declares no outputs), so this
  narrows R4/R5's "a key the template writes or gates on" to the checkable
  half, and a state that produces an input gates on it.

#### Chosen: a field-level `decider` block with an `answers` map

```yaml
accepts:
  verdict:
    type: enum
    values: [proceed, exit]
    required: true
    description: Is the plan outline item clear and scoped enough to implement?
    decider:
      answers:
        proceed: {description: "Names a concrete change with checkable criteria.", threshold: 0.92}
        exit:    {description: "Vague, contradictory, or needs design first.", mode: never}
      escape:  {value: unclear, description: "Missing, truncated, or unjudgeable."}
      inputs:
        - {context: context.md, label: outline_item, max_bytes: 12000}
        - {var: PLAN_DOC, label: plan_path}
```

`answers` is keyed by value (`true` and `false` for booleans), and each entry
holds `description`, an optional `mode` (default `shadow`), and an optional
`threshold` (default 0.9). `escape` is required on enums and refused on
booleans. `inputs` entries name exactly one of `context` or `var` (captures
share the variable namespace), plus a unique `label` and an optional
`max_bytes` (default 8192). The source structs inside the block are strict
(`deny_unknown_fields`), so a typo fails on a current koto, while
`SourceFieldSchema` stays lenient, so an older koto drops the whole block.

The block compiles to `FieldSchema.decider: Option<FieldDecider>`, skipped
when `None`, with every default resolved at compile time. A template without
the block therefore serializes byte for byte as before and keeps its
`template_hash`. The declaration hash (R23) isn't stored. It's computed on
demand by `declaration_hash(&FieldDecider, question)` from an explicitly
destructured fingerprint: question, values, answer descriptions, escape, and
inputs, but not modes or thresholds. The fingerprint is destructured rather
than serialized-and-blanked, so adding a field to the struct is a compile
error until someone decides whether it belongs in the hash.

A new `validate_deciders` step in `CompiledTemplate::validate` runs ahead of
evidence routing and emits `E-DECIDER-*` errors for the R5 rules and the R6
floor (`E-DECIDER-FLOOR`). The floor walks every transition whose `when`
tests the field at an `auto` value and refuses a terminal target, a target
whose `default_action` has `requires_confirmation: true`, or a `when` that
also tests `gates.*`. `derive_expects` in `src/cli/next_types.rs` adds
`description` and `value_descriptions` only when the field has a decider.

#### Alternatives Considered

- **Per-answer data as a `values:` list of objects.** This needs its own
  duplicate check, is more verbose, and reuses the name `values`, which makes
  errors about "values" ambiguous. Rejected.
- **Three parallel maps (descriptions, modes, thresholds).** This scatters one
  answer across three places, each needing its own key-set check, and puts
  the promotion edit (mode) far from the evidence it rests on (description).
  Rejected.
- **A state-level `deciders:` map.** koto v0.12.2 rejects unknown state-level
  keys with a generic parse error, so every shirabe template using it would
  fail on older binaries. A field rename could also leave a dangling entry.
  Rejected.
- **Storing the declaration hash in the compiled JSON.** A stored derived
  value can disagree with its inputs, and it changes the cache format.
  Rejected.
- **Enforcing the floor at runtime only.** A runtime-only floor would let a
  template promising `auto` on a terminal route ship, compile, and pass review.
  The mistake would surface only as a silent fallback on some later run, and
  a reader of the template couldn't tell which answers can ever act. R6 asks
  the compiler to reject it where the author can fix it. The runtime keeps a
  second check (Decision 2) for routes the compiler can't see, but it doesn't
  replace the compile-time one.

### Decision 2: Where and how koto consults

The hook has to fire exactly when the state
would otherwise stop for evidence, reuse the visit's result across processes,
serialize concurrent ticks, and keep every rule the acceptance criteria test
in pure, unit-testable code.

Key assumptions:

- When the visit's evidence already holds a declared field, koto doesn't
  consult. Agent input wins over last-write-wins.
- A process that loses the lock race falls back without waiting.
- A tie for the top enum probability counts as the escape.

#### Chosen: consult in the `NeedsEvidence` arm, with sticky visits derived from the log

The hook sits in step 8 of `advance_until_stop`, at the exact point the loop
would return `EvidenceRequired`. Gates have already run there, so
`gates_failed` is known, `skip_if` and agent evidence have already had their
chance, and states reached by auto-advance hit the same arm. The existing
`advance_until_stop` becomes a one-line wrapper around a new
`advance_until_stop_with_decider(..., decider: Option<&mut dyn DeciderPort>)`
that passes `None`, so the 36 existing engine tests don't change.

A visit is consulted if and only if a `decider_consulted` event exists for
`(state, visit_seq)`, where `visit_seq` is the seq of the event that began the
visit. That's the boundary `entry_slice(Boundary::AnyEntry)` in
`src/engine/persistence.rs` already implements. The loop doesn't know the seq
of its own appends, so the CLI's port re-reads the local log to find it.

For concurrency, each consultation takes a non-blocking `flock` on a
dedicated per-session `decider.lock` file in the session directory, not on the
state file. The state file already carries `_batch_lock`, which `handle_next`
takes only when the tick *starts* on a batch-scoped state, and the blocking
lock in `append_event_idempotent`. Locking it a second time from the same
process fails whenever a tick auto-advances out of a batch-scoped state, so a
separate file is the only choice that never collides.

`consult()` returns a `VisitGuard` that owns the lock. The engine drops it
only after it has appended `decider_consulted`, any decider evidence, and the
`transitioned` event. Under the lock, the port re-reads the local log through
a new `SessionBackend::read_events_local`, because `CloudBackend::read_events`
pulls from S3 first and would put a network round trip inside the lock. The
re-read yields the visit seq through a new `visit_start_index` helper built on
`entry_slice`, so there's one boundary rule, and it also double-checks for a
prior consultation. A process that loses the race returns the opted-out
response and records nothing. On the cloud backend the appends still push to
S3 while the lock is held. That's accepted, because the cloud backend already
adds that latency to every mutating tick.

Evaluation is pure code in a new `src/engine/decider.rs`. The winning value is
the argmax (a tie counts as the escape), or R2's rule for booleans.
Confidence is the winning value's probability. Per-field outcomes are checked
in order: escape, below threshold, never, shadow, qualified. An answer applies
only if every field is qualified and `conditional_matches` finds exactly one
conditional transition. `conditional_matches` is extracted from
`resolve_transition`, because `Resolved` can mean the unconditional fallback.
The target must also not already be in `visited`, and the R6 floor must hold
for the transition actually matched. That last check closes the gap where a
`when` tests only `vars.*` or `evidence.*` keys and so escapes compile-time
analysis.

Applied evidence is an ordinary `EvidenceSubmitted` with a new optional
`source: "decider"` field (a string, so a future value can't break older
readers), followed by a normal `transitioned` whose `condition_type` stays
`auto`. A counter caps consultations at four per call, and `input_unavailable`
and error consultations count toward it.

#### Alternatives Considered

- **A new loop step before resolution.** To know the state "would otherwise"
  stop, it would have to repeat resolution. Without that, it would consult on
  ticks where agent evidence or a fallback would have resolved anyway, which
  breaks R11 and costs money. Rejected.
- **Consulting in `handle_next` and re-entering the loop.** Re-entry re-runs
  the stopping state's gates, duplicates `GateEvaluated` events, can apply
  evidence against a different gate result, and resets cycle detection and
  the chain limit on every call. Rejected.
- **A sidecar file or in-memory cache for stickiness.** A sidecar duplicates
  the R21 event, can disagree with the log after a crash, and needs its own
  cleanup and sync. Memory doesn't survive between `koto next` processes.
  Rejected.
- **Reusing the state-file `flock`.** It collides with `_batch_lock` when a
  tick starts on a batch-scoped state and auto-advances into a declared one,
  and nothing in the port would say when to release it. Rejected in favor of
  the dedicated `decider.lock` with an explicit guard.
- **Locking the whole tick when opted in.** That introduces a new
  `ConcurrentTick` error on ordinary workflows, which violates R15, and holds
  the lock while commands and polling actions run. Rejected. A blocking
  per-consultation lock stays in reserve if the loser's stale response ever
  matters.
- **A separate evidence event, or values only on `decider_consulted`.**
  Evidence merging, status, export, and older binaries wouldn't see the
  values, and it contradicts R14's single marked submission. Rejected.

### Decision 3: The provider interface, client, and configuration

The decider has to be provider-neutral, synchronous, bounded in time, built
on a client already in the dependency tree, and configured so that no
repository can switch it on, raise its mode, or redirect a user's key.

Key assumptions:

- Jev accepts the constant model name `jev-latest` and echoes the dated build
  in `model`.
- Cargo's `[env]` table with `force = true` reaches test binaries and their
  children. A guard test checks this on the first CI run.

#### Chosen: a sync `Decider` trait, a Jev client on attohttpc, and a layer-aware config merge

`src/decider/` holds:

- `mod.rs`: the `Decider` trait (`provider()`, `decide(&DecisionRequest) ->
  Result<DecisionResponse, DeciderError>`), neutral request and response
  types (`Choice` and `Proposition` questions, per-value probabilities or
  `p_true`), `ErrorClass` (`timeout`, `connect`, `http_status`, `malformed`,
  `mismatched`), and `build_decider()`, which returns `None` unless the user
  has opted in.
- `jev.rs`: the Jev client. One `POST` per consultation sends every field of
  the state as a question: enums become `choice` with criteria from the value
  and escape descriptions, and booleans become `noul`. The ordered `state`
  map holds the labelled inputs. Responses are validated (key sets, finite
  probabilities in [0, 1], sums within 0.01 of 1). The winner and confidence
  come from `probabilities`, never from Jev's entropy-based `confidence`
  field, which is kept only for evaluation. A missing model records
  `unknown`.
- `http.rs`: `post_json_with_deadline()` wraps attohttpc with connect, read,
  and whole-request timeouts. An outer watchdog thread waiting on
  `recv_timeout` covers DNS resolution, which attohttpc's own deadline
  doesn't. Redirects are off, the body is capped at 1 MiB, and the error
  classes carry no response body.

koto depends on `attohttpc` directly, pinned to the version and features
`rust-s3` already enables, so `cargo tree` gains no crate.

Configuration gains a `[decider]` table (`mode`, `api_key`, `endpoint`,
`timeout_ms`). `mode` stays a raw string resolved later, so a bad value warns
and becomes `off` instead of failing to parse the whole config file. A new
layer-aware `merge_decider` in `src/config/resolve.rs` copies only `mode` from
project config into a separate project-mode field, and drops `api_key`,
`endpoint`, and `timeout_ms` at load time. The existing `koto config set`
check alone doesn't cover a checked-in `.koto/config.toml`.

The env overrides are `KOTO_DECIDER`, `KOTO_DECIDER_API_KEY`, and
`KOTO_DECIDER_ENDPOINT`. `resolve_decider` computes the effective global mode
as the minimum of the global and project modes, plus any warnings.
Redaction and `koto config get` print `<set>` for the key, which is held in
an `ApiKey` type with no `Display` and a `Debug` that prints `<redacted>`.
`timeout_ms` is capped at 10000. Unknown or mistyped values inside `[decider]`
are parsed leniently and warned about, so a bad project table never fails the
whole config file.

Four endpoint rules close the ways a key could be sent somewhere unintended:

- **Same layer.** The key is sent only to an endpoint from the same layer: an
  env key with an env or default endpoint, a user-config key with a
  user-config or default endpoint. Otherwise the user isn't opted in and koto
  warns. This stops an agent, steered by injected text, from running
  `KOTO_DECIDER_ENDPOINT=<host> koto next` to redirect a key stored in user
  config.
- **Scheme.** The endpoint must be `https`. Plain `http` is allowed only for
  loopback, decided by IP literal (127.0.0.0/8, `::1`) or the exact name
  `localhost`, never by resolving a name. That exception exists for the test
  stub.
- **No userinfo.** An endpoint with userinfo is rejected, and warnings print
  only its scheme, host, and path.
- **No proxy for loopback.** Proxy environment variables are ignored for
  loopback hosts, because attohttpc honors them by default.

The model string is trimmed, stripped of control characters, and capped at
128 characters before it's recorded. `koto config set` writes
`~/.koto/config.toml` with mode 0600, tightening an existing file on every
write.

For test isolation, `.cargo/config.toml` sets `[env] KOTO_DECIDER = { value =
"off", force = true }`. Opted-in tests set the variable on their own
`Command`. The test stub is written on `std::net::TcpListener`.

#### Alternatives Considered

- **One request per field.** N round trips break the all-or-nothing rule's
  single answer set and the R28 latency budget. Rejected.
- **A trait shaped like Jev's payload** (`choice`, `score`, `noul`, `criteria`,
  and Jev's own `confidence`). It would be the least code today. But the engine
  and the event record would then speak one vendor's vocabulary, and every
  threshold would be compared against a confidence field whose meaning
  (distribution entropy) differs from the PRD's (the winner's probability). A
  second provider would force a rewrite of the engine side as well. Rejected,
  per R20.
- **ureq or reqwest.** Both add HTTP stacks, against R29 and the `cargo tree`
  check. Going through rust-s3 offers no general HTTP API, and hand-rolling
  HTTP over rustls is too much code for one POST. Rejected.
- **attohttpc timeouts alone.** Name resolution isn't covered, so the
  timeout-plus-250-ms bound can break. Rejected.
- **Adding `.env("KOTO_DECIDER","off")` to each of the ~25 test helpers.**
  Every new helper could forget it. Rejected in favor of the cargo `[env]`
  table, with this as the fallback if the guard test fails.
- **Mock-server dev-dependencies (httpmock, wiremock).** They pull tokio into
  `cargo tree`, which fails the dependency check. Rejected.

### Decision 4: The event, the ledger, and the report

Evidence for promotion has to outlive session cleanup, including child and
abandoned sessions that never reach a terminal tick. It has to pair each
consultation with the agent's answer and support per-value metrics without
storing input content.

Key assumptions:

- The declaration hash is per field.
- A fixture case that ends in a provider error makes every value ineligible
  for that run.

#### Chosen: one shared record type, an append-only two-kind ledger, and a `koto decider report` verb

`EventPayload::DeciderConsulted(DeciderConsultation)` is a newtype variant
over a struct in `src/decider/record.rs` that the ledger's `consulted` record
shares. It carries:

- `state`, `visit_seq`, `provider`, and `model`;
- `input_sha256`, the overall `outcome`, and `error_class`;
- `latency_ms` and `directive_bytes`;
- a `fields` map. Each field holds its `declaration_hash`, the effective
  `modes`, `probabilities` (4 dp), `winning`, `confidence`, `threshold`,
  `at_threshold` (the runtime's own comparison on unrounded numbers), and its
  per-field `outcome`.

It's registered at tier 2 in `docs/reference/session-feed.md`. The
probabilities stay inside the `fields` object, because `validate-feed` has no
float type. `outcome` gets an enum there and `error_class` doesn't.

The ledger lives at `~/.koto/_decider_ledger.jsonl`, next to the terminal
index. It's written through `append_bounded_line()`, a single-write O_APPEND
and fsync function extracted from `src/engine/terminal_index.rs`, which the
terminal index then calls too. Lines are capped at 4 KiB. An oversized line
drops its probabilities first and is skipped with a warning if still too
long. New files are created with mode 0600. Records are joined on the
session header's `session_id` UUID and `visit_seq`, because session names are
reused across runs.

Each consultation also records `endpoint_origin` (`default`, `user`, or
`env`). The report excludes non-default endpoints from promotion eligibility
unless `--include-custom-endpoints` is passed, so stub runs and redirected
runs can't count toward the bar. `directive_bytes` is computed by the CLI port,
which renders the state's directive and details the way `koto next` would.
Sessions whose header has no `session_id` (older logs) get ledger records
with a null id, and those records are excluded from pairing.

The `answered` record is written in `handle_next`'s `--with-data` path, right
after the `EvidenceSubmitted` append, when the current visit holds a
`decider_consulted` event whose outcome isn't `applied`. It carries only the
declared fields. Prune and session cleanup only remove session directories,
so the ledger survives with no change to prune. A test pins that, and
`docs/workspace-layout.md` lists the ledger as authoritative.

`koto decider report [--ledger <path>] [--state <s>] [--json]` prints a table
per declaration hash and value. It covers paired observations, a confusion
matrix, recall, coverage (well-formed answers as the denominator, with the
share of all consultations shown alongside), disagreements, fallback and
error rates, latency p50 and p95, and the success measures. Malformed lines
are skipped and counted. `--fixtures <path> --template <path> --state <s>
[--field <f>]` compiles the template to get the declaration and its current
hash. It then runs the JSONL cases (`{"id", "inputs": {label: text},
"expected"}`) through the same `build_request`, provider, and evaluation code
the runtime uses. The command requires opt-in and exits 2 without it, and it
writes no events or ledger lines. The report never changes a mode.

#### Alternatives Considered

- **Storing records in `_terminal_index.jsonl`.** That file is compacted,
  deduplicated by session id, and documented as derived and safe to delete.
  Rejected.
- **Emitting `answered` at the terminal tick, or pairing from session logs.**
  Children are always cleaned up, abandoned sessions never reach a terminal
  tick, and logs are deleted. The pairs would be lost. Rejected.
- **A composite `<session>/<state>/<seq>` join key.** Session names repeat
  across runs, and renames would orphan pairs. Rejected.
- **Carrying the hash in the fixture file.** The runner can't build requests
  from a hash, and the hash would go stale silently. Compiling the named
  template gives both. Rejected.
- **A separate fixture client.** It would measure code the runtime doesn't
  run. Rejected.

### Decision 5: How shirabe changes ship and stay compatible

shirabe's templates have to keep working on koto v0.12.2. The declared
questions need states whose inputs koto holds and whose routes carry no gates
(so R6 never blocks later promotion), and the deterministic fixes shouldn't
wait on a koto release.

Key assumptions:

- koto v0.12.2 compiles a template with `decider` blocks identically to the
  same template with the blocks stripped. A local check confirmed this.
- `ubuntu-latest` ships mikefarah `yq` v4.
- The koto feature ships as the next minor release.

#### Chosen: compute facts before asking, gate-free question states, and two shirabe PRs

**`/execute`.** A new mechanical `drift_facts` state runs before the rebase.
Its script `skills/execute/scripts/drift-facts.sh <session> <plan-doc>`
fetches and takes the base as the merge-base of the PLAN's last commit and
`origin/main`. The facts have to be computed before the rebase erases a
branch-only PLAN's fork point. The script collects referenced paths from the
PLAN's `upstream:`, `**Files**:` lines, and backticked path tokens, resolved
against the base tree at file or directory granularity. It then computes
overlap and deleted references against `origin/main`.

The script writes `plan_intent.md` (title, upstreams, Scope Summary, outline
goals) and then `drift_facts.json` (compact JSON with `route` first: `none` or
`judge`). Both stay within 8192 bytes, with `truncated` forcing `judge`. There
are no commit subjects or diff text, to keep third-party prose out of the
payload. `worktree_sync` becomes rebase-only and routes `none` straight to
`spawn_and_await` through a `context-matches` gate. The `impact_classified`
gate and its wip artifact go away. `worktree_discipline_check` keeps no gates
and two values, `informational` and `intent-changing`, both `never`, because
the script now owns `none`.

**`/work-on`.** Several states change:

- `analysis` records `impl_base` once.
- A `complete` implementation goes to a new mechanical `changed_paths_record`
  state, whose script writes `changed_paths.txt`.
- It then goes to a new single-field `issue_type_routing` state: `code` in
  shadow, `docs` and `task` never.
- The commit check moves from the `code` route to `scrutiny`, so `code` stays
  promotable. Only the `docs` route keeps a gate.
- `issue_type` leaves `analysis` and `implementation`, and `context_gathered`
  leaves `research`.
- `plan_validation` gets its declaration over `context.md`.

**`batch_outcome`.** `spawn_and_await` routes on `all_complete` plus
`all_success: true/false`, with `needs_attention` on the failure route. koto
treats two routes as exclusive only when they share a field with different
values, and it has accepted these fields since v0.12.0.

**Fixtures.** Fixtures live beside each template as
`<stem>.<state>.<field>.decider.jsonl`. `scripts/check-decider-declarations.sh`
checks the declared modes against R24's table and the fixture minimums.

**Compatibility.** A new shirabe workflow, `check-koto-floor.yml`, installs
koto v0.12.2 with koto's `install.sh --version=v0.12.2` into its own
directory. It strips `decider` blocks with `yq` and requires the same
compiled output. Then it runs scripted scenarios on both forms and diffs the
per-tick transitions, asserting that no escape value appears. koto's
`validate.yml` gains a matching job for the event-log read check, and that job
owns a boolean-declaration fixture, since every shirabe declaration is an enum.
Both jobs fetch `install.sh` from a pinned koto commit and fail if no checksum
tool is available. PR B's CI also compiles the declared templates with the new
koto, not only v0.12.2. `check-decider-declarations.sh` makes no network call,
so a fork's pull request can't spend a real key. shirabe's README states the
v0.12.2 floor.

**Ordering.** shirabe PR A (the deterministic moves plus the floor job) can
merge at any time. PR B (declarations and fixtures) merges after the koto
release, so a declaration is never shipped unvalidated.

#### Alternatives Considered

- **Computing drift facts after `worktree_sync`.** The rebase erases the fork
  point for PLANs that exist only on the branch, so every run would look like
  "main didn't move". Rejected.
- **Keeping `issue_type` on `implementation`, or copying today's gates onto
  every route of a new state.** The first bundles it with a generative field,
  so it's never eligible. The second trips R6 on the `code` route forever.
  Rejected.
- **Compile-only compatibility checking.** It proves v0.12.2 accepts the
  templates, but not that they route the same. A dropped block that changed
  which transitions exist, or an escape value leaking into `values`, would
  pass a compile check and misroute at runtime. R27 asks for identical
  transitions, so the job diffs scripted runs. Rejected.
- **One shirabe PR before the koto release.** Declarations would ship
  unvalidated, and a mistake would break every `koto init` once the release
  lands. **One PR after the release** holds back the deterministic fixes for
  no reason. Rejected.

## Decision Outcome

The five decisions form one path through koto. A template declares a decision
inside the `accepts` field it's about (Decision 1). Old koto drops that
declaration, and new koto compiles it, enforces the floor, and renders the
descriptions to the agent. When the loop reaches that field's state and would
stop for evidence, a port built only for opted-in users consults the decider
once for the visit, under a lock (Decision 2). The port uses a synchronous,
bounded, provider-neutral client that the config layer allows no repository to
steer (Decision 3). Whatever happens, one `decider_consulted` event and one
ledger record describe it. When the agent answers instead, an `answered`
record pairs with it, and the report turns those pairs and golden fixtures
into per-answer promotion eligibility (Decision 4). A template edit then
promotes the value. shirabe feeds the loop with states whose inputs are
computed facts and whose routes carry no gates, and ships them in an order
that never exposes an unvalidated declaration (Decision 5).

The fallback runs through every layer. No port means no consultation. A
consultation that doesn't fully qualify means today's response. A failed
ledger write means a warning. An old koto means the block is ignored. The only
way a run behaves differently from today is an answer that is opted in, in
`auto`, above threshold, not the escape, unambiguous in routing, and clear of
the floor.

## Solution Architecture

### Overview

The feature adds a decider module, a consultation arm in the advance loop, a
declaration in the template compiler, a config layer, an event and ledger, and
a report verb. It also changes three shirabe templates and adds two scripts
and a CI job. Nothing runs unless the user opts in, apart from the
`expects` descriptions and the compiler checks.

### Components

```
src/template/decider.rs        FieldDecider, DeciderAnswer, DeciderMode, DeciderEscape,
                               DeciderInput(Source), declaration_hash()
src/template/compile.rs        SourceDecider* structs (strict), lower_decider()
src/template/types.rs          FieldSchema.decider; validate_deciders() (E-DECIDER-*)
src/cli/next_types.rs          derive_expects: description + value_descriptions
pure (no I/O; the engine depends only on these):
src/decider/types.rs           Decider trait, DecisionRequest/Response, ErrorClass, ApiKey
src/decider/request.rs         build_request() from a declaration + assembled inputs
src/decider/evaluate.rs        winner(), per-field outcomes, evaluate()
src/decider/record.rs          DeciderConsultation, FieldConsultation (event + ledger)
I/O:
src/decider/jev.rs             JevDecider (wire mapping + response validation)
src/decider/http.rs            post_json_with_deadline() (attohttpc + watchdog,
                               no redirects, no proxy for loopback)
src/decider/ledger.rs          ledger_path(), append consulted/answered (0600)
src/decider/report.rs          ledger join, metrics, eligibility, fixture runner
src/decider/mod.rs             build_decider() (opt-in + same-layer endpoint rule)
src/engine/decider.rs          DeciderPort, VisitGuard, policy, effective_mode,
                               visit_start_index, prior_consultation
src/engine/advance.rs          advance_until_stop_with_decider, conditional_matches,
                               take_transition helper, NeedsEvidence arm
src/engine/jsonl_append.rs     append_bounded_line() (shared with terminal index)
src/engine/types.rs            EventPayload::DeciderConsulted; EvidenceSubmitted.source
src/session/*                  SessionBackend::read_events_local (cloud override)
src/cli/decider_port.rs        CliDeciderPort (decider.lock, local re-read, inputs,
                               directive_bytes, provider)
src/cli/mod.rs                 build port in handle_next; answered record; `decider report`
src/config/{mod,resolve,validate}.rs  [decider] table, merge_decider, resolve_decider
.cargo/config.toml             KOTO_DECIDER=off for tests
.github/workflows/validate.yml cargo tree check (no tokio/hyper/reqwest/ureq);
                               v0.12.2 log-read + boolean compat job
docs/reference/session-feed.md decider_consulted (tier 2)
plugins/koto-skills/*          koto-author, koto-user, koto-adhoc guidance + evals

shirabe:
skills/execute/scripts/drift-facts.sh           facts + plan intent
skills/work-on/scripts/record-changed-paths.sh  impl_base + changed paths
skills/*/koto-templates/*.md                    state splits, declarations
skills/*/koto-templates/*.decider.jsonl         golden fixtures
scripts/check-decider-declarations.sh           modes table + fixture minimums
.github/workflows/check-koto-floor.yml          v0.12.2 compatibility
```

### Key Interfaces

```rust
// src/engine/decider.rs
pub trait DeciderPort {
    fn policy(&self) -> &DeciderPolicy;
    fn consult(&mut self, req: &ConsultRequest<'_>) -> ConsultReply; // Skipped | Consulted
    // Consulted carries visit_seq, input hash, latency, model, directive_bytes,
    // endpoint_origin, the provider result, and a VisitGuard the engine drops
    // after its appends.
    fn recorded(&mut self, event: &EventPayload) {}                  // ledger hook
}

// src/decider/types.rs
pub trait Decider {
    fn provider(&self) -> &str;
    fn decide(&self, req: &DecisionRequest) -> Result<DecisionResponse, DeciderError>;
}
```

- Template: `accepts.<field>.decider { answers, escape?, inputs }`.
- Response: `expects.fields.<field>.description` and
  `.value_descriptions` for declared fields only.
- Event: `decider_consulted` (tier 2), and `evidence_submitted.source =
  "decider"`.
- Ledger lines: `{"kind":"consulted", ...DeciderConsultation, "session_id"}`
  and `{"kind":"answered","session_id","visit_seq","state","values"}`.
- CLI: `koto decider report [--ledger] [--state] [--json] [--fixtures
  --template --state --field]`.
- Config: `decider.mode | api_key | endpoint | timeout_ms`, plus
  `KOTO_DECIDER`, `KOTO_DECIDER_API_KEY`, and `KOTO_DECIDER_ENDPOINT`.

### Data Flow

1. `koto next` loads config. `resolve_decider` gives the effective global mode
   and key. If the user has opted in, `handle_next` builds `CliDeciderPort`,
   and otherwise passes `None`.
2. `advance_until_stop_with_decider` runs as today. At a `NeedsEvidence` stop
   on a declared state, `try_decider` checks eligibility: gates passed,
   declared fields present, the cap, some value mode not `off`, no agent
   evidence for those fields, and no prior consultation.
3. The port takes the per-session `decider.lock`, re-reads the local log, derives `visit_seq`, re-checks for a prior consultation, assembles
   inputs within their budgets, and calls the provider under the timeout.
4. The engine evaluates the answer and appends `decider_consulted`. The port's
   `recorded` hook writes the ledger `consulted` record. If the answer
   applies, the engine appends a `source: decider` evidence submission and
   takes the transition. Otherwise it returns today's `evidence_required`.
5. When the agent later submits evidence for that visit, `handle_next` appends
   `answered` to the ledger.
6. `koto decider report` reads the ledger (and optionally runs fixtures) and
   prints per-answer metrics and eligibility. A maintainer promotes a value by
   editing its `mode`.

## Implementation Approach

Delivery favors as few pull requests as the dependencies allow. That means
one koto PR, plus two shirabe PRs that the koto release separates. Inside the
koto PR the work lands as ordered commits.

### Phase 1: Declaration, compile rules, and agent-facing descriptions (koto)

The compiler and response changes, with no runtime behavior yet. Nothing
depends on it except Phase 3, which reads the compiled declaration.

Deliverables:

- `src/template/decider.rs`, the `compile.rs` source structs and lowering, and
  `validate_deciders` with `E-DECIDER-*` codes.
- `FieldSchema.decider` and `declaration_hash()`.
- The `derive_expects` additions.
- Tests: an unchanged `template_hash` and baselines, every R5 and floor case,
  threshold bounds, defaults, and input resolution.

### Phase 2: Configuration and provider client (koto)

Independent of Phase 1: it touches config and a new module only.

Deliverables:

- The `[decider]` config table, `merge_decider`, `resolve_decider`, env
  overrides, lenient parsing, the invalid-mode warning, and 0600 config
  writes.
- The `ApiKey` type, the same-layer endpoint rule, userinfo rejection, and
  loopback-by-literal.
- The pure `src/decider/` types, `request.rs`, and `evaluate.rs`; the Jev
  client; and `http.rs` with the watchdog, no redirects, and no proxy for
  loopback.
- The direct `attohttpc` dependency.
- `.cargo/config.toml` test isolation with its guard test, the `std::net`
  stub under `tests/support/`, and the `cargo tree` CI check.

### Phase 3: Consultation in the advance loop (koto)

Depends on Phase 1 (it reads the compiled declaration) and Phase 2 (it calls
the provider through the pure types).

Deliverables:

- The `DeciderConsulted` event variant and the `record.rs` structs, which the
  stickiness rule reads, plus `EvidenceSubmitted.source`.
- `src/engine/decider.rs`, `visit_start_index`, and `conditional_matches`
  extraction.
- The `take_transition` helper and the `NeedsEvidence` arm.
- `advance_until_stop_with_decider`, `read_events_local`, `CliDeciderPort`
  with the `decider.lock` guard, and `endpoint_origin`.
- Engine unit tests with a fake port, and stub integration tests: stickiness,
  rewind, self-loop, the concurrent pair, `koto status`, the batch-scoped
  state, the cap, and every failure kind.

### Phase 4: Event, ledger, and report (koto)

Depends on Phase 3, whose event the ledger mirrors.

Deliverables:

- The session-feed registration for `decider_consulted`.
- `append_bounded_line` extraction, the ledger, and the `answered` emission.
- `koto decider report` with metrics, eligibility boundaries, the
  custom-endpoint exclusion, and the fixture runner.
- The `workspace-layout.md` entry.

### Phase 5: Skills, docs, and compatibility check (koto)

Depends on Phases 1-4, because the skills and guides document their surface
and `doc_names` requires the verb to exist.

Deliverables:

- `koto-author`, `koto-user`, and `koto-adhoc` guidance, with evals run and
  their results recorded.
- `docs/guides` for opt-in and the report.
- The `validate.yml` job that reads a `decider_consulted` log with koto
  v0.12.2 and runs the boolean-declaration compatibility fixture, installing
  through the pinned installer.

### Phase 6: Deterministic shirabe moves (shirabe PR A, any time)

Independent of Phases 1-5: every change works on koto v0.12.2.

Deliverables:

- `drift-facts.sh`, the `drift_facts` and `worktree_sync` rework, and the
  gate-free `worktree_discipline_check`.
- `batch_outcome` routing.
- `record-changed-paths.sh`, `changed_paths_record`, the single
  `issue_type_routing` state, and the commit-check move.
- Removal of `context_gathered`.
- `check-koto-floor.yml` (pinned installer, mandatory checksum) and the
  README floor.

### Phase 7: shirabe declarations (shirabe PR B, after the koto release)

Depends on Phase 6 (the reshaped states) and on a koto release containing
Phases 1-5, so the declarations are validated by the koto that reads them.

Deliverables:

- The three `decider` blocks with R24's modes.
- The golden fixture files.
- `check-decider-declarations.sh`.
- The floor job extended to the declared templates, plus a compile with the
  new koto.

## Security Considerations

The decider sends session content to a third party and lets that party's
answer route a workflow, so it's reviewed against four concerns: what goes
out, who can steer it, what comes back, and what's stored.

**Trust baseline.** A koto template is already trusted code: its
`default_action` commands run arbitrary shell and its `when` clauses decide
all routing. The agent can already submit any declared evidence value. The
decider gives neither of them a capability they lack today. What it adds is a
third party, the provider, which sees declared inputs and, for values in
`auto`, picks among them.

**What leaves the machine.** For an opted-in user, a consultation sends the
question, the answer and escape descriptions, field and value names, and each
declared input up to its byte budget (8192 bytes by default), at most four
times per `koto next`. Inputs can only name context-store keys and template
variables; a declaration can't read environment variables or files. Nothing
else from the session is serialized. Opting in applies to every template the
user runs that carries a declaration, not only shirabe's, and the koto-author
guidance tells authors to declare the narrowest key that answers the
question. shirabe's `/execute` drift payload deliberately leaves out commit
subjects and diff text so upstream prose never reaches the provider.

**Who can turn it on or redirect it.** The API key comes only from
`KOTO_DECIDER_API_KEY` or `decider.api_key` in user config, and the endpoint
only from `KOTO_DECIDER_ENDPOINT` or user config. `merge_decider` drops
`api_key`, `endpoint`, and `timeout_ms` from a project's `.koto/config.toml`
at load time, so a checked-in config can't supply a key or point the user's
key at its own server. Project config can set `mode`, but the effective mode
is the minimum of the global and project modes, so a repository can only
lower it. The global mode defaults to `off`, and an unrecognized mode string
means `off`. The environment is part of the user's trust boundary: a
repository can only set koto's environment through mechanisms the user must
first trust (an agent harness's project settings, `.envrc`, a devcontainer),
and each of those already allows arbitrary command execution.

**Key handling.** `ApiKey` has no `Display` and its `Debug` prints
`<redacted>`; `koto config get` and config redaction print `<set>`. Error
details are built from fixed text and the error kind, never from a header, a
response body, or the URL. Redirects are disabled, so the bearer token can't
follow a 3xx to another host. A 401 or 403 prints a fixed message naming the
status, not the key. An endpoint containing userinfo is rejected at
validation, and warnings about the endpoint print only its scheme, host, and
path. `koto config set` creates `~/.koto/config.toml` with mode 0600 and
tightens an existing file to 0600 on every write.

**Same-layer key and endpoint.** The key is sent only to an endpoint from the
same configuration layer: an env key with an env or default endpoint, a
user-config key with a user-config or default endpoint. An agent can set
environment variables on its own commands, so without this rule injected text
could steer it into running `koto next` with `KOTO_DECIDER_ENDPOINT` pointing
elsewhere and receive the key stored in user config. Each consultation also
records its `endpoint_origin`, and the report leaves non-default endpoints out
of promotion eligibility unless asked. Stub runs and redirected runs can't
count toward the bar. Agreement with the agent isn't evidence against
injection, because both read the same inputs.

**Transport.** The endpoint must be `https`. Plain `http` is accepted only
when the host is an IP literal in 127.0.0.0/8 or `::1`, or exactly
`localhost`; koto never resolves another name to decide it's loopback. That
exception exists for the CI stub and shouldn't be used with a real key on a
shared machine, where another user could bind the port. Proxy environment
variables are ignored for loopback hosts. TLS uses rustls with bundled webpki
roots. Every request is bounded by connect, read, and whole-request timeouts
plus a watchdog for name resolution, and any failure returns the opted-out
response.

**What comes back.** The response body is capped at 1 MiB. The key sets must
match exactly what was asked, probabilities must be finite and in [0, 1], and
enum probabilities must sum to within 0.01 of 1; anything else is a
`malformed` or `mismatched` consultation that falls back. The reported model
string is trimmed, stripped of control characters, and capped at 128
characters before it's recorded. The winner comes from the probabilities, not
from the provider's own confidence field.

**Steering the answer.** Inputs can contain text written by the agent or by
third parties, such as an issue body, and that text can try to steer the
provider. In `shadow` and `never` an answer is never applied, so the worst
case is misleading evaluation data. In `auto`, a steered answer can route the
workflow, but only within these limits:

- the compiler refuses `auto` on any value whose transition reaches a
  terminal state, a state whose `default_action` requires confirmation, or a
  route that also tests a gate, and the runtime rechecks this on the
  transition it actually takes;
- every declared field on the state must qualify at its threshold, none may
  be the escape, and the combined evidence must match exactly one
  conditional transition whose target hasn't already been visited;
- promotion to `auto` is a per-answer template edit, reviewed like any other
  change, and shirabe ships every declared value in `shadow` or `never`.

The koto-author guidance says `auto` belongs on answers whose wrong outcome
costs a reversible step, and that inputs carrying externally authored text
make a weaker promotion candidate. The same limits bound a compromised or
malicious provider: it can choose among declared, non-floor answers but can't
name an undeclared value or write content into the log.

**What's stored.** The `decider_consulted` event and the ledger's `consulted`
record hold state names, the visit seq, provider and model, a SHA-256 of the
assembled input, probabilities, winning values, modes, outcomes, the error
class, latency, and the byte count of the directive the agent would have
received. `answered` records hold only the declared closed-set values. No
input content is stored in either place, so the cloud backend syncs nothing
beyond hashes and numbers for this feature. The ledger lives at
`~/.koto/_decider_ledger.jsonl`, is created with mode 0600, is never synced,
and is appended with single O_APPEND writes capped at 4 KiB per line.

**Dependencies.** koto depends on `attohttpc` directly at the version and
features `rust-s3` already enables, so the dependency tree gains no crate, and
a CI check on `cargo tree` keeps it that way. Mock-server crates that would
pull in an async runtime are excluded; the test stub uses `std::net`. The
shirabe compatibility job installs koto v0.12.2 through `install.sh`, which
verifies the release's SHA-256 checksum; the job fetches the installer from a
pinned koto commit and fails if no checksum tool is available. shirabe's
fixture check makes no network call, so a fork's pull request can't spend a
real key.

**Existing issue, filed separately.** A checked-in `.koto/config.toml` can
already set cloud-sync endpoint and credential keys, because the project-key
allowlist is enforced only by `koto config set`, not at load. This design fixes
the decider's keys at load time. The same fix for cloud sync is tracked as its
own issue, because it changes existing behavior outside this feature.

## Consequences

### Positive

- Users who haven't opted in see only extra descriptions in `expects`, which
  make today's prompts clearer.
- Evidence about the decider accumulates without risk: shadow is inert by
  test, and promotion is per answer and reviewable as a template diff.
- shirabe's deterministic moves remove silent misroutes (a mistyped
  `batch_outcome`, the dropped second `issue_type`) whether or not anyone
  ever enables a decider.
- The design adds no dependencies, needs no schema bump, and changes no
  existing response shape.

### Negative

- koto gains a subsystem-sized surface: a module, a loop arm, config, an
  event, a ledger, and a verb. Maintenance grows with it.
- Two code paths now exist for declared states (consulted and not). Tests
  cover both, but behavior differs between opted-in and opted-out users.
- The lock loser can get a stale `evidence_required` for a state the winner
  is settling.
- The ledger is per machine, so evidence from CI or other machines isn't
  merged.
- Routing `batch_outcome` on the gate means no agent inspects children at
  that tick, which makes the existing "`validation_exit` counts as success"
  bug easier to hit unseen.
- The drift prefilter can route to `none` when a PLAN names its code only in
  unquoted prose.

### Mitigations

- The module boundaries (template, decider, engine, cli port) keep each piece
  small and independently testable. Every behavior has a stub-based
  acceptance test in CI.
- The stale response is the same class of race two concurrent ticks already
  produce. A blocking per-consultation lock is the documented fallback.
- The report makes the per-machine limit explicit, and cross-machine
  aggregation is out of scope for now.
- `plan_validation.exit` stays `never`, and the `validation_exit`
  classification is recorded as a known limitation to fix separately.
- When the reference set is empty or truncated, the drift script routes to
  `judge`. The script's reference extraction is covered by fixtures, including
  a prose-only PLAN.
