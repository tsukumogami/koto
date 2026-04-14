# Phase 6 Security Peer Review — Round 1 Delta (folded-in result)

## Context

Round-1 security review (`wip/research/design_batch-child-spawning_phase5_security_round1.md`)
concluded Option 2 — document-considerations, no design changes. Six
additions were folded into `docs/designs/DESIGN-batch-child-spawning.md`
lines 3768-3947. This peer review re-examines the folded-in text against
the round-1 concerns and against koto's source-of-truth for claims that
are asserted but not demonstrated (machine_id cloud-sync exposure,
flock/libc supply-chain, `--children=auto` default).

## 1. Attack vectors

### Confirmed-present, mitigated in folded-in text

- **SpawnedTaskMutated echoes both old and new `vars`** (CD11 + CD10).
  Folded-in text at L3797-3810 now warns explicitly, plus adds a
  best-effort scheduler-side redaction of `vars` keys matching
  `*_TOKEN|*_SECRET|*_KEY|*_PASSWORD`. The round-1 review suggested the
  redaction as optional; the design now commits to it as a concrete
  scheduler behavior. **Good.**

- **`paths_tried` / `compile_error` echo** (CD14). Folded-in text at
  L3864-3874 treats these as equivalent to state-file exposure, which
  aligns with koto's trust model (the user has read-access to both
  the state file and the error response).

- **`machine_id` on cloud-mode responses** (CD12). Verified against
  source: `src/session/version.rs:148-152` generates an 8-char SHA256
  prefix of the hostname, and `src/session/cloud.rs:206-209` / the
  `push_version` path pushes `version.json` (containing the same
  `machine_id`) alongside every state-file upload. The folded-in claim
  at L3876-3884 ("same value cloud sync already uploads with every
  state-file push") is **accurate**. Not a new capability.

- **`koto session resolve --children=auto` default.** Re-checked
  decision report at `wip/design_batch-child-spawning_decision_12_report.md`
  L465-476: `auto` applies remote *only when the local log is a strict
  prefix of the remote* (trivially reconcilable) or when parent-log
  consensus already forced the outcome. Non-trivial divergence
  escalates to per-child manual resolve. The round-1 review framed
  `--children=auto` as blanket-trust-remote; in practice it is
  trust-remote-only-when-trivial, with manual escalation otherwise.
  Folded-in text at L3908-3918 now accurately describes the escape
  hatches (`--children=skip|accept-local|accept-remote`).

### New weaknesses in folded-in text that the round-1 review did not flag

- **Secret-redaction heuristic is insufficient for common real-world
  patterns.** The folded-in list covers `*_TOKEN`, `*_SECRET`, `*_KEY`,
  `*_PASSWORD`, but MISSES:
  - `DATABASE_URL` (contains embedded credentials: `postgres://user:pass@host`)
  - `SESSION_COOKIE`, `COOKIE_*`
  - `*_AUTH`, `AUTH_*`, `BEARER`, `ACCESS_*`, `REFRESH_*`
  - Any `vars` *value* that contains a JSON blob with nested secrets
    (e.g., `vars.config = '{"api_key": "..."}'`) — the heuristic matches
    key name, not value content.
  - Arbitrary-named values that happen to hold secrets by convention
    (`GH_TOKEN` matches, but `GITHUB_PAT` doesn't unless trailing is
    tested; `OPENAI` doesn't match until the user adds `_KEY`).
  **Severity:** medium. The folded-in text acknowledges this is
  heuristic, not authoritative, and tells template authors they are
  responsible. That disclaimer is correct but under-sells the gap —
  `DATABASE_URL` in particular is the single most common place secrets
  travel through env-style `vars`. **Recommendation:** extend the
  heuristic list or add explicit "also `DATABASE_URL`, `*_URL` values
  with embedded auth, cookie-class keys" callout; or document that
  `vars` with unknown-shape values will be echoed verbatim and advise
  wrapping secrets in keys that match the redaction list.

- **Supply-chain claim for `flock` is incorrect.** Folded-in text at
  L3944-3947 says "uses the same `fs2` or `nix` crate already wired up
  for `LocalBackend`'s `ContextStore` writes." Inspection of
  `src/session/local.rs:211-244` shows koto calls
  `libc::flock(fd, LOCK_EX)` directly — not `fs2`, not `nix`. `libc` is
  already a direct dependency (Cargo.toml:31). **The fix is cosmetic**
  (replace "fs2 or nix" with "libc"); no new dependency is introduced,
  so the security posture is unchanged. But leaving the misattribution
  in the design doc could cause a future reviewer or implementer to
  pull an unneeded crate.

- **Flock is advisory; local-process bypass.** Folded-in text describes
  the parent lockfile as `flock(LOCK_EX|LOCK_NB)`. POSIX flock is
  advisory — a cooperating process that does not call flock can write
  the state file regardless. In koto's threat model (single-user local
  host, agents are trusted collaborators) a non-cooperating local
  writer is by definition koto itself misbehaving or a foreign tool
  with write access to the session dir (already privilege-equivalent
  to owning the state). **Not a new attack vector**, but the folded-in
  text does not mention advisory semantics. The fallback degradation
  (if flock fails non-blocking, a concurrent `koto next` retries or
  surfaces a `concurrent_tick` error per CD12) is safe: no state
  mutation happens without the lock. **Severity:** none inside the
  model; worth a one-sentence callout that the lock is advisory and
  that koto's contract holds only when all writers are koto processes.

### Confirmed-not-present

- **No privilege escalation.** `renameat2(AT_FDCWD, ..., RENAME_NOREPLACE)`
  operates on paths under the user-owned session dir with the invoking
  euid. `flock` on a user-owned lockfile requires no new capability.
  `libc` was already direct. No new syscall surface crosses a
  privilege boundary. Round-1 "no privilege escalation" finding
  confirmed.

- **No shell injection via reserved-action invocation string.** Child
  names are R9-constrained (`^[A-Za-z0-9_-]{1,64}$`); no shell
  metacharacters admitted. Confirmed.

- **No new cross-machine trust expansion.** `--children=auto` is
  actually *more* conservative than `--children=accept-remote` because
  it requires trivial-reconcilability. A compromised remote that
  diverges non-trivially triggers manual resolve, not silent accept.
  Consistent with koto's existing posture.

## 2. Mitigation sufficiency verdict

| Risk | Mitigation in folded-in text | Verdict |
|------|------------------------------|---------|
| Secret-rotation echo on R8 rejection | Heuristic redaction + disclaimer | **Partial**: misses `DATABASE_URL`, cookie-class, nested-JSON values |
| `paths_tried` / `compile_error` echo | Equivalent-to-state-file framing | Sufficient |
| `machine_id` on cloud-mode responses | Matches existing cloud-sync metadata | Sufficient |
| `--children=auto` default | Escape hatches documented | Sufficient |
| `renameat2` / `libc` supply chain | Correctly asserts no new direct dep | Sufficient |
| `flock` supply chain | Misattributes to `fs2`/`nix` | **Cosmetic error**: should say `libc` |
| Advisory-flock bypass by local process | Not mentioned | Minor: one-sentence callout would close the loop |
| Resource bounds (1000 tasks, 10 waits_on, depth 50) | Hard limits documented | Sufficient |
| Shell injection via reserved_actions | R9 constraint prevents | Sufficient |

## 3. Residual risk items worth escalating

1. **Expand or re-frame the secret-redaction heuristic.** The current
   list is a convenient subset; real-world secret-carrying env vars
   often don't match. Two fix options:
   - Add `DATABASE_URL`, `*_URL` when value contains `@` (URL with
     embedded auth), `COOKIE*`, `*_AUTH`, `AUTH_*`, `BEARER*`,
     `ACCESS_*`, `REFRESH_*`.
   - Or reframe: redact by default, un-redact only keys on a declared
     allowlist. Safer default; more disruptive to debugging.
   Document explicitly that value-level pattern matching (e.g.,
   detecting `ghp_*` or `sk-*` token shapes) is out of scope.

2. **Correct the `fs2`/`nix` misattribution** in Security Considerations
   L3944-3947 to say `libc` (already used by `src/session/local.rs`
   via `libc::flock`).

3. **Add one-sentence note on flock advisory semantics.** Suggest:
   "The parent-level `flock` is advisory; koto's concurrency guarantee
   holds only when all writers to the session directory are koto
   processes. Foreign writers bypassing flock are out of scope, same
   as foreign writers modifying the state file directly."

4. **Optional: consider that SpawnedTaskMutated with mutated
   `template` paths can probe filesystem layout.** A malicious
   co-submitter could submit `template: "../../etc/passwd"` and
   observe whether the rejection is `TemplateNotFound` (path probed)
   or `SpawnedTaskMutated` (path differs from previous submission).
   Under the trusted-submitter model this is not a capability expansion
   (the submitter can already `cat /etc/passwd` directly), but a
   multi-agent shared-parent scenario would allow one agent to probe
   another's submitted `template` values via R8 diff.

None of these rise to "needs scope rework." All are either text fixes
or heuristic extensions.

## 4. Overall verdict

**Security story holds** — with three small text-level corrections:

1. Fix `fs2`/`nix` misattribution to `libc` (L3944-3947).
2. Extend the secret-redaction heuristic to cover `DATABASE_URL` and
   common auth/cookie patterns, or explicitly document that the
   heuristic is intentionally narrow and agents should use the declared
   patterns.
3. Add a one-sentence advisory-flock note to Symlink and directory
   assumptions or a new "Concurrency primitives" subsection.

No design change is required. No privilege boundary is crossed. No
round-0 or round-1 finding is invalidated. The cloud-sync
`machine_id` exposure, `--children=auto` trust posture, and
`renameat2`/`libc` supply-chain assertions all hold up against the
source of truth (`src/session/version.rs`, `src/session/cloud.rs`,
`src/session/local.rs`, `Cargo.toml`).
