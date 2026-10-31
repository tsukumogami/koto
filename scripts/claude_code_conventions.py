"""Probe helper for scripts/check-claude-code-conventions.sh.

Reads a Claude Code release's executable as bytes and checks the conventions
koto's /workflows entry depends on, as listed in
tests/fixtures/native-workflows/claude-code-conventions.json. The executable is
someone else's code: this helper never runs, imports or decodes it. It
memory-maps the file read-only and only searches it, so a hostile bundle can at
worst make a probe pass or fail wrongly.

Run it with `python3 -I`, which keeps the interpreter from importing anything
from the current directory, the script's directory or the user's site-packages.

Subcommands:

  expectations <file> [--alter <slug>]
      Validates the expectations file and prints the package name and the
      tarball member, one per line. With --alter, also checks the slug names a
      convention.
  extract <tarball> <member> <out> <max-bytes>
      Copies the one tarball member named <member> to <out>, refusing a link,
      a duplicate entry, or a member larger than <max-bytes> (counted while
      streaming, so a compressed bomb stops at the cap). Nothing else in the
      tarball is written anywhere.
  scan <file> <evidence> [--alter <slug>]
      Evaluates every probe of every convention against <evidence> and prints
      a FAIL line per failing probe, then one result line per convention.
      With --alter, applies that convention's demonstration mutation to a
      private copy-on-write map of the evidence first; the file itself is
      never written.
  self-test <file>
      Checks the guard against itself, with no release and no network: builds
      synthetic evidence from the expectations alone (each probe's anchor
      followed by its token), requires every convention to pass on it, then
      applies each convention's demonstration mutation in turn and requires
      that convention to fail. Exits 1 when any of those doesn't hold, so a
      probe or a demonstration that no longer catches anything fails here
      before it can pass a real release silently.

Exit statuses: 0 every convention holds (self-test: the guard caught every
mutation), 1 a convention failed (self-test: the guard missed one), 2 the
package or executable could not be read, 3 the expectations file is bad, 64
usage, 70 an internal error. A crash must never exit 1, which would read as a
convention failure, so main() maps any unexpected exception to 70.
"""

import json
import mmap
import os
import re
import sys
import tarfile

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_OBTAIN = 2
EXIT_INPUT = 3
EXIT_USAGE = 64
EXIT_INTERNAL = 70

PACKAGE_RE = re.compile(r"^@anthropic-ai/claude-code-[a-z0-9][a-z0-9-]*$")
MEMBER_RE = re.compile(r"^[A-Za-z0-9._-]+(/[A-Za-z0-9._-]+)*$")
SLUG_RE = re.compile(r"^[a-z][a-z0-9-]*$")
# A probe with no anchor passes on the token appearing anywhere, so the token
# has to be distinctive on its own (CLAUDE_CODE_SESSION_ID is 22 bytes).
MIN_UNANCHORED_TOKEN = 12
MAX_WINDOW = 1_000_000
# Bounds the work one probe can cost on a pathological bundle.
MAX_ANCHOR_HITS = 100_000
CHUNK = 1 << 20


class InputError(Exception):
    """A bad expectations file; the message names what is wrong."""


class ObtainError(Exception):
    """The package does not hold a readable executable."""


class UsageError(Exception):
    pass


def _require(cond, msg):
    if not cond:
        raise InputError(msg)


def _nonempty_str(value):
    return isinstance(value, str) and value != ""


def load_expectations(path):
    """Loads and validates the expectations file, raising InputError."""
    try:
        return _load_expectations(path)
    except InputError as e:
        raise InputError("%s: %s" % (path, e))


def _load_expectations(path):
    try:
        with open(path, "rb") as f:
            data = json.loads(f.read().decode("utf-8"))
    except FileNotFoundError:
        raise InputError("file not found")
    except (OSError, UnicodeDecodeError, ValueError) as e:
        raise InputError("not readable JSON: %s" % e)

    _require(isinstance(data, dict), "top level must be an object")
    _require(data.get("schema") == 1, "schema must be 1")
    package = data.get("package")
    _require(
        isinstance(package, str) and PACKAGE_RE.match(package),
        "package must be an @anthropic-ai/claude-code-<platform> name",
    )
    member = data.get("member")
    _require(
        isinstance(member, str)
        and MEMBER_RE.match(member)
        and ".." not in member.split("/"),
        "member must be a relative path with no '..' component",
    )
    conventions = data.get("conventions")
    _require(
        isinstance(conventions, list) and conventions,
        "conventions must be a non-empty list",
    )
    seen = set()
    for ci, conv in enumerate(conventions):
        where = "conventions[%d]" % ci
        _require(isinstance(conv, dict), where + " must be an object")
        _require(_nonempty_str(conv.get("name")), where + ".name must be a string")
        slug = conv.get("slug")
        _require(
            isinstance(slug, str) and SLUG_RE.match(slug),
            where + ".slug must be lower-case letters, digits and '-'",
        )
        _require(slug not in seen, where + ".slug %r is repeated" % slug)
        seen.add(slug)
        demo = conv.get("demo")
        _require(isinstance(demo, dict), where + ".demo must be an object")
        dfrom, dto = demo.get("from"), demo.get("to")
        # Same length keeps every other offset in the evidence where it was,
        # so a demonstration changes only the item it names.
        _require(
            _nonempty_str(dfrom)
            and _nonempty_str(dto)
            and len(dfrom.encode()) == len(dto.encode())
            and dfrom != dto,
            where + ".demo needs different from/to strings of the same length",
        )
        probes = conv.get("probes")
        _require(
            isinstance(probes, list) and probes,
            where + ".probes must be a non-empty list",
        )
        items = set()
        for pi, probe in enumerate(probes):
            pw = "%s.probes[%d]" % (where, pi)
            _require(isinstance(probe, dict), pw + " must be an object")
            item = probe.get("item")
            _require(_nonempty_str(item), pw + ".item must be a string")
            # One probe per item, so a FAIL line names exactly one probe.
            _require(item not in items, pw + ".item %r is repeated" % item)
            items.add(item)
            token = probe.get("token")
            _require(_nonempty_str(token), pw + ".token must be a string")
            anchor = probe.get("anchor")
            if anchor is None:
                _require(
                    len(token) >= MIN_UNANCHORED_TOKEN,
                    pw + ".token must be at least %d characters without an anchor"
                    % MIN_UNANCHORED_TOKEN,
                )
                _require("window" not in probe, pw + ".window needs an anchor")
            else:
                _require(_nonempty_str(anchor), pw + ".anchor must be a string or null")
                window = probe.get("window")
                _require(
                    isinstance(window, int)
                    and not isinstance(window, bool)
                    and 0 < window <= MAX_WINDOW,
                    pw + ".window must be an integer from 1 to %d" % MAX_WINDOW,
                )
        # Fields koto writes that the release doesn't read, each with the
        # reason. The guard itself ignores them; the offline coverage test
        # (tests/native_workflows_conventions.rs) accepts a shape-fixture field
        # that is either a probe's item or listed here, so leaving a field
        # unprobed is a reviewed choice, never an omission.
        unread = conv.get("unread", [])
        _require(isinstance(unread, list), where + ".unread must be a list")
        fields = set()
        for ui, entry in enumerate(unread):
            uw = "%s.unread[%d]" % (where, ui)
            _require(isinstance(entry, dict), uw + " must be an object")
            field = entry.get("field")
            _require(_nonempty_str(field), uw + ".field must be a string")
            _require(_nonempty_str(entry.get("reason")), uw + ".reason must be a string")
            _require(field not in fields, uw + ".field %r is repeated" % field)
            _require(field not in items, uw + ".field %r also has a probe" % field)
            fields.add(field)
    return data


def find_convention(data, slug):
    for conv in data["conventions"]:
        if conv["slug"] == slug:
            return conv
    raise UsageError(
        "unknown convention %r; known: %s"
        % (slug, ", ".join(c["slug"] for c in data["conventions"]))
    )


def extract(tarball, member, out, max_bytes):
    """Copies the single regular-file entry named `member` to `out`."""
    found = False
    try:
        with tarfile.open(tarball, mode="r|gz") as tar:
            for info in tar:
                if info.name != member:
                    continue
                if found:
                    raise ObtainError("the package lists %s more than once" % member)
                if not info.isreg():
                    raise ObtainError("%s in the package is not a regular file" % member)
                if info.size > max_bytes:
                    raise ObtainError(
                        "%s is %d bytes, over the %d-byte cap" % (member, info.size, max_bytes)
                    )
                src = tar.extractfile(info)
                if src is None:
                    raise ObtainError("%s could not be read from the package" % member)
                # 0o600 and O_EXCL: the copy is never executable and never
                # lands on a file someone else put there.
                fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                written = 0
                with os.fdopen(fd, "wb") as dst:
                    while True:
                        chunk = src.read(CHUNK)
                        if not chunk:
                            break
                        written += len(chunk)
                        if written > max_bytes:
                            raise ObtainError("%s grew past the %d-byte cap" % (member, max_bytes))
                        dst.write(chunk)
                found = True
    except (tarfile.TarError, OSError, EOFError) as e:
        raise ObtainError("the package tarball could not be read: %s" % e)
    if not found:
        raise ObtainError("the package has no %s" % member)


def find_all(m, needle, limit=None):
    hits = []
    i = m.find(needle)
    while i != -1:
        hits.append(i)
        if limit is not None and len(hits) >= limit:
            break
        i = m.find(needle, i + 1)
    return hits


def apply_demo(m, demo):
    """Rewrites every occurrence of demo.from in the private map."""
    dfrom = demo["from"].encode()
    dto = demo["to"].encode()
    count = 0
    i = m.find(dfrom)
    while i != -1:
        m[i : i + len(dfrom)] = dto
        count += 1
        i = m.find(dfrom, i + len(dfrom))
    return count


def evaluate(m, probe, anchor_cache):
    """Returns None when the probe passes, else the reason it failed."""
    token = probe["token"].encode()
    anchor = probe.get("anchor")
    if anchor is None:
        return None if m.find(token) != -1 else "not_found"
    if anchor not in anchor_cache:
        anchor_cache[anchor] = find_all(m, anchor.encode(), MAX_ANCHOR_HITS)
    hits = anchor_cache[anchor]
    if not hits:
        return "anchor_missing"
    window = probe["window"]
    alen = len(anchor.encode())
    size = len(m)
    for a in hits:
        lo = max(0, a - window)
        hi = min(size, a + alen + window)
        if m.find(token, lo, hi) != -1:
            return None
    return "not_near"


def scan(data, evidence, alter):
    alter_conv = find_convention(data, alter) if alter else None
    try:
        with open(evidence, "rb") as f:
            if os.fstat(f.fileno()).st_size == 0:
                raise ObtainError("the executable is empty")
            # ACCESS_COPY is a private mapping: a demonstration mutation
            # dirties only the pages it touches, in memory, and never reaches
            # the file. Without --alter nothing is written at all.
            access = mmap.ACCESS_COPY if alter_conv else mmap.ACCESS_READ
            m = mmap.mmap(f.fileno(), 0, access=access)
    except OSError as e:
        raise ObtainError("the executable could not be read: %s" % e)

    try:
        if alter_conv:
            demo = alter_conv["demo"]
            count = apply_demo(m, demo)
            print(
                "altering %s: %s -> %s (%d occurrences)"
                % (alter_conv["name"], demo["from"], demo["to"], count)
            )
        fail_lines, results = check(data, m)
    finally:
        m.close()

    for line in fail_lines:
        print(line)
    for name, ok in results:
        print("%s: %s" % (name, "ok" if ok else "FAIL"))
    return EXIT_OK if all(ok for _, ok in results) else EXIT_FAIL


def check(data, buf):
    """Evaluates every probe of every convention against `buf`.

    Returns the FAIL lines and a (convention name, ok) pair per convention,
    in file order. Every convention is evaluated, so one break never hides
    another. `buf` is a map or a bytes-like object; it is only searched.
    """
    anchor_cache = {}
    fail_lines = []
    results = []
    for conv in data["conventions"]:
        ok = True
        reported_anchors = set()
        name = conv["name"]
        for probe in conv["probes"]:
            reason = evaluate(buf, probe, anchor_cache)
            if reason is None:
                continue
            ok = False
            if reason == "anchor_missing":
                # One line per missing anchor, not per probe that uses it.
                if probe["anchor"] not in reported_anchors:
                    reported_anchors.add(probe["anchor"])
                    fail_lines.append('FAIL %s: anchor "%s" not found' % (name, probe["anchor"]))
            elif reason == "not_near":
                fail_lines.append(
                    'FAIL %s: %s not found within %d bytes of "%s"'
                    % (name, probe["item"], probe["window"], probe["anchor"])
                )
            else:
                fail_lines.append("FAIL %s: %s not found" % (name, probe["item"]))
        results.append((name, ok))
    return fail_lines, results


def synthetic_evidence(data):
    """Evidence built from the expectations alone: every probe passes on it.

    Each probe contributes its anchor directly followed by its token (or the
    token alone when it has no anchor), so a token sits within any window of
    at least its own length. Probes are separated by newlines, which no
    anchor or token in the file needs.
    """
    parts = []
    for conv in data["conventions"]:
        for probe in conv["probes"]:
            if probe.get("anchor") is not None:
                parts.append(probe["anchor"].encode())
            parts.append(probe["token"].encode())
    return b"\n".join(parts) + b"\n"


def self_test(data):
    """Checks that the probes pass on synthetic evidence and that each
    convention's demonstration mutation makes that convention fail."""
    base = synthetic_evidence(data)
    fail_lines, results = check(data, base)
    caught_all = True
    if all(ok for _, ok in results):
        print("self-test: unaltered evidence: every convention ok")
    else:
        caught_all = False
        for line in fail_lines:
            print("self-test: unaltered evidence: %s" % line)
    for conv in data["conventions"]:
        buf = bytearray(base)
        count = apply_demo(buf, conv["demo"])
        _, results = check(data, buf)
        failed = [name for name, ok in results if not ok]
        if conv["name"] in failed:
            print("self-test: --alter %s: %s FAIL, as expected" % (conv["slug"], conv["name"]))
        else:
            caught_all = False
            print(
                "self-test: --alter %s left %s passing (%d occurrences of %s rewritten)"
                % (conv["slug"], conv["name"], count, conv["demo"]["from"])
            )
    print("self-test: %s" % ("ok" if caught_all else "FAIL"))
    return EXIT_OK if caught_all else EXIT_FAIL


def _pop_alter(args):
    if "--alter" not in args:
        return args, None
    i = args.index("--alter")
    if i + 1 >= len(args):
        raise UsageError("--alter needs a convention slug")
    return args[:i] + args[i + 2 :], args[i + 1]


def run(argv):
    if not argv:
        raise UsageError("expected a subcommand: expectations, extract, scan or self-test")
    cmd, args = argv[0], argv[1:]
    if cmd == "expectations":
        args, alter = _pop_alter(args)
        if len(args) != 1:
            raise UsageError("expectations <file> [--alter <slug>]")
        data = load_expectations(args[0])
        if alter is not None:
            find_convention(data, alter)
        print(data["package"])
        print(data["member"])
        return EXIT_OK
    if cmd == "extract":
        if len(args) != 4 or not args[3].isdigit():
            raise UsageError("extract <tarball> <member> <out> <max-bytes>")
        extract(args[0], args[1], args[2], int(args[3]))
        return EXIT_OK
    if cmd == "scan":
        args, alter = _pop_alter(args)
        if len(args) != 2:
            raise UsageError("scan <file> <evidence> [--alter <slug>]")
        data = load_expectations(args[0])
        return scan(data, args[1], alter)
    if cmd == "self-test":
        if len(args) != 1:
            raise UsageError("self-test <file>")
        return self_test(load_expectations(args[0]))
    raise UsageError("unknown subcommand %r" % cmd)


def main():
    try:
        return run(sys.argv[1:])
    except InputError as e:
        print("claude_code_conventions: %s" % e, file=sys.stderr)
        return EXIT_INPUT
    except ObtainError as e:
        print(str(e), file=sys.stderr)
        return EXIT_OBTAIN
    except UsageError as e:
        print("claude_code_conventions: %s" % e, file=sys.stderr)
        return EXIT_USAGE
    except Exception as e:  # noqa: BLE001 - see the module docstring
        print("claude_code_conventions: internal error: %r" % (e,), file=sys.stderr)
        return EXIT_INTERNAL


if __name__ == "__main__":
    sys.exit(main())
