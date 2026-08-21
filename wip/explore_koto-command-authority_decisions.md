# Exploration Decisions: koto-command-authority

Run in `--auto` mode. Each entry follows the lightweight decision protocol:
frame, gather from evidence in hand, decide, record. Status is `confirmed` where
the evidence was unambiguous and `assumed` where it was not.

## Round 1

- **The authority ruling is settled input, not a question.** (confirmed) Every
  lead was briefed that koto executing commands outside the agent's tool layer is
  deliberate and was instructed to design inside it. None re-litigated it. This
  exploration exists because of the ruling, not in spite of it.

- **Runtime containment is not the investment.** (confirmed) The blast-radius and
  anchoring leads independently concluded that every option which would bound
  what a command can reach — path allowlists, containers, restricted users, Linux
  namespaces, destructive-pattern denylists — either collapses into unenforced
  documentation or breaks koto's single-binary, no-sudo, four-platform
  distribution. The investment goes into deciding *what runs*, not into
  sandboxing what already runs.

- **Anchoring's promise is scoped down, deliberately.** (confirmed) It guarantees
  the directory a session can be ticked from, not the blast radius of an
  authorized command. The design should say that plainly rather than let "primary
  guard" imply containment it cannot deliver.

- **Template trust splits in two, and only half needs building.** (confirmed)
  Revision pinning already exists — a marketplace entry can pin a git-backed
  plugin to an exact commit `sha`. Binding a specific template to a reviewed hash
  at the moment `koto init` accepts it does not exist and is the half worth
  building. They compose rather than compete.

- **`--expect-hash` on `koto init` is the working shape for that half.**
  (assumed) It reuses `sha256_hex` and `compile_cached`, inventing nothing, and
  puts the check at the point where the trust decision is actually made. Not
  final — where the expected value is stored, and whether shirabe's
  `assert-child-template.sh` should carry it instead, are open.

- **Reversibility has two axes, and the second one governs.** (confirmed) Whether
  the local artifact can be reset is not the question. Whether the command fires
  an externally-visible event that cannot be un-fired is. `gh pr close` undoes a
  `gh pr create`'s state and not its notification.

- **The operative authoring rule: does the risk live in a bad success or a bad
  failure?** (confirmed) Risk in a bad failure is fixed by the action-output
  plumbing already scoped in the prior exploration. Risk in a bad success is
  fixed by nothing on the roadmap, because a post-hoc signal cannot gate an event
  that already happened. Commands of the second kind stay agent-run permanently,
  not pending a feature.

- **Both `gh pr create` sites and `gh pr ready` are permanently agent-run.**
  (confirmed) Conceded by the re-map lead against its own earlier verdict once
  the notification axis was applied. `git push` to a branch you solely own,
  before any PR references it, remains convertible.

- **Two disagreements are carried forward unresolved, on purpose.** (confirmed)
  Whether the finalization cascade's push and `--force-with-lease` belong with
  `gh pr create` or on the plumbing-then-convert path, and whether `gh pr edit`
  on the run's own draft is quiet enough to convert. The confirmation lead never
  evaluated either, so these are unadjudicated rather than settled splits, and
  the chain should decide them with the argument visible rather than inherit a
  false consensus.

- **`requires_confirmation` is a misnomer to be renamed, not a primitive to be
  built.** (assumed) Confirm-before-execute is buildable and expensive, and the
  one case motivating it is better served by the agent running the command
  through its own tool layer with a koto gate verifying the result. Assumed
  rather than confirmed because the rename's blast radius across templates and
  docs was not scoped.

- **`pr_creation` needs a verifying gate regardless.** (confirmed) It has no
  `gates:` block at all today; the state's only truth is the agent's self-report.
  Cheap, concrete, and worth doing independent of every other decision here.

- **The `command` field is the real event-log exposure, not stdout.** (confirmed)
  stdout and stderr are capped; the post-substitution command string is not, so a
  secret interpolated into a command's arguments is written down verbatim even
  when the command prints nothing. The design doc's "committed to feature
  branches" premise is stale; the live distribution path is the opt-in cloud sync.

- **One chain, not two.** (assumed) This exploration's conclusions amend and
  extend the `koto-runs-commands` chain rather than opening a parallel one.
  Running two chains against the same engine surface would produce conflicting
  designs. Revisit only if the template-trust work outgrows the rest.
