# leanck

A Claude Code plugin for working in Lean 4 projects. Core capabilities are exposed as MCP tools through a single Rust binary:

1. **Live Lean state** — diagnostics, current proof goal, declaration lists, and sorry locations, pulled from a `lake serve` the plugin manages itself (no editor required).
2. **Search over an LLM-described declaration registry** — hybrid lexical + semantic search across your project's declarations and a cached Mathlib snapshot, with one-line natural-language descriptions generated offline for every declaration. The fused candidates are optionally re-scored by a hosted cross-encoder reranker (Voyage `rerank-2.5`) before they're returned.
3. **Loogle precision search** — exact Mathlib-style lookup by constant, name substring, subexpression, or conclusion type via Loogle.

The live Lean and registry halves share a stable identifier model (`source_key = "lean-decl:" + fully_qualified_name`) and a unified SQLite schema, so search results can be hydrated against live LSP queries with no impedance mismatch.

## Components

```
leanck/
├── Cargo.toml             # workspace root
├── plugin/                # Claude Code plugin manifest + agents/skills
├── leanck-mcp/            # MCP server (binary). Forwards LSP tools to the
│                          # leanckd daemon over a Unix socket; hosts the
│                          # registry in-process for search tools.
├── leanckd/               # Lean server daemon (binary). Owns one warm
│                          # `lake serve` per worktree and answers proof
│                          # queries over a Unix socket. Auto-spawned by
│                          # leanck-mcp; shared across all agents.
├── leanck-cli/            # `leanck` binary — human companion to the MCP
│                          # server. Top-level commands mirror the agent tools
│                          # (diagnostics/goal/sorry/list/restart_file hit the
│                          # same leanckd daemon; search/premise_search/loogle
│                          # the same registry); registry bootstrap lives under
│                          # `leanck registry …`. `cargo install --path leanck-cli`.
├── leanck-registry/       # Library: SQLite schema, stable identifiers,
│                          # FTS5 + sqlite-vec indexes, RRF retrieval, and an
│                          # optional cross-encoder rerank stage. Also exposes
│                          # the snapshot extract/embed pipeline under the
│                          # `cli` feature.
├── leanck-describe/       # Library: doc-gen4 → LLM-informalized registry
│                          # pipeline. Driven by `leanck registry describe`.
├── leanck-docs/           # Helper library: reads doc-gen4 `api-docs.db`.
├── leanck-normalize/      # Library + binary: normalizes Lean comment
│                          # formatting and frontmatter. Backs `leanck
│                          # normalize`; depends on markdown-reflow.
└── markdown-reflow/       # Library: markdown-aware reflow engine.
```

`leanck-describe/` and `leanck-docs/` retain their own `.git/` directories (they predate this repo, under their older names). They participate in the Cargo workspace but are versioned independently. The Rust port of the search layer in `leanck-registry/` was originally derived from a separate Python reference implementation; that reference still exists in a sibling directory outside this repo and is no longer load-bearing here.

## Architecture

```
┌────────────────────── Claude Code (MCP client over stdio) ───────────────────────┐
│                                                                                  │
│  leanck-mcp (Rust binary, one per agent)                                         │
│    ├─ LSP tools  ──────► Unix socket ──► leanckd ──► lake serve (Lean 4 LSP)     │
│    │   diagnostics, goal, sorry, list, restart_file   (one warm daemon/worktree, │
│    │                                                   auto-spawned, shared)      │
│    ├─ Registry tools  ──────► in-process call ──► leanck-registry library        │
│        search, premise_search, set_workspace                                     │
│                                                                                  │
│    └─ External search  ─────► HTTP ──► local Loogle (+Econlib), then self-hosted │
│        loogle                          and main Loogle                           │
│                                                                                  │
│    health reports leanckd reachability plus registry/snapshot status             │
│                                                                                  │
└──────────────────────────────────────────────────────────────────────────────────┘

leanck-registry opens:
  main   = <workspace>/.lean-registry/registry.db   (writable, per-project)
  mathlib = $XDG_CACHE_HOME/lean-registry/
              mathlib-<rev>-<embedder>.db           (read-only, ATTACHed when
                                                     project's lake-manifest.json
                                                     pins a matching rev)

The Mathlib snapshot is produced once by `leanck registry snapshot {extract,embed}`
against a full Mathlib api-docs.db and dropped into the cache directory.
After that it's shared across every project that pins the same Mathlib commit.
```

## Quickstart

```
# 1. Build + stage the plugin binaries (leanck-mcp + leanckd) and install the CLI
just build                            # stages plugin/bin/{leanck-mcp,leanckd,leanck-agent}
cargo install --path leanck-cli       # installs the `leanck` binary on PATH

# 2. Configure the default Voyage embedder
keyring set voyage api_key
# or: export VOYAGE_API_KEY=...

# 3. Install the Claude Code plugin (pointed at the local checkout)
claude --plugin-dir leanck/plugin
```

Then start a Claude Code session in a Lean project directory — `leanck-mcp`
auto-spawns the `leanckd` daemon (which runs `lake serve`) on the first Lean
tool call; no editor needs to be open. Bootstrap the per-project registry from
your shell once:

```
leanck registry describe --input api-docs.db --output .lean-registry/registry.db
leanck registry index
```

After that the plugin's `leanck:search` MCP tool lights up. The separate
`leanck:loogle` tool is available for exact Loogle queries. It tries the local
endpoint (`localhost:8118`, which also indexes Econlib) first, then the
self-hosted server, then the main Loogle server (`LOOGLE_URL` prepends a custom
endpoint). `leanck loogle` warns when a non-local endpoint serves the request,
since only the local one searches Econlib.

See [`plugin/README.md`](plugin/README.md) for the full plugin instructions
and snapshot-building procedure.

## Reading the rest of the repo

- [`plugin/README.md`](plugin/README.md) — plugin manifest, where things live, snapshot-building.
- [`leanck-mcp/README.md`](leanck-mcp/README.md) — MCP server CLI flags, tool surface, daemon lifecycle.
- [`leanck-describe/README.md`](leanck-describe/README.md) — LLM-population pipeline, doc-gen4 expectations (now invoked via `leanck registry describe`).
- `leanck-cli/src/main.rs` — the `leanck` binary's subcommand surface.
- The `leanck-registry/` crate has no separate README — see `leanck-registry/src/lib.rs` for the public API and `tests/integration.rs` for usage examples.
