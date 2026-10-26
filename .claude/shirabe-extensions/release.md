# release extension: koto

koto's declarations for shirabe's `/shirabe:release`, read when it cuts a koto
release.

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
