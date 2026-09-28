#!/usr/bin/env python3
"""Decide whether the nested claude session run-evals.sh started executed anything.

run-evals.sh saves the session's `--output-format stream-json` transcript as
runner_session.jsonl in the iteration directory. When a run grades nothing,
the transcript tells two causes apart:

  - the session executed and the evals produced no grades: look at the suite
    or the skill (run-evals.sh exit 2);
  - the session executed nothing: it stopped in plan mode, or every command
    and write it tried was denied. The runner or the host is at fault, and
    the skill under test was never exercised (exit 4).

A session executed when at least one Bash, Write, Edit, MultiEdit or
NotebookEdit call came back and was not a permission denial. A command that
ran and exited nonzero still ran. Subagent calls count, since they appear in
the same stream. A session whose init message reports plan mode did not
execute, whatever ran: plan mode lets read-only commands through and writes
its own plan file. A session outside plan mode that ran something and then
stopped at ExitPlanMode is counted as executed: that reads as exit 2 rather
than 4, never as a pass, so the rarer case isn't worth a rule of its own.

Usage:
  classify-eval-session.py report <transcript> <requested-mode>
      Exit 0 when the session executed, printing a note if it ran in a mode
      other than the requested one. Exit 4, printing a NESTED SESSION DID NOT
      EXECUTE block, when it did not. Exit 2 when the transcript is missing or
      holds nothing to decide from.
  classify-eval-session.py result-text <transcript>
      Print the session's final message (what `claude -p` prints in text mode).
"""

import json
import sys

EXECUTING_TOOLS = {"Bash", "Write", "Edit", "MultiEdit", "NotebookEdit"}

# Tool-result texts Claude Code writes for a denied call, used when the
# transcript was cut short before the permission_denied event or the result
# message's permission_denials list.
DENIAL_TEXTS = ("requires approval", "haven't granted", "requested permissions to")

EXIT_EXECUTED = 0
EXIT_UNKNOWN = 2
EXIT_NOT_EXECUTED = 4


def read_events(path):
    events = []
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                events.append(json.loads(line))
            except ValueError:
                continue
    return events


def blocks(event):
    message = event.get("message")
    if isinstance(message, dict) and isinstance(message.get("content"), list):
        return [b for b in message["content"] if isinstance(b, dict)]
    return []


def text_of(block):
    content = block.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return " ".join(p.get("text", "") for p in content if isinstance(p, dict))
    return ""


def classify(events):
    mode = None
    calls = {}  # tool_use_id -> tool name
    returned = {}  # tool_use_id -> (is_error, text)
    denied = set()
    result = None
    for event in events:
        kind, subtype = event.get("type"), event.get("subtype")
        if kind == "system" and subtype == "init" and mode is None:
            # Background agents emit init messages of their own; the first one
            # is the session the runner started.
            mode = event.get("permissionMode")
        elif kind == "system" and subtype == "permission_denied":
            if event.get("tool_use_id"):
                denied.add(event["tool_use_id"])
        elif kind == "assistant":
            for b in blocks(event):
                if b.get("type") == "tool_use":
                    calls[b.get("id")] = b.get("name", "")
        elif kind == "user":
            for b in blocks(event):
                if b.get("type") == "tool_result":
                    returned[b.get("tool_use_id")] = (bool(b.get("is_error")), text_of(b))
        elif kind == "result":
            for d in event.get("permission_denials") or []:
                if isinstance(d, dict) and d.get("tool_use_id"):
                    denied.add(d["tool_use_id"])
            if result is None or event.get("parent_tool_use_id") is None:
                result = event

    for tool_id, (is_error, text) in returned.items():
        if is_error and any(marker in text for marker in DENIAL_TEXTS):
            denied.add(tool_id)

    executing = [i for i, name in calls.items() if name in EXECUTING_TOOLS]
    ran = [i for i in executing if i in returned and i not in denied]

    if mode is None and result is None and not calls:
        verdict = "unknown"
    elif mode == "plan" or not ran:
        verdict = "not_executed"
    else:
        verdict = "executed"
    return {
        "verdict": verdict,
        "permission_mode": mode,
        "tool_calls": len(calls),
        "executing_calls": len(executing),
        "executing_calls_ran": len(ran),
        "permission_denials": len(denied),
        "result_subtype": (result or {}).get("subtype"),
        "result_is_error": bool((result or {}).get("is_error")),
        "result_text": (result or {}).get("result") or "",
    }


def report(summary, transcript, requested):
    overridden = bool(summary["permission_mode"] and summary["permission_mode"] != requested)
    if summary["verdict"] == "unknown":
        print("")
        print("  The nested claude session left no transcript to classify:")
        print(f"    {transcript}")
        print("  It may have failed to start; its output is above.")
        return EXIT_UNKNOWN
    if summary["verdict"] == "executed":
        if overridden:
            print("")
            print(f"  Note: the nested session ran in permission mode {summary['permission_mode']},"
                  f" not the {requested} the runner requested.")
        return EXIT_EXECUTED

    print("")
    print("  NESTED SESSION DID NOT EXECUTE")
    print("  The claude session this runner started stopped in plan mode, ran no")
    print("  command and wrote no file, or ended in an error before running anything,")
    print("  so no eval ran. The runner or the host is at fault, not the skill under")
    print("  test: its grades are absent, not failing.")
    print(f"    Permission mode in effect: {summary['permission_mode'] or 'unknown (no init message)'}")
    print(f"    Permission mode requested: {requested}"
          + (" (something on this host overrode the runner's flag)" if overridden else ""))
    print(f"    Session result: {summary['result_subtype'] or 'none'}"
          + (", reported as an error: " + summary["result_text"][:200] if summary["result_is_error"] else ""))
    print(f"    Tool calls: {summary['tool_calls']} ({summary['executing_calls']} that run"
          f" commands or change files, {summary['executing_calls_ran']} of them ran)")
    print(f"    Permission denials: {summary['permission_denials']}")
    print(f"    Transcript: {transcript}")
    return EXIT_NOT_EXECUTED


def main(argv):
    if len(argv) < 3 or argv[1] not in ("report", "result-text") or (
            argv[1] == "report" and len(argv) < 4):
        print(__doc__.strip(), file=sys.stderr)
        return EXIT_UNKNOWN
    command, transcript = argv[1], argv[2]
    try:
        events = read_events(transcript)
    except OSError:
        events = []
    summary = classify(events)
    if command == "result-text":
        if summary["result_text"]:
            print(summary["result_text"])
        return 0
    return report(summary, transcript, argv[3])


if __name__ == "__main__":
    sys.exit(main(sys.argv))
