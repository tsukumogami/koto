# Phase 2 Research: Anchoring and Shared-Path Defects

## Lead A — Execution anchoring, current behavior

### Findings

**Where the working directory comes from today.** `handle_next` (`src/cli/mod.rs:2892`, `#[cfg(unix)]`) captures the process's live working directory exactly once per tick, immediately after confirming the workflow exists and before the state file is read:

```rust
let current_dir = std::env::current_dir()?;
```
(`src/cli/mod.rs:3082`)

That single value is threaded, unmodified, into both execution surfaces:

- The gate closure (`src/cli/mod.rs:3945-3960`) passes `&current_dir` straight into `evaluate_gates`, which forwards it as `working_dir` to `run_shell_command` for every `type: command` gate (`src/gate.rs:206-207`).
- The action closure's `wd` computation (`src/cli/mod.rs:3979-3983`):
  ```rust
  let wd = if action.working_dir.is_empty() {
      current_dir.clone()
  } else {
      std::path::PathBuf::from(variables.substitute(&action.working_dir))
  };
  ```
  When a template's `default_action` declares no `working_dir`, the action runs in `current_dir` — i.e., wherever `koto next` happened to be invoked from. When it does declare one, the value is a bare `PathBuf::from(...)` of the substituted string, with no `current_dir.join(..)` and no canonicalization.
- `execute_with_polling` (`src/cli/mod.rs:995-1041`) takes the same `working_dir: &Path` and reuses it for every poll iteration and its own nested gate evaluation (`src/cli/mod.rs:4012`).

`run_shell_command` (`src/action.rs:26-107`), the single shared execution primitive for both gates and actions, does zero validation on the `working_dir: &Path` it's given — no canonicalize, no containment check, no existence check beyond letting `Command::spawn` fail with `exit_code: -1` (`src/action.rs:49-58`). It is passed directly to `Command::current_dir` (`src/action.rs:36`).

This confirms the claim precisely: **the working directory a gate or `default_action` runs in is, today, simply the process CWD of wherever `koto next` was typed**, recomputed fresh on every tick with no memory of where the previous tick ran.

**What the session state file records — and doesn't.** The persisted on-disk unit is `StateFileHeader` (`src/engine/types.rs:219-267` roughly; the field of interest at `src/engine/types.rs:260`), the first line of a session's state file, plus the append-only event log that follows it. Its fields are: `schema_version`, `workflow` (name), `template_hash`, `created_at`, `parent_workflow: Option<String>`, `template_source_dir: Option<PathBuf>`, `session_id`, and an intent description. The derived, in-memory `MachineState` struct (`src/engine/types.rs:1618-1627`) — produced by replaying the event log, never itself persisted — carries only `current_state`, `template_path`, and `template_hash`.

**Neither struct has a field naming a directory, tree, or repo root that governs where gates/actions execute.** The one directory-shaped field that does exist, `template_source_dir`, is explicitly documented (`src/engine/types.rs:245-259`) as "Captured from the parent directory of the absolute template path passed to `handle_init`" — i.e., the directory the *template file itself* lives in (for a shirabe-driven workflow, something like `.../shirabe/skills/execute/koto-templates/`), not the working tree the workflow acts on. Its one consumer is the batch scheduler's child-template path resolver (`src/engine/path_resolution.rs`, module docstring lines 1-21): it never touches the gate/action execution path in `handle_next`. `koto init` itself doesn't record its own invocation cwd anywhere durable, either — a freshly initialized top-level session has no field recording where it was started.

**The `working_dir` containment trap.** `ActionDecl.working_dir` (`src/template/types.rs:200-208`, field at line 203) is a plain `String`, substituted via `variables.substitute(...)` and turned into a `PathBuf` with no joining or canonicalization (`src/cli/mod.rs:3979-3983`, quoted above). Because this is `PathBuf::from`, not `current_dir.join(...)`, there is no containment relationship to break in the first place — but that also means any fix that *adds* containment by doing `execution_root.join(working_dir)` has a specific, documented Rust trap: `PathBuf::join` **discards the base entirely when the joined path is absolute** (`root.join("/etc")` == `/etc`, not `root/etc`). A substituted `working_dir` value that is itself an absolute path — or one that escapes via `..` — is not caught today because no containment check exists at all; if one is added by joining-then-canonicalizing without first checking for an absolute value, the absolute-path case would silently defeat it. Round-1 research on this branch demonstrated the escaping case concretely (`working_dir: ".."`).

### Implications for Requirements

- Any anchoring requirement should state the observable contract at the `koto next` boundary: given a session with a recorded anchor and a live cwd that does not match it, `koto next` refuses to run the tick's gate/action commands, rather than silently executing wherever it was invoked from.
- A testable requirement can be written purely in terms of current absence: "the state file records no directory that constrains where a gate or `default_action` command executes" is a factual, verifiable-today baseline requirement statement (verify via `StateFileHeader`'s field list and `handle_next`'s `current_dir` capture).
- If a `working_dir` containment mechanism is required, the requirement should explicitly cover the absolute-path case as a distinct testable scenario, not just the relative `..`-escape case — the two need separate test assertions because a join-based fix that handles one doesn't automatically handle the other.

### Open Questions

- None beyond what's captured in Lead B — the current-behavior facts here are fully determined by reading the code; the open questions are all about what a new anchor *should* do, not what happens today.

## Lead B — The three open anchoring design questions

### Findings

All three questions were named as open questions in `wip/research/explore_koto-command-authority_r1_lead-anchoring.md` (round 1 of this branch's own research) and originate from `explore_koto-runs-commands_r2_lead-anchoring.md` on shirabe's `docs/koto-runs-commands` branch. Restated with their trade-offs, without choosing:

**1. Does the anchor default silently, or require an explicit flag?** The shirabe r2 lead frames this as: silent default (an anchor is recorded automatically from `koto init`'s cwd, the same way every other implicit-cwd convention in the codebase already works, and the same assumption round 1's proof-of-concept made) versus an explicit flag akin to `--template` (more honest about the hazard being closed, but adds a required argument to every `koto init` call site shirabe's templates already use — `work-on.md`, `execute.md` — that would need updating). The koto-command-authority round-1 lead doesn't reopen this question; it assumes silent recording as part of its "one inseparable first increment" recommendation, but that's a recommendation, not a resolution the design doc should treat as settled.

**2. Do pre-existing sessions refuse until bound, or warn once?** For state files written before an anchor field exists (`execution_root: None`), the shirabe r2 lead frames the extremes as: hard-refuse every in-flight session until an operator manually binds it (safest, but "breaks everything the day this ships" for any session already in flight), versus silently trust cwd forever for legacy sessions (never starts enforcing, so old sessions stay permanently exposed to the exact hazard anchoring exists to close). The koto-command-authority round-1 lead proposes a specific middle path — "implicit bind-once": the first successful post-upgrade tick records whatever cwd is live as the anchor and enforces from then on — and flags this as its own recommendation rather than a resolved trade-off, explicitly noting a legitimate alternative ask: a fleet operator who wants to hard-refuse every legacy session on principle so they can audit each one manually before it resumes.

**3. Is the check root-equality or root-containment?** The shirabe r2 lead names this explicitly as unresolved: equality (live cwd must canonicalize to exactly the recorded root) is "the safer default and matches every proven round-1 scenario"; containment (live cwd must canonicalize to the root or a descendant of it) "would tolerate an agent that `cd`s into a subdirectory of the intended tree without an explicit rebind, which is a real, benign thing agents do (e.g. `cd recipes/` to run a scoped command) but which round 1 never actually tested as unsafe or safe." Neither round of research resolves which is correct — it's flagged as untested rather than merely undecided.

**Is there an existing subcommand that would be a natural home for a deliberate rebind?** No. The full CLI surface today (`src/cli/mod.rs:84` onward, `pub enum Command`) is: `Version`, `Init`, `Next`, `Cancel`, `Rewind`, `Workflows`, `Template`, `Session`, `Context`, `Status`, `Decisions`, `Overrides`, `Config`, `Workspace`, `Request`, `Dashboard`. Under `koto session` (`SessionCommand`, `src/cli/mod.rs:376` onward): `Start` (create a child session under `--parent`), `Dir` (print session path), `List`, `Cleanup`, `Resolve` (cloud version-conflict resolution — `--keep local|remote`, unrelated to directory), and `Update` (currently sets only the workflow's `intent` field). None of these names, records, or checks a directory. `koto workspace` has only `Prune`. Grepping the tree for `execution_root`, `SessionBind`, or `fn bind` outside of `request_store::bind_leg` (an unrelated request-lifecycle verb) returns nothing. Both prior research rounds independently proposed a new `koto session bind` verb as the natural extension point — `session update` is the closest existing shape (a session-metadata-mutation verb with no per-tick execution role), but it does not exist today as something that touches a directory.

### Implications for Requirements

- The PRD can state the requirement "koto must provide a deliberate, auditable way to change which directory a session is anchored to" without naming the verb, since no existing subcommand already does this and the CLI surface has an established pattern (`Session` subcommand group) for adding one.
- Each of the three questions can be captured as an explicit design-decision placeholder in the PRD rather than answered — e.g., "the design must state whether anchor recording is automatic or requires an explicit flag" rather than picking one.
- Question 3 (equality vs. containment) is the one with a concrete, testable follow-up: whichever the design chooses, the PRD can require a test case for the "cd into a subdirectory of the anchored root" scenario specifically, since neither prior round exercised it.

### Open Questions

- Whether shirabe's own templates would need updating regardless of which answer to question 1 is chosen (an explicit-flag default would require every `koto init` call site in `work-on.md`/`execute.md` to pass it).
- Whether "warn once" for pre-existing sessions (as opposed to hard-refuse or implicit bind-once) was ever separately evaluated — the round-1 lead only names hard-refuse and silent-trust-forever as the extremes, with implicit-bind-once as its own proposed middle ground, not a fourth option surfaced by prior research as equally live.

## Lead C — The session-migration / remap-remote defect

### Findings

**Note on file naming:** the file `wip/research/explore_koto-command-authority_r1_lead-remap-remote.md` in this repo is about a different topic entirely — it catalogs which `git push`/`gh pr` commands are candidates for conversion to koto-native verbs ("remap" as in remapping git *remote* operations, not machine relocation). It contains no content about cross-machine session migration. The relevant mechanism was researched fresh for this lead.

**The cloud-sync path, confirmed opt-in.** `koto`'s session backend defaults to local-only storage: `default_backend()` returns `"local".to_string()` (`src/config/mod.rs:167-169`), the value for the `session.backend` config key. Switching to cloud requires an operator to explicitly set `session.backend = cloud` (via `koto config set`) plus populate `CloudConfig` (`src/config/mod.rs:159-165`: `endpoint`, `bucket`, `region`, `access_key`, `secret_key`) — all `Option<String>`, none defaulted. `CloudBackend::new` (`src/session/cloud.rs:74`) is only constructed when this config is present (`src/cli/mod.rs:722-724`). This matches "opt-in": nothing syncs anywhere unless an operator deliberately configures S3-compatible object storage.

**What the cloud backend actually transfers.** `CloudBackend` wraps `LocalBackend` (module doc, `src/session/cloud.rs:1-11`): every filesystem operation happens locally first, then syncs. Two distinct sync mechanisms exist:

- **Session state (the header + event log)**: transferred as a single whole-file blob, not incrementally. `append_header` and `append_event` (`src/session/cloud.rs:712-729`) both call `self.local.<op>(...)` followed unconditionally by `self.sync_push_state(id)`, which reads the entire local state file from disk and `PUT`s it to S3 under `state_key` = `{prefix}/{id}/{state_file_name}` (`src/session/cloud.rs:113-136`). Symmetrically, `read_events` and `read_header` (`src/session/cloud.rs:733-744`) both call `sync_pull_state(id)` first, which `GET`s the object and overwrites the local file (`src/session/cloud.rs:143-158`), before delegating to the local read. So on every state-mutating or state-reading call, the *entire* header-plus-events file is round-tripped to S3, verbatim.
- **Content context** (`koto context add/get`, separate from session state): synced per-key, incrementally, described in `src/session/sync.rs`'s module docstring (lines 1-7) — not the mechanism that carries session state.
- Both sync directions are non-fatal on failure: `sync_push_state` and `sync_pull_state` print a `warning: cloud sync ... failed: {e}` to stderr (`src/session/cloud.rs:115-117`, `152-156`) and let the local operation stand.

**What this means concretely: everything in the header is carried, faithfully, because the whole file moves.** A session migrated between machines via this opt-in path arrives on the new machine with `schema_version`, `workflow`, `template_hash`, `created_at`, `parent_workflow`, `template_source_dir` (if set), `session_id`, `intent`, and the full event log — all byte-identical to the origin. **The defect is not a transport gap.** Per Lead A, there is no field in `StateFileHeader` (or anywhere else in the persisted format) that records the directory a gate/action should execute in. Cloud sync faithfully carries a session that already carries nothing about execution location, because that information was never captured at `koto init` time on the origin machine either. "A session that moves between machines arrives with nothing recording where it ran" is therefore accurate, but the root cause is upstream of sync: the field doesn't exist to be synced, not that sync drops it.

**The adjacent, already-solved analog.** A structurally similar problem — a recorded path that goes stale after a cross-machine move — already exists and is already handled, for a *different* field: `template_source_dir`, used only by the batch scheduler's child-template path resolver. `resolve_template_path_with_base_status` (`src/engine/path_resolution.rs:130-198`) checks whether the recorded `template_source_dir` still exists on the current machine; if not, it emits `SchedulerWarning::StaleTemplateSourceDir` (`src/engine/scheduler_warning.rs:77-90`, carrying the stale `path`, a best-effort `machine_id` via `current_machine_id()` reading `/etc/machine-id` or `$HOSTNAME` at `src/engine/path_resolution.rs:59-75`, and the directory it fell back to) and falls back to `submitter_cwd`. This is the batch-scheduler's answer to "what happens when a recorded directory doesn't exist here" — but it only covers relative *child-template* path resolution, not the gate/action execution-anchor concept Lead A/B are about. A `TODO` at `src/engine/path_resolution.rs:63-66` explicitly anticipates reuse: "a future revision may swap this for the same identifier the cloud-sync layer attaches to state files ... so a session migrated between machines surfaces matching IDs across the `sync_status` and `StaleTemplateSourceDir` channels" — confirming that no such cloud-sync machine-identifier convention exists yet either, only an aspiration to align with one.

**User-visible failure.** Concretely: an operator runs a workflow on machine A with `session.backend = cloud`, the state file (with no execution anchor of any kind) syncs to S3. On machine B, `koto session resolve` / a fresh `koto next <name>` pulls that state file down and — per Lead A — captures `std::env::current_dir()` fresh, on machine B, with nothing to check it against, and proceeds to run gates/actions wherever `koto next` happens to be invoked from on the new machine. There is no error, no warning, and no `StaleTemplateSourceDir`-style diagnostic for this case, because the mechanism that produces that diagnostic only fires for `template_source_dir`, which governs a different, narrower thing.

### Implications for Requirements

- The PRD can state a testable current-behavior requirement: "a session synced via the opt-in cloud backend and resumed on a second machine produces no warning, error, or diagnostic related to execution location, regardless of whether the second machine's cwd differs from the first" — this is verifiable today by tracing the code path with no additional research.
- The `SchedulerWarning::StaleTemplateSourceDir` / `current_machine_id()` machinery is a plausible reuse target for whatever cross-machine diagnostic an anchoring design eventually adds, per the TODO already left in the code — but that's a design choice, not a current-behavior fact, and should be left to the DESIGN doc.
- Because cloud sync is opt-in and off by default, any requirement here should be scoped to "when the cloud backend is enabled" — the single-machine, local-only case (the default) doesn't have a migration scenario at all; the hazard is the same *kind* of directory-trust gap as Lead A/B describe for a single machine, just triggered by a machine change instead of a plain `cd`.

### Open Questions

- Whether a future anchoring design should reuse `current_machine_id()` (already implemented, already TODO'd for this exact reuse) or need a different mechanism — left open per the instruction not to design here.
- Whether `koto session resolve`'s existing conflict-resolution UX (`--keep local|remote`, `--children <policy>`) is a place a migration-anchoring check would naturally hook into, or whether it belongs earlier, at `koto next`'s existing tick-time refusal point (per Lead A/B) — not determined by this research.

## Lead D — Warning volume

### Findings

**The migration-warning mechanism (koto issue #193).** `LocalBackend::new()` (`src/session/local.rs:37-44`) unconditionally calls `migrate_if_needed(&base_dir)` (`src/session/local.rs:657-720`) on every session-touching invocation — i.e., every `koto` command except `koto version`, which never constructs a `LocalBackend`. `migrate_if_needed` walks `~/.koto/sessions/` looking for old-layout per-repo subdirectories (16-character lowercase-hex names — the old `repo_id()` scheme) and, for each session name inside one, either renames it up to the flat layout or, if a session of that name already exists at the destination (a collision), prints one line to stderr and leaves the stale directory in place:

```rust
eprintln!(
    "koto: migration skipped {}: session already exists at {}",
    session_name.to_string_lossy(),
    dest.display()
);
```
(`src/session/local.rs:690-694`)

Because a colliding directory is never removed (`migrated_count` isn't incremented for it, and `fs::remove_dir(&old_dir)` at the end of the function fails silently on a non-empty directory), the same collision is rediscovered and re-reported on **every subsequent invocation** — this is confirmed as a self-repeating, non-converging cost, not a one-time migration tax, tracked upstream as koto issue #193 ("Session migration never converges: name collisions strand 1000+ sessions and reprint skip lines on every invocation," referenced in `docs/briefs/BRIEF-koto-runs-commands.md:268`). Prior research on the shirabe branch measured this producing on the order of 1000+ lines of stderr output on an install with enough accumulated colliding session names.

**How the user sees this noise today: stderr, unconditionally, on (almost) every invocation.** Every emission point in `migrate_if_needed` uses `eprintln!` — the per-collision skip line (line 690-694), the per-error rename-failure line (line 700-705), and the summary "migrated sessions from X to Y" line (line 709-714) all go to stderr, not stdout, and are entirely independent of `koto`'s structured JSON output contract (the `NextError`/`{"error": ...}` envelopes used elsewhere). A caller reading only `koto next`'s stdout JSON never sees this text in the structured response; it appears as raw, unstructured lines on the process's stderr stream alongside (and before) whatever JSON that invocation eventually writes to stdout.

**Verified against current source: the pipe-buffer deadlock connection is real and confirmed at the current commit.** `run_shell_command` (`src/action.rs:26-107`) spawns the child with both `stdout` and `stderr` set to `Stdio::piped()` (lines 36-38), then calls `child.wait_timeout(timeout)` (line 60) **before reading either pipe** — the reads happen only inside the `Ok(Some(status))` branch, after `wait_timeout` has already returned. Nothing drains the pipes while `wait_timeout` blocks. On Linux, a pipe has a fixed kernel buffer (typically 64KB); a child process that writes more than that to a pipe no one is reading blocks on `write()` and never exits. `wait_timeout` then returns `Ok(None)` at the deadline, the process group is killed (`src/action.rs:86-92`), and the function returns `exit_code: -1, stdout: "", stderr: "command timed out..."` — the command's actual output (including a successful exit that never got the chance to happen) is discarded entirely, not truncated. This is the identical mechanism, at the identical file, that shirabe's own research (`explore_koto-runs-commands_r3_lead-deadlock.md` on shirabe's `docs/koto-runs-commands` branch) already traced in detail.

The connection to #193 specifically: **when a `koto` invocation is nested inside a gate or `default_action` command** (i.e., a template's shell command itself shells out to `koto`), the nested `koto` process's stderr — including every `migrate_if_needed` skip line, on an install with enough accumulated collisions — becomes the *outer* `run_shell_command`'s captured stderr stream. If that nested invocation's stderr payload exceeds the pipe buffer (which #193's own measured ~1000+ lines / ~100KB comfortably does), the nested `koto` blocks on writing to its own stderr pipe, the outer `run_shell_command`'s `wait_timeout` never sees it exit, and the outer call reports a false `timed_out` after 30 seconds even though the nested command would otherwise have succeeded. This is confirmed as a real, verified-at-the-code-level connection: #193's own warning volume is large enough to be exactly the kind of payload that trips the deadlock in `action.rs`, and the two defects compound specifically in the scenario where koto commands are nested inside koto-executed shell commands.

**Scope of exposure today, for completeness (from the same prior research, re-verified against current line numbers):** of shirabe's shipped `type: command` gates, most are structurally immune to the deadlock because their shell composition (`$(...)` command substitution, piping to `test`/`grep -q`/`[`) keeps the *outer* captured stdout/stderr near zero regardless of inner command verbosity; the one gate that writes directly to captured stdout (`tests_passing`, running `go test ./...`) was measured well under the 64KB trigger size on the reference monorepo at the time of that research. The #193-nesting scenario is the one place this moves from theoretical to self-inflicted, because it's koto's own diagnostic noise, not template-author-controlled command output, that supplies the payload.

### Implications for Requirements

- The PRD can state, as a testable current-behavior requirement, that koto emits unstructured stderr warnings on every session-touching invocation once a name collision exists in the legacy per-repo layout, with no cap, no dedup across invocations, and no structured/JSON representation — verifiable by reading `migrate_if_needed` directly, no execution needed.
- The PRD can state, separately, that `run_shell_command` reads child output only after `wait_timeout` returns, making any command (nested-`koto` or otherwise) whose combined stdout+stderr exceeds the OS pipe buffer indistinguishable, from the caller's perspective, from a genuine timeout — this is a general defect, not specific to #193, but #193 is a documented, reproducible way to trigger it via koto's own diagnostic output.
- Because both defects are independently real and independently fixable, the requirements can treat them as two separate testable claims joined by one causal-connection claim ("A causes/enables B under condition C"), rather than requiring one fix to imply the other.

### Open Questions

- Whether the current line numbers in `src/action.rs`/`src/session/local.rs` still match the prior shirabe-branch research exactly enough to cite verbatim, or whether this phase-2 pass's own citations (re-verified against the current koto working tree, branch `docs/koto-command-authority`) should be treated as the authoritative ones going forward — this document uses its own fresh reads.
- Whether other callers besides `migrate_if_needed` and `run_shell_command`'s own error paths (e.g., `sync_push_state`/`sync_pull_state`'s `eprintln!` warnings in `src/session/cloud.rs`, noted in Lead C) contribute meaningfully to the same nested-pipe hazard — not measured in this pass; flagged as a related but unverified surface.

## Summary

Today, the directory a gate or `default_action` command runs in is nothing more than `std::env::current_dir()` read fresh at the top of every `handle_next` tick (`src/cli/mod.rs:3082`) and threaded unchecked into `run_shell_command`; neither the persisted `StateFileHeader` nor the derived `MachineState` has any field naming a directory, tree, or repo root for this purpose, and the one directory-shaped field that does exist (`template_source_dir`) means something unrelated (the template file's own location) and is consumed only by the batch scheduler's child-template resolver. The three open design questions — silent vs. explicit anchor recording, refuse-until-bound vs. warn-once for legacy sessions, and root-equality vs. root-containment — are each named with their trade-offs in prior research but genuinely unresolved, and no existing `koto session` subcommand is a natural home for a deliberate rebind (the CLI's `Session` group has `Start`/`Dir`/`List`/`Cleanup`/`Resolve`/`Update`, none of which touch a directory). Cross-machine session migration, via the opt-in cloud backend, transfers a session's full header-and-event-log file byte-for-byte on every mutation and read — the defect isn't a sync gap, it's that the field to carry execution location doesn't exist anywhere in the format being synced, upstream of transport entirely. Finally, koto issue #193's migration-warning volume is real, unconditional, unstructured stderr noise on every session-touching invocation, and it is confirmed — at the current source, not just by prior citation — to be exactly the kind of payload that can trip the separate `run_shell_command` pipe-buffer deadlock (reads happen only after `wait_timeout` returns) the moment a `koto` invocation is nested inside a gate or action, turning a log-noise annoyance into a false 30-second timeout.
