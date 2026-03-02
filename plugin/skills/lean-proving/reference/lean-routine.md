# `leanck-agent routine` — reference

Workspace-write Lean 4 proof-leaf helper. Wraps an isolated LLM
session with a private `leanck` MCP server (tools: `health`, `diagnostics`,
`goal`, `sorry`, `search`, `loogle`; sandbox `workspace-write`). Designed to be
invoked directly from a Bash shell.

## When to invoke

- A specific `sorry` whose proof shape is obvious and you just need the
  tactic combination ground out: `simp`/`omega`/`linarith`/`decide`/`exact?`,
  short rewrite chains, structural `constructor`/`cases` plumbing.
- Mechanical leaves where you already know the right strategy.

**Not** for: open-ended proof design, statement changes, hypothesis
additions, weakening conclusions, deep Mathlib search (use `explore`
first), or anything where the right strategy is unclear. The CLI will edit
files in place — calling it on a vague brief wastes a turn and may
leave narrow edits that miss the intent.

## Invocation

```sh
${CLAUDE_PLUGIN_ROOT}/bin/leanck-agent routine --skip-git-check "<full brief>"
```

Run it backgrounded from Claude Code (`Bash` with `run_in_background: true`)
and read the result when the harness reports completion. The binary writes
the final response to stdout and thread metadata to stderr.

### Flags

- `--cwd <path>` — Lean project root. Defaults to the invoker's working
  directory, which is normally what you want. Pass explicitly only if you're
  launching the binary from outside the project.
- `--skip-git-check` — pass this; the wrapped CLI otherwise refuses to
  run outside a git repository.
- `--reasoning {minimal|low|medium|high|xhigh}` — leave at default unless
  the routine is borderline.
- `--sandbox read-only` — use when you only want a tactic *proposal*, not an
  edit. Default is `workspace-write`.
- `--stream` — stream Codex events to stderr; useful when watching the run.
- `--thread <id>` — resume a prior attempt (capture from stderr).

If `${CLAUDE_PLUGIN_ROOT}` isn't exported, resolve the absolute path of the
bundled binary explicitly. Don't fall back to `npx`/`npm`.

## Brief structure

The session sees **only your brief**. Include:

1. **Target** — file path, declaration name, line of the specific `sorry`.
   If there are multiple sorrys, name the exact one to discharge.
2. **Goal shape** — the goal as printed by `leanck:goal` at the sorry site,
   pasted verbatim. Don't make the downstream agent re-derive it.
3. **Available hypotheses** — visible context names and types, especially
   anything you expect to be load-bearing.
4. **Tactic hints** — tactics you think should work (`exact integral_const`,
   `linarith [h_pos]`, etc.), listed as suggestions, not commands.
5. **Constraints** — explicitly forbid statement changes, hypothesis
   additions, and new sorrys.
6. **Verification** — require the agent to call `leanck:diagnostics` on the
   declaration after every edit and to confirm no new sorrys via
   `leanck:sorry` before reporting success.

## Sample brief

```
Target: Math/StieltjesIBP.lean, lemma `stieltjes_ibp_local`, sorry at
line 142.

Goal shape:
  ⊢ IntegrableOn (fun x => f x * g.deriv x) (Set.Ioc a b) volume

Hypotheses:
  hf : Continuous f
  hg : ContDiff ℝ 1 g
  hab : a ≤ b

Tactic hints: try `Continuous.intervalIntegrable` chained with
`ContDiff.continuous` for g.deriv; or `Continuous.integrableOn_Ioc`.

Constraints: do not change the lemma statement, add hypotheses, or
introduce new sorrys.

Verification: after editing, run `leanck:diagnostics` on
`stieltjes_ibp_local` and `leanck:sorry` on the file; confirm zero
errors and that no new sorry was introduced.
```

## After it returns

- `Read` the touched file yourself and confirm the change looks sane
  before reporting success to the user.
- If the agent reports "blocked", do **not** retry with more reasoning —
  escalate (route to `lean-explore` for a Mathlib survey, or hand the proof
  to the user / a deeper agent).
- Re-check `leanck:sorry` on the file from your own session to confirm
  the leaf actually closed.
