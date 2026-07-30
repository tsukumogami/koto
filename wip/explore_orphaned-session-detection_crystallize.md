# Crystallize Decision: orphaned-session-detection

## Chosen Type

Design Doc (scoped as a small addendum extending the existing
`docs/designs/current/DESIGN-batch-child-spawning.md`, not a freestanding
new document -- see Rationale)

## Rationale

The exploration found that koto already ships almost the exact mechanism
issue #189 asks for -- `SchedulerWarning::StaleTemplateSourceDir`, from
Decision 14 in DESIGN-batch-child-spawning.md -- but scoped narrowly to the
batch scheduler's per-tick path resolution. What's genuinely undecided is
*how* to extend that existing, working pattern to three new call sites
(`koto status`, `koto init`'s collision path, `koto session list`), and that
extension surfaces real architectural questions with more than one viable
answer:

- Where does the new session-level signal live -- a new variant alongside
  `SchedulerWarning`, a new warning enum scoped to status/list, or a field
  directly on the JSON response? (candidate-direction-fit, template-source-dir-plumbing
  leads)
- How does `SessionInfo` grow to carry `template_source_dir` without
  breaking its documented additive-only stability tier, and how does
  `CloudBackend`'s remote-only placeholder-row gap get handled?
  (candidate-direction-fit lead)
- What does the new flag/column get named, given `koto workflows --orphaned`
  already means something different and reusing the word risks real operator
  confusion? (candidate-direction-fit lead)
- Should the signal differ by `session.backend` (local vs cloud), given
  cloud-synced sessions are the main legitimate false-positive source?
  (opt-in-posture lead, left explicitly open)

These are technical "how" questions, not "what"/"why" questions -- the "what"
(flag sessions whose recorded source directory is gone) is clear and was
given as input by the issue itself. Multiple viable implementation paths
exist for the "how," and this exploration already made several decisions
during Round 1 (defer direction 3's sweep, reuse `StaleTemplateSourceDir`'s
shape/`machine_id` vocabulary, reject the `--orphaned` name for this signal)
that need to be preserved somewhere durable -- `wip/` is cleaned before
merge, so these choices are lost unless written into a committed doc.

The doc should take the form of an addendum to the existing
DESIGN-batch-child-spawning.md (a new Decision entry, following that doc's
own pattern) rather than a new standalone design doc, because the
load-bearing precedent -- `template_source_dir`, `StaleTemplateSourceDir`,
`current_machine_id()` -- already lives there, and a fresh document would
have to re-derive context that document already owns. This keeps the
project's own convention (Decision 14 already anticipated exactly this kind
of follow-on work in its Non-goals section) rather than fragmenting related
decisions across multiple documents.

## Signal Evidence

### Signals Present

- **What to build is clear, but how to build it is not**: the issue's ask
  (flag orphaned sessions) is clear; the mechanism/naming/backend-handling
  questions above are not resolved.
- **Technical decisions need to be made between approaches**: naming
  (avoid `--orphaned` collision), signal home (new warning type vs. reuse
  existing shape vs. plain field), backend-parity handling (local vs cloud).
- **Architecture, integration, or system design questions remain**: how the
  new signal interacts with `CloudBackend`/`sync_status`/`machine_id`
  (already-shipped remote-state architecture) so local-orphan and
  cross-machine-stale don't get reported two incompatible ways.
- **Exploration surfaced multiple viable implementation paths**: direction 1
  alone vs. 1+2 combined; new warning type vs. extending
  `SchedulerWarning`'s sibling concept; per-backend behavior variants.
- **Architectural decisions were made during exploration that should be on
  record**: direction 3 deferred, existing-shape reuse, `--orphaned` name
  rejected -- all captured in `wip/explore_orphaned-session-detection_findings.md`
  Round 1 Decisions and need a durable home before that file is cleaned.
- **Core question is "how should we build this?"**: yes, per Accumulated
  Understanding in the findings file.

### Anti-Signals Checked

- **What to build is still unclear**: not present -- the issue and research
  agree on what "flag it" means.
- **No meaningful technical risk or trade-offs**: not present -- the naming
  collision and cloud-backend false-positive risk are concrete, non-trivial
  trade-offs.
- **Problem is operational, not architectural**: not present -- this touches
  `SessionInfo`'s stability-tiered struct, a cross-backend consistency
  question, and where a new signal type lives in the codebase's existing
  warning taxonomy.

## Alternatives Considered

- **Plan**: Ranked lower and demoted by its anti-signal ("open architectural
  decisions need to be made first" -- present: naming, signal home, and
  backend-parity are all still open). A plan could sequence issues for
  direction 1 vs 2, but it can't resolve which the naming/architecture
  choice should be without first deciding it, which is exactly what a design
  doc is for. The tiebreaker rule (Design Doc vs Plan: does a design doc
  already exist for *this* topic?) also favors Design Doc -- the existing
  DESIGN-batch-child-spawning.md covers the adjacent scheduler-side
  mechanism but not these three new call sites.
- **No Artifact**: Ranked lower and demoted by its anti-signal ("architectural
  or structural decisions were made during exploration" -- present, per the
  Decisions list in the findings file). Direction 1 alone is small enough
  that a contributor could plausibly wing it, but the naming collision and
  backend-parity questions are exactly the kind of choice that goes wrong
  silently without being written down, and this issue already has one prior
  round of "someone built this without writing it down" (Decision 14 solved
  the same field for one narrow consumer and it took this exploration to
  even notice the more general fix already existed in embryo).
- **Decision Record**: A closer fit than PRD/Plan/No-artifact, since a real
  comparison-of-options happened. But its anti-signal ("multiple interrelated
  decisions need a design doc") is present -- this isn't one isolated
  decision, it's several coupled ones (naming, signal home, backend
  handling, direction-3 deferral) that read better as one coherent doc than
  as a single decision record.
- **PRD**: Ranked lowest -- its anti-signal ("requirements were provided as
  input to the exploration") is squarely present; the issue itself specifies
  the requirement, and no stakeholder-alignment-on-scope question surfaced.

## Deferred Types

None scored competitively.
