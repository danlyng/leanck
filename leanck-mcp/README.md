# leanck-mcp

The MCP server that fronts the `leanck` plugin. Stdio transport. Three families of tools:

- **LSP-backed** — `diagnostics`, `goal`, `sorry`, `list`, `restart_file`. These are forwarded over a Unix socket to the [`leanckd`](../leanckd) daemon, which owns a warm `lake serve` (the Lean 4 LSP). `leanck-mcp` auto-spawns one daemon per worktree, shared by every agent; the client lives in `src/daemon.rs`.
- **Registry-backed** — `search`, `premise_search`, `set_workspace`. These hit or reopen an in-process [`leanck-registry`](../leanck-registry) library, which owns the project SQLite DB plus an optionally `ATTACH`ed Mathlib snapshot. `search` runs one hybrid query (optionally cross-encoder reranked); `premise_search` fans out several sub-queries and fuses them for proof-planning.
- **Loogle-backed** — `loogle`. This calls the self-hosted Loogle endpoint first and falls back to the main Loogle server. Override with `LOOGLE_URL` when needed.

The MCP `health` tool reports the local state: `leanckd` reachability, plus registry document counts and Mathlib snapshot lookup status.

See the workspace [README](../README.md) for the full architecture and plugin lifecycle.

## CLI

```
leanck-mcp [OPTIONS]

  --workspace <PATH>        Lean project root. Auto-detected from cwd via
                            lakefile.lean / lakefile.toml / lean-toolchain.
  --registry-db <PATH>      Per-project SQLite registry.
                            Default: <workspace>/.lean-registry/registry.db
  --cache-dir <PATH>        Mathlib snapshot cache.
                            Default: $XDG_CACHE_HOME/lean-registry
                                   or ~/.cache/lean-registry
  --embedder <KIND>         voyage | openai | none           [default: voyage]
  --embedder-model <MODEL>  Model override for the chosen embedder.
```

With the default Voyage embedder, `leanck-mcp` reads `VOYAGE_API_KEY` if set;
otherwise it runs `keyring get voyage api_key`.

## Daemon discovery & auto-spawn

`leanck-mcp` and `leanckd` both derive the same per-worktree socket path —
`${XDG_RUNTIME_DIR:-$TMPDIR}/leanckd-{sha1(workspace_path)[0:16]}.sock` (see
`leanckd/src/paths.rs`). On the first Lean tool call, `leanck-mcp` connects to
that socket; if nothing is listening it spawns `leanckd` (resolved via
`LEANCKD_BIN`, then a sibling of its own binary, then `PATH`) and waits for the
socket to come up. The daemon idle-reaps after an hour; a later call respawns it.

## Build / run

```sh
cargo build --release -p leanck-mcp

# Smoke test
target/release/leanck-mcp --help
```

The MCP server itself is normally launched by the Claude Code plugin (see `plugin/.mcp.json`); running it manually is only useful for debugging.
