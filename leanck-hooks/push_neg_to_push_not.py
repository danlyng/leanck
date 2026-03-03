#!/usr/bin/env python3
"""PostToolUse hook: rewrite the deprecated `push_neg` tactic to `push Not`.

Mathlib (v4.29.0) deprecated `push_neg` in favor of the general `push` tactic:
`push_neg` is exactly `push Not`. This hook runs after Claude Code's
Edit/Write/MultiEdit or Codex's apply_patch touches `.lean` files and rewrites any
`push_neg` invocations the model just introduced.

Two subtleties handled here:

  1. Config ordering. In the `push` tactic the optional config comes *before* the
     head term, so `push_neg +distrib` must become `push +distrib Not` (NOT
     `push Not +distrib`). Trailing locations (`at h`, `at *`) are left untouched.

  2. Option-name collision. `push_neg.use_distrib` is a real `set_option` name.
     We must not rewrite `push_neg` when it is followed by `.`, or we'd produce the
     broken `push Not.use_distrib`. The negative lookahead `(?![\\w.])` guards this.

Limitation: replacement is textual, so a literal `push_neg` inside a string or
comment would also be rewritten. That is harmless in practice and vanishingly rare.
Config with nested parentheses (e.g. `(disch := (...))`) is not reordered.

The hook never blocks the edit: on any error it exits 0 silently.
"""

import json
import os
import re
import sys

# `push_neg` as a standalone token (not followed by a word char or `.`), capturing
# any immediately-following config atoms (`+distrib`, `-distrib`, or `(...)`) so we
# can move them ahead of the `Not` head. A trailing `at ...` location is not matched.
PATTERN = re.compile(r"\bpush_neg(?![\w.])((?:[ \t]*[+\-]\w+|[ \t]*\([^()]*\))*)")


# Codex apply_patch headers naming a file that exists after the patch applies.
PATCH_TARGET = re.compile(r"^\*\*\* (?:Add File|Update File|Move to): (.+?)\s*$", re.MULTILINE)


def _replace(match: "re.Match[str]") -> str:
    cfg = match.group(1)  # "" or e.g. " +distrib" / " (config := foo)"
    return "push" + cfg + " Not"


def _edited_paths(payload: dict) -> list[str]:
    """Return edited files: Claude Code passes `file_path`, Codex passes a patch."""
    tool_input = payload.get("tool_input") or {}
    if tool_input.get("file_path"):
        return [tool_input["file_path"]]
    patch = tool_input.get("command")
    if not isinstance(patch, str):
        return []
    cwd = payload.get("cwd") or os.getcwd()
    return [os.path.join(cwd, path) for path in dict.fromkeys(PATCH_TARGET.findall(patch))]


def _rewrite(file_path: str) -> int:
    try:
        with open(file_path, "r", encoding="utf-8") as fh:
            original = fh.read()
    except Exception:
        return 0

    updated, count = PATTERN.subn(_replace, original)
    if count == 0 or updated == original:
        return 0

    try:
        with open(file_path, "w", encoding="utf-8") as fh:
            fh.write(updated)
    except Exception:
        return 0
    return count


def main() -> int:
    try:
        payload = json.load(sys.stdin)
    except Exception:
        return 0

    if not isinstance(payload, dict):
        return 0

    rewritten = {}
    for file_path in _edited_paths(payload):
        if file_path.endswith(".lean"):
            count = _rewrite(file_path)
            if count:
                rewritten[file_path] = count
    if not rewritten:
        return 0

    count = sum(rewritten.values())
    plural = "occurrence" if count == 1 else "occurrences"
    files = ", ".join(rewritten)
    message = (
        f"Auto-rewrote {count} deprecated `push_neg` {plural} to `push Not` "
        f"in {files}. Re-read the file before making further edits to it."
    )
    print(
        json.dumps(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "additionalContext": message,
                }
            }
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
