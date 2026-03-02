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

`leanck:loogle` uses Loogle over HTTP, trying the local endpoint, then the
self-hosted endpoint, then the main server. Corpus coverage depends on the
endpoint. Use `rg` to search local project and dependency source files.

## Workflow

1. **Search before you write.** Use `leanck:loogle` for constants, name
   substrings, subexpressions, and conclusion types. Search project and Mathlib
   source with `rg` for broader discovery. Look for a theorem that closes the
   goal before decomposing it; check each candidate's full type signature.
   See [`reference/search-queries.md`](reference/search-queries.md).
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

## Subagents

For work that needs an advanced subagent — deep reasoning, multi-file
context, judgment calls — the plugin ships two opus-tier agents under
`plugin/agents/`. Use them for proof design and mathematical review.

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

Use `math-review` first when you suspect the statement itself is wrong.

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
| `diagnostics` | Errors / warnings / info for a file or single declaration. Reach for this after every edit. | `file` (required, workspace-relative path or `file://` URI); optional `ident` to scope to one declaration; `severity` object: `{"mode":"all"}` or `{"mode":"only","severities":["error","warning"]}`; names are `error`/`warning`/`information`/`hint`; omission defaults to errors only, and an empty `severities` array selects none; `limit` (default 5) + `offset` for pagination; `verbose` (default false — compact text; true returns full JSON). `Goals accomplished!` diagnostics are always filtered out. |
| `goal` | Lean's pretty-printed proof goal at a position. The ground-truth view of what you still have to prove. | `file`, 0-indexed `line` and `character`. |
| `sorry` | Enumerate declarations with `sorry` under a file or directory path. The "what's left" list. Directories are scanned recursively for `.lean` files. | `path`. |
| `loogle` | Search declarations by constant, name, expression, or conclusion. | Arrays `constant`, `substr`, `subexpr`, `conclusion`; optional `limit` (default 10). Tries local, then main Loogle. |
| `build` | Run the project build. | Optional `warnings`, `strict`, and `json` booleans. |
| `list` | Enumerate the declarations in a file (name, fully-qualified name, kind, range). | `file` (required); optional typed `kind` array, e.g. `["theorem","def","enum_member","type_parameter"]`. Only canonical snake_case names are accepted; invalid names fail. Omission/`[]` selects all; selectors combine with OR; `def`/`definition` are synonyms. |
| `health` | Check the Lean daemon, starting it if needed. | Optional `json` (default false); compact mode returns `ok` when healthy, JSON returns `{ok, lsp, issues}`. |
| `restart_file` | Force a single-file elaborator restart when the file gets stuck (rare). | `file`. |
| `set_workspace` | Switch subsequent tools to another Lake project. | `path` to a directory with `lakefile.lean`, `lakefile.toml`, or `lean-toolchain`; returns `{ok, workspace, socket_path}`. |

For example, call `list` with `{"file":"Main.lean","kind":["theorem","def"]}`. Lexical Lean kinds take priority: `method` does not select a refined `theorem`; coarse LSP fallbacks apply only to unrefined entries. Structured list results now use tagged kinds (`{"source":"lean","kind":"theorem"}` or `{"source":"lsp","kind":"enum_member"}`), not strings; human display is unchanged. See [the full selector and migration reference](../../../leanck-mcp/README.md#declaration-kinds-and-migration). Protocol 3 clients require a matching daemon; upgrade together and explicitly shut down older daemons. Diagnostic severity semantics above are unchanged.

Per-tool parameter docs are also embedded in the tool definitions in
`leanck-mcp/src/mcp.rs`; the table above mirrors those at a higher level.
