# Verdict: PASS

## Findings

**1. "No new attack surface" — NONE (verified, claim holds).**
Checked every element of the claim against the code the change touches. The new
code is a `Copy` enum, one extra `&&` per event in an existing classification
match, and two zero-cost wrappers. No new input is parsed: `Boundary` is a
compile-time value chosen by which wrapper the caller names, never deserialized
and never derived from log content. No new file is opened: the natural path's
`backend.read_events` (`src/cli/mod.rs:4295`) and the directed path's in-memory
`post_events` construction (`src/cli/mod.rs:3405-3416`) both pre-date the change
and neither gains or loses a read. No command execution, no network path, no
privilege boundary. No allocation is made from an attacker-controlled size — the
one size-proportional allocation on either path is the directed path's
`Vec<Event>` clone of the tick-start events, which exists in the shipped tree
unchanged. The claim is accurate as written.

The rename of `instructions_delivered_this_occupancy` (a `pub fn` under
`pub mod persistence`) is semver-visible to downstream crate consumers, but
`docs/STABILITY.md` freezes only the four `SessionBackend` methods and the event
schema, so the design's "nothing in `docs/STABILITY.md` moves" is correct. Not a
security matter; noted so the claim is not taken as broader than it is.

**2. Starvation argument — MINOR (both halves verified; the universal quantifier
in the design is one case too strong).**

Half one, `koto status` returns the instructions without moving the workflow:
verified. `handle_status` (`src/cli/mod.rs:4977-5170`) reads the log, derives the
phase, and serves `directive`/`details`/`expects` from the template at
`src/cli/mod.rs:5112-5120` with no consultation of the delivery predicate at all.
The function contains no `append_event`, no `acquire_state_flock`, and no write of
any kind — I grepped the whole body rather than trusting the comment at
`src/cli/mod.rs:5077-5080` that asserts it. `details` is served whenever the phase
declares any and is not terminal; terminal phases carry no instructions to starve
for.

Half two, the pointer is gated on the phase declaring instructions rather than on
the response carrying them: verified at both sites. Natural path,
`src/cli/mod.rs:4310` — `if final_template_state.details.is_empty()`, i.e. the
*template state's* field, spliced at `:4313`. Directed path, `src/cli/mod.rs:3426`
— `if target_template_state.details.is_empty()`, spliced at `:3429`. Neither
consults `already_delivered` or `resp.carries_details()`. Suppression happens
earlier and independently, in
`NextResponse::with_details_suppressed_unless_full`
(`src/cli/next_types.rs:492`-region), so a suppressed response still gets the
pointer. The gating is already correct in the shipped tree and the change does not
move it.

The caveat: `with_directive_prefix` returns `Terminal` and `Error` unchanged
(`src/cli/next_types.rs:243-245`). `Terminal` is irrelevant — no instructions
exist to be starved of. `Error` is a real if narrow gap: a tick that errors at an
instruction-carrying phase carries neither details nor the pointer, so an agent
that has lost context and is guessing at evidence fields gets no recovery hint on
exactly the responses its guessing produces. It recovers on the next non-error
tick, and this is unchanged by the design, but the design's Security section
states the guarantee as "every response for an instruction-carrying phase carries
a pointer naming that command," which is false for the `Error` variant. Recommend
narrowing that sentence rather than changing code.

What would break the argument, concretely:

- Re-gating either pointer splice on `resp.carries_details()` or on
  `already_delivered` instead of on `<target>_template_state.details.is_empty()`.
  This is a one-token edit at `src/cli/mod.rs:4310` or `:3426`, nothing
  type-checks against it, and it converts the saving into the trap the design
  names. The design's Phase 3 already lists "the pointer on a suppressed
  response" as an integration case; that test is the only structural guard and
  should be treated as load-bearing rather than incidental.
- Teaching `koto status` to respect the delivery rule. Nothing in the code
  couples them today — status reads the template directly and never calls the
  predicate — but the coupling is cheap to add and would make starvation absolute.
- Making `koto status` append a delivery record. It does not, and doing so would
  not by itself starve anyone, but it would put a window-populating write on the
  one path that is supposed to be observation-only.

**3. Unbounded-scan argument — NONE (verified; the worst case is what the design
says it is, and no other consumer is affected).**

The chain is `delivery_window(events, state).iter().any(|e| matches!(..
InstructionsDelivered { state } if state == current_state))` — today's shape at
`src/engine/persistence.rs:1099-1106`, with only the slice function substituted.
`.any()` scans *forward* from the head of the slice and short-circuits on the
first match, so the relevant question is how far the opening arrival's delivery
record sits from the head.

Verified it sits near the head: the record is appended by the same tick that
printed the details, after the response prints, at `src/cli/mod.rs:4602-4613`
(natural) and `:3457-3468` (directed). On the natural path the events between the
opening transition and the record are the advance loop's gate evaluations plus any
`SchedulerRan`/`BatchFinalized` from the batch scheduler at `:4334+` — a handful,
independent of loop length. "First handful of events after it" is accurate.

The true worst case is a window containing no record: one forward pass over the
whole window, over data already in memory, and — this is the part worth stating
that the design does not — it happens *at most once per loss*, not once per lap,
because a scan that finds nothing delivers and appends a record, so the next tick
short-circuits again. So the cost is O(window) once, not O(window) per lap. That
makes the design's claim conservative rather than optimistic.

No other consumer is affected by the longer-lived window. `delivery_window` is
new and has exactly one caller. `epoch_slice` is `occupancy_slice` renamed with
the `AnyEntry` arm, so the gate classification's slice length is unchanged, and
its consumers (`src/cli/dashboard_data.rs:458`,
`src/workflows_surface/project.rs:183`) see no difference. The three open-coded
scans (`derive_evidence` `:722`, `derive_overrides` `:796`,
`derive_last_gate_evaluated` `:844`) keep their own boundaries and are untouched.
The events *list* itself does not change — only which suffix of it one predicate
considers.

**4. Log integrity and the koto#200 interaction — MINOR (the tampering argument
holds; the concurrency interaction is neutral-to-favorable, but the design should
say so rather than be silent).**

Permission model: state files are created `0600` on both paths
(`src/engine/persistence.rs:165` for appends, `src/session/local.rs:275` for
initialization), which is what actually gates write access to a session log on a
shared host. The design's "`~/.koto/` at 0700" is approximate: `ensure_koto_root`
(`src/session/local.rs:741-772`) sets `0700` only when it created the directory
(`if needs_create`), so a pre-existing `~/.koto` with looser modes is never
tightened, and session subdirectories are created by plain `create_dir_all` under
the process umask. Neither weakens the argument — writing another user's log still
requires write on a `0600` file, and creating files in their session directory
still requires write on a directory that a default umask leaves at `0755` — but
the conclusion rests on the file mode, not on the directory mode the design
cites. The substance of "an attacker who can write the session log already
controls the phase, the evidence and the gate verdicts" is correct: the same write
lets them append a `Transitioned` to any phase, which is strictly more powerful
than suppressing a `details` field.

The concurrent-writer interaction, which I expected to be the sharp edge, turns
out to be blunted by fail-closed reads. `read_log_inner`
(`src/engine/persistence.rs:628-700`) validates sequence contiguity and rejects
the whole log on a gap (`:663-671`) or on a malformed *non-final* line
(`:687-694`). Only a torn *final* line is silently recovered (`:677-686`). That
matters, because the scenario I was looking for — a mid-log arrival event lost
while later events survive, which under the new rule would reopen the window at a
prior visit and silently suppress on a genuine arrival — is a sequence gap, and a
sequence gap bricks the read for `koto next` and `koto status` alike instead of
producing a wrong answer. The differential silent-suppression case does not exist.

What remains is the torn-tail case, which drops exactly the last event. If that is
the delivery record, the window holds no record and the next tick re-delivers —
the safe direction, identical under both rules. If it is a transition,
`derive_state_from_log` (`:707-715`) also loses it and the predicate is asked
about a different phase entirely — again identical under both rules.

koto#200's own mechanism is visible at `src/engine/persistence.rs:139-182`: the
plain `append_event` computes `next_seq` from `read_last_seq(path)? + 1` and then
opens `O_APPEND` with no lock, so two concurrent appends can claim the same seq
and produce a log that `read_log_inner` then refuses. Note that
`append_event_idempotent` does take an exclusive flock (`:314`) and the plain path
does not — the delivery record uses the plain path. The interaction with the new
rule is *favorable*, not adverse: the record append is gated on
`resp.carries_details()` (`src/cli/mod.rs:4602`, `:3457`), so a suppressing lap
appends one event where today it appends two, roughly halving the write volume of
a long loop and correspondingly shrinking the window in which two writers can
collide. The one thing worth naming is that `koto status` is now the designated
recovery path and is equally dead on a log that has hit koto#200 — but so is
`koto next`, under both the old rule and the new, so the change creates no new
dependency on a path that fails differently. Reporting only; no fix attempted.

**5. Cross-session and multi-agent exposure — NONE (verified, no change).**
`instructions_delivered_this_window(events, state)` is a pure function over one
slice, and both callers supply the *current* session's own events —
`backend.read_events(&name)` at `src/cli/mod.rs:4295`, and the in-memory
`post_events` built from that same session's tick-start read at `:3405-3416`.
Nothing in the predicate or its callers reaches another session's log. On the
batch path, `run_batch_scheduler` (`src/cli/batch.rs:764-806`) reads the *parent's*
events solely to extract the task list from the latest `EvidenceSubmitted`, and
children are separate session files created through `init_state_file` with their
own initial events; no `InstructionsDelivered` crosses that boundary in either
direction. A freshly spawned child sits at its initial state with no entry event
naming it, so `delivery_window` falls back to the whole (short) log and the child
delivers on its first tick — exactly as today. The dispatch-epoch fence in
`src/engine/epoch.rs` is an unrelated mechanism and is not touched. One session
learns nothing new about another; a child inherits nothing new from a parent.

**6. Security-relevant cases the design does not mention — MINOR (two, both
small; and one non-finding I want on the record as checked).**

(a) The directed path's read becomes decision-bearing, and the design celebrates
that without saying what it now depends on. `post_events` is built from `events`
read at tick start (`src/cli/mod.rs:3094`) plus the synthetic
`DirectedTransition`. Under the shipped rule the answer was provably `false`
regardless of what that list contained, so its freshness was irrelevant. Under
`delivery_window` the scan reaches past the synthetic opener into that list, so
staleness now shows up in the answer as an absence. A concurrently-appended
delivery record missed by the stale read causes a re-delivery (safe); a
concurrently-appended arrival missed by it opens the window further back and could
suppress on a genuine arrival (unsafe direction). This requires two concurrent
`koto next` invocations on one session, which koto does not support and which
koto#200 already makes unsafe for unrelated reasons — so it is a sentence for the
design's Security section, not a code change.

(b) The pointer-coverage claim excludes `Error`, as detailed in finding 2.

Checked and explicitly *not* a finding: `koto status` fails open on a
template-hash mismatch — it reports `template_hash_mismatch` at
`src/cli/mod.rs:5057-5064` and still serves instructions from the unverified
template — whereas `koto next` fails closed and exits (`:3219-3231`). Because the
new rule makes `koto status` the sole delivery path inside a loop, this looked at
first like a shift of reliance from a fail-closed path to a fail-open one. It is
not: on a hash mismatch `koto next` exits before constructing any response, so it
delivers nothing under the old rule either. The asymmetry is real, pre-existing,
and deliberate (the shipped design accepted it precisely so recovery is not denied
at the moment it is needed), and this change does not move it.

I found no BLOCKING or MODERATE issue. The design's Security section is
substantively correct on all three concerns it rules out; the corrections above
are to the precision of two sentences and to two cases it omits.

## Verification log

| Claim | Evidence | Result |
|---|---|---|
| No new input parsed / file opened / command run | `src/engine/persistence.rs:1028-1106`; `src/cli/mod.rs:3405-3416`, `:4293-4299`; `src/cli/next_types.rs:479-501` | verified |
| No allocation from attacker-controlled size | only size-proportional alloc is the pre-existing `post_events` clone, `src/cli/mod.rs:3405-3416` | verified |
| Nothing in `docs/STABILITY.md` moves | `docs/STABILITY.md:107-134` freezes four `SessionBackend` methods only; `persistence` not listed | verified |
| `koto status` returns instructions without moving the workflow | `src/cli/mod.rs:4977-5170`; details served `:5112-5120`; no append/flock in body | verified |
| `koto status` ignores the delivery rule | `src/cli/mod.rs:5112-5114` reads `state.details` from the template, never calls the predicate | verified |
| Pointer gated on the phase declaring instructions, not on the response carrying them | `src/cli/mod.rs:4310` + `:4313` (natural); `:3426` + `:3429` (directed) | verified |
| "Every instruction-carrying response carries the pointer" | `src/cli/next_types.rs:243-245` — `Terminal` and `Error` returned unchanged | refuted for `Error` (MINOR) |
| Suppression is independent of pointer splicing | `with_details_suppressed_unless_full` at `src/cli/next_types.rs:479-501`, applied before the splice at both sites | verified |
| Delivery record short-circuits near the head of the window | appended post-print at `src/cli/mod.rs:4602-4613` and `:3457-3468`; intervening events are gate evals + scheduler events, loop-length-independent | verified |
| Worst case is one pass over in-memory data | `.iter().any()` at `src/engine/persistence.rs:1100`; a no-record scan delivers and appends, so it is once per loss, not per lap | verified (design is conservative) |
| No other consumer affected by the longer window | `epoch_slice` == old behavior for `src/cli/dashboard_data.rs:458`, `src/workflows_surface/project.rs:183`; open-coded scans `:722`, `:796`, `:844` untouched | verified |
| Log-write access already implies full control | `0600` state files at `src/engine/persistence.rs:165`, `src/session/local.rs:275` | verified |
| "`~/.koto/` at 0700" | `src/session/local.rs:762-772` — set only `if needs_create`; session subdirs use umask | imprecise, conclusion unaffected |
| Corrupt log could silently suppress on a real arrival | `read_log_inner` rejects seq gaps `src/engine/persistence.rs:663-671` and non-final malformed lines `:687-694` | refuted — reads fail closed |
| Torn-tail recovery direction under the new rule | `:677-686`; lost record → re-deliver; lost transition → phase also changes | verified safe, no differential |
| koto#200 mechanism and interaction | unlocked `read_last_seq + 1` at `:145-149` vs. flock at `:314`; record append gated on `carries_details` `src/cli/mod.rs:4602`, `:3457` → fewer writes per lap | verified, interaction favorable |
| No cross-session read | predicate callers pass current-session events only, `src/cli/mod.rs:4295`, `:3405`; `run_batch_scheduler` reads parent events for the task list only, `src/cli/batch.rs:764-806` | verified |
| Child inherits nothing new | children get own state file via `init_state_file`; no entry event → whole-log fallback → delivers on first tick, as today | verified |
| Directed path's tick-start read now decision-bearing | read at `src/cli/mod.rs:3094`, consumed at `:3405-3416`; previously inert by the "provably false" argument at `:3377-3402` | verified — unmentioned in design (MINOR) |
| `koto status` fails open on hash mismatch vs. `koto next` fails closed | `src/cli/mod.rs:5057-5064` vs. `:3219-3231` | verified, pre-existing, no differential |

## Summary

The design's "no new attack surface" claim survives inspection: no new parsing,
file access, command execution, or size-dependent allocation, and the starvation
and unbounded-scan arguments both check out against the code — `koto status`
provably appends nothing and takes no lock, the recovery pointer is gated on the
template state rather than on the response, and the scan short-circuits on a
record the same tick writes. Two sentences are one case too strong: the pointer
does not reach the `Error` variant, and the `0700` cited for `~/.koto` is applied
only at creation (the argument rests on `0600` state files, which does hold).
Two things go unmentioned and deserve a line each — the directed path's tick-start
read becomes decision-bearing where it was provably inert, and the koto#200
interaction is worth stating because it is favorable: suppressed laps skip the
delivery-record append, halving write volume in a loop, while fail-closed
sequence validation rules out the silent wrong-answer case I went looking for.
Nothing blocking; PASS.
