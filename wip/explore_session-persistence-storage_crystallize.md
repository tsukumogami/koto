# Crystallize Decision: session-persistence-storage

## Chosen Type
PRD

## Rationale
The exploration established directional choices (files as medium, Terraform-style
backends, session dir API) but the requirements aren't locked: which backends are
MVP vs future, what the CLI commands look like from a user perspective, what the
config format is, what the cloud auth model is. A PRD captures what to build before
a design doc decides how. No backward compatibility constraints — there are no
existing users, so we can break the current wip/ model cleanly.

## Signal Evidence
### Signals Present
- Requirements are partially unclear: backends, API surface, config format need
  scoping decisions
- The core question is "what should we build" for this feature
- User stories are missing: developer experience for local, cloud, and git modes

### Anti-Signals Checked
- Requirements were partially provided by the user: present but not disqualifying —
  the user gave the problem and constraints, not the full spec

## Alternatives Considered
- **Design Doc**: partially fits (multiple technical approaches exist) but risks
  scope creep without locked requirements. Better as the follow-up after PRD.
- **No artifact**: not viable — this is a multi-component feature affecting the
  engine, CLI, config, and all skills.
