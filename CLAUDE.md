# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

leanck is a Claude Code plugin for working in Lean 4 projects. It exposes Lean tooling as MCP tools served by a single Rust binary (`leanck-mcp`), backed by a SQLite-based declaration registry with hybrid lexical+semantic search. Read `README.md` and `plugin/README.md` for the user-facing picture; this file is for working on the code.

## Build & test

A `Justfile` drives the common workflows (`just --list` to see all):

- `just build` — full build: stage `leanck-mcp` and `leanckd` into `plugin/bin/`, install the `leanck` CLI, bundle `leanck-agent`.
- `just test` — entire suite (`cargo test --workspace`). Single test: `cargo test -p leanck-registry <test_name>`.
- `just build-mcp` / `just build-leanckd` — release-build the binary and stage it. **macOS detail:** the staged binary must be re-signed ad-hoc (`codesign --force -s -`) after `cp`, or the kernel SIGKILLs it on launch. The Justfile already does this; replicate it if you copy a binary by hand.
- `cargo build --release --workspace` builds all Rust crates; `cargo install --path leanck-cli` installs the user CLI.
- **Live tests** (ignored by default, need `lake`): `LEANCKD_TEST_PROJECT=leanck-mcp/test-project cargo test -p leanckd --test live_smoke -- --ignored`; the socket and MCP-client end-to-ends are `--test socket_smoke` (leanckd) and `--test daemon_e2e` (leanck-mcp, also needs `LEANCKD_BIN`).

## Workspace layout

Cargo workspace (`edition = "2024"`, `resolver = "3"`) of nine Rust crates plus one TypeScript package:

- **leanck-mcp** — the MCP server binary (one per agent; the thing the plugin runs). Forwards LSP tools to `leanckd` over a Unix socket (client in `src/daemon.rs`); hosts `leanck-registry` in-process for search. Tool methods live in `src/mcp.rs`.
- **leanckd** — the Lean server daemon binary. Owns one warm `lake serve` per worktree, ports the `leanck-lsp` state machine / wait gate / query logic to Rust, and answers proof queries over a Unix socket. Auto-spawned by `leanck-mcp` (one daemon per worktree, shared by all agents). Modules: `server/{jsonrpc,transport,lifecycle}`, `state/{docstate,wait}`, `query`, `resolve`, `daemon` (socket server), `workspace` (engine), `cache` (best-effort `lake exe cache get` on first start). **Eager warmup:** `daemon::run` takes an exclusive per-worktree `flock` (the `paths::lock_path` file) before doing any slow work, then `Workspace::start` runs `lake exe cache get` (when `lake-manifest.json` lists Mathlib) and starts `lake serve`, and only *then* binds the socket — so the daemon advertises readiness exactly when it can serve, and a fresh worktree fetches prebuilt oleans instead of building Mathlib from source. The fetch is best-effort (failures logged and ignored) and skipped when `LEANCKD_NO_CACHE_GET` is set. Because the socket appears only after warmup, the client (`client::ensure_running`) waits liveness-aware: it polls the daemon's pid (written before the fetch) and keeps waiting while the daemon is alive (up to a 30-min cap), rather than a flat startup timeout. The lib also exposes the **shared client surface** both front-ends use: `client` (connect/auto-spawn/transport — `leanck-mcp/src/daemon.rs` and the `leanck` CLI both wrap it) and `format` (the compact, token-frugal diagnostics renderer).
- **leanck-registry** — library: SQLite schema, stable identifiers, FTS5 + sqlite-vec indexes, RRF retrieval, optional cross-encoder rerank. Snapshot extract/embed pipeline lives behind the `cli` feature.
- **leanck-cli** — the `leanck` user binary: the human-facing companion to the MCP server, feature-equivalent to it. Top-level commands mirror the MCP tools one-for-one — the live-Lean tools `diagnostics` (alias `d`), `goal`, `sorry`, `list`, `restart_file` speak to the **same `leanckd` daemon** the MCP server uses (client/renderers in `src/commands/lean.rs`, auto-spawned via `leanckd::client`); `build` runs a full `lake build` (project toolchain) via the shared `leanckd::build`; `search`, `premise_search`, `loogle` hit the same registry/Loogle paths; `doctor` (alias `health`) reports full-pipeline health including a non-spawning `leanckd` probe; `normalize` reflows Lean comments/frontmatter via `leanck-normalize`; `agent {explore,review} <prompt_path>` forwards to the bundled `leanck-agent` binary (resolved via `LEANCK_AGENT_BIN`, else a sibling of `leanck`, else `PATH`), passing the prompt as a *file path*. Like the MCP tools, the CLI is **token-frugal by default** — `health` prints just `ok` (or only the warnings/failures) unless `--verbose`/`--json` is passed, and registry-only issues are tagged "(affects search only)". Registry bootstrap/maintenance lives under the `registry` subcommand group: `registry {init, describe, index, update, snapshot {extract,embed}, eval}`. Runs in the user's shell with their API keys — **not** invoked by the plugin.
- **leanck-describe** — library: doc-gen4 → LLM-informalized registry pipeline (driven by `leanck registry describe`).
- **leanck-docs** — library: reads a doc-gen4 `api-docs.db`.
- **leanck-loogle** — library: Loogle client. Tries the local endpoint (`localhost:8118`, which also indexes **Econlib**) first, then the self-hosted server, then the main Loogle server; `LOOGLE_URL` prepends a custom endpoint. `leanck loogle` warns on stderr when a non-local endpoint serves the request, since only the local one searches Econlib (`endpoint_searches_econlib`).
- **leanck-normalize** — library + standalone `leanck-normalize` binary: normalizes Lean comment formatting and top-of-file frontmatter (reflow, import sorting, spelling, etc.). The library backs the `leanck normalize` subcommand; the binary is kept for the plugin's style worker. Depends on **markdown-reflow**.
- **markdown-reflow** — library (+ a small standalone bin): markdown-aware reflow engine used by `leanck-normalize`.
- **leanck-agent** (TypeScript) — Codex SDK wrapper, bundled into `plugin/bin/`. Exposes `explore` / `review` subagents; takes a `prompt_path` (a file whose contents are the brief). No MCP — the agents reach Lean tooling by shelling out to the `leanck` CLI, and each agent's model/reasoning/sandbox is fixed in its declaration (no config flags). Driven by the `leanck agent` subcommand, which forwards to this binary.

`leanck-describe/` and `leanck-docs/` carry their own `.git/` directories — they predate this repo and are versioned independently while still participating in the workspace.

## Architecture

```
Claude Code ──stdio──► leanck-mcp (per agent)
                         ├─ LSP tools  ──Unix socket──► leanckd ──► lake serve (Lean 4 LSP)
                         │    diagnostics, goal, sorry, list, restart_file
                         │    (one warm daemon per worktree, auto-spawned, shared by all agents)
                         ├─ build ──subprocess──► `lake build` (project toolchain) in the worktree
                         │    full `lake build`, compact pass/fail report
                         ├─ Registry tools ──in-process──► leanck-registry
                         │    search, premise_search, set_workspace
                         └─ loogle ──HTTP──► self-hosted Loogle, then main Loogle
```

The MCP server forwards live-Lean tools to the `leanckd` daemon and answers search tools in-process. `health` reports `leanckd` reachability plus registry/snapshot status.

The `build` tool runs a full `lake build` of the worktree with the project's **own** `lake` (the elan toolchain resolved on `PATH` in the worktree) and greps the output into error/warning diagnostic blocks (a diagnostic header is recognized by its `line:col` source location, which excludes lake's locationless summary lines like `error: build failed`). Warnings are included only when requested. The shared parse/render logic lives in `leanckd::build` (so the MCP tool and the `leanck build` CLI command render identically, like `leanckd::format`).

**Hotfix note:** `build` previously used a **forked `lake`** with a global `--json` flag for a structured report. That fork compiled with its own bundled Lean (a different git hash than the project toolchain), so the `.olean`s it produced carried an incompatible header — `lake build` (trace-based) passed, but `lake serve` (which actually imports them) failed with "incompatible header". Until the fork is rebuilt against the matching release tag (`__clones/lean4` at the project's toolchain tag, with only the lake `--json` patch), `leanck build` uses the real toolchain and forgoes the structured JSON report. The `leanck-lake` / `LEANCK_LAKE_BIN` / `just stage-lake` machinery is currently unused by the Rust code.

The daemon is the single shared owner of the Lean server, which is what fixes the multi-agent deadlocks the HTTP-to-VSCode path had: every LSP request is timeout-bounded and `$/cancelRequest`-cancellable; agents waiting on the same file share one elaboration wait (`tokio::Notify` per URI); restarts are serialized per URI; a semaphore caps in-flight elaboration work; and all in-memory state sits behind `std::sync::Mutex` so a guard can never span an `.await` (a compile error). Socket path / pid / lock are derived per-worktree in `leanckd/src/paths.rs`; the wire protocol (`proto.rs`) is shared with the `leanck-mcp` client.

### Stable identifier model

The live-Lean and registry halves share `source_key = "lean-decl:" + fully_qualified_name` and a unified SQLite schema, so search hits can be hydrated against live LSP queries with no impedance mismatch. Row identity in `documents` is `(source_key, signature_hash)` (see schema migrations in `leanck-registry/src/schema.rs`), with a per-worktree `worktree_view` for visibility.

### Search pipeline (`leanck-registry/src/query.rs`)

Three rankers — prose FTS, identifier-trigram FTS, dense KNN (sqlite-vec) — fused with Reciprocal Rank Fusion. Fusion runs per-database (the project DB plus each attached snapshot) and then re-fuses globally on the `(db_origin, doc_id)` keyspace so the Mathlib snapshot and the project DB compete on equal footing. An optional cross-encoder rerank stage (Voyage `rerank-2.5`) re-scores the top fused candidates; **on reranker error the pipeline falls back to RRF order rather than failing** — retrieval-only is graceful degradation. The SQLite connection guard (`!Send`) is contained in a synchronous helper so it never spans the `.await` on the reranker.

### Databases & paths

- **Per-project registry** — `<workspace>/.lean-registry/registry.db`, writable. Sibling of `.lake/` (not inside it) so `lake clean` doesn't wipe embeddings. `.lean-registry/` is gitignored.
- **Mathlib snapshot cache** — `$XDG_CACHE_HOME/lean-registry/mathlib-<rev>-<embedder>.db` (or `~/.cache/lean-registry/`), read-only, shared across all projects on the machine. `ATTACH`ed when the project's `lake-manifest.json` pins the matching `<rev>`; hits are tagged `db_origin: "mathlib"`. Built once via `leanck registry snapshot {extract,embed}`.
- Both paths are overridable: `--registry-db` / `--cache-dir` on `leanck-mcp` and `leanck registry index`.

The plugin does **not** auto-index on load. Bootstrap is manual: `leanck registry describe` (LLM-informalize doc-gen4 output) then `leanck registry index` (embed). `leanck registry update` re-runs both incrementally (skips already-described/already-embedded content), so re-runs after small edits are cheap.

### Embedders & credentials

Voyage is the default embedder (`leanck registry index` / `leanck registry snapshot embed` need a key): `keyring set voyage api_key` or `VOYAGE_API_KEY`. `--embedder openai` uses `OPENAI_API_KEY` instead. Embedder/reranker abstractions live in `leanck-registry/src/{embedders,rerankers}/`.

## Plugin packaging

The plugin manifest (`plugin/.claude-plugin/plugin.json`, MCP config in `plugin/.mcp.json`) references `${CLAUDE_PLUGIN_ROOT}/bin/leanck-mcp`, so both `leanck-mcp` and the sibling `leanckd` must be staged into `plugin/bin/` (via `just build`) before the plugin works. `leanck-mcp` finds `leanckd` as a sibling of its own `current_exe()` (override with `LEANCKD_BIN`, else `PATH`) and auto-spawns it. Ships subagents (`lean-expert`, `math-review`, `lean-style-worker`) and skills (`lean-proving`, `lean-simplify`, `lean-style`) under `plugin/`.

Plugin subagents call `mcp__leanck__*` tools heavily but the plugin sandbox can't pre-approve its own MCP tools — add `"permissions": { "allow": ["mcp__leanck__*"] }` to `.claude/settings.json` to avoid per-call prompts.

## Conventions

- Several MCP tools and CLI commands are deliberately defined but commented out / not exposed (e.g. `lsp_request`, an indexing MCP tool). When re-enabling, uncomment both the input struct and the tool method together — comments in `leanck-mcp/src/mcp.rs` mark the pairs.
- Large `*.db` / `*.jsonl` files in the repo root are local snapshots/fixtures; `*.db*` is gitignored.
