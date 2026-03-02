---
name: "lean-proving"
description: "Lean 4 theorem proving workflow. Use this skill when asked to work on Lean proofs."
allowed-tools: mcp__leanck__* mcp__plugin_leanck_leanck__*
---

# Lean Proving

The guiding principle is **fast feedback**: rather than one-shot a complicated
proof, incrementally build it up by stating smaller lemmas and discharging
them with realtime checks from the Lean elaborator. Every step you take
should be checked against the live elaborator before the next step, because
Lean type-checks proofs as you write them — there is no "compile-time vs.
runtime" distance to bridge.

## Prerequisites

Before invoking this skill, confirm the workspace is in a state where the
MCP server can actually talk to a Lean elaborator:

* `leanck:health` returns `{ok: true}`. If it doesn't, the Lean server
   daemon couldn't be reached or started. It auto-starts on first use, so
   this usually means the project doesn't build. Stop and confirm with the
   user:
  * The project is a valid Lake project (`lake build` succeeds) on a
    toolchain `elan` can resolve from `lean-toolchain`.

If `leanck:search` is also expected to work, inspect the `registry`
section of `leanck:health` and confirm it shows a Mathlib snapshot
attached (look for `mathlib_attached: true`). If `mathlib_attached` is
false, the `mathlib_lookup.reason` field tells you why (no embedder
credentials, missing `lake-manifest.json`, no cached snapshot for the
pinned Mathlib rev, etc.).

`leanck:loogle` is independent of the registry. It uses Loogle over HTTP,
preferring the self-hosted endpoint and falling back to the main server.

## Workflow

1. **Search before you write.** Use `leanck:search` to look up existing
   theorems and lemmas before reaching for a proof. Mathlib is huge — most
   "obvious" intermediate steps already have a named lemma — and the local project
   may have additional lemmas not in Mathlib. Search with either natural-language
   phrases ("monotonicity of integral over filter") or identifier substrings
   (`StrictMono`); the index covers both. Use `leanck:loogle` when you have
   a precise Loogle-shaped query: a constant, a name substring, a subexpression,
   or a conclusion type. For open-ended surveys (multiple candidates, comparison,
   "what infrastructure exists for X?"), delegate to the bundled
   `leanck-agent explore` helper (see
   [Out-of-band helpers](#out-of-band-helpers-leanck-agent) below).
   For *how* to phrase queries well — look for a closing theorem first, query
   definitions / high-level theorems / step lemmas separately, and filter hits
   by type signature — see [`reference/search-queries.md`](reference/search-queries.md).
2. **State the theorem first.** Write the theorem signature and confirm its
   shape is what the user actually asked for *before* attempting a proof.
   A common failure mode is to prove a slightly-wrong conclusion correctly.
   Read the goal back to the user in natural language and confirm.
3. **Sketch with `have ... := by sorry`.** Compose the proof skeleton as a
   sequence of `have` statements with `sorry` bodies. The skeleton tells you
   whether the *decomposition* is sound before you commit time to the leaves.
   If the skeleton's final step doesn't close the goal even with all `have`s
   admitted, the decomposition is wrong — fix that before going deeper.
4. **Discharge `sorry`s in order of risk.** Pick the hardest-looking
   sublemma first. If that one turns out to be false or unprovable as
   stated, you'll find out before sinking time into the easy ones.
5. **Factor large sublemmas.** If a `have` body is going to need more than
   a handful of tactics, hoist it into its own named lemma. Smaller proofs
   are easier for the elaborator to type-check incrementally and easier for
   you (and reviewers) to read.
6. **Check the proof state continuously.** After every meaningful edit,
   call `leanck:diagnostics` on the file, optionally scoped to the declaration
   via `ident`. If necessary, call `leanck:goal` to see the current goal state.
   The elaborator is the ground truth — don't reason about the proof state from memory.
7. **Track remaining `sorry`s.** Use `leanck:sorry` to enumerate
   what's left. A proof isn't done until this list is empty for the declarations
   you're working on.

## Anti-patterns

- **Don't batch many tactic edits then check.** One edit, one diagnostic,
  one goal check. Batching obscures which line broke the proof.
- **Don't add or change hypotheses without user confirmation.** Hypotheses
  should be added or changed only after the user has confirmed they are correct.
- **Don't weaken the conclusion without user confirmation.** The conclusion
  should be weakened only after the user has agreed that the change is necessary.

## Out-of-band helpers (`leanck-agent`)

For work that exceeds a single `leanck:search` or a single tactic edit,
the plugin bundles a CLI helper at `${CLAUDE_PLUGIN_ROOT}/bin/leanck-agent`.

**Invoke them directly via `Bash` with `run_in_background: true`.** The
harness notifies you when the process completes; until then, continue with
other work or wait. Do not poll. Final response goes to stdout; metadata 
(`thread: <id>`, usage stats, streamed events) goes to stderr.

`--cwd` defaults to your working directory -- pass it
explicitly only if you're launching the binary from elsewhere.

The Codex session sees **only the brief you pass on the command line**,
not this conversation. Brief it like a smart colleague who just walked
into the room. A vague brief returns a vague answer and wastes a turn.

### `leanck-agent explore` — read-only Mathlib / project research

Sandbox `read-only`, MCP tools `health`, `search`, `loogle`. Use for:
- Multi-query Mathlib surveys ("what's available for Berge's Maximum Theorem?").
- Side-by-side comparison of several candidate lemmas.
- Coverage maps of existing project infrastructure on a topic.

For a one-shot lookup, call `leanck:search` directly for local/semantic
lookup or `leanck:loogle` directly for exact Loogle queries.

Minimal invocation:

```sh
${CLAUDE_PLUGIN_ROOT}/bin/leanck-agent explore --skip-git-check "<brief>"
```

Full flag list and a brief template live in
[`reference/lean-explore.md`](reference/lean-explore.md). Consult it before
constructing the brief.

### `leanck-agent routine` — workspace-write routine proof leaves

Sandbox `workspace-write`, MCP tools `health`, `diagnostics`, `goal`,
`sorry`, `search`, `loogle`. Use for:
- A specific `sorry` closed by mechanical tactics (`simp`, `omega`,
  `linarith`, `decide`, `exact?`, short rewrite chains).
- Mechanical leaves where you already know the proof shape.

**Not** for: open-ended proof design, statement changes, hypothesis
additions, or anything where the right strategy is unclear. The CLI will
edit files in place — vague briefs leave narrow edits that miss the intent.

Minimal invocation:

```sh
${CLAUDE_PLUGIN_ROOT}/bin/leanck-agent routine --skip-git-check "<brief>"
```

Full flag list and a brief template live in
[`reference/lean-routine.md`](reference/lean-routine.md). Consult it before
constructing the brief.

After the helper returns, re-`Read` the touched file yourself and re-check
`leanck:diagnostics` + `leanck:sorry` from this session before telling the
user the leaf is closed.

## Subagents

For work that needs an advanced subagent — deep reasoning, multi-file
context, judgment calls — the plugin ships two opus-tier agents under
`plugin/agents/`. Reach for these instead of the `leanck-agent` helpers
when the task requires mathematical taste, not just tactic throughput.

Brief them well. Subagents see only what you pass; a vague brief gets a
vague result. Include file paths, the goal as printed by `leanck:goal`,
hypotheses with their types, and what you've already tried.

### `lean-expert` — hard proofs

Opus-tier Lean 4 formalization specialist. Edits files in place against
the live `leanck` MCP elaborator. Use when:

- A proof needs Mathlib-fluent intuition (measure theory, functional
  analysis, intricate `simp`/rewrite chains, deep type-class plumbing).
- Routine tactics have failed and you've already searched Mathlib for the
  obvious lemmas.
- You expect the right answer is a short, clever proof — not a long
  mechanical grind.

Don't use for routine leaves (use `leanck-agent routine`) or for problems
where you suspect the statement itself is wrong (use `math-review` first).

### `math-review` — proof correctness review

Opus-tier mathematical reviewer. Read-only — never edits. Use when:

- You're about to report a long or load-bearing proof back to the user and
  want a second pair of eyes.
- The statement has subtle hypotheses (integrability, non-vacuity,
  measurability, sign conditions) and you want them sanity-checked.
- A proof type-checks but you suspect the conclusion isn't what you meant
  (silent-zero integrals, vacuous quantification, flipped inequalities).
- A `sorry` is closed in a way that feels too easy.

Run this *before* `lean-expert` if you're not sure the proof is even the
right thing to prove.

## MCP Endpoints

These are the tools used in the proving loop. All live under the `leanck`
MCP server; address them as `leanck:<tool>` in `Agent` invocations.

| Tool | Purpose | Key arguments |
|---|---|---|
| `diagnostics` | Errors / warnings / info for a file or single declaration. Reach for this after every edit. | `file` (required, workspace-relative path or `file://` URI); optional `ident` to scope to one declaration; `severity` array of `error`/`warning`/`info`/`hint`/`all` (defaults to `["error"]`); `limit` (default 5) + `offset` for pagination; `verbose` (default false — compact text; true returns full JSON). `Goals accomplished!` diagnostics are always filtered out. |
| `goal` | Lean's pretty-printed proof goal at a position. The ground-truth view of what you still have to prove. | `file`, 0-indexed `line` and `character`. |
| `sorry` | Enumerate declarations with `sorry` under a file or directory path. The "what's left" list. Directories are scanned recursively for `.lean` files. | `path`. |
| `search` | Hybrid semantic + lexical search over the project DB and any attached Mathlib snapshot. The first thing to reach for when looking for existing infrastructure. | `query` (required, free-form natural language and/or identifier substring); `k` (default 10); `kinds` array (e.g. `["theorem","lemma"]`); `module_prefix` (e.g. `"Mathlib.Topology"`); `include_sorried` (default false); `verbose` (default false — returns compact text like `loogle`; set true for the full JSON record including informal text, snippet, score, and `db_origin: "project"` \| `"mathlib"`). Requires the registry to be initialized — see `health.registry`. |
| `loogle` | Exact Loogle search over Lean/Mathlib declarations. Use when you know a constant, name substring, expression pattern, or desired conclusion shape. Separate from registry `search`, so it remains useful when the local snapshot is missing or when Loogle syntax is more precise. | Structured arrays only: `constant` (exact constants), `substr` (quoted name-substring search), `subexpr` (expression patterns), `conclusion` (adds `|-` if omitted), and `limit` (default 10). Calls the self-hosted Loogle endpoint first and falls back to the main server. |
| `list` | Enumerate the declarations in a file (name, fully-qualified name, kind, range). | `file` (required); optional `kind` array of Lean keywords (`theorem`, `def`, `lemma`, `instance`, …) or LSP kinds (`namespace`, `class`, …) to filter. |
| `health` | Reachability of the Lean server daemon + status of the registry, worktree visibility, doc-gen4 coverage, and Mathlib snapshot. | Optional `json` (default false). Compact mode returns `ok` when there are no issues; JSON mode returns `{ok, lsp, registry, issues}`. |
| `restart_file` | Force a single-file elaborator restart when the file gets stuck (rare). | `file`. |
| `set_workspace` | Repoint the MCP server at a different Lake project root. Useful when the user pivots to a different project in the same Claude Code session. | `path` to a directory containing a Lake marker (`lakefile.lean`, `lakefile.toml`, or `lean-toolchain`). The server reopens the registry under `<path>/.lean-registry/registry.db` and best-effort-attaches a cached Mathlib snapshot matching the new project's pinned rev. |

Per-tool parameter docs are also embedded in the tool definitions in
`leanck-mcp/src/mcp.rs`; the table above mirrors those at a higher level.
