---
schema: brief/v1
status: Accepted
problem: |
  Turning on koto's cloud backend means editing a file every live session
  on the host re-reads, the bucket key reaches every AWS tool, and a stopped
  session can't be imported into a fresh workspace on the same host without
  a new name, which breaks every caller that finds it by name.
outcome: |
  An operator puts one workspace on the cloud backend through its
  environment, sees where each session setting came from, keeps the bucket
  key under koto's own names, and moves a stopped session to a fresh
  workspace on the same host under its own name; other hosts see no change.
motivating_context: |
  A planned handoff between a coordinator session and its replacement
  starts from an imported session. koto imports only on its cloud
  backend, and the host that runs the coordinator is on the local backend
  with live sessions that must not switch. Session migration (see
  DESIGN-session-migration) made a cross-workspace move possible; this
  brief frames what stands between that verb and a host that can use it.
---

# BRIEF: koto-cloud-session-config

## Status

Accepted

## Problem Statement

`koto session import` moves a stopped session from one workspace's copy
in the bucket into a new local session, and it runs only on koto's cloud
backend. Three things stand between that verb and a host that wants to
use it.

The first is how the backend gets turned on. koto reads the session
backend and the cloud location (endpoint, bucket, region, path style)
from two places: the user file `~/.koto/config.toml` and a project file
in the exact working directory. koto re-reads the user file on every
command, so changing it on a host with running sessions moves all of
them to the new backend mid-run, including sessions nobody meant to
touch. A project file has to sit in the directory each command runs
from, and a workspace manager that provisions instances writes neither
file. Other koto settings, like the request store's limits and the
decider, already come from environment variables that a workspace can
set for its own processes; the session backend doesn't.

The second is credentials. The only environment variables koto reads a
bucket key from are `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`.
Setting those for a session's processes hands the same key to every
AWS-aware tool the session runs, which is more reach than a session
store key should have.

The third is names. Session names are machine-wide: under the cloud
backend every workspace on a host shares one local store. An import into
a second workspace on the same host is refused as `import_name_taken`,
because the source's own local copy still holds the name. The way out
today is `--as <new-name>`, and that breaks callers that find a session
by its name: shirabe's session convention says an import under another
name is outside it, and its import step never passes `--as`. So the one
move that should be easiest, between two workspaces on the same machine,
is the one that doesn't work.

## User Outcome

An operator who wants one workspace's sessions in the bucket sets
environment variables for that workspace's processes and leaves every
config file alone. Sessions in other workspaces on the same host keep
their backend. When they want to know why a session is or isn't on the
cloud backend, `koto config list` tells them which layer each session
setting came from: the environment, the project file, the user file, or
koto's default.

The bucket key travels under koto's own variable names, so the session's
processes don't export a key every AWS tool would pick up. Hosts that
already use the `AWS_*` pair or a config file see no change.

When a coordinator session has to move to a fresh workspace on the same
host, its replacement imports it under the name it already has, and
picks up where it stopped. Nobody picks a new name, and every caller
that finds the session by name still finds it.

## User Journeys

### An operator puts one workspace on the cloud backend

An operator running several workspaces on one host wants the sessions in
a new workspace to sync to a bucket, while the others stay local. They
add the backend, the cloud location and the bucket key to that
workspace's environment, start a session there, and run `koto config
list`, which shows each session setting with its origin as the
environment. A session in another workspace on the same host, started
before and after, stays on the local backend, and nobody edited
`~/.koto/config.toml`.

### A replacement coordinator takes over on the same host

A coordinator session is stopped mid-unit in one workspace. A fresh
workspace is provisioned beside it on the same host with the cloud
backend set in its environment, and the replacement runs the shared
import step, which names the session and the old workspace and nothing
else. The import keeps the session's own name, and the replacement's
next `koto next` continues from the state the first coordinator left.

### An operator upgrades an existing host and notices nothing

An operator whose host keeps its cloud settings in `~/.koto/config.toml` and its
bucket key in `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` installs
the new koto. The host's sessions stay on the same backend and bucket, with the
same credentials, and a host with no cloud settings at all stays on the
local backend. The only visible difference is that `koto config list`
now says where each session setting came from.

### Someone debugs a session that went to the wrong backend

A session that was meant to sync didn't. Its operator runs `koto config
list` in that workspace and sees that the bucket came from the project
file while the backend came from koto's default, because the variable
that should have turned it on was never set for that process. They fix
the environment, not the file.

## Scope Boundary

**In scope:**

- Environment variables, under a koto prefix, for the session backend and
  the cloud location: endpoint, bucket, region and path style.
- koto-prefixed credential variables that win over the `AWS_*` pair when
  both halves are set, with the `AWS_*` pair still working as today.
- The origin of each session setting in `koto config list`.
- An import into a fresh workspace on the same host that keeps the
  session's name, with no name passed by hand, and the source's copy
  refusing afterwards as it does on a cross-host move.
- The guide and the two designs this changes: config-and-cloud-sync and
  session migration.

**Out of scope:**

- A move between hosts. It already works by `--from` and `--as`, and its
  acceptance waits for a separate decision.
- Writing anyone's environment. Which workspace sets the variables, and
  the secrets behind them, is each operator's own configuration; this
  feature adds the names koto reads, not the entries that set them.
- Environment variables for the other cloud keys,
  `session.cloud.marker_retain_days` included: it guards a deletion from
  the bucket and stays in the user file.
- Making session names per-workspace in the local store. The same-host
  fix works with machine-wide names, not around them.
- Changes to shirabe's import step, unless the design finds koto alone
  can't keep the name.
- Moving the live sessions on any existing host to the cloud backend.
