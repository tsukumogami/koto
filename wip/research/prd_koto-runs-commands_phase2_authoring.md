# Phase 2 Research: Authoring Guidance and Open Questions

## Lead A

### Findings

**Where `default_action` is documented today, exactly.**

The BRIEF's claim ("one row in a format table and a single integration test") is
accurate, and both citations resolve to single, specific locations.

- The row: `plugins/koto-skills/skills/koto-author/references/template-format.md:142`,
  inside the "Feature-to-action mapping" table (table starts at line 132):
  `| State with default_action + requires_confirmation | confirm |`. This is the
  *only* row in that table naming `default_action`, and it appears mid-file, well
  after the Layer 1/2/3 structure (see below) has already been laid out — an author
  reading top-to-bottom hits it as a side effect of learning what `action` values
  mean, not as a deliberate authoring section.
- No other prose in `template-format.md` mentions `default_action`: no schema
  block, no field list (`command`, `working_dir`, `requires_confirmation`,
  `polling`), no worked example, no guidance on when to reach for it versus a
  gate. The file's own worked examples (`references/examples/*.md`) never declare
  `default_action` — confirmed by `grep -rln "default_action:" --include="*.md"
  plugins/` returning nothing.
- The one other authoring-doc mention is
  `plugins/koto-skills/skills/koto-author/references/batch-authoring.md:88`, a
  single sentence noting `default_action` as one of three ways to write
  `failure_reason` in a batch worker's terminal state — again no schema, and
  scoped to the batch-worker convention, not general authoring.
- `koto-author/SKILL.md` (154 lines) never mentions `default_action`,
  `ActionDecl`, or the `confirm` action value at all. Its own action-dispatch
  instructions to the authoring agent (`SKILL.md:63-66`, "Check the `action`
  field to determine what's needed") list only `evidence_required`,
  `gate_blocked`, and `done` — omitting `confirm` outright, even though
  `template-format.md`'s own mapping table documents `confirm` as a real value
  two files away. The "What to expect" walkthrough (`SKILL.md:88-96`) and
  "Reference material" pointer list (`SKILL.md:98-107`) never route an author
  toward `default_action` authoring.
- The single test: `tests/integration_test.rs:3846-3870`
  (`template_with_default_action_creating_file`) is the only place in the entire
  repository — `grep -rln "default_action:" --include="*.md" .` across all of
  `docs/`, `plugins/`, and top-level `.md` returns exactly one hit, the design
  doc's own code snippet in `docs/designs/current/DESIGN-default-action-execution.md`,
  which is prose about the mechanism, not a usable template. The test's paired
  assertion (`default_action_creates_file_and_auto_advances`,
  `tests/integration_test.rs:3877-3924`) is the only place a `default_action`
  block runs end-to-end and is checked against real behavior anywhere in the
  codebase. There is no `.md` template anywhere — not in `test/`, `docs/`, or
  `plugins/` — that declares `default_action`.

**The documentation asymmetry between koto-author and koto-user.** koto-user's
`SKILL.md:72` documents the runtime/consumer side correctly and completely: its
action-dispatch table has a `confirm` row — "A default action ran and requires
your confirmation before advancing. Read `directive` and `action_output`
(command, exit code, stdout, stderr). Confirm if correct, or submit evidence to
redirect." So an agent *running* a workflow that happens to use `default_action`
is told what to do. An agent *authoring* a template that wants to use
`default_action` is not told the field exists in any structured way.

**koto's authoring documentation structure — what a template author reads.**

The two plugin skills, both under `plugins/koto-skills/skills/`:

- `koto-author/` — for an agent writing a new template or converting an existing
  skill to one. `SKILL.md` (154 lines) drives an 8-state koto-backed authoring
  workflow itself (states: `entry`, `context_gathering`, `phase_identification`,
  `state_design`, `template_drafting`, `compile_validation`, `skill_authoring`,
  `integration_check`). Its `references/` directory holds:
  `template-format.md` (882 lines, the core schema reference),
  `batch-authoring.md` (fan-out/batch-worker conventions), and
  `examples/` (four worked templates: `complex-workflow.md`,
  `evidence-routing-workflow.md`, `batch-coordinator.md`, `batch-worker.md`, each
  with a paired `.mermaid.md` diagram export where applicable).
- `koto-user/` — for an agent *running* an already-authored koto workflow.
  `SKILL.md` (496 lines) plus `references/`: `command-reference.md`,
  `response-shapes.md`, `error-handling.md`, `batch-workflows.md`.
- `koto-adhoc/` — a third skill for ad-hoc (non-template-backed) workflows, out
  of scope for this lead.

`template-format.md`'s "layer" structure (its own framing, stated in the file's
opening paragraph and echoed in `koto-author/SKILL.md:105-107`):

- **Layer 1: Structure** (`template-format.md:5-145`) — frontmatter schema,
  variables, states, transitions, directive-body sections, the
  `<!-- details -->` marker, and the feature-to-action mapping table (where the
  one `default_action` row lives). Marked "always read this."
- **Layer 2: Evidence routing** (`template-format.md:146-274`) — the `accepts`
  block, reserved evidence fields, the `when` condition, mutual exclusivity.
  Marked "read if your workflow has decision points."
- **Layer 3: Advanced features** (`template-format.md:275-799`) — gates
  (including `children-complete`), gate output fields, gate-output routing,
  `override_default`, self-loops, split topology, parent-child template pairs,
  `fan_out`/`synthesize`, mermaid previews, and a "Security note" section.
  Marked "read if you need gates, self-loops, or split topology."
- A final unnumbered "Batch template primitives" section (`:800-876`) and a
  "References" pointer (`:877-882`) close the file.

`default_action` is structurally homeless in this scheme: it isn't Layer 1
(it's not core structure — a template compiles and runs fine without it), isn't
Layer 2 (it's not evidence routing), and the one place it's mentioned sits inside
Layer 1's mapping table rather than inside Layer 3, where gates — the mechanism
it's most often compared against — actually live. An author following the
"read only the layers you need" guidance in `koto-author/SKILL.md:105-107` has no
layer that tells them to read anything about `default_action` at all; they'd
only encounter it by reading the mapping table closely enough to ask "wait, what
is `default_action`?" and then having no cross-reference to follow.

**Candidate homes for the authoring documentation, without choosing among
them:**

1. **A new subsection inside Layer 1 (Structure).** Argument for: `default_action`
   is a per-state field declared in frontmatter, structurally a sibling of
   `gates`/`accepts`/`transitions`, and Layer 1 is the layer everyone reads
   regardless of workflow complexity — this guarantees every author sees it at
   least once. Argument against: Layer 1 is currently pure structure (schema,
   no behavior/judgment calls); `default_action` uniquely changes engine
   *behavior* (auto-execution) rather than just declaring shape, which is a
   different kind of content than the rest of Layer 1.
2. **A new subsection inside Layer 3 (Advanced features), alongside gates.**
   Argument for: this is where the *judgment content* already lives — gates,
   `override_default`, self-loops are all "when should I reach for this
   mechanism and what does it cost" material, which is exactly the kind of
   guidance `default_action` needs (a decision rule between gate/action/prose,
   per Finding 5 below). It's also opt-in like the rest of Layer 3 ("read if you
   need..."). Argument against: `default_action` is not advanced in the sense
   gates or split topology are — it doesn't require understanding the rest of
   Layer 3 as a prerequisite, so filing it there could bury it behind material
   an author reaching for a simple auto-execute action doesn't need.
3. **A new, numbered Layer of its own** (e.g., "Layer 4: Command execution" or
   inserted as a new Layer 2, renumbering evidence routing to Layer 3).
   Argument for: gives `default_action` — and the authoring rule the BRIEF wants
   written (which commands the engine may run) — a dedicated, discoverable home
   proportional to its safety weight, matching how the design doc itself treats
   it as a distinct capability with its own Security Considerations section.
   Argument against: renumbering an established, cross-referenced layer scheme
   (SKILL.md's state-design instructions cite layers by number,
   `koto-author/SKILL.md:105-107`) has a footprint beyond this one doc, and a
   fourth layer for one field/one authoring rule may be disproportionate.
4. **A standalone reference file**, e.g. `references/action-authoring.md`,
   paralleling `batch-authoring.md`'s existing pattern (a focused guide for one
   feature area, referenced from `SKILL.md`'s "Reference material" list and
   read during a specific state). Argument for: this is the closest existing
   precedent in the same skill — `batch-authoring.md` already demonstrates
   "one feature, one file, referenced conditionally" and does so for a
   comparably-scoped capability (batch fan-out). It can hold the authoring rule,
   the schema, and worked examples without perturbing `template-format.md`'s
   existing structure or numbering. Argument against: fragments the "what
   states can do" picture across more files an author has to discover and
   thread together (`template-format.md` for gates/evidence, a separate file
   for actions), and `default_action` interacts with the feature-to-action
   mapping table that already lives in `template-format.md`, so some
   duplication or cross-referencing is unavoidable either way.
5. **koto-user's SKILL.md/references also need a corresponding fix**, independent
   of where the *authoring* guidance lands: its action-dispatch table
   (`SKILL.md:63-66`) omits `confirm` and `integration`/`integration_unavailable`
   entirely, which is a runtime-documentation gap sitting next to, but distinct
   from, the authoring-documentation gap this lead was asked about.

### Implications for Requirements

- A testable requirement can cite the exact absence: "the compiled feature has a
  single-row mention (`template-format.md:142`) and zero worked examples
  anywhere in `.md` form" is falsifiable today and will stay falsifiable after a
  fix (grep for `default_action:` across `--include="*.md"` should return more
  than one design-doc hit).
- Any requirement to "document `default_action`" should specify which of the
  candidate homes it targets, since the five options above are not equivalent in
  discoverability, and the BRIEF's own scope item ("real authoring documentation
  for `default_action`") doesn't pick one.
- A requirement should also close the koto-author/koto-user asymmetry (Finding
  2) — koto-author's SKILL.md dispatch table should at minimum list `confirm` as
  a value an authored template can produce, matching what koto-user already
  documents correctly.
- If the BRIEF's authoring rule ("which commands an engine should run") is meant
  to live beside the schema documentation, the two need to land in the same
  candidate home, since an author deciding whether to declare `default_action`
  at all needs the rule before the schema is useful.

### Open Questions

- Should the authoring rule (a durable statement of which commands qualify) live
  in the same file as the schema documentation, or in a separate policy
  document (e.g. a design doc's Security Considerations section, which is where
  the closest existing rule — "reversibility determines execution policy" —
  currently lives, per Lead D)?
- Does fixing koto-author's stale dispatch table belong in this feature's scope,
  or is it a pre-existing bug independent of the authoring-rule work?

## Lead B

### Findings

**What koto's context store is.** `ContextStore` is a trait
(`src/session/context.rs:24-39`) with five methods (`add`, `get`, `ctx_exists`,
`remove`, `list_keys`), implemented by `LocalBackend`
(`src/session/local.rs:517-660` roughly) alongside `SessionBackend`. Its own doc
comment states the intent plainly: "Agents submit and retrieve context through
this trait instead of writing directly to the filesystem. Content is keyed by
hierarchical path strings" (`src/session/context.rs:20-23`). It is exposed to
callers via `koto context add|get|exists|remove|list`
(`docs/guides/cli-usage.md:375-470`) and via two gate types
(`context-exists`, `context-matches`) that read it during gate evaluation.

**Where it lives on disk.** `<base_dir>/<session>/ctx/`
(`src/session/local.rs:418-419`, `fn ctx_dir`), with a manifest at
`ctx/manifest.json` (`local.rs:423-425`) tracking `{created_at, size, hash}` per
key (`src/session/context.rs:6-11`, `KeyMeta`). Actual content bytes are written
to per-key files under `ctx/` (`content_path`, exercised at
`local.rs:515-548`), separate from the manifest and — critically — separate from
the session's JSONL event log entirely. This is a structurally different storage
shape from `DefaultActionExecuted`'s stdout/stderr, which is embedded directly
in the state-file event log (per prior research,
`wip/research/explore_koto-command-authority_r1_lead-event-log.md`). A context
write only leaves a small metadata trace in the event log — the `ContextAdded`
event (`src/engine/types.rs:521-528`) carries `key`, `hash`, and `size`, not
content (`ContextAddedPayload`, `src/engine/types.rs:1444-1448`) — while the
actual bytes live only in `ctx/<key>`.

**Who can write to it.** Two distinct write paths exist, not one:
1. **Agent-invoked, via CLI verb.** `koto context add <name> <key>`
   (`src/cli/context.rs`, dispatched from `src/cli/mod.rs:1158-1161`) — an
   explicit, agent-issued command. This is the only path the CLI docs describe
   (`docs/guides/cli-usage.md:379-401`).
2. **Engine-internal, automatic.** The engine itself writes to the context
   store without any agent action, today, for one case: batch finalization.
   `src/cli/mod.rs:4470-4480` — when a batch's aggregate gate reports
   `all_complete`, the engine appends a `BatchFinalized` event *and* calls
   `context_store.add(&name, "batch_final_view", serialized.as_bytes())`
   directly inside the `koto next` handler, with a comment explaining why:
   "Persist batch_final_view to the context store so agents can retrieve it via
   `koto context get <wf> batch_final_view` without parsing the event log or
   terminal response." So the precedent for the engine autonomously writing to
   its own context store — no agent instruction, no `koto context add` call —
   already exists in the shipped codebase, independent of `default_action`.

   By contrast, `default_action`'s own execution path does **not** currently
   write to the context store: the `action_closure` in `src/cli/mod.rs` and the
   gate-evaluation closure both receive `context_store` as a read parameter
   (`Some(context_store)` passed into `evaluate_gates`,
   `src/cli/mod.rs:3956,4013`, for `context-exists`/`context-matches` gate
   reads), but nothing in the action-execution path (`src/cli/mod.rs:3985-4048`,
   per the prior confirmation-lead research) calls `context_store.add` today.
   The question this lead was asked to inform — would routing a
   `default_action`'s output into the context store be a *new kind* of write —
   has a concrete answer: no, the engine already writes to its own context
   store autonomously (batch finalization); it just hasn't been wired to
   `default_action` output specifically yet.

**Committed to a branch, or synced anywhere.** Not committed to git. Per
`docs/workspace-layout.md` (cited in prior event-log research) the whole session
directory, including `ctx/`, lives under `~/.koto/sessions/<id>/` by default —
outside any git working tree. It *is* synced when the opt-in cloud backend is
enabled: `src/session/sync.rs:126,143,164,187,207,247,256,259,277,290` all
reference `ctx/` — every context key and the manifest get pushed to and pulled
from the S3-compatible bucket under `<prefix>/<session>/ctx/<key>` and
`<prefix>/<session>/ctx/manifest.json`. Per the prior event-log research's
distinction between incremental and wholesale sync, context artifacts get
**incremental**, per-key sync (`sync.rs`), unlike the state file's wholesale
full-file push on every mutating call — but they are still, once cloud sync is
enabled, replicated in full to a shared, team-readable bucket, with no
client-side encryption (confirmed by the prior research's `grep -rni encrypt
src/` returning nothing, which covers this file too).

**Is a write reversible/undoable?** No mechanism unwinds it. `koto rewind`
(`handle_rewind`, `src/cli/mod.rs:1985-2081`) only appends a `Rewound` event
that repoints the session's current-state pointer to a prior state
(`EventPayload::Rewound { from, to, rationale }`); it walks
`state_changing` events (`Transitioned`/`DirectedTransition`/`Rewound`) to find
the target state and, for the `materialize_children` case, relocates children
to an epoch branch — but at no point does it touch `ctx_dir`, the manifest, or
call `ContextStore::remove`. A context key written before a rewind stays on
disk, in the manifest, and reachable via `koto context get` after the rewind,
even though the workflow logically returned to an earlier point. The only way
to make a key disappear is an explicit, separate `koto context remove` call,
documented as idempotent (`docs/guides/cli-usage.md:443-462`) precisely because
there's no automatic cleanup tied to state transitions. `context add` itself is
overwrite-not-append ("Overwrites any existing content for the same key," per
the CLI docs) — so a *later* write to the same key does erase the earlier
content, but nothing about rewinding, gate failure, or workflow branching
triggers that overwrite automatically; it only happens if a subsequent
`context add` call for the identical key occurs.

**Externally visible to anyone outside the local machine?** By default, no —
local-backend sessions never leave `~/.koto/sessions/`. With cloud sync enabled
(opt-in, requires `session.backend = "cloud"` and bucket credentials per
`docs/guides/cloud-sync-setup.md`), yes: every context write becomes visible to
anyone with read access to the configured team bucket, continuously and
automatically, the same team-wide exposure pattern the prior event-log research
established for the state file, just via incremental per-key sync instead of
wholesale.

### Implications for Requirements

- The "is a context-store write a side effect under the authoring rule"
  question has a factual answer to build the policy on: unlike a `git push` or
  `gh pr create`, a context-store write never leaves the local machine unless
  cloud sync is separately opted into, and even then it's koto's own storage,
  not a third party's system. It is *not reversible via rewind* today, which
  matters if the authoring rule's test (per the BRIEF's Journey 4 framing —
  "does the risk live in a bad success, or only in a bad failure") is meant to
  turn on undoability rather than external visibility.
- A requirement could state: local-only context writes are categorically
  different from repository/remote-service side effects and should be exempted
  from (or treated as a lighter case within) the authoring rule by default,
  with cloud-sync-enabled sessions called out as the one condition that changes
  the answer.
- Any requirement letting `default_action` write to the context store should
  note it is extending an existing pattern (engine-autonomous context writes
  already ship for batch finalization), not introducing one.

### Open Questions

- Does the authoring rule need a distinct answer for "cloud sync is enabled" vs.
  "local only," or is a single blanket answer ("context-store writes are not
  side effects for this rule's purposes") acceptable given cloud sync is opt-in
  and already carries its own disclosed trust model (per
  `docs/guides/cloud-sync-setup.md`)?
- Should `koto rewind` gain the ability to invalidate/remove context keys
  written after the rewind target, to make the store's reversibility match the
  event log's, or is "context writes are durable, rewind doesn't touch them" an
  acceptable permanent property to design the authoring rule around?

## Lead C

### Findings

GitHub's own documentation does not define a distinct notification category for
"new commits pushed to an already-open pull request" (a `synchronize`-shaped
event, in webhook terms), and it does not state that pushing to an open PR is
quieter than opening one. What it defines instead is a "reason" taxonomy for
*why* a given person receives a given notification, and that taxonomy has to be
used to infer the answer rather than reading it off directly.

- **The REST API docs for notifications** (`https://docs.github.com/en/rest/activity/notifications`)
  document a `reason` field with these values, each verbatim:
  - `subscribed` — "You're watching the repository."
  - `state_change` — "You changed the thread state (for example, closing an
    issue or merging a pull request)."
  - `author` — "You created the thread."
  - `comment` — "You commented on the thread."
  - `review_requested` — "You, or a team you're a member of, were requested to
    review a pull request."
  - `mention` — "You were specifically @mentioned in the content."
  - (plus `approval_requested`, `assign`, `ci_activity`, `invitation`, `manual`,
    `member_feature_requested`, `security_advisory_credit`, `security_alert`,
    `team_mention`, per the full table.)

  There is **no `push`, `synchronize`, or "new commits" reason** in this
  taxonomy. A commit pushed to an open PR's branch does not create a new,
  distinctly-labeled notification category the way opening a PR (`author`) or
  requesting review (`review_requested`) does.

- **"About notifications"**
  (`https://docs.github.com/en/subscriptions-and-notifications/concepts/about-notifications`)
  states you are "automatically subscribed to conversations by default when
  you have ... opened a pull request or issue ... [or] commented on a thread,"
  among other triggers (assignment, being requested to review, manual
  subscription, or not having disabled default repo/team watching). This
  establishes *who* is in the notified set for a PR's thread, but the page does
  not separately describe what happens on a subsequent push to that thread once
  someone is already subscribed to it.

- **"About email notifications for pushes to your repository"**
  (`https://docs.github.com/articles/about-email-notifications-for-pushes-to-your-repository`)
  describes a distinct, explicitly **opt-in** feature (per-repository, enabled
  under repository Settings → Notifications) that emails a configured address
  "when anyone pushes to the repository," listing new commits with a diff link.
  This is a repository-level watch-all-pushes mechanism, not part of the
  default participant/subscriber notification path, and the page does not
  state whether or how it distinguishes a push to a PR branch from a push to
  any other branch.

- **What can be inferred, not what is stated**: opening a PR fires `author` (for
  the opener — self, so no notification is actually sent to them for this
  reason) and, if reviewers are specified at creation, `review_requested` for
  each of them — a reason that reaches people who were not previously
  following the PR at all. A later push of new commits to that same open PR
  does not re-fire `review_requested` (no re-documented mechanism ties a push
  to review re-requesting) and does not re-fire `author`; anyone who receives a
  notification from that push receives it under the generic `subscribed`
  reason, which only reaches people already subscribed to the thread. On this
  reading, a subsequent push reaches a subset of (or the same set as, never a
  superset of) the audience that opening reached, because it can't independently
  trigger the reviewer-recruiting reason (`review_requested`) the way opening
  can. But this is an inference built by combining two separate doc pages'
  reason definitions, not something either page states about pushes-to-open-PRs
  directly.

### Implications for Requirements

- The BRIEF's framing — "one checkable claim about how loudly a push to an open
  pull request notifies" — is not fully checkable against GitHub's public
  documentation as a single, citable sentence. The documentation supports a
  *directional* claim (a push can't recruit new reviewer-notifications the way
  opening can, so it is no louder, and is likely quieter, for the already-
  subscribed audience) but does not supply a page that says "pushing to an open
  PR notifies more quietly than opening one."
- A requirement resting on this distinction should cite the reason taxonomy
  (`review_requested` fires only at open/re-request time, not on plain pushes)
  rather than asserting a general "GitHub docs say pushing is quiet" claim that
  isn't directly supported.
- If the PRD needs a firmer empirical answer than the docs give, that would
  require either GitHub support/staff confirmation or an empirical test (open a
  PR, observe notification reasons received by a set of watchers/reviewers,
  then push a commit and observe again) rather than further doc research — the
  public docs top out at the reason taxonomy above.

### Open Questions

- Is the directional inference (no push-triggered reviewer-recruitment reason
  exists, so a push can't be louder than opening for people not yet
  subscribed) sufficient to satisfy the BRIEF's falsifiable claim, or does the
  PRD need a harder empirical test given the docs don't state the comparison
  directly?
- Does the opt-in "email notifications for pushes to your repository" feature
  (which the docs don't say excludes or specially handles PR branches) change
  the answer for any repository where a maintainer has that setting enabled —
  i.e., is the "quiet" claim conditioned on that feature being off, which the
  BRIEF's framing doesn't currently account for?

## Lead D

### Findings

**No existing rule in koto or shirabe states, generally, which commands an
engine may run.** Neither repository has a document titled or scoped as a
command-authority policy. What exists is narrower and phrased as a safety
constraint tied to one flag, in two documents that both trace back to the same
authorship and the same week of work:

- `docs/designs/current/DESIGN-shirabe-work-on-template.md:541-543`: "**Safety
  constraint: reversibility determines execution policy.** Only reversible
  actions execute by default; irreversible or externally-visible actions (PR
  creation) require agent confirmation." This is followed by a reversibility
  table (`:544-552`) classifying five deterministic states as reversible (file
  overwrite, branch deletion ×2, read-only ×2) and separately listing
  `pr_creation` as `judgment (irreversible)` (`:688`) — i.e., not a
  `default_action` state at all, a judgment state where the agent itself runs
  `gh pr create`.
- `docs/designs/current/DESIGN-default-action-execution.md:444-446` (Security
  Considerations, "Reversibility constraint"): "The `requires_confirmation`
  flag prevents irreversible actions from auto-executing. The engine enforces
  this at the loop level — when the flag is set, the loop stops and returns to
  the caller." As prior research
  (`wip/research/explore_koto-command-authority_r1_lead-confirmation.md`)
  established in detail, this prose does not match what the code does
  (`requires_confirmation` gates *after* execution, not before), and the design
  doc's own architecture section elsewhere specifies the confirm-after
  behavior that shipped — so the "rule" as stated in prose and the rule as
  actually enforced by the code disagree with each other in the same document.

Neither statement is a general command-authority rule; both are scoped
narrowly to "does this one boolean flag gate execution," and both assume
reversibility, not any broader taxonomy (data classification, network
reachability, cost, etc.), is the entire criterion.

**`DESIGN-default-action-execution.md`'s Consequences section (`:453-478`)
concedes two risks**, under "Negative" (`:463-472`):
1. "Action output in the event log increases state file size, though bounded
   by truncation" — with the Mitigations subsection noting "Output truncation
   at 64KB prevents unbounded growth" (`:477`). (Its Security Considerations
   section, immediately above Consequences, separately and more sharply frames
   this as "**New risk: action output in event log.** ... If an action
   command's output contains secrets ..., those secrets end up in the event
   log. Mitigation: document that action commands should not produce sensitive
   output," `:435-441` — prior event-log research established this mitigation's
   premise, "committed to feature branches," was already stale by the time it
   shipped.)
2. "The polling loop blocks the CLI process during polling. If koto needs
   concurrent workflow advancement, this would need restructuring. Current use
   case (one workflow at a time) is fine" (`:469-471`), mitigated by "Polling
   timeout enforcement at compile time prevents indefinite blocking" (`:478`).

Both conceded risks are about *mechanism* (log growth, process blocking), not
about the authority question this feature (koto-runs-commands) is centered on
— neither risk is "the engine might run a command it shouldn't." That question
is absent from this design doc's own risk accounting.

**Issue #71's acceptance criteria** (`gh issue view 71 --repo tsukumogami/koto`)
include, verbatim: "Template schema supports marking an action as requiring
confirmation (irreversible flag), preventing auto-execution" — the same
reversibility-as-sole-criterion framing as the two design docs above, and (per
prior confirmation-lead research) an AC the shipped code does not actually
satisfy, since `requires_confirmation` gates after execution rather than
before it. The issue's Goal section states the same rule even more plainly: "A
safety constraint governs which actions auto-execute: only reversible actions
run by default. Irreversible or externally-visible actions (PR creation,
posting comments) must require agent confirmation."

**Nothing in shirabe states a broader or different rule.** A search across
shirabe's `docs/designs/current/*.md` and `docs/briefs/*.md` (as synced at
`origin/docs/koto-runs-commands`) for reversibility/engine-authority language
surfaces only unrelated uses of "irreversible" (chain-cascade finalization,
ROADMAP deletion, artifact-decision contracts) — none of it about koto's
command-execution boundary specifically. The one on-topic hit,
`DESIGN-execute-skill.md:236`, is a passing reference to "destructive/
irreversible action requiring confirmation" in a list of things that are "not
blockers" for a different mechanism (`/execute`'s own approval gate), not a
statement of a rule for what koto's engine may run.

### Implications for Requirements

- The PRD is **amending** existing guidance, not writing on a blank page: a
  rule already exists (reversibility gates auto-execution) in two design docs
  and one issue's AC, all converging on the same single-criterion framing. Any
  new authoring rule needs to explicitly supersede or reconcile with this
  existing text, particularly since prior research shows the existing rule
  already fails to match shipped behavior (`requires_confirmation` doesn't gate
  pre-execution).
- The BRIEF's own Journey 4 (the "bad success vs. bad failure" test) is a
  materially different criterion than "is this reversible" — it can be stated
  as a requirement that explicitly replaces the reversibility-only framing
  found in Lead D's citations, rather than one that's silent about the
  conflict.
- Because the design doc's Consequences section never names "wrong command
  runs" as a risk, a requirement introducing an authority rule is filling a gap
  the design's own risk accounting missed, not tightening an existing control.

### Open Questions

- Should the new authoring rule explicitly retire the "reversibility
  determines execution policy" language in
  `DESIGN-shirabe-work-on-template.md:541` and
  `DESIGN-default-action-execution.md:444-446`, or coexist with it as a
  narrower, redundant statement?
- Is issue #71's AC ("preventing auto-execution" via `requires_confirmation`)
  expected to be corrected as part of this feature, given the new authoring
  rule would make the flag's original purpose (gating irreversible actions)
  moot if irreversible actions are barred from `default_action` entirely
  (per the BRIEF's out-of-scope note on renaming `requires_confirmation`)?

## Summary

Lead A confirms the BRIEF's framing precisely: `default_action` has exactly one
documentation row (`template-format.md:142`) and exactly one working example, a
Rust integration test (`tests/integration_test.rs:3846-3924`) — zero `.md`
templates anywhere use it. koto-author's SKILL.md dispatch table is stale
relative to its own reference doc (omits `confirm`), unlike koto-user's, which
documents the runtime side correctly. Five candidate homes for authoring
guidance exist (new Layer 1 subsection, new Layer 3 subsection, a new numbered
layer, a standalone reference file mirroring `batch-authoring.md`, plus a
required parallel fix to koto-user's stale table) with different discoverability
trade-offs and no clear winner from the research alone.

Lead B establishes that koto's context store is local-machine-only by default
(`~/.koto/sessions/<id>/ctx/`), synced only if cloud sync is separately opted
into, and — the sharpest finding — **not reversible by `koto rewind`**: rewind
only repoints the state pointer via a `Rewound` event and never touches `ctx/`
or the manifest, so a context write outlives any rewind past it. The engine
already writes to its own context store autonomously today (batch finalization,
`src/cli/mod.rs:4470-4480`), independent of any agent `koto context add` call,
which means routing `default_action` output into context would extend an
existing pattern rather than invent one.

Lead C found that GitHub's own docs don't state the loudness comparison the
BRIEF needs directly — there's no `push`/`synchronize` entry in the documented
notification-reason taxonomy at all, only inferable structure: `review_requested`
fires at open/re-request time and not on a plain push, so a push can't recruit
new reviewer notifications the way opening can, but no GitHub doc states "a
push to an open PR is quieter than opening it" as a sentence. This is a
directional, not a direct, answer to the BRIEF's falsifiable claim.

Lead D found the reversibility-gates-execution rule already exists, verbatim,
in two design docs and issue #71's own AC — the PRD amends existing (and,
per prior research, already-broken) guidance rather than filling a void. The
design doc's Consequences section concedes two risks (event-log growth,
polling-loop blocking) but never names "the engine ran a command it shouldn't
have" as a risk at all — that gap is what this feature's authoring rule is
actually there to close.
