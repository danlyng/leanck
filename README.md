# leanck — the Lean infoview for agents

`leanck` provides utilities for accelerating agentic Lean 4 development. The motivation is a faster feedback loop: agents write Lean and can immediately inspect proof states and file diagnostics without needing `lake env lean` or running a full `lake build` on every edit. If you've noticed that your editor's infoview gives you feedback far faster than your agents are able to get it, `leanck` is for you.

`leanck` is also attentive to minimizing token usage. Agents can request diagnostics for specific declarations and all output is limited to essential information by default (for example, linter warnings are suppressed by default with the expectation that they will be cheap to clean up later).

It exposes the same capabilities through a command-line interface (`leanck`) and a Model Context Protocol server (`leanck-mcp`), along with skills, subagents, and hooks packaged as a plugin for Claude Code and Codex.

## Features

- **Live proof information:** Query file or declaration diagnostics (`leanck diagnostics`), inspect tactic goals (`leanck goal`), list declarations (`leanck list`), and locate unfinished proofs (`leanck sorry`) against a warm Lean elaborator.
- **Token-efficient builds:** Run `leanck build` to wrap `lake build` in a compact pass/fail summary that strips progress noise and surfaces only the failing modules and errors (with optional `--warnings` and `--strict` modes).
- **Loogle search:** Search Lean and Mathlib declarations by constant, name substring, subexpression, or conclusion pattern via `leanck loogle` (with configurable custom endpoints).
- **Source normalization:** Format comments, reflow Markdown prose, sort imports, standardize attribute placement, and maintain file headers and frontmatter with `leanck normalize`.

## Quickstart

Install prebuilt `leanck` *(install script coming soon)*:

```bash
curl -fsSL https://lean4.party/leanck-install.sh | sh
```

Or build and install from source (requires Rust 1.89+ and [`just`](https://github.com/casey/just)):

```bash
git clone https://github.com/danlyng/leanck.git
cd leanck
just install-cli
```

To also package the Claude Code / Codex plugin into `./leanck-plugin`:

```bash
just package-plugin
```

## Usage

From any Lean 4 project directory:

```bash
# Check workspace and daemon status
leanck doctor

# List leanckd daemons on this machine, or stop the idle ones
leanck daemons
leanck daemons stop --idle

# Wait for elaboration and report errors (or add --severity error,warning / --all-severities)
leanck diagnostics Main.lean
leanck diagnostics Main.lean --ident my_theorem

# Inspect the proof goal at a 0-indexed (line, column) position
leanck goal Main.lean 10 4

# Find `sorry` occurrences in a file or directory
leanck sorry .

# List declarations in a file (optionally filtered by kind)
leanck list Main.lean
leanck list Main.lean --kind theorem,def

# Restart elaboration for a single file
leanck restart-file Main.lean

# Run a workspace build with compact output
leanck build

# Search Mathlib / Lean declarations with Loogle
leanck loogle --constant Nat.add --conclusion "_ <= _"

# Normalize comments, imports, and frontmatter
leanck normalize Main.lean
```

## How it works

`leanck` runs a background daemon (`leanckd`) per Lean worktree that manages a single persistent `lake serve` process over a local Unix socket. When an agent runs a live query through the `leanck` CLI or `leanck-mcp`, the client connects to the workspace's daemon (starting it automatically on first use, including fetching the Mathlib cache when applicable) and queries the running Lean server.

This mirrors how an editor uses the Lean LSP: in both cases, the server process keeps imported `.olean` artifacts and elaborated file states warm in memory, re-elaborating only what changed instead of re-initializing the environment from scratch on each invocation. Because `leanckd` owns and coordinates access to the single `lake serve` instance, multiple concurrent agents and CLI invocations can query the same workspace safely without editor dependencies or multi-client deadlocks. An idle daemon shuts down automatically after an hour. To bound memory, at most `instances.limit` daemons (8 by default) run on a machine; when the limit is reached, starting a daemon for another worktree first stops the longest-idle one. The daemon is asked to stop only if it is still idle, so a daemon that has accepted work in the meantime stays up. `leanck daemons` lists and stops them.

By default, `leanck` also enables Lake's local artifact cache (`LAKE_ARTIFACT_CACHE=true`), allowing worktrees and projects on the same machine to reuse compiled build outputs instead of rebuilding dependencies from scratch (disable with `cache.shared = false`).

```text
CLI (leanck) / MCP (leanck-mcp)
  ├── live queries ──► leanck-client ──► leanckd (per worktree) ──► lake serve
  ├── build ─────────► leanck-lake ────► project's lake build
  ├── loogle ────────► leanck-loogle ──► Loogle HTTP endpoint(s)
  └── normalize ─────► leanck-normalize
```

## Configuration

`leanck` works out of the box with sensible defaults and supports optional TOML configuration at `~/.config/leanck/config.toml`[^0] (user) and `leanck.toml` (project).

```bash
leanck config init      # Write a template leanck.toml in the current directory
leanck config show      # Print the effective configuration
leanck config check     # Validate configuration files and report warnings
```

Any command can also take `--config <FILE>`, `--no-config`, or repeatable `--set KEY=VALUE` overrides. See [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md) for the full configuration reference and [`docs/leanck.example.toml`](docs/leanck.example.toml) for all available options.

## Claude Code and Codex plugin

`just package-plugin` builds release binaries and assembles a ready-to-use plugin in `leanck-plugin/` containing:

- The `leanck-mcp` server and `leanck` CLI
- Skills for theorem proving (`lean-proving`), proof simplification (`lean-simplify`), and style checking (`lean-style`)
- Subagents (`lean-expert`, `lean-grind`, `lean-style-worker`)
- A `PostToolUse` hook for `push_neg` migration hints

See [`packaging/plugin/README.md`](packaging/plugin/README.md) for setup instructions in Claude Code and Codex, and [`crates/leanck-mcp/README.md`](crates/leanck-mcp/README.md) for the MCP tool reference.

## Repository layout

| Path | Description |
|---|---|
| `crates/leanck-cli/` | The `leanck` command-line interface. |
| `crates/leanck-mcp/` | Stdio MCP server (`leanck-mcp`) exposing live Lean queries, builds, and Loogle. |
| `crates/leanckd/` | Per-worktree background daemon managing `lake serve` over a Unix socket. |
| `crates/leanck-client/` | Shared workspace-bound daemon client, auto-start, transport, and output rendering. |
| `crates/leanck-core/` | Shared workspace identity, socket/lock paths, and wire protocol types. |
| `crates/leanck-lake/` | Bounded `lake build` runner, output parser, and Mathlib cache coordination. |
| `crates/leanck-loogle/` | Structured Loogle client with endpoint fallback. |
| `crates/leanck-normalize/` | Lean source and comment normalizer (`leanck normalize` / `leanck-normalize`). |
| `crates/leanck-config/` | Shared versioned TOML configuration loader and validator. |
| `packaging/plugin/` | Plugin manifests and MCP configs for Claude Code and Codex. |
| `leanck-skills/`, `leanck-subagents/`, `leanck-hooks/` | Agent skills, subagent definitions, and hooks bundled into the plugin. |

[^0]: Relax, we use XDG_CONFIG_HOME if it's set
