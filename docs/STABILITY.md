# Stability Contract

This document pins the koto crate's public stability surface against
breaking changes. It is the operator- and integrator-facing artifact
referenced by `#[stable]` doc-comment blocks throughout the source.

The contract is the outcome of Decision 5 in
`docs/designs/current/DESIGN-request-store-converge.md` (lines 626-749). Bunki BK2
and other external substrates import `koto::engine::types::*` and the
four frozen `SessionBackend` methods listed below; this document is
the durable record of what they can rely on.

## Sections

1. [`CURRENT_SCHEMA_VERSION` bump protocol](#current_schema_version-bump-protocol)
2. [Frozen `SessionBackend` surface](#frozen-sessionbackend-surface)
3. [`StateFileHeader` additive evolution](#statefileheader-additive-evolution)
4. [`EventPayload` additive variants](#eventpayload-additive-variants)
5. [Session store read surface](#session-store-read-surface)

---

## `CURRENT_SCHEMA_VERSION` bump protocol

The constant lives at `koto::engine::types::CURRENT_SCHEMA_VERSION`.
It encodes the maximum wire-format version koto knows how to read; a
state file whose `schema_version` exceeds the constant rejects with
`EngineError::IncompatibleSchemaVersion`. That check runs in
`parse_header`, on line one, before any event is looked at.

The constant is still 1, and has been since the event-log format was
introduced. Its value moves under one of three rules.

### The rule: additive change does NOT bump the constant

An earlier version of this document said an additive change raises the
constant by 1. That was wrong in both directions — it contradicts what
the crate has actually done, and following it would break the very
consumers this contract protects.

Practice first: several additive `EventPayload` variants and several
additive `StateFileHeader` fields have shipped with the constant
unmoved. The worked examples further down this page say so themselves.

The reason is where the version gate sits. Because it fires at the
header line, a bumped constant makes an entire log unreadable to any
older build — including the events it could have read perfectly well,
and including the header fields it would have quietly defaulted to
`None`. So bumping on an additive change would convert the graceful
degradation the additive rules are designed around into a hard error:
instead of an old reader parsing a new log and landing unrecognized
events in `Unknown { type_name, raw_payload }`, it would refuse the
file outright with `IncompatibleSchemaVersion`. The additive rules and
a bump are alternatives, not companions.

The operative test is therefore not "did the wire format change" but
"can a reader that predates the change still make sense of the file":

- **Bump** when it cannot — a new *required* event type the reader must
  dispatch on, a required field removed from an existing event type, or
  a change to the event envelope keys (`seq`, `timestamp`, `type`,
  `payload`).
- **Leave it alone** when it can — any new `EventPayload` variant, any
  new `Option<T>` header field carrying `#[serde(default)]`, any change
  that an older reader either skips or degrades on.

### Patch releases — `CURRENT_SCHEMA_VERSION` rises by **0**

The vast majority of patch-level changes do NOT alter the wire format:
bugfixes, performance improvements, internal refactors, dependency
bumps, and additive doc updates all leave the constant untouched.

### Minor releases — `CURRENT_SCHEMA_VERSION` rises by **0**

Minor releases are permitted to introduce additive evolution to the
wire format under the rules in
[`StateFileHeader` additive evolution](#statefileheader-additive-evolution)
and [`EventPayload` additive variants](#eventpayload-additive-variants).
None of that evolution moves the constant, by the rule above. A minor
release that needed to move it would be carrying a change that isn't
additive, which belongs in a major release instead.

### Major releases — `CURRENT_SCHEMA_VERSION` rises by **1+**, with a 6-week deprecation window

A breaking change to the wire format (field removal, type-shape
change, rename, semantics shift) requires:

1. **6-week deprecation window** before the major release ships.
   During the window the breaking change is announced via release
   notes and the project changelog; bunki BK2 and other downstream
   consumers get warning of the upcoming break.
2. **Migration tool** ships in the same major release. The tool reads
   state files in the pre-break format and rewrites them in the
   post-break format. Tool is published as `koto migrate` or under a
   similar discoverable subcommand.
3. **`CURRENT_SCHEMA_VERSION` rises by at least 1** so post-break
   files are unambiguously identified.
4. **Pre-break reader retained** for at least one full release cycle
   after the major bump so operators who haven't migrated yet can
   still read their existing files. The retained reader emits a
   warn-level log naming the deprecation status.

Major bumps are explicitly costly under this protocol and are
expected to be rare — once or twice in the crate's lifetime.

---

## Frozen `SessionBackend` surface

Four methods on the `SessionBackend` trait are part of the **Stage 1
frozen surface**. Bunki BK2 imports these methods by name; signature
changes, removal, or rename require a 6-week deprecation window.

| Method | Why frozen |
|--------|-----------|
| `list` | Surfaces session metadata for the discovery scan and bunki's workspace inventory. |
| `read_events` | Single canonical read path for the JSONL event log. |
| `create` | Initializes the session directory; bunki spawns child sessions through this. |
| `init_state_file` | Atomic create-or-fail header + initial events; collision semantics are part of the contract. |

The remaining `SessionBackend` methods (`session_dir`, `exists`,
`cleanup`, `append_header`, `append_event`, `read_header`,
`rewrite_header`, `ensure_pushed`, `relocate`, `lock_state_file`) carry an
**additive-only doc note**. Their signatures may evolve in minor
releases; downstream consumers should not depend on their exact
shape.

**Adding new methods** to the trait is permitted in minor releases.
The additive-only policy applies to the trait as a whole: introducing
a new method is a non-breaking change as long as existing
implementations don't need to provide it (i.e., the new method has a
default implementation or the trait gains the method as part of an
extension trait).

Each frozen method carries a `# Stability: Stage 1 — Frozen` doc
block in `src/session/mod.rs` so `cargo doc --no-deps` readers see
the lockdown status prominently.

---

## `StateFileHeader` additive evolution

The header at `koto::engine::types::StateFileHeader` evolves under a
strict additive-only policy:

### Rules

1. **New fields are `Option<T>` only.** A required field cannot be
   added in a minor release — pre-bump state files lack the field
   and deserialization would fail.
2. **Every new field carries `#[serde(default, skip_serializing_if = "Option::is_none")]`.**
   The `default` attribute lets the deserializer fill `None` when the
   field is absent on disk; the `skip_serializing_if` keeps the
   serialized form bytes-identical for callers that don't use the
   field.
3. **No field is ever renamed in a minor release.** A rename is a
   breaking change and requires the major-bump deprecation window.
4. **No field is ever removed in a minor release.** Field removal
   breaks downstream consumers that read it; removal goes through
   the deprecation window.

### Worked examples

Issue 1 added the request-store fields under these rules:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub parent_workflow: Option<String>,

#[serde(default, skip_serializing_if = "Option::is_none")]
pub requested_by: Option<String>,

#[serde(default, skip_serializing_if = "Option::is_none")]
pub assignment_claim: Option<AssignmentClaim>,
```

Pre-Issue-1 state files lack these fields; the `default` attribute
fills `None` on read and the `skip_serializing_if` keeps the
on-disk representation unchanged for callers that didn't set them.

Issue 16 added `respawn_generation` under the same discipline:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub respawn_generation: Option<u32>,
```

Both additions shipped in a minor release without bumping
`CURRENT_SCHEMA_VERSION`.

---

## `EventPayload` additive variants

The enum at `koto::engine::types::EventPayload` admits new variants
in minor releases under these rules:

### Rules

1. **New variants are additive only.** Adding a variant in a minor
   release is permitted; removing or renaming an existing variant is
   not.
2. **The `Unknown` catch-all is the forward-compatibility hook.** Any
   variant the reader doesn't recognize falls into `Unknown { type_name,
   raw_payload }`. The reader preserves the original `type` string and
   raw JSON payload so a future version with the variant defined can
   re-parse the same file losslessly.
3. **No variant ever changes its serialized name.** The `type` field
   on disk pins the variant; renaming the variant breaks all
   pre-rename writers and goes through the major-bump deprecation
   window.

### How the catch-all works

The `EventPayload` enum uses `#[serde(untagged)]` for the body, but
the outer `Event` struct's custom `Deserialize` impl reads the `type`
field FIRST and matches it against the variant names. Any `type` value
not in the match table falls into:

```rust
other => EventPayload::Unknown {
    type_name: other.to_string(),
    raw_payload: payload_val.clone(),
},
```

Round-trip is byte-identical: `Event::Serialize` writes back the
original `type_name` and `raw_payload`. A koto 1.5 reader can parse a
koto 1.6 event log without losing data — the new variants survive
in the `Unknown` form and a future upgrade re-decodes them losslessly.

The `Unknown` arm has existed since v0.9.0. A build at v0.8.4 or
earlier has no catch-all and hard-errors on an unrecognized event
type, so that release is the back-compat floor for this guarantee.

### Worked example: the `request.` family

The six `request.`-prefixed variants shipped as an additive change
with `CURRENT_SCHEMA_VERSION` left at 1, and a test asserts it. An
older build reading a request-lifecycle log lands those events in
`Unknown` and keeps going; had the constant moved, the same build
would have rejected the file at its header line instead.

Request logs carry their own version constant,
`request_store::REQUEST_SCHEMA_VERSION`, deliberately independent of
this one. Coupling them would mean a session-format change made every
request log unreadable to the older build.

---

## Session store read surface

The `koto-session-reader` crate reads koto's session store for programs
other than koto: the dashboard's scan, one session's log
(`read_session_events`) and one context value (`read_context`), all in
`koto_session_reader::source`. This section promises exactly what that
reader reads from disk, so a program pinned to an older crate release
keeps reading a store a newer koto wrote. Everything else in the sessions
directory is koto's own and can change in any release;
`docs/workspace-layout.md` describes those files without promising them.

The crate and koto release together under one version, which the crate
exports as `KOTO_VERSION`. Each name below is defined once in the crate
and koto's writer imports it, and
`koto-stability-tests/tests/store_layout_literals.rs` pins each one as a
literal, so a rename fails CI and points here.

### Finding the store

`sessions_dir(None)` returns the sessions directory koto uses, found by
koto's own rule. The koto root is `$KOTO_HOME` when it is set and not
empty (a relative value is taken against the current directory), else
`~/.koto`. `Some(BackendKind::Cloud)` gives `<root>/sessions`, the cloud
backend's local mirror. `Some(BackendKind::Local)` and `None` give
`KOTO_SESSIONS_BASE` when it is set, else `<root>/sessions`.

`None` is exact inside every koto tick, and outside one except when the
cloud backend is configured and someone exported `KOTO_SESSIONS_BASE`,
a variable koto documents for testing, by hand. A caller that wants
certainty and has koto on `PATH` reads the `sessions` field of
`koto session dir --json`, which comes from the same function. Neither
reads anything but the environment, and neither creates a directory.

`LocalDirSource::new(dir)` opens a sessions directory for reading once
its layout marker passes.

### What is promised

1. **Sessions.** A session is a directory directly under the sessions
   directory, named for its id, that holds `koto-<id>.state.jsonl`, its
   state log. A directory whose name starts with `.koto-staging-` isn't
   a session, and neither is one whose name holds `~`. Any other entry
   without that file is ignored, so koto can add files and dot-directories
   freely.
2. **Context values.** The value of a key is the raw bytes of
   `<session dir>/ctx/<key>`, for any key `validate_context_key` accepts;
   the `/` separators of a hierarchical key are subdirectories.
   `manifest.json` and `manifest.lock` directly under `ctx/` are koto's
   own files and name no key.
3. **The leg pointer.** `<session dir>/request-leg.toml` is a TOML file
   with three string fields: `request_id`, `leg_name` and `bound_at`. A
   session is bound to a request leg exactly when its pointer parses.
4. **The template path rule.** A session's compiled template is the file
   its `workflow_initialized` event's `template_path` names: an absolute
   path as written, a relative one against the session directory. An
   absolute path that doesn't exist, whose file name is 64 lowercase hex
   digits plus `.json`, falls back to the file of that name in the
   session directory when one is there, which is how an imported session
   runs. From the template a reader uses only `states.<name>.terminal`,
   so that is the only part of the template's shape promised here.
5. **The events that set a context key.** Exactly four event types set or
   supersede a key: a `transitioned` event's `context_assignments` set
   keys, and `context_added`, `context_removed` and `context_cleared`
   supersede them. The crate lists them as `CONTEXT_KEY_EVENTS`.
6. **The layout marker.** `<sessions dir>/.koto-store-layout` holds the
   store's layout number as ASCII digits, surrounding whitespace allowed,
   in at most 64 bytes. A missing marker means layout 1, and nothing
   writes one yet. A reader reads layouts up to its
   `STORE_LAYOUT_VERSION`, which is 1.

### The additive rule

The promised surface changes only additively, and additive means an
older reader stays correct, not just that it still parses. A change an
older reader would read without an error and get wrong is breaking: a
fifth event type that sets a key, a second place a context value can
live, a template fallback an older reader would miss, or a terminal flag
that moves out of `states.<name>.terminal`. New files and dot-directories
beside the promised ones, and fields an older reader ignores, are
additive.

### Breaking changes and the marker

A breaking change to the promised surface goes through the major-release
rules of the
[`CURRENT_SCHEMA_VERSION` bump protocol](#current_schema_version-bump-protocol),
in this order:

1. A crate release that reads both the old layout and the new one ships
   first.
2. Only then does a koto release's migration change the store and write
   the marker, naming the new layout number.

A reader checks the marker before it lists anything: `check_layout`,
which `LocalDirSource::new` and the dashboard's scan both call. A marker
naming a layout above the reader's refuses the whole store with an error
that names the marker's path, the layout found and the layout supported,
the way a log with a higher `schema_version` is refused. A marker that is
a symlink, isn't a regular file, is over 64 bytes or doesn't parse as a
number refuses the store the same way rather than being guessed at.

### What a dependent can rely on

A reader built from a given crate release:

- reads every state log whose `schema_version` is at most its
  `CURRENT_SCHEMA_VERSION`;
- reads an event type it doesn't know as `EventPayload::Unknown`, keeping
  its type name and raw payload, and goes on;
- ignores header fields it doesn't know;
- marks a session whose log has a higher `schema_version` unreadable,
  that one session only, while every other session still reads.

The scan, `read_session_events` and `read_context` read state logs, leg
pointers, context values and the marker through one hardened read: it
never blocks on a FIFO or device, refuses a symlink at the file and a
path that resolves outside the store, and is capped at 512 MiB for a
state log, 64 KiB for a leg pointer, 16 MiB for a context value and 64
bytes for the marker. A refused state log or context value marks that
one session or value unreadable, and a refused leg pointer reads as no
pointer; the scan goes on. Where the reader gives a reason, it is a
fixed category and the file's path, never the file's bytes or a parser's
message, which can quote them.

### Reading a context value

`read_context(session_dir, key, events)` answers a key as
`koto context get` does, without taking a lock, recording a read or
writing anything. Given the session's events from `read_session_events`,
a key whose latest writer is a transition's assignment answers from the
log, whatever `ctx/` holds; otherwise the value is the file under `ctx/`.
It refuses a symlink at any component of the value's path, where
`koto context get` follows one; koto never writes one.

Two limits are part of the promise:

- A read that races koto writing a value's content can see a torn value,
  since that write takes no lock a reader sees. The next read corrects
  it, and `koto context get` can see the same.
- Under the cloud backend the read returns this host's local mirror of
  the value. That matches the dashboard's local rows, and can trail a
  write made on another host.

---

## Future Stage 2 surface (informational)

The request-store substrate-orchestration traits (`SubstrateSpawner`,
`SubstrateWaker`, `SubstrateRespawner`) are bunki BK2's swap-in
surface for dispatching agents, delivering wakes, and respawning
crashed coordinators. Their APIs are still in flux as Issues 15 and
16 land; **Stage 2 lockdown is deferred** to a future minor release.
External substrates may exercise these traits but should not depend
on their exact shape across minor releases until the Stage 2 lockdown
ships.

---

## Rollback to 0.9.x

The forward path is well-defined: `StateFileHeader` evolves additively
under serde-default `Option<T>` fields, and `EventPayload` accepts new
variants via the `Unknown` catch-all. A 0.9.x reader can parse a
0.10.0 state file without losing data — new fields and variants are
preserved in the on-disk representation and re-decode losslessly when
a newer reader picks them up again.

**Reverse compatibility (0.10.0 → 0.9.x) is NOT officially supported.**
Once a workspace has been touched by 0.10.0 it should remain on
0.10.0. The retained-reader policy in the major-bump protocol above
does not extend to minor-version downgrade.

If a downgrade is unavoidable (e.g., a 0.10.0 regression forces a
roll-back to 0.9.x for an emergency patch), the operator should
first remove the request-store derived files that 0.9.x has no
machinery to understand:

```bash
# Run these BEFORE downgrading the koto binary.
rm -f ~/.koto/_terminal_index.jsonl
rm -f ~/.koto/_terminal_index.compact.lock
rm -rf ~/.koto/coordinators/
find ~/.koto/sessions/ -name claim.lock -delete
```

What 0.9.x will silently ignore on read (additive 0.10.0 header fields
that `serde(default)` to `None` when absent and that 0.9.x doesn't
look at): `parent_workflow`, `requested_by`, `assignment_claim`,
`coordinator_of_record`, `needs_agent`, `role`, `inputs`,
`dispatch_epoch`, `respawn_generation`. These remain on disk after
the downgrade; 0.9.x just doesn't read them. They become live again
on re-upgrade.

**One-way doors** — fields that 0.9.x cannot "undo" even if it ignored
them on read:

- `assignment_claim` on a header — 0.9.x has no claim-honoring
  machinery, so a session whose dispatched agent is mid-flight when
  the downgrade happens loses claim coherence. The sidecar removal
  above masks the disk side of this, but the in-memory coordinator
  state on the 0.9.x process won't know to back off.
- `dispatch_epoch` writes on a `ChildDispatched` event — 0.9.x cannot
  match wake events to specific epochs (Fix 1 in the 0.10.0 polish
  pass), so a re-dispatched child will not surface a second wake on
  the 0.9.x side.
- `respawn_generation` on a header — 0.9.x has no F1 cold-restart
  re-priming machinery (Issue 16); a respawned child carries the
  generation forward but 0.9.x will treat it as a normal session.

If you've made progress on a dispatched child under 0.10.0 and then
downgrade to 0.9.x, the safest path is to manually `koto session
cleanup` the affected children before the downgrade so the parent's
coordinator state on 0.9.x doesn't depend on request-store-specific
fields it can't interpret.

**Recommendation:** pin 0.10.0 once upgraded. If a regression is
discovered, file an issue against the 0.10.0 patch line rather than
attempting a downgrade.

---

## Operator-facing references

- `docs/workspace-layout.md` — operator catalog of request-store derived files
  (`_terminal_index.jsonl`, scan cursors, compaction lock, claim
  sidecars) and their safe-deletion semantics.
- `docs/designs/current/DESIGN-request-store-converge.md` — full request-store design,
  Decision 5 (this contract's source of authority).
