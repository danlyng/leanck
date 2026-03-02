# `leanck-agent explore` — reference

Read-only Lean 4 research helper. Wraps an isolated LLM session with a
private `leanck` MCP server (tools restricted to `health`, `search`, `loogle`,
sandbox `read-only`). Designed to be invoked directly from a Bash shell.

## When to invoke

- Multi-query Mathlib survey ("what's available for Berge's Maximum Theorem?").
- Side-by-side comparison of several candidate lemmas.
- Coverage map of existing project infrastructure on a topic.
- Anything where a single `leanck:search` or `leanck:loogle` query is clearly
  insufficient.

For a one-shot lookup, call `leanck:search` or `leanck:loogle` directly —
this is intended for more complicated queries.

## Invocation

```sh
${CLAUDE_PLUGIN_ROOT}/bin/leanck-agent explore --skip-git-check "<full brief>"
```

Run it backgrounded from Claude Code (`Bash` with `run_in_background: true`)
and read the result when the harness reports completion. The binary writes
the final response to stdout and Codex thread metadata to stderr.

### Flags

- `--cwd <path>` — Lean project root. Defaults to the invoker's working
  directory, which is normally what you want. Pass explicitly only if you're
  launching the binary from outside the project.
- `--skip-git-check` — pass this; the CLI otherwise refuses to
  run outside a git repository.
- `--reasoning {minimal|low|medium|high|xhigh}` — bump for genuinely
  open-ended searches; leave at the default medium for direct lookups.
- `--stream` — stream Codex events to stderr; useful when watching the run.
- `--thread <id>` — resume a prior exploration (Codex prints `thread: <id>`
  on first run; capture it from stderr).
- `--json` — structured output for downstream post-processing. Default is
  plain text.

If `${CLAUDE_PLUGIN_ROOT}` isn't exported, resolve the absolute path of the
bundled binary explicitly (it lives at `<plugin>/bin/leanck-agent`). Don't
fall back to `npx`/`npm` — that runs the dev source tree, not the bundled
artifact.

## Brief structure

The session sees **only your brief**, not your Claude conversation.
A good brief is several short paragraphs:

1. **Goal** — one sentence stating what you want to know.
2. **Context** — project, files in scope, mathematical object or concept
   involved.
3. **Constraints** — Mathlib version (if known), module scope (`Mathlib.*`,
   local project, or both), declaration kinds to prefer (`theorem`, `lemma`,
   `instance`).
4. **Already-checked** — every search term you've already tried, every
   lemma already known to exist or not exist. The downstream agent should
   not repeat your work.
5. **Output shape** — ranked list, yes/no with evidence, coverage map, etc.

## Sample brief

```
Goal: identify Mathlib infrastructure for proving Berge's Maximum Theorem
(continuity of argmax correspondences) in this project.

Context: working in Math/Berge.lean. The relevant objects are
upper/lower hemicontinuous set-valued maps and continuous selections.

Constraints: Mathlib v4.29.0. Prefer declarations under
Mathlib.Topology.SetValued or Mathlib.Topology.Continuous*. Local
project Math/* is also in scope.

Already-checked: searched "Berge" (no hits), "hemicontinuous" (found
Mathlib.Topology.SetValued.UpperSemicontinuous — already considered).

Output shape: ranked list of candidate lemmas with one-line use-cases.
```

## After it returns

- If the response carries "search is limited to the project DB" (no
  Mathlib snapshot attached), surface that warning to the user verbatim.
- Verify any cited declaration exists before recommending it (a quick
  `leanck:search` on the exact name, or `leanck:loogle` for Loogle-shaped
  queries, is enough).
- Condense the report — don't paste the full Codex output back to the user.
