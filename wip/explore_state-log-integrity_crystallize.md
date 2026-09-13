# Crystallize Decision: state-log-integrity

## Chosen Type

Split. PR 1 goes straight to `/work-on` on the existing issues #236 and #200:
refuse writes to a log whose first line isn't a header. The concurrency work
(write lock, atomic rewrites, cloud pull) is a separate later task entering at
`/scope`. Decided by the coordinator at the explore checkpoint, following the
review of round 1.

## Candidacy

- /execute: not a candidate. No `docs/plans/PLAN-*.md` and no `schema: plan/v1`
  file exists.
- Competitive analysis: not a candidate (public repo).

## Rationale

Round 1 found one root cause for both issues: every append goes through
`persistence::append_event`, which creates a missing file and never checks for a
header, and `context add` recreates a missing session directory first. That part
is fully specified: the guard, the error contract (`workflow '<name>' not found`,
exit 2, before `store.add`), the tests and the doc changes are all known. Nothing
in it needs a design, so it's issue-sized work on issues that already exist.

The concurrency findings (duplicate seqs, lost appends under rename-replace,
fused lines, an unlocked cloud pull) are real and measured. But they need design
decisions nobody has made yet: the sidecar lock path, the deadline, the error and
exit code for contention, and how the write lock relates to the tick lock. That's
`/scope` altitude, and adding it would widen the PR past what closes the two
issues. The brief asked for exactly this split if #200 turned out to need a
larger change.

## Stage 1 Evidence

### Signals Present

- A chain: converged on something to build; decisions need a durable home; a
  scope boundary emerged (refusal vs locking vs recovery); the core question is
  "what do we build and how".

### Anti-Signals Checked

- Spike report: feasibility was never the question (present, demoted).
- Decision record: several related decisions, with work attached (present,
  demoted).
- Rejection record: the conclusion is to proceed (present, demoted).

### Ranking

- A chain: 4
- Spike report, decision record, rejection record: demoted

## Stage 2 Evidence

### Signals Present

- File an issue / `/work-on` (PR 1): the issues already exist; one person, one
  PR; the error contract and tests are specified; the next step is to just do it.
- `/scope` (concurrency work): how to build it is open; lock design decisions
  are needed; there are several viable paths (sidecar vs inode lock, blocking vs
  bounded wait).

### Anti-Signals Checked

- File an issue for the whole thing: architectural decisions (the lock design)
  were made during exploration. Present for the concurrency part, absent for
  PR 1.
- `/scope` for the whole thing: one person can act on the refusal without a
  written contract. Present for PR 1.

### Ranking

- PR 1: `/work-on` on #236 (existing issue; closes #236 and #200)
- Concurrency work: `/scope`, later

## Tiebreakers Applied

- `/scope` vs file an issue: can one person act without a written contract? For
  the refusal and the header guard, yes, so `/work-on`. For the write lock, no,
  so `/scope`.

## Alternatives Considered

- **One PR through `/scope`**: rejected at review. It mixes three levels of risk
  (refusal, a new failure mode on every writer, automatic data moves).
- **Scheduler moves headerless child logs aside**: out of PR 1. It's the user's
  call, and a small follow-up if approved.
- **Tolerant reader**: rejected. It can't make a session tickable and it hides
  lost history.
