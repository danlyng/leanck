# leanck Claude Code plugin

Combines four components:

- **leanck-mcp** — Rust MCP server (bundled at `${CLAUDE_PLUGIN_ROOT}/bin/leanck-mcp`). Provides LSP-backed tools (`diagnostics`, `goal`, `sorry`, `list`, `restart_file`), registry-backed tools (`search`, `set_workspace`), `loogle` for exact Loogle queries, and `health` for Lean-server plus registry/snapshot status.
- **leanckd** — Rust daemon (bundled at `${CLAUDE_PLUGIN_ROOT}/bin/leanckd`) that owns one warm `lake serve` per worktree and answers proof queries over a Unix socket. `leanck-mcp` finds it as a sibling binary and **auto-starts it on first use** — nothing to install, no editor required. One daemon is shared by all agents on a worktree.
- **leanck-registry** — Rust library owning the shared SQLite schema, FTS5 indexes, sqlite-vec embeddings, and stable `source_key` model. Linked into `leanck-mcp` directly; no separate process. Also exposes the snapshot extract/embed pipeline under the `cli` feature.
- **leanck-cli** — `leanck` user-facing binary (`cargo install --path leanck-cli`), the human companion to the MCP server. Top-level commands mirror the agent tools (`diagnostics`/`goal`/`sorry`/`list`/`restart_file`/`search`/`premise_search`/`loogle`/`doctor`); registry bootstrap lives under `leanck registry {init,describe,index,update,snapshot,eval}`. **Not** invoked from the plugin — runs in your shell with your API keys.

## Prerequisites

1. **Build + stage the plugin binaries** from the workspace root:
   ```
   just build           # populates plugin/bin/{leanck-mcp,leanckd,leanck-agent}
   ```
   The plugin manifest references `${CLAUDE_PLUGIN_ROOT}/bin/leanck-mcp`, which
   auto-spawns the sibling `leanckd` daemon on first use. No editor or VSCode
   extension is required — `leanckd` runs `lake serve` itself.

2. **Install the bootstrap CLI** (one-time, shell-side):
   ```
   cargo install --path leanck-cli
   leanck --help        # mirrors the agent tools; bootstrap under `leanck registry`
   ```

3. **Set embedder credentials** before running `leanck registry index` or launching Claude Code:
   ```
   keyring set voyage api_key
   # or: export VOYAGE_API_KEY=...
   ```
   Voyage is the default embedder. Pass `--embedder openai` (with
   `OPENAI_API_KEY` set) for OpenAI embeddings instead.

## Install the plugin

From the project root:

```
claude --plugin-dir leanck/plugin
```

## Pre-approve MCP tools (recommended)

The plugin ships subagents (`lean-expert`, `math-review`) that call
`mcp__leanck__*` tools heavily, and Claude Code's plugin sandbox does not
let the plugin pre-approve its own MCP tools (the plugin's `settings.json`
only carries `agent` / `subagentStatusLine` keys, and plugin subagents
have their `mcpServers` / `permissionMode` frontmatter ignored). To skip
the per-call permission prompts, add this to `.claude/settings.json` (or
`~/.claude/settings.json` for all projects) in any repo where you use the
plugin:

```json
{
  "permissions": {
    "allow": ["mcp__leanck__*"]
  }
}
```

Without this, the main session still works (the `lean-proving` skill
pre-approves its own MCP calls), but spawned subagents will block on
every MCP call.

## Where things live

- **Per-project registry** — `<workspace>/.lean-registry/registry.db`. Sibling of `.lake/` rather than inside it, so a `lake clean` doesn't wipe your embeddings. Add `.lean-registry/` to your project's `.gitignore`.
- **Mathlib snapshot cache** — `$XDG_CACHE_HOME/lean-registry/` (or `~/.cache/lean-registry/`). Shared across every project on the machine. Snapshot files are named `mathlib-<rev>-<embedder>.db`. `<rev>` is taken from your project's `lake-manifest.json` (the same file `lake` uses to pin Mathlib); `<embedder>` is the embedder id (e.g. `voyage_voyage-4-large`, or `unembedded` for the raw snapshot before vectors are added).

Both paths are overridable on `leanck-mcp` via `--registry-db <PATH>` / `--cache-dir <PATH>`, and on `leanck registry index` via the same flags.

## First-time indexing

The plugin does **not** auto-index on load. To populate the per-project DB, run from the project's working directory:

1. Generate doc-gen4 output for your project (separate Lean tooling).
2. LLM-informalize the declarations:
   ```
   leanck registry describe -i api-docs.db -o .lean-registry/registry.db \
       -p Mathlib.Topology -k theorem -n 100
   ```
3. Embed the per-project DB:
   ```
   leanck registry index
   ```

After that the plugin's `leanck:search` tool sees the project's declarations.
Use `leanck:loogle` separately for exact Mathlib-style queries by constant,
name substring, subexpression, or conclusion type. It uses the self-hosted
Loogle endpoint first and falls back to the main Loogle server; set
`LOOGLE_URL` to override the endpoint.

## Building the Mathlib snapshot once

The plugin `ATTACH`es a cached snapshot read-only and merges its hits into search results tagged with `db_origin: "mathlib"`. Build one with the `leanck registry snapshot` subcommands; the source is the full LLM-informalized Mathlib DB produced by `leanck registry describe` against full Mathlib doc-gen4 output.

```
# 1. (Done once, separately) populate informal-registry.db against full Mathlib
#    via `leanck registry describe` — see leanck-describe/README.md.

# 2. Prune to the mathlib4 + stdlib subset and write the "unembedded"
#    snapshot. --workspace points at a Lean project so the Mathlib rev
#    used in the filename matches what lake-manifest.json pins.
leanck registry snapshot extract \
    --input  ./informal-registry.db \
    --workspace /path/to/your/lean-project

# 3. Embed every document with voyage-4-large (the default). Produces a
#    sibling .db named with the embedder id; resume-safe across runs.
export VOYAGE_API_KEY=...
leanck registry snapshot embed \
    --input ~/.cache/lean-registry/mathlib-<rev>-unembedded.db
```

After that any project pinning the same Mathlib `<rev>` will attach the snapshot automatically on plugin load.
