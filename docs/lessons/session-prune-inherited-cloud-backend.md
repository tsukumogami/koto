# A test prune on an inherited cloud backend

Area: session removal, storage backends and test isolation.

## What failed

A test script meant to run against a scratch store ran
`koto session prune --apply --all` with a one-second retention
(`KOTO_RETENTION_KEEP=1s KOTO_RETENTION_IDLE_PERIOD=1s`). Its scratch setup
didn't take effect, so the command ran against a real store that used the
cloud backend. A retention that short releases almost every finished
session. The prune removed them locally, and under the cloud backend each
removal also deleted the session's copy in the bucket. Running sessions and
the sessions the retention rule still kept survived. Nothing asked first, and
the removal couldn't be undone from koto.

## The mechanism

Three things lined up, and in koto before 0.25.0 none of them could be changed
by the usual test isolation:

- **The store followed `HOME`.** koto built `~/.koto` from the home directory in
  several places and didn't read `KOTO_HOME` at all. Overriding `HOME` was the
  only way to move the store, and `KOTO_SESSIONS_BASE` moved only the local
  backend's session directory.
- **The backend came from the environment.** `KOTO_SESSION_BACKEND=cloud` and the
  `KOTO_SESSION_CLOUD_*` variables select the cloud backend and its bucket on
  their own, with no config file. A test process inherits them from the shell
  that started it, so a scratch `HOME` still talked to the real bucket.
- **An apply removes remote copies.** On the cloud backend, `--apply` deletes each
  removed session's objects in the bucket as well as the local copy. The direct
  `--apply --all` form had no preview step, unlike the preview-then-plan path
  (`koto session prune` without `--apply`, then `--apply --plan <token>`).

So a scratch `HOME` gave a false sense of isolation. The store path might have
moved, but the backend, and with it the bucket the deletes went to, didn't.

## How to recognize it

- Sessions you didn't remove are gone, locally and from the bucket, right after a
  test or a trial run that used a short retention or a bulk selection.
- The command that ran was a direct apply (`--apply --all`, `--apply` with
  `--execution-dir` alone, or `koto workspace prune --yes`), not
  `--apply --plan <token>`.
- The process that ran it had `KOTO_SESSION_BACKEND` or `KOTO_SESSION_CLOUD_*` in
  its environment, or ran from a directory whose project config selects the
  cloud backend, while the test believed it was isolated by `HOME` alone.
- What survived matches what the retention rule keeps: running sessions and
  live hierarchies.

## What to check

Before any prune, cleanup, import or delete in a test or a trial:

- On koto 0.26.0 or later, point `KOTO_HOME` at the scratch directory, then run
  `koto session dir --json`
  and refuse to go on unless `backend` is `local` and `root` sits under the
  scratch directory. `backend_ignored`, when present, names a cloud setting that
  `KOTO_HOME` set aside, which is the run being protected rather than a problem.
- On koto 0.25.x, `KOTO_HOME` moves the root but an inherited cloud backend still
  applies, and on anything older `KOTO_HOME` does nothing. There, unset
  `KOTO_SESSION_BACKEND` and every `KOTO_SESSION_CLOUD_*` variable in the test's
  environment, and give it a scratch `HOME`, before running anything that removes.
- Unset `KOTO_SESSIONS_BASE`, or check that it also points under the scratch
  directory. The reported root doesn't cover it, and it wins over the root for
  the local backend's session directory.
- Don't rely on a scratch `HOME`. It moves none of the backend selection.
- Prefer the preview-then-plan path for any removal a person cares about.

The [CLI usage guide](../guides/cli-usage.md) documents `KOTO_HOME`, the
`koto session dir --json` fields and the prune refusals as they stand today.

## Guards

- [#458](https://github.com/tsukumogami/koto/issues/458), fixed by
  [#470](https://github.com/tsukumogami/koto/pull/470) in 0.25.0, is the product
  guard. On the cloud backend, `koto session prune --apply` without `--plan`,
  and `koto workspace prune --yes`, refuse with exit 2 and
  `prune_short_retention` when `retention.keep` or `retention.idle_period` is
  under an hour, the selection has no age bound of an hour or more, and
  something would be removed. The error names the retention, the backend, the
  store and how many sessions would go. `--confirm-short-retention` removes the
  set anyway.
- [#471](https://github.com/tsukumogami/koto/pull/471), in 0.25.0, makes
  `KOTO_HOME` move the whole koto root: sessions, including the cloud backend's
  local copies, the request store, the config file and the directory a prune
  walks.
- [#484](https://github.com/tsukumogami/koto/pull/484), in 0.26.0, keeps a root
  that `KOTO_HOME` points at off a bucket the root didn't choose. Under
  `KOTO_HOME`, only the root's own `config.toml` can select the cloud backend.
  An inherited `KOTO_SESSION_BACKEND=cloud`, or a project config asking for
  cloud, is ignored with a warning. The same change added
  `koto session dir --json`.
