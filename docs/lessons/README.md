# Lessons from past failures

Each file here records one failure that went through koto's code: what failed,
the mechanism behind it, how to recognize the same failure again, and what to
check so it doesn't happen. Read the lesson for an area when an investigation
reaches it. You'll start from the known mechanism instead of rediscovering it.

A lesson is a record, not an instruction. It says what happened and how to spot
it. A standing rule that comes out of a lesson ("always check X before Y") is
its own change to whatever states the rule, reviewed on its own terms, and the
lesson links to it. If a check can catch the failure, the guard belongs in the
code or a test, and the lesson names it.

## Index

Find a lesson by the area your investigation is in, or by the words you'd
search for.

| Area | Lesson | Search words |
|------|--------|--------------|
| Session removal, storage backends and test isolation | [A test prune on an inherited cloud backend](session-prune-inherited-cloud-backend.md) | `koto session prune`, `--apply --all`, short retention, scratch `HOME`, `KOTO_HOME`, `KOTO_SESSION_BACKEND`, cloud backend, bucket copies deleted, sessions removed by a test |

## Writing a lesson

Name the file for the mechanism, in kebab-case, and give it these sections:

- **What failed**: what someone saw, in terms of koto's own behaviour.
- **The mechanism**: why it happened, with the koto version it applies to when
  a later release changed the behaviour.
- **How to recognize it**: the signs that a new failure is the same one.
- **What to check**: what to verify before the action that failed.
- **Guards**: the issues and pull requests that changed koto because of it.

Keep facts about the machine it happened on out of it: no host paths, user
names, counts of what was lost, or recovery steps. A lesson describes koto's
behaviour, so someone who never saw the incident can follow it, and it links
only to public koto issues, pull requests and docs.

Land a lesson by pull request, like any other change. Add its row to the index
above. If it covers an area the index doesn't list yet, update the lessons line
in the repository's `CLAUDE.md` in the same pull request, so the line and the
lessons are reviewed together. That line names every area covered and points
here; it's how someone working in a fresh checkout finds this directory without
being told.
