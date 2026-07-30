# Security Review: orphaned-session-detection

## Dimension Analysis

### External Artifact Handling

**Applies:** No

The design does not download, execute, or otherwise process any artifact
originating outside the local machine. The only input to the new
`check_template_source_dir` helper is `header.template_source_dir`, a path
string that koto itself wrote into the session's own state file at `koto
init` time (`src/engine/persistence.rs`, `StateFileHeader`). The check
performs a single `Path::exists()` syscall against that path and otherwise
just moves data (a `PathBuf`, a `bool`, an `Option<String>`) between Rust
structs and JSON fields. There is no parsing of untrusted file content
beyond the header itself (already-existing, already-trusted parsing via
`persistence::read_header`/`parse_header`), no network fetch, no shelling
out, and no execution of anything found at `template_source_dir` -- only a
presence check. This dimension does not apply.

### Permission Scope

**Applies:** Yes, narrowly -- no material new risk found.

The one genuine permission-scope change is `koto init`'s collision path
gaining new I/O: it now opens and reads the *colliding session's* header
file (`src/cli/mod.rs:1682-1691` and the `SpawnErrorKind::Collision`
handler at `~1707-1716`), something the pre-check does not do today (today
it's a pure `backend.exists(name)` filesystem-presence test with no file
open). Verified in `src/session/local.rs`: sessions live under
`~/.koto/sessions/<id>/`, and `ensure_koto_root` explicitly sets `0o700` on
the `~/.koto` root (`src/session/local.rs:842`) and `0o600` on state files
(`src/session/local.rs:271-272`). That means both the "attacker's" and the
"victim's" sessions live under the *same* user's home directory with
restrictive permissions -- there is no cross-user boundary for this new
read to cross. A process that can invoke `koto init` in a given user's
environment already has filesystem access to that user's entire
`~/.koto/sessions/` tree (same UID), so reading one more header there is
not a privilege escalation; it's strictly within the existing trust
boundary of "whoever can run `koto` as this user can already read every
session's raw state file directly."

The `Path::exists()` check itself (both the existing scheduler path and
the three new call sites) probes an arbitrary filesystem path recorded in
`template_source_dir` -- which could point anywhere the user who ran
`koto init` had access to at creation time. This is not new: the batch
scheduler's `path_resolution.rs` already performs the identical probe
today. The new call sites add no new *kind* of filesystem access, only
new call frequency of a pre-existing operation type. No escalation risk
identified.

### Supply Chain or Dependency Trust

**Applies:** No

No new external dependencies, crates, downloaded binaries, or third-party
artifacts are introduced. The design is a pure refactor-and-extend within
existing first-party modules (`src/engine/`, `src/session/`, `src/cli/`).
This dimension does not apply.

### Data Exposure

**Applies:** Yes -- assessed as consistent with existing exposure, not a
new concern.

**Path disclosure.** The design's example JSON (`"path":
"/home/user/repo-that-was-deleted"`) does put an absolute local filesystem
path into CLI/JSON output. Two things bound this: (1) the path is not new
information -- `template_source_dir` is already stored, in plaintext, in
the session's own `0o600` state file, and the design doc itself notes
today's only workaround is "manually open[ing] the colliding session's raw
state JSONL" to read this exact value by hand; this design surfaces
through a formatted CLI field a value the same local actor could already
read directly. (2) koto is a single-user local CLI, not a multi-tenant
service: session storage is confined to `~/.koto/sessions/` under a
`0o700`-permissioned root owned by one OS user (verified in
`src/session/local.rs`). There is no server boundary, no other-user
viewer, and no network transmission implied anywhere in this design --
output goes to the same terminal/stdout the invoking user already
controls. Whether a home-directory path leaking into a *log aggregator* or
*shared CI output* is a concern is a pre-existing property of every koto
command that already echoes paths (status output, error messages,
`submitter_cwd` fallback text) -- this design does not change that
posture, it extends an existing one to three more call sites.

**`machine_id` exposure.** `current_machine_id()`
(`src/engine/path_resolution.rs:66-79`) reads `/etc/machine-id` first
(a random, systemd-generated UUID with no relationship to hardware
identifiers like a MAC address or serial number), falling back to the
`HOSTNAME` environment variable only if that file is absent. Neither
source is a secret, and this design does not add a new derivation --
`current_machine_id()` is an existing, `pub(crate)` function already used
by `SchedulerWarning::StaleTemplateSourceDir` since
DESIGN-batch-child-spawning.md's Decision 14, and this design's stated
approach is to *reuse* that exact call, not introduce a new one. Widening
the set of surfaces that expose it (init/status/list, in addition to
scheduler warnings) is a broadening of an already-accepted disclosure, not
a new category of disclosure. Since output stays local (same user, same
machine, no network hop introduced by this design), there is no scenario
where this design lets `machine_id` reach a party who couldn't already
obtain the equivalent value by running `hostname` or reading
`/etc/machine-id` directly on that box.

**Multi-tenancy:** Not applicable. Confirmed via `src/session/local.rs`
that koto's session store is scoped to a single OS user's home directory
with `0o700`/`0o600` permissions; there is no shared session namespace
across users or machines that this design's new reads could leak across.

## TOCTOU Consideration

The new `Path::exists()` calls (already an accepted pattern per the
design's "Decisions Already Made" section) gate only message *content* --
a `note`/clause appended to informational JSON or an error string. No
action (create, delete, overwrite, spawn) is conditioned on the result
anywhere in this design; Direction 3 (any destructive sweep/gc) is
explicitly out of scope. A race between the check and a human reading the
resulting message (e.g., the directory reappears a moment later, or is
removed a moment after being reported present) produces, at worst, a
stale or slightly wrong hint in text shown to the same local user who
already has full read access to the underlying header -- not a
security-relevant condition. This is benign by construction, and the
design correctly scopes a stronger fingerprint-based check as unnecessary
until/unless a future destructive-action design (Direction 3) needs one.

## Resource Exhaustion / DoS Consideration

`koto init`'s collision path reads exactly one colliding session's header
per invocation -- the one session whose name matches the name being
`init`'d -- not an enumeration over all sessions in the store. An attacker
who could create many same-named-colliding sessions gains no
amplification: each `koto init <name>` call still performs O(1) extra
work (one small first-line file read), the same cost class as the header
reads `koto status` and `koto session list` already perform today for
every row. To generate meaningful load, an attacker would need to invoke
`koto init` many times themselves, at which point they are the source of
the load, not a victim of amplification. Separately, and more
fundamentally: mounting this as an attack at all requires the ability to
create sessions in the target user's `~/.koto/sessions/` store, which
(per the Permission Scope analysis above) already requires the same-user
access that would let an attacker do far more damage directly. There is no
remote or cross-user vector into this code path. Severity: negligible, no
mitigation required beyond what already exists.

## Recommended Outcome

**OPTION 2 - Document considerations:**

No design changes are needed on security grounds. The implementer should
know, and a short Security Considerations section should record, the
following so future readers don't have to re-derive it:

---

### Security Considerations

This design introduces no new external inputs, dependencies, or network
surfaces -- it only reads local session state that koto itself already
wrote, and formats already-stored values (a path, a machine identifier)
into CLI/JSON output at three additional call sites.

**New I/O is bounded and same-trust-boundary.** `koto init`'s collision
path now opens the colliding session's header (previously a pure
existence check). Both the checking process and the checked session live
under the same OS user's `~/.koto/sessions/` tree, which is
`0o700`/`0o600`-permissioned (`src/session/local.rs`); this read does not
cross any privilege or user boundary that wasn't already crossable by
directly opening the raw state JSONL, which is the documented status quo
workaround this design replaces. Because the read is keyed to the single
colliding name (not an enumeration), it is also not a viable
denial-of-service amplification vector: cost is O(1) per `koto init` call,
same class as existing header reads in `koto status`/`session list`.

**Surfaced values are not new disclosures.** Both `template_source_dir`
(a local path) and `machine_id` (a non-secret value from `/etc/machine-id`
or the `HOSTNAME` env var, via the existing `current_machine_id()`) are
already stored in the session header or already exposed via
`SchedulerWarning::StaleTemplateSourceDir`. This design broadens *where*
those values surface (three more CLI commands); it does not broaden *who*
can see them, since koto is a single-user local tool with no cross-user or
network session-store access. The doc comment already planned for the
`SessionInfo.template_source_status` field's `CloudBackend`
None-means-two-things ambiguity (per the "Consequences / Mitigations"
section) is the right place to also note that this field surfaces a
locally-recorded path and should not be treated as safe to forward
verbatim into any future shared/multi-user or telemetry surface without
re-evaluating this analysis's single-user assumption.

**The `Path::exists()` check is read-only and gates messages, not
actions,** so no TOCTOU mitigation is required for this design's scope;
if a future Direction 3 (destructive sweep/gc) is built on top of this
signal, that design will need to revisit staleness-check robustness
(fingerprinting, not just existence) as it already anticipates.

---

## Summary

No security-relevant risk was found that requires changing the design.
The new I/O (`koto init` opening a colliding session's header) and the
new output fields (a local path, `machine_id`) all stay within koto's
existing single-user, local-filesystem trust boundary and existing
disclosure precedent (`StaleTemplateSourceDir`) -- this design widens
where an already-accepted signal appears, not what is disclosed to whom.
Recommend documenting these points briefly in the design (Option 2) rather
than changing the architecture.
