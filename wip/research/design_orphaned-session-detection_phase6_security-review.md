# Security Review (fresh pass): orphaned-session-detection

This is an independent re-review. A prior pass (`design_orphaned-session-detection_phase5_security.md`)
recommended Option 2 (document considerations only, no design change). I
verified each of its four cited claims directly against current source
rather than taking them on faith, and looked specifically for anything a
first pass focused on classic disclosure/escalation/supply-chain framing
might miss.

## Claims re-verified against source

1. **Same-trust-boundary I/O.** Confirmed. `LocalBackend` roots sessions at
   `~/.koto/sessions/<id>/`. `ensure_koto_root` (`src/session/local.rs:818-844`)
   sets `0o700` on the `~/.koto` ancestor, and `init_state_file`
   (`src/session/local.rs:264-272`) sets `0o600` on the state file. True as
   far as it goes -- **but** `ensure_koto_root` only chmods on the
   `needs_create` branch (line ~838: `if needs_create { ... set_permissions
   ... }`). An install whose `~/.koto` predates this enforcement, or was
   created by an older koto binary, or has had its mode changed since, is
   never retroactively fixed. The "same trust boundary" argument is
   correct for fresh installs; it is an unverified assumption, not a
   guarantee, for upgraded ones. Not introduced by this design, but the
   design's Security Considerations section states the permission fact as
   settled without that caveat.

2. **`machine_id` and `template_source_dir` are not new disclosures.**
   Confirmed. `current_machine_id()` (`src/engine/path_resolution.rs:66-79`)
   is the same `pub(crate)` function the scheduler already calls, and
   `SchedulerWarning::StaleTemplateSourceDir`'s JSON (`kind`, `path`,
   `machine_id`, `falling_back_to`) is already printed to stdout via `koto
   batch` (`src/cli/batch.rs`, `warnings` field on the tick outcome). The
   claim that this data already reaches the same local user through an
   existing channel holds up.

3. **`Path::exists()` gates messages, not actions -- no TOCTOU needed.**
   Confirmed structurally: no create/delete/spawn is conditioned on the
   result anywhere in the design, and Direction 3 (destructive sweep) is
   out of scope. Correct as a TOCTOU/correctness argument.

4. **The two `koto init` collision-error strings are "byte-identical."**
   **Not confirmed -- this is currently false.** The pre-check at
   `src/cli/mod.rs:1685` emits: `"workflow '{}' already exists; run \`koto
   session cleanup {}\` to reuse the name, or \`koto cancel --cleanup {}\`
   to stop a running workflow first"`. The `SpawnErrorKind::Collision`
   handler at `src/cli/mod.rs:1713` emits only: `"workflow '{}' already
   exists"`. These are materially different strings today, despite the
   code comment at line 1710 ("Match the pre-check's error text so callers
   can rely on a stable ... string") and the design's "Implicit Decision"
   section asserting the code "deliberately keeps the pre-check's error
   text ... and the ... collision handler's error text byte-identical."
   This doesn't change the security posture (no new disclosure either way,
   since both are the same "already exists" fact), but it means one of the
   design's own "verified against source" claims doesn't hold, which
   should temper confidence in the rest of the section until an
   implementer re-checks it once code is touched. Low severity, but a
   factual defect worth fixing at implementation time regardless.

## Gap not covered in the existing Security Considerations / DoS analysis

The prior review's Resource Exhaustion section only reasons about
**attacker-driven amplification** ("an attacker would need to invoke `koto
init` many times themselves") and concludes negligible risk. That's correct
as far as it goes, but it isn't the shape of risk this change actually
introduces. `Path::exists()` is a `stat()` syscall, and `stat()` on a path
that resolves through an **unreachable, not merely absent** mount (a stale
NFS handle, a hung FUSE/bind mount, a network filesystem the ephemeral
sandbox used) can block for a long time rather than returning ENOENT
promptly. That failure mode -- unreachable mount, not absent path -- is
exactly the scenario this feature exists to detect (reaped ephemeral
sandboxes, torn-down containers, removed worktrees), so it isn't a
far-fetched edge case; it's adjacent to the design's own core motivating
scenario.

The new exposure to this isn't confined to `koto init`'s O(1)-per-call path
(the only path the existing Security Considerations section discusses).
`SessionInfo.template_source_status` is populated by `LocalBackend::list()`
for **every** session on **every** call, and per
`src/cli/dashboard_data.rs:290`/`562` plus `src/cli/dashboard.rs:195-242`,
`backend.list()` is invoked on every dashboard refresh tick (default 500ms
poll interval, throttled by `poll_every_n_ticks`, but still a recurring
background call for as long as the TUI runs) -- a comment already in
`local.rs`'s existing `list()` confirms this call pattern predates this
design ("`list()` is called on every dashboard refresh tick"). Today that
loop pays one header-parse per session and no `Path::exists()` calls at
all. After this design, it pays one additional `stat()` per session, per
refresh, for as long as any session in the store carries a
`template_source_dir`. A single session whose recorded directory sits on a
hung mount would block that `stat()` call and, because the dashboard's
refresh is synchronous with the tick loop (`dashboard.rs:239-242`), freeze
the entire interactive TUI -- not just delay one `koto status`/`init`
invocation. This is a materially larger and more availability-sensitive
surface than the `koto init` collision path the design's Security
Considerations section analyzes, and it isn't mentioned anywhere in the
document.

This doesn't require re-opening Decision 1 or 2 -- the chosen shape is
still right, and a bare `Path::exists()` is still the correct check for a
read-only, informational feature. But the Security Considerations section
should say, explicitly, that `check_template_source_dir` inherits the
scheduler's existing willingness to block on a slow/hung stat(), that this
design multiplies that exposure from "once per scheduler tick" to "once
per session per dashboard refresh tick," and that if this turns out to
matter in practice (a hung network mount freezing the dashboard), the fix
is a bounded-timeout or async stat, not a redesign of the signal shape.
That's a documentation gap, not an architecture defect, but it's a real
gap in the "no material risk" framing as written -- the current section
implies the only cost dimension is call count ("O(1) per call"), when
latency-per-call on the new degenerate path is the actual risk.

## Answers to the four review questions

1. **Attack vectors not considered:** Yes -- the DoS/availability angle is
   about a hung `stat()` call amplified across the dashboard's polling
   loop, not attacker-driven load. Not classically "attacker" framed, which
   is likely why the prior pass's amplification-focused DoS section missed
   it.
2. **Mitigations sufficient for identified risks:** For the risks the
   design does identify (permission scope, disclosure, TOCTOU), yes, the
   existing framing holds up under direct source verification.
3. **"Not applicable"/quick justifications that are wrong or too fast:**
   The "byte-identical" premise in the Implicit Decision section is
   factually wrong against current source (see #4 above) -- low security
   impact, but it's exactly the kind of unverified claim this review was
   asked to check. The DoS section's "O(1) per call, same class as
   existing reads" framing answers the wrong question (call count instead
   of call latency) and should not be treated as closing the topic.
4. **Residual risk to escalate:** One item worth a documentation addition
   before this ships, not a design change: note the stat()-on-hung-mount
   risk and its amplification via the dashboard's per-tick `list()` call,
   and fix the byte-identical claim in the Implicit Decision section (or
   verify/restore it in the same PR that touches those two call sites).
   Neither blocks Option 2 (document-only); both should be folded into the
   documentation this design already commits to writing.
