# Exploration Findings: koto-command-authority

## Core Question

The author has ruled that koto executing commands outside the agent's tool layer
is deliberate: loading a skill that drives koto is itself the broad grant, so
invoking a koto-backed workflow authorizes every command that workflow bakes in.
That settles an objection and relocates the safety question. If the template is
where authority is granted, the template is the security boundary — and nothing
in koto or shirabe currently treats it as one. This exploration asks what that
boundary has to look like, what it lets through today, and how much more of the
existing workflows become convertible now that the permission argument withholds
nothing.

## Round 1

### Key Insights

**The trust chain is one sentence long.** (lead-template-boundary) Whoever
controls the file at the path the harness expands `${CLAUDE_PLUGIN_ROOT}` to
controls what `koto init` will execute. Every template route — `--template`,
`--from-stdin`, `--parent`, batch child spawn, shirabe's plugin-relative paths —
resolves to a plain local-disk read at `src/template/compile.rs:156-157`. No
network source, no cloud source, no config-driven source exists anywhere. That
is a genuinely bounding finding: the trust decision happens once, at install or
authoring time, and never at runtime.

**koto already built the enforcement half and applied it one step too late.**
(lead-template-boundary) A SHA-256 of the *compiled* template is recorded in the
state file header and fail-closed re-verified on nearly every session-mutating
command — the same read-hash-compare pattern at `src/cli/mod.rs:3210-3226`,
`src/cli/mod.rs:4750-4767`, and `src/cli/overrides.rs:178-199`. This is not a
cache key; it is an active integrity check. But it verifies that an
already-accepted template has not changed under a running session. Nothing asks
whether accepting it was right. The primitives for the missing half —
`sha256_hex`, `compile_cached`, and a `TemplateSubcommand` dispatch that already
follows "take a path, load it, report something" — are all present.

**The concrete proposal that falls out of it**: an optional `--expect-hash` on
`koto init`, so a caller can assert "only run if this compiles to exactly the
template I reviewed," reusing existing primitives and inventing nothing.

**shirabe checksums the binary and not the templates.** (orchestrator probe) The
release workflow runs `sha256sum shirabe-*` and publishes `checksums.txt`
(`.github/workflows/release-binaries.yml:93-112`), covering the compiled CLI. The
skills — and therefore both koto templates — ship as a Claude Code plugin from
`.claude-plugin/marketplace.json` with `"source": "./"`, a directory copy with no
hash, no manifest of digests, and no signature. The artifact that merely runs
validation logic is verifiable; the artifact that grants command authority is
not. Extending the existing `sha256sum` line is a smaller change than anything
else this exploration recommends.

**A revision-pinning surface already exists, and it halves the problem.**
(lead-plugin-integrity) Claude Code lets a marketplace entry pin a git-backed
plugin to an exact 40-character commit `sha`, which takes precedence over `ref`;
archive sources carry a real `sha256` that refuses the install on mismatch. So
"which revision of the templates am I running" is answerable today with
configuration rather than engineering. What does not exist for git sources is any
content hash, any install-time or update-time hook, and any load-time
verification — once installed, plugin files are trusted implicitly. The half
worth building is therefore binding a *specific template* to a reviewed hash at
the moment `koto init` accepts it. The two compose: pin the plugin so the file on
disk is known, check the compiled hash at init so nothing substituted something
else in between.

**No version pinning exists between shirabe and koto.** (lead-template-boundary)
No `KOTO_VERSION` anywhere in `install.sh` or the workflows; CI installs whatever
`tsuku install` currently resolves to. And shirabe's own cross-skill guard,
`assert-child-template.sh:14-28`, fails closed only on the child template's
*existence*, not its content — which makes it the natural place for a hash check
to grow.

**The blast radius, stated plainly.** (lead-blast-radius) A koto-spawned command
runs as `sh -c` with the invoking user's full identity, the complete inherited
environment including whatever secrets are in it, and unrestricted filesystem and
network access. Standing between it and the machine: a 30-second default timeout,
a process-group kill, and — for actions only — a 64KB log cap. Empirically, a
`setsid`-detached or simply backgrounded process defeats the kill outright. The
`--var` allowlist and compile-time checks guard substituted values and undeclared
variables; neither inspects what the template's own command string says.

**That baseline already exists and is not new.** (lead-blast-radius) The eleven
command gates shirabe ships today run through the identical path. Adopting
`default_action` widens how much runs this way; it does not introduce the
exposure. Any discussion of "should we let koto run commands" is arguing about a
line that was crossed when gates shipped.

**`requires_confirmation` fails the acceptance criterion of the issue that
commissioned it.** (lead-confirmation) `src/cli/mod.rs:3985-4048` executes the
command unconditionally and only afterward branches on the flag to choose between
`Executed` and `RequiresConfirmation`, both carrying identical already-obtained
output. koto issue #71 required "preventing auto-execution." The design doc's
security section says one thing and its architecture section describes the
interface that shipped; the contradiction went through PR #75 with zero review
comments, and a test at `advance.rs:2878-2936` encodes a `create-pr` action
reporting `"PR #42 created"` as its confirmation output. The flag is a misnomer,
not merely a late checkpoint.

**Its recommended fix is a rename and an authoring rule, not a primitive.**
(lead-confirmation) Confirm-before-execute is buildable — a new stop reason,
hash-bound approval evidence so an approval cannot be replayed against a
different command, rewind invalidation, and a third branch in the
override-evidence check — but expensive. And the one genuinely irreversible state
that exists, `pr_creation`, already has the agent run `gh pr create` itself with a
koto gate verifying the result, which the lead calls defensible on its own terms.

**Anchoring guards the home directory, not the blast radius.**
(lead-anchoring) The prior exploration's design holds up against the current
tree, and refusing on cwd mismatch fully closes the misdirection hazard it was
built for. But it constrains where a command *starts*, not what it can *reach* —
a command can name absolute paths or `cd` elsewhere regardless. Every option that
would close the reach gap (path allowlists, containers, restricted users, Linux
namespaces, destructive-pattern denylists) either collapses into unenforced
documentation or breaks koto's single-binary, no-sudo, four-platform
distribution. The honest scope is to promise the workflow's home directory and
say so, rather than let "primary guard" imply more.

**One implementation trap to fix before anchoring ships.** (lead-anchoring)
`Path::join` silently discards containment when `working_dir` is absolute, so the
absolute case has to be rejected explicitly ahead of the join. Everything else in
the design is sound, and root-plus-refuse-plus-safe-join should ship as one
inseparable increment, with the bind verb and structured errors following.

**The event log's stale premise, corrected.** (lead-event-log) The design doc's
claim that state files are committed to feature branches was accurate for a few
days and has not been true since local session storage moved to
`~/.koto/sessions/`. The real distribution path today is the opt-in cloud
backend, which uploads the entire state file wholesale to a team-shared bucket
after every mutating call, with no client-side encryption.

**And the sharper leak nobody had named.** (lead-event-log) `stdout` and `stderr`
are capped at 64KB (`src/cli/mod.rs:61,833-845,4025-4026`) — the only enforced
content bound in the log. The `command` field is persisted post-substitution and
unbounded, so a secret interpolated into a command's own arguments is written
down verbatim even when the command prints nothing. Gate-override payloads,
evidence fields, and init-time variables are likewise unbounded, and there is no
redaction anywhere in the codebase. Gate commands, by contrast, do not capture
stdout at all. Since neither shipped template uses `default_action` yet, current
exposure through this mechanism is zero.

**The ruling moved less than expected on conversion, and for an instructive
reason.** (lead-remap-remote) Of the ten writes-remote commands, two —
`orchestrator_setup`'s branch push and draft-PR create (`execute.md:395,397`) —
were already technically reachable before the ruling. What the ruling removed
was a separate recommendation arguing against shipping them anyway, so their
practical yield goes from zero endorsed to fully endorsed with no new capability
needed. A third, `/work-on`'s branch push at `phase-6-pr.md:20`, converts now too
and was surfaced by this re-map rather than by the ruling — the prior map simply
never broke it out as its own row. Five wait specifically on failure output
reaching the agent or on an evidence-consuming action capability, both
diagnosability gaps rather than authorization ones. Two stay agent-run for a
reason outside the principle entirely.

**The highest-value single conversion is the create-or-reuse block.**
(lead-remap-remote) `orchestrator_setup`'s branch decision block is the largest,
worst-guarded, highest-current-risk prose in either template, converts today at
zero koto cost, and is *strictly safer converted than it is now* — because
failures surface as an ordinary `blocking_conditions` result instead of silently
falling through, which is what the settled-branch incident did.

**A state split unlocks a safe half held hostage by a risky one.**
(lead-remap-remote) `gh pr ready` is trivially reversible, cheaply
gate-verifiable, and idempotent — it clears every bar for converting today on its
own. It is welded into the same `default_action` block as the finalization
cascade, the single highest-consequence write in the bucket, which genuinely
needs the diagnosability plumbing first. Splitting `plan_completion` into a
cascade state and a mark-ready state ships the safe half immediately. This is the
same state-granularity lesson the prior exploration drew for
`setup_issue_backed` — bundling blocks the part that was ready — applied here to
two mechanical steps of very different risk rather than to judgment mixed with
mechanics.

**Two commands are unreachable for a structural reason, not a capability one.**
(lead-remap-remote) `/execute`'s coordinated-mode `gh pr edit` and `gh pr close`
sit inside a prose iteration loop in SKILL.md, not inside any koto template.
Converting them would mean designing a koto-driven coordination-loop template,
which is a different and much larger project than anything else in the bucket.

**The disagreement resolved into the exploration's most useful idea: a second
axis.** (lead-remap-remote, reconciling with lead-confirmation) Asked to argue
with the confirmation lead rather than around it, the re-map lead found its own
table had scored reversibility on one axis only — can the local artifact be
reset — and missed an independent one: does the command fire an
externally-visible event that cannot be un-fired. `gh pr close` undoes a
`gh pr create`'s *state*; it does not undo the "opened" notification every
watcher already received. That second axis is what koto issue #71 was actually
about.

Re-scored on it, the lead conceded three of its own rows. `gh pr create` in both
templates and `gh pr ready` move to stays-agent-run: each fires an unrecallable
notification and, in the PR case, consumes an identifier, the instant it
succeeds. `git push` to a branch you solely own, before any PR references it,
survives as convertible on both axes — nobody is watching that ref.

**And the rule that falls out of it is durable, not a staging decision.**
(lead-remap-remote) Action output on the failure path fixes diagnosability of a
failed attempt. It does nothing for the success path, which is exactly where an
irreversible externally-visible event lives — structurally the same critique the
confirmation lead levelled at `requires_confirmation`, since a post-hoc signal
cannot gate an event that already happened. So the operative question for a
template author is **whether a command's risk lives in a bad success or a bad
failure**. Risk in a bad failure is fixed by the plumbing already scoped. Risk in
a bad success is not fixed by anything on the roadmap, and those commands stay
with the agent permanently.

**A correction that makes the recommended pattern true rather than aspirational.**
(lead-remap-remote) The confirmation lead called `pr_creation`'s
agent-runs-it-with-a-koto-gate-verifying pattern defensible. Checked directly:
`work-on.md:695` has no `gates:` block at all — `pr_creation` goes straight to
`accepts:`, and the state's only truth is the agent's self-reported `pr_status`
enum. Nothing independently confirms the PR exists. Keeping `gh pr create`
agent-run remains right, but the pattern is currently agent self-report, not
verification. Adding the gate is small, concrete, cheap, and worth doing
regardless of everything else here.

### Tensions

- **Two genuine disagreements survive the reconciliation, and both are worth
  carrying forward rather than papering over.** The re-map lead keeps the
  finalization cascade's push and `--force-with-lease` on the convert-after-
  plumbing path rather than banning them, arguing that pushing commits to an
  already-open PR fires a quiet "synchronize" event rather than the loud
  "opened" or "ready for review" one, and that `--force-with-lease` is a command
  that can refuse itself — it fails atomically before rewriting history if the
  remote moved, which `gh pr create` has no equivalent of. The confirmation lead
  never evaluated either push case, so this is unadjudicated ground rather than a
  settled split. Separately, `gh pr edit` on the run's own draft PR is flagged as
  contestable: editing a PR you just opened consumes no new identifier and
  notifies more quietly, but "quieter" is a judgment call rather than the bright
  line that "no notification at all" and "notification every time" both are.

- **Everything that would bound the blast radius is either theatre or breaks
  distribution.** The blast-radius and anchoring leads reached this
  independently. Nothing in the plausible option set meaningfully contains a
  careless or hostile template without changing what koto is. That argues for
  investing in the template boundary — deciding what runs — rather than in
  runtime containment, which is the opposite of where a conventional security
  review would start.

- **The debuggability goal and the exposure bound pull in opposite directions.**
  The prior exploration wants an action's stdout and stderr to reach the agent on
  the failure path. This one finds the log's contents under-bounded and
  cloud-synced. Both are right; the resolution is about where output travels, not
  whether it is captured.

### Gaps

- Where an expected-hash value should live: a koto-side trusted-templates file, a
  shirabe release manifest, or an argument the skill passes at init.
- Nobody has scoped what capping or hashing the `command` field would break for
  the audit trail it exists to serve.

### Decisions

Recorded in `wip/explore_koto-command-authority_decisions.md`.

## Decision: Crystallize
