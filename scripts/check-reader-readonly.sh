#!/usr/bin/env bash
# Check the rules that keep koto-session-reader unable to write, where the
# crate's clippy ban list can't see them.
#
#  1. lib.rs still carries #![forbid(unsafe_code)] and
#     #![deny(clippy::disallowed_methods, clippy::disallowed_types)].
#  2. No file under the crate's src/ carries an allow, expect or cfg_attr
#     attribute naming those two lints (or their old singular names),
#     clippy::all, clippy::style or warnings, and none carries
#     cfg(not(clippy)), outside the two sanctioned sites: src/fs_read.rs and
#     src/koto_root.rs. koto_root.rs holds exactly one such attribute,
#     #[allow(clippy::disallowed_methods)], on the line before its one
#     home-directory lookup, `let home = dirs::home_dir();`, and names
#     home_dir nowhere else.
#  3. custom_flags appears exactly once, in fs_read.rs, passed
#     mode_flags(&mode), whose arms return only IN_STORE_OPEN_FLAGS and
#     TEMPLATE_OPEN_FLAGS, defined once each as exactly
#     libc::O_NOFOLLOW | libc::O_NONBLOCK and libc::O_NONBLOCK.
#  4. The crate's clippy.toml still holds every required entry, and no
#     .clippy.toml beside it takes its place.
#  5. The crate has no build script (no build.rs, no `build =` key).
#  6. Neither the crate's manifest, the workspace root's, nor
#     .cargo/config.toml carries [patch] or [replace].
#  7. Every normal and build dependency of the crate, direct or not, comes
#     from the crates.io registry according to cargo metadata.
#  8. open_plain is defined once in fs_read.rs, which offers no other public
#     function besides read_store_file, and is called only as
#     crate::fs_read::open_plain(...) from the functions OPEN_PLAIN_CALLERS
#     lists; dashboard.rs and source.rs name nothing in SCAN_FORBIDDEN.
#  9. `cargo tree --depth 1 -e normal,build` lists no direct dependency
#     outside ALLOWED_DEPS, and the full `-e normal,build` graph names no
#     crate in BANNED_CRATES. Both use --target all, so a dependency behind
#     another platform's cfg still counts.
#
# Comment lines (// and its doc forms) are ignored by the source checks.
#
#   scripts/check-reader-readonly.sh [--self-test]
#
# Prints PASS:/FAIL: lines and exits 1 on any failure. --self-test copies the
# crate and the workspace manifests into a temporary directory, checks that
# the clean copy passes, then plants each violation above in a fresh copy
# and passes only if every planted copy fails with the expected message.
# It runs cargo with --offline, so the dependency cache must already hold
# the locked crates (any earlier build or clippy run fills it).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATE="koto-session-reader"

# Lines lib.rs must hold exactly.
FORBID_ATTR='#![forbid(unsafe_code)]'
DENY_ATTR='#![deny(clippy::disallowed_methods, clippy::disallowed_types)]'

# Files under the crate's src/ allowed to carry the lint allows.
SANCTIONED_ALLOW_FILES=("fs_read.rs" "koto_root.rs")

# The one allow koto_root.rs may carry, and the one call it covers.
ROOT_ALLOW_ATTR='#[allow(clippy::disallowed_methods)]'
ROOT_HOME_CALL='let home = dirs::home_dir();'

# The flag constants' definitions and the one custom_flags call.
IN_STORE_DEF='pub const IN_STORE_OPEN_FLAGS: libc::c_int = libc::O_NOFOLLOW | libc::O_NONBLOCK;'
TEMPLATE_DEF='pub const TEMPLATE_OPEN_FLAGS: libc::c_int = libc::O_NONBLOCK;'
CUSTOM_FLAGS_CALL='opts.custom_flags(mode_flags(&mode));'

REQUIRED_METHODS=(
  std::fs::write std::fs::copy std::fs::rename std::fs::remove_file
  std::fs::remove_dir std::fs::remove_dir_all std::fs::create_dir
  std::fs::create_dir_all std::fs::hard_link std::fs::soft_link
  std::fs::set_permissions
  std::fs::File::create std::fs::File::create_new std::fs::File::set_len
  std::fs::File::set_times std::fs::File::set_modified
  std::fs::File::set_permissions std::fs::File::sync_all
  std::fs::File::sync_data std::fs::File::lock std::fs::File::lock_shared
  std::fs::File::try_lock std::fs::File::try_lock_shared std::fs::File::unlock
  std::fs::OpenOptions::write std::fs::OpenOptions::append
  std::fs::OpenOptions::create std::fs::OpenOptions::create_new
  std::fs::OpenOptions::truncate
  std::os::unix::fs::OpenOptionsExt::mode
  std::os::unix::fs::OpenOptionsExt::custom_flags
  std::os::unix::fs::symlink std::os::unix::fs::chown
  std::os::unix::fs::fchown std::os::unix::fs::lchown
  std::net::ToSocketAddrs::to_socket_addrs
  std::env::set_var std::env::remove_var
  dirs::home_dir std::env::home_dir
  std::fs::File::open std::fs::File::options std::fs::read
  std::fs::read_to_string std::fs::OpenOptions::new
)
REQUIRED_TYPES=(
  std::fs::DirBuilder std::process::Command
  std::net::TcpStream std::net::TcpListener std::net::UdpSocket
  std::os::unix::net::UnixStream std::os::unix::net::UnixListener
  std::os::unix::net::UnixDatagram
)

ALLOWED_DEPS=(anyhow serde serde_json thiserror regex aho-corasick toml libc sha2 hex dirs)
BANNED_CRATES=(rust-s3 rustls attohttpc http url ratatui crossterm tempfile fs2 fs4 fd-lock nix rustix)

# Who may call open_plain, as <path under src/>:<function>. This is the
# first phase's list: the three log readers, generate_session_id, and until
# the template read moves to read_store_file, is_terminal_state and the
# dashboard's template read in read_detail.
OPEN_PLAIN_CALLERS=(
  engine/persistence.rs:read_fence
  engine/persistence.rs:read_header_only
  engine/persistence.rs:read_log_inner
  engine/persistence.rs:is_terminal_state
  engine/types.rs:generate_session_id
  dashboard.rs:read_detail
)
# Names dashboard.rs and source.rs may not use. Empty in the first phase;
# the log readers join it once the scan reads through read_store_file.
SCAN_FORBIDDEN=()

# --locked for a real run; the self-test swaps in --offline so a planted
# dependency can update the copy's lock file from the local cache.
CARGO_FLAGS=(--locked)

CRATES_IO='registry+https://github.com/rust-lang/crates.io-index'
CRATES_IO_SPARSE='sparse+https://index.crates.io/'

# Print each normal or build dependency of the crate, direct or reached
# through the resolved graph, whose source isn't crates.io.
# shellcheck disable=SC2016 # jq variables, not shell ones
FOREIGN_DEPS_JQ='
  def from_crates_io: . == $a or . == $b;
  . as $m
  | ($m.packages | map({key: .id, value: .}) | from_entries) as $pkgs
  | ($m.packages[] | select(.name == $crate) | .id) as $root
  | ($m.resolve.nodes | map({key: .id, value: [.deps[]
      | select(any(.dep_kinds[]; .kind == null or .kind == "build")) | .pkg]})
      | from_entries) as $graph
  | def reach($seen; $todo):
      if ($todo | length) == 0 then $seen
      elif any($seen[]; . == $todo[0]) then reach($seen; $todo[1:])
      else reach($seen + [$todo[0]]; $todo[1:] + ($graph[$todo[0]] // []))
      end;
    ( ($pkgs[$root].dependencies[]
        | select(.kind != "dev")
        | select((.source // "") | from_crates_io | not)
        | "\(.name) (declared \(if .path then "by path" else (.source // "with no source") end))"),
      (reach([]; [$root])[]
        | select(. != $root)
        | $pkgs[.]
        | select((.source // "") | from_crates_io | not)
        | "\(.name) \(.version) (source \(.source // "local path"))") )'

failed=0

fail() {
  echo "FAIL: $*"
  failed=1
}

pass() {
  echo "PASS: $*"
}

contains() {
  local needle="$1" item
  shift
  for item in "$@"; do
    [ "$item" = "$needle" ] && return 0
  done
  return 1
}

# Print a file with every comment line blanked, so line numbers still match.
strip_comments() {
  sed -E 's#^[[:space:]]*//.*$##' "$1"
}

# Print every attribute in a file as "<line>: <text>", whitespace removed,
# joining an attribute that spans lines.
attributes() {
  strip_comments "$1" | awk '
    {
      line = $0
      while (1) {
        if (!inattr) {
          i = index(line, "#[")
          j = index(line, "#![")
          pos = i
          if (j > 0 && (i == 0 || j < i)) pos = j
          if (pos == 0) break
          inattr = 1; depth = 0; text = ""; start = NR
          line = substr(line, pos)
        }
        closed = 0
        for (k = 1; k <= length(line); k++) {
          c = substr(line, k, 1)
          text = text c
          if (c == "[") depth++
          else if (c == "]") {
            depth--
            if (depth == 0) { closed = k; break }
          }
        }
        if (!closed) { text = text " "; break }
        gsub(/[[:space:]]/, "", text)
        print start ": " text
        inattr = 0
        line = substr(line, closed + 1)
      }
    }
  '
}

# The name of the function enclosing each line that names open_plain, as
# "<line>:<function>:<text>".
open_plain_sites() {
  strip_comments "$1" | awk '
    match($0, /(^|[^A-Za-z0-9_])fn[[:space:]]+[A-Za-z0-9_]+/) {
      s = substr($0, RSTART, RLENGTH)
      sub(/^.*fn[[:space:]]+/, "", s)
      fname = s
    }
    /open_plain/ { print NR ":" (fname == "" ? "<none>" : fname) ":" $0 }
  '
}

check() {
  local root="$1"
  local crate="$root/$CRATE"
  local src="$crate/src"
  failed=0

  if [ ! -d "$src" ]; then
    fail "$CRATE/src not found under $root"
    return 1
  fi

  # --- 1. lib.rs attributes ----------------------------------------------
  local bad=0
  if ! grep -qxF "$FORBID_ATTR" "$src/lib.rs"; then
    fail "lib.rs lacks $FORBID_ATTR"
    bad=1
  fi
  if ! grep -qxF "$DENY_ATTR" "$src/lib.rs"; then
    fail "lib.rs lacks $DENY_ATTR"
    bad=1
  fi
  [ "$bad" -eq 0 ] && pass "lib.rs forbids unsafe code and denies both ban lints"

  # --- 2. no lint allows outside fs_read.rs -------------------------------
  local lint_re='clippy::disallowed_(methods?|types?)([^A-Za-z0-9_]|$)|clippy::(all|style)([^A-Za-z0-9_]|$)|(^|[^A-Za-z0-9_:])warnings([^A-Za-z0-9_]|$)'
  local f rel hits
  bad=0
  while IFS= read -r f; do
    rel="${f#"$src"/}"
    contains "$rel" "${SANCTIONED_ALLOW_FILES[@]}" && continue
    hits="$(
      attributes "$f" | grep -E "^[0-9]+: #!?\[(allow|expect|cfg_attr)\(" | grep -E "$lint_re"
      attributes "$f" | grep -F 'not(clippy)'
    )" || true
    if [ -n "$hits" ]; then
      while IFS= read -r line; do
        fail "src/$rel:${line%%: *} lowers or hides the write ban: ${line#*: }"
      done <<<"$hits"
      bad=1
    fi
  done < <(find "$src" -name '*.rs' -type f | sort)
  [ "$bad" -eq 0 ] && pass "no lint allow, expect, cfg_attr or cfg(not(clippy)) outside fs_read.rs and koto_root.rs"

  # koto_root.rs: one allow, on the one home-directory lookup.
  bad=0
  local root_file="$src/koto_root.rs" root_attrs n_attrs home_lines n_home home_text
  if [ ! -f "$root_file" ]; then
    fail "src/koto_root.rs is missing"
    bad=1
  else
    root_attrs="$(
      attributes "$root_file" | grep -E '^[0-9]+: #!?\[(allow|expect|cfg_attr)\('
      attributes "$root_file" | grep -F 'not(clippy)'
    )" || true
    n_attrs="$(printf '%s\n' "$root_attrs" | grep -c . || true)"
    home_lines="$(strip_comments "$root_file" | grep -nE '(^|[^A-Za-z0-9_])home_dir([^A-Za-z0-9_]|$)' || true)"
    n_home="$(printf '%s\n' "$home_lines" | grep -c . || true)"
    home_text="$(printf '%s\n' "${home_lines#*:}" | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//')"
    if [ "$n_attrs" -ne 1 ] || [ "${root_attrs#*: }" != "$ROOT_ALLOW_ATTR" ]; then
      fail "src/koto_root.rs must hold exactly one lint attribute, $ROOT_ALLOW_ATTR, found $n_attrs:"
      [ -n "$root_attrs" ] && printf '%s\n' "$root_attrs"
      bad=1
    elif [ "$n_home" -ne 1 ] || [ "$home_text" != "$ROOT_HOME_CALL" ]; then
      fail "src/koto_root.rs must make one home-directory lookup, exactly '$ROOT_HOME_CALL', found $n_home:"
      [ -n "$home_lines" ] && printf '%s\n' "$home_lines"
      bad=1
    elif [ "${home_lines%%:*}" -ne $((${root_attrs%%:*} + 1)) ] \
      || [ "$(strip_comments "$root_file" | sed -n "${root_attrs%%:*}p" | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//')" != "$ROOT_ALLOW_ATTR" ]; then
      fail "src/koto_root.rs's allow (line ${root_attrs%%:*}) is not on the line before its home-directory lookup (line ${home_lines%%:*})"
      bad=1
    fi
  fi
  [ "$bad" -eq 0 ] && pass "koto_root.rs carries one allow, on its one home-directory lookup"

  # --- 3. the one custom_flags call and its constants ---------------------
  bad=0
  local flag_lines
  flag_lines="$(grep -rn --include='*.rs' 'custom_flags' "$src" \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
  local n_flags=0
  [ -n "$flag_lines" ] && n_flags="$(printf '%s\n' "$flag_lines" | wc -l | tr -d ' ')"
  if [ "$n_flags" -ne 1 ]; then
    fail "custom_flags appears $n_flags times in $CRATE/src, expected exactly once (in fs_read.rs)"
    [ -n "$flag_lines" ] && printf '%s\n' "$flag_lines"
    bad=1
  else
    local flag_file="${flag_lines%%:*}" flag_text
    flag_text="$(printf '%s\n' "$flag_lines" | cut -d: -f3- | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//')"
    if [ "$flag_file" != "$src/fs_read.rs" ]; then
      fail "the custom_flags call is in ${flag_file#"$root"/}, not fs_read.rs"
      bad=1
    elif [ "$flag_text" != "$CUSTOM_FLAGS_CALL" ]; then
      fail "the custom_flags call reads '$flag_text', expected '$CUSTOM_FLAGS_CALL'"
      bad=1
    fi
  fi
  local body
  body="$(strip_comments "$src/fs_read.rs" | awk '
    /^fn mode_flags\(/ { on = 1; next }
    on && /^}/ { exit }
    on { print }
  ')"
  if [ -z "$body" ]; then
    fail "fs_read.rs has no top-level fn mode_flags"
    bad=1
  else
    local unexpected
    unexpected="$(printf '%s\n' "$body" | grep -vE \
      '^[[:space:]]*(match mode \{|\}|ReadMode::InStore \{ \.\. \} => IN_STORE_OPEN_FLAGS,|ReadMode::Template => TEMPLATE_OPEN_FLAGS,)?[[:space:]]*$' || true)"
    if [ -n "$unexpected" ]; then
      fail "mode_flags returns something other than the two flag constants:"
      printf '%s\n' "$unexpected"
      bad=1
    fi
  fi
  local name def n_def
  for name in IN_STORE_OPEN_FLAGS TEMPLATE_OPEN_FLAGS; do
    if [ "$name" = IN_STORE_OPEN_FLAGS ]; then def="$IN_STORE_DEF"; else def="$TEMPLATE_DEF"; fi
    n_def="$(grep -rhE --include='*.rs' "(const|static)[[:space:]]+${name}[[:space:]]*:" "$src" | wc -l | tr -d ' ')" || true
    if [ "$n_def" -ne 1 ] || ! grep -qxF "$def" "$src/fs_read.rs"; then
      fail "$name must be defined once, in fs_read.rs, as: $def"
      bad=1
    fi
  done
  [ "$bad" -eq 0 ] && pass "custom_flags appears once, in fs_read.rs, with one of the two fixed flag constants"

  # --- 4. clippy.toml entries ----------------------------------------------
  bad=0
  local conf="$crate/clippy.toml" section entry
  if [ -e "$crate/.clippy.toml" ]; then
    fail "$CRATE/.clippy.toml exists and would take the place of clippy.toml"
    bad=1
  fi
  if [ ! -f "$conf" ]; then
    fail "$CRATE/clippy.toml is missing"
    bad=1
  else
    for section in disallowed-methods disallowed-types; do
      local entries
      entries="$(grep -vE '^[[:space:]]*#' "$conf" | awk -v key="$section" '
        $0 ~ "^[[:space:]]*" key "[[:space:]]*=[[:space:]]*\\[" { on = 1; next }
        on && /^[[:space:]]*\]/ { exit }
        on { print }
      ')"
      if [ "$section" = disallowed-methods ]; then
        set -- "${REQUIRED_METHODS[@]}"
      else
        set -- "${REQUIRED_TYPES[@]}"
      fi
      for entry in "$@"; do
        if ! printf '%s\n' "$entries" | grep -qF "{ path = \"$entry\""; then
          fail "clippy.toml lacks the $section entry $entry"
          bad=1
        fi
      done
    done
  fi
  [ "$bad" -eq 0 ] && pass "clippy.toml holds every required disallowed-methods and disallowed-types entry"

  # --- 5. no build script ---------------------------------------------------
  bad=0
  if [ -e "$crate/build.rs" ]; then
    fail "$CRATE has a build script (build.rs)"
    bad=1
  fi
  if grep -qE '^[[:space:]]*build[[:space:]]*=' "$crate/Cargo.toml"; then
    fail "$CRATE/Cargo.toml names a build script (build =)"
    bad=1
  fi
  [ "$bad" -eq 0 ] && pass "$CRATE has no build script"

  # --- 6. no [patch] or [replace] ------------------------------------------
  bad=0
  local manifest
  for manifest in "$crate/Cargo.toml" "$root/Cargo.toml" "$root/.cargo/config.toml" "$root/.cargo/config"; do
    [ -f "$manifest" ] || continue
    if grep -nE '^[[:space:]]*(\[[[:space:]]*(patch|replace)([^A-Za-z0-9_-]|$)|(patch|replace)[[:space:]]*[.=])' "$manifest"; then
      fail "${manifest#"$root"/} carries [patch] or [replace]"
      bad=1
    fi
  done
  [ "$bad" -eq 0 ] && pass "no [patch] or [replace] in the crate's manifest, the root manifest or .cargo/config.toml"

  # --- 7. every dependency from crates.io ----------------------------------
  local meta foreign
  # stderr stays out of the JSON; a failed run is repeated to show it.
  if ! meta="$(cargo metadata --format-version 1 "${CARGO_FLAGS[@]}" --manifest-path "$root/Cargo.toml" 2>/dev/null)"; then
    fail "cargo metadata failed: $(cargo metadata --format-version 1 "${CARGO_FLAGS[@]}" --manifest-path "$root/Cargo.toml" 2>&1 >/dev/null | tail -5)"
  elif ! foreign="$(printf '%s\n' "$meta" | jq -r --arg crate "$CRATE" --arg a "$CRATES_IO" --arg b "$CRATES_IO_SPARSE" "$FOREIGN_DEPS_JQ" | sort -u)"; then
    fail "could not read cargo metadata's dependency graph with jq"
  else
    if [ -n "$foreign" ]; then
      while IFS= read -r line; do
        fail "dependency $line does not come from crates.io"
      done <<<"$foreign"
    else
      pass "every normal and build dependency of $CRATE comes from crates.io"
    fi
  fi

  # --- 8. open_plain callers -------------------------------------------------
  bad=0
  local defs
  defs="$(strip_comments "$src/fs_read.rs" | grep -n 'open_plain' || true)"
  if [ "$(printf '%s\n' "$defs" | grep -c .)" -ne 1 ] \
    || ! printf '%s\n' "$defs" | grep -qE '^[0-9]+:pub fn open_plain[<(]'; then
    fail "fs_read.rs must name open_plain only in its definition, found:"
    printf '%s\n' "$defs"
    bad=1
  fi
  local pubs
  pubs="$(strip_comments "$src/fs_read.rs" \
    | sed -nE 's/^[[:space:]]*pub(\([^)]*\))?[[:space:]]+fn[[:space:]]+([A-Za-z0-9_]+).*/\2/p' \
    | grep -vxE 'open_plain|read_store_file' || true)"
  if [ -n "$pubs" ]; then
    fail "fs_read.rs offers public functions besides open_plain and read_store_file: $(echo "$pubs" | tr '\n' ' ')"
    bad=1
  fi
  local site lineno fname text
  while IFS= read -r f; do
    rel="${f#"$src"/}"
    [ "$rel" = fs_read.rs ] && continue
    while IFS= read -r site; do
      [ -n "$site" ] || continue
      lineno="${site%%:*}"
      fname="${site#*:}"
      text="${fname#*:}"
      fname="${fname%%:*}"
      if ! printf '%s\n' "$text" | grep -qF 'crate::fs_read::open_plain('; then
        fail "src/$rel:$lineno names open_plain other than as a crate::fs_read::open_plain(...) call: $(echo "$text" | sed -E 's/^[[:space:]]+//')"
        bad=1
      elif ! contains "$rel:$fname" "${OPEN_PLAIN_CALLERS[@]}"; then
        fail "src/$rel:$lineno calls open_plain from $fname, which may not call it in this phase"
        bad=1
      fi
    done < <(open_plain_sites "$f")
  done < <(find "$src" -name '*.rs' -type f | sort)
  local scan_file forbidden
  for scan_file in dashboard.rs source.rs; do
    [ -f "$src/$scan_file" ] || continue
    for forbidden in ${SCAN_FORBIDDEN[@]+"${SCAN_FORBIDDEN[@]}"}; do
      if strip_comments "$src/$scan_file" | grep -nE "(^|[^A-Za-z0-9_])$forbidden([^A-Za-z0-9_]|$)"; then
        fail "src/$scan_file names $forbidden, which the dashboard scan may not use in this phase"
        bad=1
      fi
    done
  done
  [ "$bad" -eq 0 ] && pass "open_plain is called only from ${OPEN_PLAIN_CALLERS[*]}"

  # --- 9. cargo tree allowlist and banned crates ----------------------------
  local tree direct dep
  if ! tree="$(cargo tree --manifest-path "$root/Cargo.toml" "${CARGO_FLAGS[@]}" -p "$CRATE" \
    --depth 1 -e normal,build --prefix none --target all 2>&1)"; then
    fail "cargo tree --depth 1 failed: $tree"
  else
    bad=0
    direct="$(printf '%s\n' "$tree" | tail -n +2 | grep -v '^\[' | awk 'NF { print $1 }' | sort -u)" || true
    for dep in $direct; do
      if ! contains "$dep" "${ALLOWED_DEPS[@]}"; then
        fail "direct dependency $dep of $CRATE is outside the allowlist (${ALLOWED_DEPS[*]})"
        bad=1
      fi
    done
    [ "$bad" -eq 0 ] && pass "every direct dependency of $CRATE is on the allowlist"
  fi
  if ! tree="$(cargo tree --manifest-path "$root/Cargo.toml" "${CARGO_FLAGS[@]}" -p "$CRATE" \
    -e normal,build --prefix none --target all 2>&1)"; then
    fail "cargo tree failed: $tree"
  else
    bad=0
    local graph
    graph="$(printf '%s\n' "$tree" | grep -v '^\[' | awk 'NF { print $1 }' | sort -u)" || true
    for dep in "${BANNED_CRATES[@]}"; do
      if printf '%s\n' "$graph" | grep -qxF "$dep"; then
        fail "banned crate $dep is in $CRATE's normal and build dependency graph"
        bad=1
      fi
    done
    [ "$bad" -eq 0 ] && pass "$CRATE's dependency graph holds none of ${BANNED_CRATES[*]}"
  fi

  return "$failed"
}

# --- self-test ---------------------------------------------------------------

# Copy what check reads into a fresh directory: the crate and the
# workspace manifests are copied; koto's own sources, which no check reads
# and no plant touches, are linked so cargo still finds its targets.
fresh_copy() {
  local dest="$1"
  rm -rf "$dest"
  mkdir -p "$dest"
  cp "$REPO_ROOT/Cargo.toml" "$REPO_ROOT/Cargo.lock" "$dest/"
  cp -R "$REPO_ROOT/$CRATE" "$REPO_ROOT/koto-stability-tests" "$dest/"
  [ -d "$REPO_ROOT/.cargo" ] && cp -R "$REPO_ROOT/.cargo" "$dest/"
  local linked
  for linked in src benches build.rs; do
    [ -e "$REPO_ROOT/$linked" ] && ln -s "$REPO_ROOT/$linked" "$dest/$linked"
  done
  return 0
}

# Rewrite a file through sed -E without relying on sed -i's dialect.
edit() {
  local file="$1" expr="$2"
  sed -E "$expr" "$file" >"$file.planted"
  mv "$file.planted" "$file"
}

append() {
  printf '%s\n' "$2" >>"$1"
}

# Add a line after every line matching an awk regex.
insert_after() {
  awk -v re="$2" -v add="$3" '{ print } $0 ~ re { print add }' "$1" >"$1.planted"
  mv "$1.planted" "$1"
}

self_test() {
  local scratch
  scratch="$(mktemp -d "${TMPDIR:-/tmp}/reader-readonly.XXXXXX")"
  # shellcheck disable=SC2064 # expand now: scratch is local to this function
  trap "rm -rf '$scratch'" EXIT
  CARGO_FLAGS=(--offline)
  local T="$scratch/tree"
  local C="$T/$CRATE"
  local S="$C/src"
  local out="$scratch/out"
  local cases=0 missed=0

  fresh_copy "$T"
  if ! check "$T" >"$out" 2>&1; then
    echo "FAIL: self-test: the clean copy failed"
    cat "$out"
    return 1
  fi

  # expect_fail <description> <expected FAIL text> <plant command...>
  expect_fail() {
    local desc="$1" want="$2"
    shift 2
    cases=$((cases + 1))
    fresh_copy "$T"
    "$@"
    if check "$T" >"$out" 2>&1; then
      echo "FAIL: self-test: $desc was not caught"
      missed=1
    elif ! grep -F 'FAIL:' "$out" | grep -qF "$want"; then
      echo "FAIL: self-test: $desc failed without the expected message ($want):"
      grep -F 'FAIL:' "$out" || true
      missed=1
    else
      echo "PASS: self-test: $desc is caught"
    fi
  }

  expect_fail "a removed forbid(unsafe_code)" "lib.rs lacks #![forbid" \
    edit "$S/lib.rs" '/^#!\[forbid\(unsafe_code\)\]$/d'
  expect_fail "a removed deny of the ban lints" "lib.rs lacks #![deny" \
    edit "$S/lib.rs" '/^#!\[deny\(clippy::disallowed_methods/d'
  expect_fail "an allow of disallowed_methods" "lowers or hides the write ban" \
    append "$S/source.rs" $'#[allow(clippy::disallowed_methods)]\nfn planted() {}'
  expect_fail "a multi-line allow of clippy::all" "lowers or hides the write ban" \
    append "$S/dashboard.rs" $'#[allow(\n    clippy::all,\n)]\nfn planted() {}'
  expect_fail "a module-wide allow of warnings" "lowers or hides the write ban" \
    append "$S/engine/mod.rs" '#![allow(warnings)]'
  expect_fail "an expect of clippy::style" "lowers or hides the write ban" \
    append "$S/source.rs" $'#[expect(clippy::style)]\nfn planted() {}'
  expect_fail "a cfg_attr allow of disallowed_types" "lowers or hides the write ban" \
    append "$S/source.rs" $'#[cfg_attr(test, allow(clippy::disallowed_types))]\nfn planted() {}'
  expect_fail "an allow under the lint's old name" "lowers or hides the write ban" \
    append "$S/source.rs" $'#[allow(clippy::disallowed_method)]\nfn planted() {}'
  expect_fail "a cfg(not(clippy)) item" "lowers or hides the write ban" \
    append "$S/source.rs" $'#[cfg(not(clippy))]\nfn planted() {}'
  expect_fail "an allow in a nested file named koto_root.rs" "src/session/koto_root.rs:1 lowers or hides the write ban" \
    append "$S/session/koto_root.rs" $'#[allow(clippy::disallowed_methods)]\nfn planted() {}'
  expect_fail "a second allow in koto_root.rs" "src/koto_root.rs must hold exactly one lint attribute" \
    append "$S/koto_root.rs" $'#[allow(clippy::disallowed_methods)]\nfn planted() {\n    let _ = std::fs::write("x", "");\n}'
  expect_fail "a cfg(not(clippy)) item in koto_root.rs" "src/koto_root.rs must hold exactly one lint attribute" \
    append "$S/koto_root.rs" $'#[cfg(not(clippy))]\nfn planted() {}'
  expect_fail "a second home-directory lookup in koto_root.rs" "src/koto_root.rs must make one home-directory lookup" \
    append "$S/koto_root.rs" $'fn planted() -> Option<PathBuf> {\n    dirs::home_dir()\n}'

  # shellcheck disable=SC2329 # invoked through expect_fail
  plant_fn_wide_root_allow() {
    edit "$S/koto_root.rs" '/^[[:space:]]+#\[allow\(clippy::disallowed_methods\)\]$/d'
    edit "$S/koto_root.rs" 's/^pub fn koto_root\(\)/#[allow(clippy::disallowed_methods)] pub fn koto_root()/'
  }
  expect_fail "koto_root.rs's allow moved onto the whole function" "is not on the line before its home-directory lookup" \
    plant_fn_wide_root_allow
  expect_fail "a second custom_flags call" "custom_flags appears 2 times" \
    append "$S/source.rs" $'fn planted(o: &mut std::fs::OpenOptions) {\n    std::os::unix::fs::OpenOptionsExt::custom_flags(o, 0);\n}'
  expect_fail "custom_flags passed a raw flag" "the custom_flags call reads" \
    edit "$S/fs_read.rs" 's/custom_flags\(mode_flags\(&mode\)\)/custom_flags(libc::O_CREAT)/'
  expect_fail "O_CREAT in the in-store flags" "IN_STORE_OPEN_FLAGS must be defined once" \
    edit "$S/fs_read.rs" 's/= libc::O_NOFOLLOW \| libc::O_NONBLOCK;/= libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CREAT;/'
  expect_fail "O_TRUNC in the template flags" "TEMPLATE_OPEN_FLAGS must be defined once" \
    edit "$S/fs_read.rs" 's/TEMPLATE_OPEN_FLAGS: libc::c_int = libc::O_NONBLOCK;/TEMPLATE_OPEN_FLAGS: libc::c_int = libc::O_NONBLOCK | libc::O_TRUNC;/'
  expect_fail "a second definition of a flag constant" "IN_STORE_OPEN_FLAGS must be defined once" \
    append "$S/source.rs" 'const IN_STORE_OPEN_FLAGS: i32 = 0;'
  expect_fail "a mode_flags arm with an extra flag" "mode_flags returns something other" \
    edit "$S/fs_read.rs" 's/=> TEMPLATE_OPEN_FLAGS,/=> TEMPLATE_OPEN_FLAGS | libc::O_TRUNC,/'
  expect_fail "a removed disallowed-methods entry" "clippy.toml lacks the disallowed-methods entry std::fs::write" \
    edit "$C/clippy.toml" '/path = "std::fs::write"/d'
  expect_fail "a commented-out disallowed-types entry" "clippy.toml lacks the disallowed-types entry std::process::Command" \
    edit "$C/clippy.toml" 's/^([[:space:]]*)(\{ path = "std::process::Command")/\1# \2/'
  expect_fail "a .clippy.toml beside clippy.toml" ".clippy.toml exists" \
    append "$C/.clippy.toml" '# empty'
  expect_fail "a build.rs in the crate" "has a build script (build.rs)" \
    append "$C/build.rs" 'fn main() {}'
  expect_fail "a build key in the crate's manifest" "names a build script" \
    insert_after "$C/Cargo.toml" '^rust-version = ' 'build = "src/lib.rs"'
  expect_fail "[patch.crates-io] in the root manifest" "Cargo.toml carries [patch] or [replace]" \
    append "$T/Cargo.toml" $'\n[patch.crates-io]'
  expect_fail "[replace] in the crate's manifest" "$CRATE/Cargo.toml carries [patch] or [replace]" \
    append "$C/Cargo.toml" $'\n[replace]'
  expect_fail "[patch] in .cargo/config.toml" ".cargo/config.toml carries [patch] or [replace]" \
    append "$T/.cargo/config.toml" $'\n[patch.crates-io]'

  # shellcheck disable=SC2329 # invoked through expect_fail
  plant_path_dep() {
    mkdir -p "$T/planted-dep/src"
    printf '[package]\nname = "planted-dep"\nversion = "0.1.0"\nedition = "2021"\n' >"$T/planted-dep/Cargo.toml"
    : >"$T/planted-dep/src/lib.rs"
    insert_after "$C/Cargo.toml" '^[[]dependencies[]]$' 'planted-dep = { path = "../planted-dep" }'
  }
  expect_fail "a path dependency" "dependency planted-dep (declared by path) does not come from crates.io" \
    plant_path_dep
  expect_fail "a direct dependency outside the allowlist" "direct dependency tempfile of $CRATE is outside the allowlist" \
    insert_after "$C/Cargo.toml" '^[[]dependencies[]]$' 'tempfile = "3"'
  expect_fail "a banned crate in the graph" "banned crate tempfile" \
    insert_after "$C/Cargo.toml" '^[[]dependencies[]]$' 'tempfile = "3"'
  expect_fail "open_plain called from a new function" "which may not call it in this phase" \
    append "$S/source.rs" $'fn planted() {\n    let _ = crate::fs_read::open_plain("x");\n}'
  expect_fail "open_plain from another function in an allowed file" "calls open_plain from planted_read" \
    append "$S/engine/persistence.rs" $'fn planted_read() {\n    let _ = crate::fs_read::open_plain("x");\n}'
  expect_fail "open_plain imported under another name" "names open_plain other than as a crate::fs_read::open_plain" \
    append "$S/dashboard.rs" 'use crate::fs_read::open_plain as planted;'
  expect_fail "a wrapper around open_plain in fs_read.rs" "fs_read.rs must name open_plain only in its definition" \
    append "$S/fs_read.rs" $'fn planted(p: &Path) -> io::Result<File> {\n    open_plain(p)\n}'
  expect_fail "a third public read in fs_read.rs" "fs_read.rs offers public functions besides" \
    append "$S/fs_read.rs" 'pub fn planted() {}'

  if [ "$missed" -ne 0 ]; then
    echo "FAIL: self-test: a planted violation was not caught"
    return 1
  fi
  echo "PASS: self-test: the clean copy passes and all $cases planted violations fail"
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit $?
elif [ $# -gt 0 ]; then
  echo "usage: $0 [--self-test]" >&2
  exit 2
fi

check "$REPO_ROOT"
