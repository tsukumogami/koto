# Lead: What operational constraints does an embedded network classifier impose on koto?

## Findings

### 1. koto already makes network calls, and already ships an HTTP client

koto is not a network-free binary. `Cargo.toml` carries `rust-s3 = { version = "0.37", default-features = false, features = ["sync-rustls-tls", "fail-on-err"] }` as a regular (non-optional) dependency, used by `src/session/cloud.rs` (`CloudBackend`). That backend is opt-in (`session.backend = "cloud"`), so a default install never touches the network, but the code is linked into every build.

`Cargo.lock` shows rust-s3's sync transport is `attohttpc 0.30.1`, built with `rustls`, `webpki-roots`, `serde` and `serde_json`. There is no `tokio`, `hyper`, `reqwest` or `ureq` in the tree. So a blocking HTTPS JSON POST to `https://api.typesafe.ai/v1/systemone` can be written on top of a client that is already compiled in, with an already-bundled root store. Marginal binary-size cost is close to zero if koto depends on `attohttpc` directly (pinned to the same version rust-s3 uses) instead of adding `reqwest`/`ureq`. The installed release binary is about 9.1 MB (`~/.tsuku/tools/current/koto`), with `lto = true` and `strip = true` in `[profile.release]`.

`docs/designs/current/DESIGN-config-and-cloud-sync.md` is the precedent for posture. It chose rust-s3 specifically because it "keeps koto synchronous" and rejected the AWS SDK for requiring an async runtime (Decision 3, "Rejected" paragraph; Decision 2 "Chosen"). It accepted "rust-s3 is a regular dependency, which increases binary size for all builds" and "S3 requests add latency to every mutating command (typically 50-200ms)" as negatives. A classifier that stays sync, reuses attohttpc, and is opt-in fits that posture exactly. An async stack would not.

### 2. Failure semantics precedent: network failure is non-fatal, local wins

`src/session/cloud.rs` module docs: "S3 failures are non-fatal: the local operation succeeds and a warning is printed to stderr." `sync_push_state` / `sync_pull_state` swallow errors with `eprintln!("warning: ...")`. There's also a strict variant (`strict_push_state`) used where ordering matters. The classifier should adopt the same shape: every failure (no key, DNS/connect error, timeout, 4xx, 5xx, malformed body, confidence below threshold, escape-hatch answer) degrades to "the agent decides," which is today's behavior. `koto next` must return exactly the response it would have returned with no classifier configured, plus at most an informational field. No retry loop inside `koto next`; one attempt with a hard timeout, because the agent fallback is always available and cheap.

One nuance: 401/403 (bad key) should be surfaced louder than a transient 5xx — a warning on stderr plus a recorded failure reason — because it's a persistent misconfiguration that silently turns the feature off.

### 3. Key discovery: an existing pattern to copy

`src/config/resolve.rs` layers built-in defaults < user config (`~/.koto/config.toml`) < project config < env vars. Cloud credentials come from `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` (lines 65-71) or `session.cloud.access_key`/`secret_key` in user config. `src/config/validate.rs` blocks credential keys from project config ("contains credentials and cannot be stored in project config (use user config or env vars instead)"), and `src/config/mod.rs::redact` masks them as `<set>` in `koto config` output.

A classifier key should follow this exactly:
- Env var: a vendor-neutral `KOTO_CLASSIFIER_API_KEY`, optionally also honoring the vendor's conventional name (e.g. `TYPESAFE_API_KEY`) the way cloud sync honors `AWS_*`.
- User config: `classifier.api_key`, added to the project-config blocklist in `validate.rs` and to `redact()`.
- Non-secret knobs (`classifier.endpoint`, `classifier.model`, `classifier.timeout_ms`, `classifier.enabled`) may live in project config. `classifier.endpoint` mirrors `session.cloud.endpoint` and is what makes mock-server testing possible.

Keeping the key out of the state log: the log (`EventPayload` in `src/engine/types.rs`) is JSONL and, under the cloud backend, is uploaded verbatim to S3. The key must never enter an event payload, an error string that becomes one, or a `--verbose` dump. HTTP error bodies from the vendor should be truncated and scrubbed before being recorded.

### 4. How the log records decisions today, and what replay means

`src/engine/types.rs::EventPayload` already has the right family of records:
- `GateEvaluated { state, gate, output, outcome, timestamp }` — appended every time a gate runs (`src/engine/advance.rs` ~883-925).
- `GateOverrideRecorded { state, gate, rationale, override_applied, actual_output, timestamp }` — sticky for the epoch; while active, the gate is not re-run (`advance.rs` ~839-880).
- `DecisionRecorded { state, decision }` — agent-recorded (`koto decisions record`).
- `DefaultActionExecuted` — records command output with a `truncated` flag.
- `Transitioned { from, to, condition_type, skip_if_matched }` — `condition_type` is a free string with values `auto`, `gate`, `command`, `skip_if`, `initial`, `manual` today.

Replay in koto is log derivation, not re-execution: `derive_state_from_log` (`src/engine/persistence.rs:775`) reads the last `Transitioned`/`DirectedTransition`/`Rewound`; evidence comes from `merge_epoch_evidence` (`advance.rs:1327`). Nothing re-runs gates when reading a log. But the advance loop itself does re-run command gates on every `koto next` tick while a state is blocked. A classifier cannot inherit that: re-asking the network on every tick costs money, adds latency to each tick, and can flip answers between ticks. The correct model is the override one: the classifier result is sticky for the epoch. If a classifier event already exists for (state, epoch, question-set hash), reuse it; never call again until the epoch changes (new visit via transition or rewind).

A classifier decision should be a new additive `EventPayload` variant (e.g. `ClassifierEvaluated`) recorded before the resulting `Transitioned`, carrying:
- `state`, `timestamp`
- `provider` and the `model` string exactly as returned (the Jev response echoes a dated build, e.g. `jev-1.13.0-20260917`, distinct from the requested `jev-1.13.0`)
- `questions_hash` (hash of the compiled question schema) and `state_hash` (SHA-256 of the exact pruned state payload sent)
- per-question answer: winning key / score / P(true), full `probabilities`, `confidence`
- `threshold` applied and the resulting `disposition` (`auto_applied`, `below_threshold`, `escape_hatch`, `error`, `skipped_no_key`)
- `latency_ms`, `usage` (input tokens, cost) for cost accounting
- on failure, a scrubbed `error` class (timeout, connect, http_status)

The pruned state itself should be stored so the decision is auditable and re-evaluable offline (shadow mode, concordance studies). Context content is already stored outside the log with only `hash` + `size` in `ContextAdded`; the same content-addressed approach fits here (write the payload into the session dir, log the hash). Storing the full payload inline would bloat the log and push agent-written text through cloud sync twice.

The resulting transition should be an ordinary `Transitioned` with a new `condition_type` value (e.g. `"classifier"`). Per `docs/STABILITY.md` ("EventPayload additive variants"), new variants don't bump `CURRENT_SCHEMA_VERSION`; older readers land the classifier event in `Unknown { type_name, raw_payload }` and still derive the current state correctly because state derivation only reads `Transitioned`. This is compatible with the Stage 1 frozen surface exercised by `koto-stability-tests/src/lib.rs`.

Also worth recording: when the classifier fell back, the agent's subsequent `EvidenceSubmitted` answer paired with the `ClassifierEvaluated` record is exactly the labeled data an evaluation lead would want.

### 5. Testing without the network

Current practice:
- Real-network tests are feature-gated: `tests/cloud_integration_test.rs` is `#![cfg(feature = "cloud-integration-tests")]` and skips unless `KOTO_TEST_S3_*` env vars are set (R2 in CI).
- Failure paths are tested by pointing the backend at an unreachable endpoint (`tests/batch_session_resolve_test.rs` ~155, ~620; `CloudBackend::with_parts` is `#[doc(hidden)] pub` for this).
- Gherkin functional tests (`test/functional/`, Go + godog, `features/*.feature`) build the release binary and exec it.

A classifier should get:
- A `Classifier` trait with an in-process fake for unit tests of the advance loop (deterministic answers, forced errors).
- `classifier.endpoint` override so integration tests (Rust `tests/`) and Gherkin tests can point koto at a local stub. In Go, `net/http/httptest` makes a stub server trivial; a step like `Given a classifier stub answering "route" with "fast" at confidence 0.93` fits the existing step style. Recorded JSON fixtures of Jev responses (including the 5xx, timeout, malformed and escape-hatch cases) drive the stub.
- An unreachable-endpoint test for the outage path, mirroring the cloud tests.
- A feature-gated live smoke test (`classifier-integration-tests`) that runs only when a real key is present.
- Hermeticity guard: test helpers like `koto_cmd` in `tests/cloud_integration_test.rs` only override `HOME`. A developer with `KOTO_CLASSIFIER_API_KEY` exported would make ordinary tests hit the real API. Test harnesses (Rust `koto_cmd` helpers and the Go suite) need to `env_remove` classifier variables, or koto needs a `KOTO_CLASSIFIER=off` kill switch that the suites set by default.

### 6. Latency and cost budget

Local `koto next` is a process spawn, a config read, a JSONL read/append, and whatever command gates the state runs. Cloud sync already adds 50-200 ms per mutating command, a cost the design explicitly accepted. Jev's published p50 is 110 ms, p95 340 ms (independent: 184 ms average). One classifier call per state visit, with all of that state's questions batched into a single request (Jev evaluates questions in parallel at no extra latency), lands in the same range as cloud sync. A hard client timeout around 1-2 s (configurable, default conservative) keeps the worst case bounded; on timeout the agent decides. The call should only happen on ticks where the state is actually waiting on a classifier-eligible decision, and never on read-only commands (`koto status`, `koto next` on a state with a sticky result, dashboard, export). Cost is negligible per call ($0.042 per 1M input tokens; the doc's example is $0.00002), but the stickiness rule is what keeps it bounded against polling loops.

### 7. Injection and the deterministic floor

The state payload will contain agent-written text: evidence fields (`EvidenceSubmitted.fields`), context artifacts added with `koto context add`, and command output. Jev's own documentation says it "remains vulnerable to semantic injection" and "should not be used as an isolated, unmonitored security gate for high-risk operations."

Hard rules that must stay deterministic:
- The classifier resolves only a transition choice among targets the template author explicitly marked classifier-eligible. It never evaluates or overrides a gate. Command, context and children-complete gates keep their deterministic evaluation, and a failing gate still blocks regardless of classifier output.
- It must never auto-advance into, or past, anything with `requires_confirmation: true` (`src/template/types.rs:293`) or a default action that has side effects, unless the template author opted that edge in explicitly. Safer default: compile-time rejection of classifier routing whose target state runs an action requiring confirmation.
- It cannot produce `GateOverrideRecorded`, `DirectedTransition` or `Rewound`; those remain agent/human-only.
- Classifier output enters the resolver in its own reserved namespace (as `gates.*` is reserved and `handle_next` rejects agent submissions carrying a top-level `gates` key, `advance.rs` ~955-966). The agent must not be able to submit evidence that impersonates a classifier result.
- Escape-hatch answers and below-threshold results always fall back to the agent.

### 8. Vendor neutrality (brief)

Put it behind a small sync trait (`fn evaluate(&self, state: &Value, questions: &QuestionSet, timeout) -> Result<Answers, ClassifierError>`) with Jev as the first implementation. Keep the template-facing vocabulary (choice / score / boolean question types, thresholds, escape hatches) vendor-neutral and let `provider`/`model` be config. Log the provider and model on every decision so mixed-provider histories stay interpretable. One implementation is enough for now; the trait mainly buys the test fake.

## Implications

- Dependency and binary-size objections mostly disappear: the HTTPS stack is already linked. The real constraints are staying synchronous, staying opt-in, and never letting network state change `koto next`'s contract when the classifier is absent or failing.
- Stickiness per epoch is the non-obvious design requirement. Treating the classifier like a command gate (re-run each tick) would be expensive, slow, and non-deterministic across ticks.
- Auditability wants a new additive event plus content-addressed storage of the pruned state; this is within the existing stability rules and needs no schema bump.
- Safety comes from where the classifier is allowed to act (only author-marked transition choices, never gates, never confirmation-guarded or destructive edges), not from trusting its confidence.

## Surprises

- koto already ships a TLS HTTP client (`attohttpc` via rust-s3), so "adding HTTP" is nearly free.
- Command gates re-run on every `koto next` tick; only overrides are sticky. A naive gate-style integration would re-call the network on every poll.
- The state log is synced verbatim to S3 under the cloud backend, which raises the bar for keeping both the API key and bulky agent text out of event payloads.
- Existing test helpers override `HOME` but not arbitrary env vars, so an exported real key would leak into the test suite unless explicitly scrubbed.

## Open Questions

- Should the classifier ever run on non-mutating reads, or only inside the mutating `koto next` path? (Recommendation: mutating path only.)
- Where exactly should the pruned state payload be stored (session dir, content-addressed), and is it synced to the cloud backend by default?
- Does a 401/403 disable the classifier for the rest of the session (recorded once) or retry each epoch?
- Does `koto next` output expose the auto-decision to the agent at all (the scope says the agent shouldn't see it), and if so, in what minimal form for debuggability?
- Is a per-project cost/rate cap needed, or is epoch stickiness plus low unit price enough?
- Should `koto rewind` invalidate a sticky classifier result (new epoch says yes), and should a human be able to "reject" a classifier decision the way overrides are recorded?

## Summary

koto already makes opt-in network calls (cloud sync via rust-s3, which pulls in a sync rustls `attohttpc` client), so a Jev client costs almost nothing in dependencies or binary size, provided it stays synchronous, opt-in, and follows cloud sync's rules: credentials come from env or user config only, stay blocked from project config and redacted, and any failure degrades silently to today's agent-decides behavior. For audit and replay, record each decision as a new additive `EventPayload` variant (provider, returned model build, question-schema hash, pruned-state hash, probabilities, confidence, threshold, disposition, latency and cost) followed by an ordinary `Transitioned` with a new `condition_type`, and keep it sticky for the epoch the way gate overrides are, so reading the log or polling never calls the network again. Testing needs a `Classifier` trait fake, a `classifier.endpoint` override for local stubs in the Rust and Gherkin suites, feature-gated live tests, and env scrubbing; safety needs the classifier limited to transition choices the template author marks as eligible, never gates, overrides, or edges with confirmation-guarded or destructive actions.
