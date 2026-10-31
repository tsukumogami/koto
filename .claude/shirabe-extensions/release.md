# release extension: koto

koto's declarations for shirabe's `/shirabe:release`, read when it cuts a koto
release.

## Before release

Read before tagging. Neither item blocks a release mechanically: a failure is a reason to stop and look, and the releaser decides.

- Confirm the newest scheduled run of the Claude Code convention guard passed: `gh run list --workflow claude-code-conventions.yml --event schedule --limit 1 --json conclusion,createdAt,url`. A conclusion other than `success`, or a run more than eight days old, means the conventions haven't been checked against the newest Claude Code release; start `gh workflow run claude-code-conventions.yml -f version=latest` and read its result first.
- On a machine with a logged-in Claude Code, run `scripts/smoke-native-workflows-render.sh` and put its outcome in the release notes as `Checked against Claude Code <version>: /workflows entry renders`, or with the outcome it reported (`not rendered` or `unavailable`) in place of `renders`.

## After release

Run in order once the release is published. The version check stops the
list before `niwa apply` when the update didn't land the new binary (tsuku
hasn't seen the release yet, say), so a workspace is never re-applied with the
old one.

- `tsuku update koto`: update the installed koto to the newest release. An
  `@v<version>` install would pin the recipe source's ref, leaving a second or
  pinned install that later updates skip.
- `test "$(koto version | cut -d ' ' -f 1,2)" = "koto $RELEASE_VERSION"`: fail
  unless the binary on PATH reports the released version. `koto version`
  prints `koto X.Y.Z (<commit> <build date>)`, and koto has no `--version`
  flag, so the check compares the first two fields.
- `cd "${RELEASE_WORKSPACE_ROOT:?not in a niwa workspace}" && niwa apply`:
  re-apply every instance of the workspace this repository is in, so its
  instances pick up the new koto.
