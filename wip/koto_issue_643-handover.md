# Handover: issue 643 (decision record, second consumer)

Remove this file before the pull request is ready.

## What the branch carries

Draft amendments for issue 643, not yet reviewed by a panel:

- `docs/prds/PRD-decision-record.md`: a Problem Statement paragraph on the second consumer; two
  goals; user stories 12 to 15; Terms for run, supersede marker and hop, with "above" and
  "established decider" extended; `judgment` (R1) now covers a standing ruling; R7's harness id
  is id-shaped; R22 gains a clause letting a person clear a block they caused through the same
  session; new requirements R37 to R48, numbered after R36 so R1 to R36 keep their meaning
  (succession R37 to R43, the one-decision read R44, the chain read R45, session-addressed
  commands R46, the harness id and the tick's driver R47, the hop cost R48); four new
  acceptance-criteria groups ("Succession", "Reading one decision and the chain", "Standing
  rulings and a person's own block", "The harness id and the tick's driver"); five out-of-scope
  items (who may fence, a carry riding the fence, koto-observed links, a verified person
  identity, the driver variable itself, which is #600's); two known limitations; six trade-offs.
- `docs/designs/DESIGN-decision-record.md`: frontmatter decision line; command table gains
  `koto decisions chain`, a wider `show`, `--fenced`, `supersede_unconfirmed`; new Decision 6
  (the write-once `koto/superseded-by` marker and the forward walk) with alternatives; Decision
  Outcome says six choices and describes the hop; components, key interfaces (path hop and marker
  bodies, chain output), data flow, Phase 2's additions, new Phase 5a, a security paragraph on
  standing through a fence, a negative consequence, and open question 3 answered.
- `docs/plans/PLAN-decision-record.md`: `issue_count` 14; scope summary and decomposition
  paragraph; Issue 1 gains the R44, R46 and R47 criteria; Issue 4 gains the person-clears-own-block
  criterion; Issue 10 gains the chain command and succession docs; Issue 13 walks the four new
  groups and needs Issue 14; new Issue 14 (successor standing, blocked by Issues 3 and 4);
  dependency graph and implementation sequence updated.

`shirabe validate --visibility=public` on the three files exits 0. No other check has run.

## Session state

No work-on session is open for issue 643. A local `koto session list` holds neither
`koto_issue_643` nor `issue_643`, and `koto session hierarchy koto_issue_643` reports it doesn't
exist. Planned open, from this worktree:

```
work-on-open.sh --workflow koto_issue_643 --var ISSUE_NUMBER=643 --var ARTIFACT_PREFIX=issue_643 <tokens.json>
```

with the tokens file holding `["https://github.com/tsukumogami/koto/issues/643"]`, outside the
work tree.

## Left undone

- Open the work-on session and run it as a docs change: review panel, verification, the pull
  request (which must cite tsukumogami/shirabe#1700), CI.
- Self-review points for the panel: whether R37's lock rule (the fence taking the manifest lock)
  is right against the fence's own append lock; whether R44's "any session on the host" read
  should be limited; whether R22's new clause needs a criterion under the cloud backend.
- Remove this file before the pull request is ready.
