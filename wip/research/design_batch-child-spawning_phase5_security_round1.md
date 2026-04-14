# Security Review Delta — Round 1: batch-child-spawning

## Context

Round 0's security review concluded the batch-child-spawning design is
sound under koto's local-user trusted-submitter threat model: the only
new crate (`tempfile`) was pre-existing, session-directory assumptions
match current practice, and appended cwd/template-dir fields were
incremental rather than novel exposures. Round 0 produced Option 2
(document considerations), and the current design's Security
Considerations section (lines 2255-2374) reflects those additions. This
delta review covers the six round-1 follow-up decisions (CD9-CD14) plus
the cross-validation coordination notes, focusing on whether the new
surface (reserved_actions, spawn_entry snapshots, persisted error
bodies, flock-based lockfiles, renameat2, sync_status/machine_id,
session resolve --children, BatchFinalized events, skipped_because_chain)
introduces material security consequences beyond what round 0 covered.

## Dimension Analysis

### External Artifact Handling

**Applies:** Yes

**Round-0 posture:** Task lists are agent-supplied JSON; `template`
paths are trusted; `..` is silent-accepted; `vars` have the same trust
level as `koto init --var`; resource exhaustion bounded by 1 MB
`--with-data` cap plus soft recommendations (documented as hard limits
in the current Security Considerations: 1000 tasks, 10 waits_on, depth
50). Agent-submitted templates can point at any file the invoking user
can read — matches the trusted-submitter model.

**Round-1 delta:**

- **CD14** adds explicit path-resolution warnings
  (`SchedulerWarning::MissingTemplateSourceDir` and
  `SchedulerWarning::StaleTemplateSourceDir`) emitted when
  `template_source_dir` is absent (pre-D4 state files) or exists but
  points at a location that does not resolve on the current machine.
  `BatchError::TemplateNotFound.paths_tried` lists every absolute path
  attempted — including the captured `template_source_dir` and
  `submitter_cwd`. An agent that constructs a `template:` value
  containing `..` and observes the resulting `paths_tried` field on the
  error response learns the invoking user's home-directory layout and
  repo root. This is not new information above what `submitter_cwd`
  already leaks (round 0 flagged), but `paths_tried` is a new surface
  that can be *elicited on demand* by an attacker-submitted agent even
  when submission is otherwise rejected. Severity: **low** — the data
  is the user's own path, observable only by a trusted submitter, and
  is already persisted in `EvidenceSubmitted.submitter_cwd`.

- **CD10** adds `spawn_entry` snapshot on `WorkflowInitialized` events
  (a D2 amendment). This captures the full submitted task entry
  (`template`, `vars`, canonical-sorted `waits_on`) at spawn time. The
  snapshot is persisted in the child state file and replayed by R8 on
  every mutation attempt. Relative-to-round-0: the exact task-entry
  `vars` were already persisted on the parent's `EvidenceSubmitted`
  event, so the snapshot duplicates that data in a second location (the
  child header). Severity: **low** — same content, new location.

- **CD11** persists `InvalidBatchReason` payloads in the error response
  (not the event log). `DanglingRefs.entries`, `DuplicateNames.duplicates`,
  `SpawnedTaskMutated.changed_fields` (with old and new `vars` values
  side-by-side — see Q8 example showing `ghp_old...` / `ghp_new...`),
  `InvalidName.task`, and `ReservedNameCollision.task` all echo
  agent-submitted values back in the response. These values are **not**
  persisted — rejections leave zero state (CD11's pre-append commitment)
  — so cloud sync does not upload them. But the response body crosses
  the process boundary and lands in agent logs, shell history, and any
  relay (e.g., the parent harness that invoked koto). If an agent
  submits `{vars: {GITHUB_TOKEN: "actual_secret"}}` and mutates on
  resubmission, CD11's `SpawnedTaskMutated` response echoes both the
  old and the new token in a `changed_fields` diff. Severity:
  **low-to-medium** — same-severity-class as the existing behavior that
  persists `vars` in `EvidenceSubmitted`, but the echoing on error
  response widens the set of consumers who incidentally log secrets.
  See Reserved-Action Surface for a concrete mitigation recommendation.

- **CD13**'s `BatchFinalized` event snapshots the full `BatchView`
  including per-child `skipped_because_chain` arrays (task names only,
  not vars) and `reason` strings (from `failure_reason` context key,
  which round 0 already flagged must be sanitized by template authors).
  No new free-form data channel.

- **CD9**'s `reserved_actions.invocation` field emits a ready-to-run
  shell command containing the retryable child names. Children names
  are not sensitive (they are submitter-chosen identifiers subject to
  R9's regex).

**Mitigations suggested:** Add one sentence to Security Considerations
noting that error response bodies (CD11) echo agent-submitted
`vars` values in diff form on `SpawnedTaskMutated` rejection. Template
authors and agents should avoid submitting secrets as `vars` when
resubmission-with-mutation is a plausible workflow, or accept that the
diff will appear in agent logs. This is consistent with the round-0
note that `vars` persist in event logs; the delta is that rejected
submissions also echo them.

### Permission Scope

**Applies:** Yes

**Round-0 posture:** `init_state_file` uses `tempfile::NamedTempFile::persist`
which is the same pattern as `write_manifest`. Session directory assumed
user-owned. No new privilege boundary. `retry_failed` is self-DoS only.

**Round-1 delta:**

- **CD12 Q2**: adds `libc::renameat2(AT_FDCWD, ..., RENAME_NOREPLACE)`
  on Linux ≥ 3.15, with `std::fs::hard_link` + `remove_file` fallback
  on macOS/BSD. Both primitives operate on paths already under the
  session directory (user-owned by assumption). Neither requires
  elevated capabilities. `renameat2` with `AT_FDCWD` uses the invoking
  process's cwd and euid; no new privilege is acquired. Severity:
  **none** — operates within existing permission envelope.

- **CD12 Q3**: adds `flock(LOCK_EX|LOCK_NB)` on
  `<session_dir>/<workflow>.lock`. The lock file itself is a zero-byte
  regular file. The primitive is already used by `LocalBackend` for
  `ContextStore` writes (`src/session/local.rs:216-245`). No new
  syscall privilege; no daemon; no persistent cursor across invocations
  (kernel releases flock on process exit). Severity: **none**.

- **CD12 Q7**: tempfile sweep via `read_dir` + `fs::remove_file` on
  files older than 60 seconds matching the `<parent>.*.koto-*.tmp`
  pattern inside the session directory. The sweep is scoped to the
  parent being ticked and operates only on files whose name matches the
  koto-generated pattern. A local attacker with write access to the
  session directory could pre-seed files matching the pattern to have
  them deleted by the next tick, but (a) write access to the session
  directory is already privilege-equivalent to owning koto's state
  entirely, and (b) the sweep only deletes, it does not create or
  modify arbitrary paths. Severity: **none** — within the "session
  directory is user-owned" assumption.

- **CD12 Q4**: `koto session resolve --children` reads and potentially
  overwrites child state files based on `CloudBackend::check_sync`
  results. The command is user-invoked; its permission scope is the
  invoking user's session directory plus whatever the `CloudBackend`
  is configured to reach (e.g., a cloud bucket). No privilege
  escalation; the command operates on artifacts the user already owns.
  Severity: **none**.

**Mitigations suggested:** None beyond existing Security Considerations
text noting the session-directory user-ownership assumption.

### Supply Chain / Dependency Trust

**Applies:** Yes (delta from round 0)

**Round-0 posture:** No new dependencies. `tempfile` and `serde`
pre-existing.

**Round-1 delta:**

- **CD12 Q2**'s `renameat2` call requires a raw libc binding. Koto
  must either (a) add the `libc` crate, (b) add the `nix` crate, or
  (c) write its own `extern "C" fn` declaration for `renameat2`. `libc`
  is the conventional dependency and is already in the Cargo dependency
  graph of many koto dependencies (e.g., `tempfile` pulls `libc`
  transitively on Unix). Adding `libc` as a direct dependency does not
  add any new crate to the build graph — it surfaces a transitive
  dependency as a direct one. If `nix` is chosen instead, that is a
  new direct dependency with its own transitive graph (`nix` pulls
  `bitflags`, `cfg-if`, `libc`, `memoffset`, `pin-utils`), but all of
  these are already common in the Rust Unix ecosystem and are
  widely-vetted. The design doc should pin which approach is chosen.

- **CD9** requires a new when-clause matcher (`evidence.<field>:
  present`). This is a koto-internal code change, not a dependency
  change. Severity: **none** (supply chain).

- **CD10**, **CD11**, **CD13**, **CD14** introduce no new external
  dependencies — they are all native koto code changes or serde
  extensions against the existing derive.

**Mitigations suggested:** Update the Security Considerations "Supply
chain" subsection to explicitly state the renameat2 binding approach.
Preferred: use `libc` (already transitively pulled; minimal trust
expansion). Document the choice in the design doc so future reviewers
see that the trust boundary did not expand.

### Data Exposure

**Applies:** Yes

**Round-0 posture:** `submitter_cwd` and `template_source_dir` persist
local absolute paths; agent-supplied `vars` in `EvidenceSubmitted`
persist forever (same as `koto init --var`); `reason` in batch output
must come from `failure_reason` context key, not scraped stderr.

**Round-1 delta:**

- **CD10** `spawn_entry` snapshot on `WorkflowInitialized`: persists
  agent-supplied `vars`, `template` path, and `waits_on` in the child
  state file header. Round 0 already flagged that `vars` are persisted
  in the parent's `EvidenceSubmitted` event; the delta is that the same
  data now lives in a *second* location (the child). For cloud sync,
  this means the `vars` are uploaded twice (once in the parent's event
  log, once in each child's header). Severity: **low** — same content
  class, not new content. Bounded by the task-count limit (CD1's 1000
  tasks) × vars cardinality.

- **CD11** persists structured error payloads only in the transient
  response body, not in the event log (pre-append validation commitment).
  The delta concern is that rejected submissions echo agent-submitted
  fields (names, vars diffs, paths) back in JSON. Consumers logging
  responses (shell redirection, agent harnesses, shirabe work-on) will
  capture these echoes. For `SpawnedTaskMutated`, both old-and-new
  `vars` values appear side-by-side in `changed_fields` — if the old
  value was a secret that the agent is trying to rotate, both the old
  secret and the new one land in the agent log. Severity: **medium**
  if agents rotate secrets via `vars` resubmission, **low** otherwise.

- **CD12 `sync_status`** values are one of four enum strings (`fresh`,
  `stale`, `local_only`, `diverged`). No sensitive data.

- **CD12 `machine_id`**: per CD12 Q5, "the canonical machine identifier
  used by `CloudBackend` for conflict resolution (already computed
  today per design L2337-2357)." Inspection of design lines 2337-2357
  shows `machine_id` is a hostname-derived or user-configured
  identifier. **This is uniquely-identifying at the machine level.** In
  multi-user org contexts where multiple developers run koto against
  the same cloud-sync bucket, `machine_id` reveals which machine
  produced which response — effectively an attribution signal. On a
  single-user laptop this is just a hostname echo. For koto's
  trusted-small-team model this is consistent with existing `CloudBackend`
  behavior (the same `machine_id` is already persisted in cloud-sync
  artifacts today per L2337-2357), so no new capability is being added.
  But surfacing it on **every** cloud-mode response (rather than only
  in conflict resolution artifacts as it is today) expands the set of
  callers who incidentally log the identifier. A bug report or shared
  response log now trivially includes the reporter's `machine_id`.
  Severity: **low** — matches existing behavior of cloud-sync
  metadata, but broadens the surface.

- **CD13 `BatchFinalized` event** carries the final `BatchView`:
  per-child `name`, `outcome`, `reason`, `state`, `skipped_because`,
  `skipped_because_chain`. `reason` is the sanitized `failure_reason`
  context-key value (round-0 covered). `skipped_because_chain` is a
  list of task names (R9 regex-constrained, not sensitive). No new data
  class.

- **CD13 `skipped_because_chain`** persists the transitive attribution
  path. For a failed-dep chain of length N, the chain for the terminal
  leaf task contains N-1 task names. This is accurately a denormalized
  view of information already computable from the DAG; not a new
  exposure.

- **CD14**'s error-response `paths_tried` echoes absolute paths tried
  by the resolver. Same severity as `submitter_cwd` exposure flagged
  in round 0.

**Mitigations suggested:** Add two sentences to Security Considerations:
(1) `machine_id` on responses under cloud-sync mode matches the
existing cloud-sync metadata; shared bug reports with `machine_id`
visible are equivalent to sharing a hostname. (2) `SpawnedTaskMutated`
error responses echo both old and new `vars` values in a diff; agents
rotating secrets via `vars` resubmission should expect both values in
agent logs and should not rely on those logs being sanitized.

### Reserved-Action Surface

**Applies:** Yes

CD9 adds `reserved_actions` on failed/skipped batch responses and on
terminal responses where the batch had failures. CD11 introduces
`action: "error"` as a seventh top-level response variant with a
structured `error.batch` payload. CD12 adds `batch.kind:
"concurrent_tick"` / `"init_state_collision"` /
`"session_resolve_child_divergence"` error variants. CD14 adds
`batch.kind: "template_not_found"` / `"template_compile_failed"`
payloads. All of these are error bodies crossing the process boundary.

**Probe surface analysis:**

- **`reserved_actions.applies_to`** enumerates retryable child names.
  These are R9-constrained identifiers, submitter-chosen, non-sensitive.
- **`reserved_actions.invocation`** includes a shell-ready string that
  concatenates the parent workflow name and child names. A submitter
  that controls child names can influence the emitted invocation
  string, but R9 constrains names to `^[A-Za-z0-9_-]{1,64}$` and the
  reserved list blocks `retry_failed` / `cancel_tasks`. Shell-injection
  via child name is **not possible** under R9 (no shell-metacharacters
  admitted).
- **`error.batch.reason: "cycle", "cycle": [...]`** echoes the
  agent-submitted task-name cycle. Non-sensitive.
- **`error.batch.reason: "dangling_refs"`** echoes missing waits_on
  references. Non-sensitive (submitter-supplied names).
- **`error.batch.reason: "spawned_task_mutated"`**: the
  `changed_fields` array echoes full `vars` values. This is the
  highest-severity information echo in the error surface. See Data
  Exposure above.
- **`CompileFailed.compile_error`** (CD14): echoes the template
  compiler's error message. Template compile errors in koto today
  include file paths, line numbers, and offending template syntax.
  Under the trusted-submitter model the agent already had read access
  to the template, so echoing its compile error does not cross a
  privilege boundary, but it does move template-internal details into
  the response channel. Severity: **low**.
- **`TemplateNotFound.paths_tried`**: echoes resolved absolute paths
  including the user's home directory layout. Same exposure as
  round-0's `submitter_cwd` finding. Severity: **low**.

**Attack model:** A submitter-controlled probe that crafts task lists
to trigger specific error branches can learn (a) the user's home
layout (via `paths_tried`), (b) the cycle topology of their own
submission (not sensitive — submitter supplied it), (c) other agents'
prior submissions for the same parent (via `spawned_task_mutated` —
the `spawned_value` field reveals what a prior agent put in `vars`
before this agent resubmitted). Point (c) is the only meaningful
probe: a malicious co-submitter to a shared parent workflow could use
R8 mutation rejection to leak a prior submitter's `vars` secrets. In
practice koto's single-coordinator model (CD12) restricts this —
multiple agents submitting to the same parent is already considered a
concurrency bug — but the capability exists.

**Mitigations suggested:** CD11 should consider redacting `vars` values
in `SpawnedTaskMutated.changed_fields` when the submitted value is
marked sensitive. A low-cost mitigation: add a convention that `vars`
keys matching `*_TOKEN`, `*_SECRET`, `*_KEY`, `*_PASSWORD` are
displayed as `"<redacted>"` in diff output. Document this as a
best-effort heuristic, not a security guarantee. Alternatively,
document clearly that error responses echo `vars` in full.

### Cross-Machine Trust Under Cloud Sync

**Applies:** Yes

CD12 adds `sync_status`/`machine_id` on every cloud-mode response and
extends `koto session resolve` to reconcile child state files across
machines.

**Threat model delta:** Round 0 treated cloud sync as a per-user bucket
assumption (misconfigured multi-user bucket is an operator concern,
not koto's concern). CD12 expands the in-band machine-to-machine
signaling surface: `koto session resolve <parent>` now fetches the
remote parent log AND remote child state files, computes divergence
per-child, and (under `--children=auto`, the default) overwrites local
child state with remote content when divergence is trivially
reconcilable or when the parent log's consensus prefers remote.

**New attack surface: malicious or compromised remote.** Under cloud
sync, a compromised peer machine pushing tampered state could
influence the local machine on the next `koto session resolve` call.
Specifically:

- **Peer pushes a forged parent log** declaring the parent reached a
  terminal state with fabricated children. On local resolve, the
  local parent log is overwritten (accept=remote); locally-spawned
  children may be discarded as orphans or reconciled per the remote
  parent's view.
- **Peer pushes a forged child state file** with a `failure_reason`
  context key containing an attacker-controlled string. The local
  machine reads this on `koto status <parent>` and displays it
  unaltered.
- **Peer pushes a forged `spawn_entry` snapshot** on a child header
  with modified `vars` or `template` path. On the local machine, if
  that child is still unspawned locally, CD10's R8 will enforce the
  forged snapshot on any subsequent local mutation — effectively
  locking the local user to the attacker's chosen `template`/`vars`.

**Severity assessment:** Koto's existing threat model says the cloud
sync bucket is trusted (per-user or per-trusted-team). If that trust
holds, none of the above are new vulnerabilities — they are simply
consequences of trusting the bucket. The severity inside koto's stated
model is **none**. If the bucket is compromised (threat model
escalation), CD12 expands the blast radius because resolve now
auto-propagates child-level changes rather than stopping at the
parent. Users running cross-machine koto with a shared bucket should
understand that `koto session resolve --children=auto` trusts remote
child state.

**Mitigations suggested:** Document in Security Considerations that
`koto session resolve --children=auto` (CD12's default) trusts remote
state for both parent and child reconciliation. Users operating a
shared bucket whose integrity may be partially compromised can pass
`--children=skip` to restrict resolve to parent-only behavior (matching
v0.7.0). This is a configuration knob to surface; the threat model
itself is unchanged.

### Retry Semantics Security

**Applies:** Yes

**Round-0 posture:** `retry_failed` is self-DoS only (the submitter
already has evidence-submission authority and is trusted).

**Round-1 delta:** CD9 Part 4 formalizes retry edges:

- A submitter can `retry_failed` a set of completed-failure or
  completed-skipped children. The edge rules (10.3's
  all-or-nothing atomicity, rejection of retry on running/successful
  children, rejection of mixed payloads) prevent the submitter from
  accidentally rewinding valid completed work via ill-formed
  submissions.
- A well-formed `retry_failed` payload naming a legitimately failed or
  skipped child will rewind that child. This is the intended semantic.
- A submitter targeting a successful child's ancestors to force
  cascading delete-and-respawn is blocked: the downward closure (CD9
  Part 4) cascades to *dependents* of the retry set, not ancestors.
  The submitter cannot trick koto into discarding a successful child
  by naming one of its ancestors unless that ancestor is already in
  failure/skipped outcome.

**Trust boundary confirmation:** Within the trusted-submitter model,
all retry capabilities are authorized. A malicious submitter is out of
scope. The round-0 "self-DoS by rapid retry" statement holds without
modification.

**One new consideration:** CD12 Q6's retry-ordering under cloud sync
(push parent first, then children) means a crash between steps c' and
f' leaves the parent in "retry submitted + cleared" state but children
not yet rewound. On resume the user resubmits the retry. This is
correct under the trust model (the submitter is not an attacker), but
a flaky network submitter could accidentally submit retry multiple
times (once before crash, once after). Each retry opens a new child
epoch; append-only log grows; no incorrect state. Severity: **none** —
same-class as round-0's self-DoS observation.

**Mitigations suggested:** None. CD9 Part 4's edge table is the
mitigation. Document in Security Considerations that retry semantics
are within the trusted-submitter model and no rate-limiting is
provided (round-0's retry-throttling note already covers this).

## Recommended Outcome

**OPTION 2 — Document considerations:**

The round-1 decisions do not require any design change from a security
perspective. They do, however, add three specific surfaces that warrant
one-to-three additional sentences each in the design's Security
Considerations section. Draft deltas:

1. **Error-response echo of agent-submitted values (CD11).** Add to
   the "Observer-visible output" subsection:

   > Error response bodies (per Decision 11) echo agent-submitted
   > fields back to the caller for diagnostic purposes. In particular,
   > the `SpawnedTaskMutated` rejection (Decision 10) includes a
   > `changed_fields` diff with both the spawned and submitted values
   > of `vars` entries. Agents rotating secrets via `vars` resubmission
   > should expect both old and new values to appear in response logs.
   > Error responses are transient (pre-append validation leaves zero
   > state), but response bodies are captured by shell redirection and
   > agent harnesses. This is a broader consumer set than the event log
   > which round 0 already flagged.

2. **`machine_id` on cloud-mode responses (CD12).** Add to the
   "Cloud sync concurrent submission" subsection:

   > When `CloudBackend` is configured, every `koto next` response
   > carries a top-level `machine_id` field (the same hostname-derived
   > or user-configured identifier used by cloud sync for conflict
   > attribution per design L2337-2357). This matches existing
   > cloud-sync metadata exposure and does not reveal new information
   > beyond what is already persisted in cloud-synced artifacts.
   > Bug reports and shared response logs that include
   > `machine_id` are equivalent to sharing a hostname; teams sharing
   > a bucket should be comfortable with per-machine attribution.

3. **`koto session resolve --children` trust model (CD12).** Add a new
   subsection or extend "Cloud sync concurrent submission":

   > `koto session resolve <parent>` (extended by Decision 12) now
   > reconciles child state files in addition to the parent log under
   > the default `--children=auto` mode. This trusts remote state for
   > both parent and children; a compromised cloud-sync bucket could
   > propagate forged child content on resolve. Teams whose bucket
   > integrity may be partially compromised can pass `--children=skip`
   > to restrict resolve to parent-only behavior. The threat model
   > (cloud-sync bucket is trusted) is unchanged from v0.7.0; the
   > operational surface is broader.

4. **`paths_tried` and `compile_error` echo (CD14).** Add to the
   "Persisted path information" subsection, or a new subsection under
   "Observer-visible output":

   > `BatchError::TemplateNotFound` rejections echo every absolute path
   > attempted during template resolution in `paths_tried`, and
   > `BatchError::TemplateCompileFailed` echoes the template compiler's
   > raw error message. Both can include the user's home directory
   > layout and template-internal details. This information is already
   > available to the invoking user (who has read access to the
   > templates and the filesystem); echoing it on error responses
   > widens the set of consumers who incidentally capture it.

5. **`renameat2` / `libc` supply-chain note (CD12 Q2).** Extend the
   "Supply chain" subsection:

   > Decision 12 introduces a `renameat2` syscall for atomic
   > create-only rename on Linux. The implementation uses the `libc`
   > crate (already transitively pulled by `tempfile`) rather than
   > adding a new direct dependency. The portable fallback on
   > macOS/BSD uses `std::fs::hard_link`, which is in the Rust
   > standard library and requires no additional crate.

6. **Optional: `vars` redaction in error diffs.** Consider adding to
   the "Trust boundaries" subsection a best-effort heuristic:

   > CD11's `SpawnedTaskMutated` error responses display `vars` diffs
   > in full. A future enhancement may redact values for keys matching
   > common secret patterns (`*_TOKEN`, `*_SECRET`, `*_KEY`,
   > `*_PASSWORD`). Until implemented, treat every `vars` value as
   > potentially echoed on rejection.

No design changes are required. No round-0 finding is invalidated. No
new round-0-class risk emerges. The round-1 package expands the set of
response-channel data exposures (error payloads, spawn_entry snapshots,
sync_status/machine_id) without crossing a privilege boundary or
introducing a new trust model.

## Summary

Round 1's six decisions (CD9-CD14) introduce no new class of
vulnerability and do not invalidate any round-0 finding. The material
delta is response-channel exposure — error bodies echo `vars` diffs
(CD11), `machine_id` surfaces on every cloud-mode response (CD12), and
`paths_tried`/`compile_error` echo user-local paths and template
internals (CD14) — all within the trusted-submitter model but across a
broader consumer set than round 0's event-log-only exposures. The
recommended outcome is documentation: six focused additions to the
Security Considerations section (three sentences each) covering the
new echoes, the `--children=auto` trust default, and the `libc`/renameat2
supply-chain non-expansion. No design change is required.
