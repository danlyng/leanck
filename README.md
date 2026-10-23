# leanck — the Lean infoview for agents

`leanck` provides utilities for accelerating agentic Lean 4 development. The motivation is a faster feedback loop: agents write Lean and can immediately inspect proof states and file diagnostics without needing `lake env lean` or running a full `lake build` on every edit. If you've noticed that your editor's infoview gives you feedback far faster than your agents are able to get it, `leanck` is for you.

`leanck` is also attentive to minimizing token usage. Agents can request diagnostics for specific declarations and all output is limited to essential information by default (for example, linter warnings are suppressed by default with the expectation that they will be cheap to clean up later).

 `leanck` is distributed as a CLI tool. Accompanying skills, subagents, and hooks are packaged as a plugin for Claude Code and Codex.

## Features

- **Live proof information:** Query file or declaration diagnostics (`leanck diagnostics`), inspect tactic goals (`leanck goal`), list declarations (`leanck list`), and locate unfinished proofs (`leanck sorry`).
- **Token-efficient builds:** Run `leanck build` to wrap `lake build` in a compact pass/fail summary that strips progress noise and surfaces only the failing modules and errors (with optional `--warnings` and `--strict` modes).
- **Loogle search:** Search Lean and Mathlib declarations by constant, name substring, subexpression, or conclusion pattern via `leanck loogle` (with configurable custom endpoints).
- **Source normalization:** Format comments, reflow Markdown prose, sort imports, standardize attribute placement, and maintain file headers and frontmatter with `leanck normalize`.

## Install

Install prebuilt `leanck` (macOS or Linux, ARM64 or x86-64):

```bash
/bin/bash -c "$(curl -fsSL https://github.com/danlyng/leanck/releases/latest/download/install.sh)"
```

The installer needs only `bash` (or any POSIX `sh`, as `sh install.sh`), `curl`, `tar`, and `sha256sum` or `shasum`; it does not use Python. [`install.sh`](install.sh) downloads the release archive for your platform, verifies its SHA-256 checksum, and runs the installer bundled in that release (`leanck _install`). The interactive installer walks through installing the CLI, shell completions, and the Claude Code / Codex plugin (with skills, subagents, hooks, and optional MCP server). Pass flags after `--` for non-interactive or custom setups: `--yes` approves without prompts (required without a terminal), `--dry-run` previews, `--json` reports one JSON object, and `--version vX.Y.Z` pins a release. `--help` lists these bootstrap options; every other option (plugin or `--standalone` mode, hosts, skills, agents, completions, directories) goes to the release's installer, and `leanck _install --help` lists them all.

```bash
/bin/bash -c "$(curl -fsSL https://github.com/danlyng/leanck/releases/latest/download/install.sh)" -- --help
```

To install from a source checkout (requires Rust 1.89+ and Cargo), run the same installer with `--from-repo`. It first asks whether to build, because building runs the repository's build scripts (`--yes` answers for you); then it runs `cargo build --release --locked -p leanck-cli`, packages the checkout with `cargo xtask package-local`, and installs the result through the same flow:

```bash
git clone https://github.com/danlyng/leanck.git
cd leanck
sh install.sh --from-repo .
```

For development, `cargo build` builds `leanck`; with [just](https://github.com/casey/just), `just install` stages the plugin executable and installs `leanck` on PATH, and `just package-plugin` assembles `./leanck-plugin`.

Update an installer-managed installation with `leanck update`, or remove it with [`uninstall.sh`](uninstall.sh) (`--dry-run` to preview, `--yes` to approve without a prompt). It runs `leanck _uninstall` from the installed executable or, if that is missing, from the plugin bundle, the release archive holding the script, or PATH.

## Usage

From any Lean 4 project directory:

```bash
# Check workspace and daemon status
leanck doctor

# Wait for elaboration and report errors (add --severity error,warning or --all-severities)
leanck diagnostics Main.lean
leanck diagnostics Main.lean --ident my_theorem

# Inspect the proof goal at a 0-indexed (line, column) position
leanck goal Main.lean 10 4

# Find `sorry` occurrences in a file or directory
leanck sorry .

# Scan text only, without starting the daemon or waiting for elaboration
leanck sorry --fast .

# List declarations in a file (optionally filtered by kind)
leanck list Main.lean --kind theorem,def

# Restart elaboration for a single file
leanck restart-file Main.lean

# Run a workspace build with compact output
leanck build

# Search Mathlib / Lean declarations with Loogle
leanck loogle --constant Nat.add --conclusion "_ <= _"

# Normalize comments, imports, and frontmatter
leanck normalize Main.lean

# List or stop background leanckd daemons
leanck daemons
leanck daemons stop --idle
```

`leanck sorry` elaborates each Lean file in the requested file or directory and combines Lean's declaration warnings with source-token locations, including sorries generated by macros. Directory checks recurse through `.lean` files, skipping `.git`, `.lake`, `build`, `target`, and `node_modules`. Use `--fast` for a daemon-free lexical scan. If elaboration or the request times out, the report includes notes explaining its incomplete diagnostics or text-only fallback; use `--timeout SECS` for larger directory checks.

## How it works

`leanck` runs a background daemon (`leanck serve daemon`) per Lean worktree that manages a single persistent `lake serve` process over a local Unix socket. When an agent runs a live query through the CLI or MCP server, the client connects to the workspace's daemon—starting it automatically on first use and fetching the Mathlib cache when applicable.

Like an editor's Lean LSP session, the server keeps imported `.olean` artifacts and elaborated file states warm in memory, re-elaborating only what changed. Because the daemon coordinates access to a single `lake serve` instance, multiple concurrent agents can query the same workspace safely without editor dependencies or multi-client deadlocks. Idle daemons shut down after an hour, and at most `instances.limit` daemons (8 by default) run per machine, evicting the longest-idle daemon when needed.

By default, `leanck` also sets `LAKE_ARTIFACT_CACHE=true` so worktrees on the same machine share compiled build outputs.

```text
CLI (leanck) / MCP (leanck serve mcp)
  ├── live queries ──► leanck-client ──► leanck serve daemon (per worktree) ──► lake serve
  ├── build ─────────► leanck-lake ────► project's lake build
  ├── loogle ────────► leanck-loogle ──► Loogle HTTP endpoint(s)
  └── normalize ─────► leanck-normalize
```

## Configuration

`leanck` works out of the box and supports optional TOML configuration at `~/.config/leanck/config.toml`[^0] (user) and `leanck.toml` or `.leanck.toml` (project).

```bash
leanck config init      # Interactive setup for project or user configuration
leanck config show      # Print the effective configuration
leanck config check     # Validate configuration files
```

See [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md) and [`docs/leanck.example.toml`](docs/leanck.example.toml) for all options.

## Claude Code and Codex plugin

The plugin bundles the `leanck` binary, skills (`leanck`, `lean-proving`, `lean-style`), subagents (`lean-expert`, `lean-grind`, `lean-style-worker`), and a `PostToolUse` hook that rewrites `push_neg` to `push Not` on Lean 4.29.0+ when `hooks.push_not` is enabled. The hook is off by default and needs no Python. The `leanck` skill is a minimal tool reference without a prescribed proving workflow.

For a la carte installation, use `--standalone` to choose the CLI, individual skills, and custom agents without plugin registration or hooks. Interactive setup briefly describes each skill. For automation, `--skills both --skill leanck` selects only the minimal reference skill for both hosts; repeat `--skill` to include others.

See [`packaging/plugin/README.md`](packaging/plugin/README.md) for manual plugin setup and [`crates/leanck-mcp/README.md`](crates/leanck-mcp/README.md) for the MCP tool reference.

## Repository layout

| Path | Description |
|---|---|
| `crates/leanck-cli/` | The `leanck` command-line interface. |
| `crates/leanck-mcp/` | Stdio MCP server (`leanck serve mcp`). |
| `crates/leanckd/` | Per-worktree daemon (`leanck serve daemon`) managing `lake serve`. |
| `crates/leanck-client/` | Shared daemon client, auto-start, transport, and rendering. |
| `crates/leanck-core/` | Workspace identity, socket/lock paths, and wire protocol types. |
| `crates/leanck-lake/` | Bounded `lake build` runner, output parser, and Mathlib cache coordination. |
| `crates/leanck-loogle/` | Structured Loogle client with endpoint fallback. |
| `crates/leanck-normalize/` | Lean source and comment normalizer. |
| `crates/leanck-update/` | Release checks, the installer behind `install.sh` and `uninstall.sh`, and the self-updater (`leanck update`). |
| `crates/leanck-config/` | TOML configuration loader and validator. |
| `packaging/plugin/` | Plugin manifests and MCP configs for Claude Code and Codex. |
| `xtask/` | Plugin and release packaging (`cargo xtask`). |
| `leanck-skills/`, `leanck-subagents/`, `leanck-hooks/` | Skills, subagents, and hooks bundled into the plugin. |

[^0]: Relax, we use `XDG_CONFIG_HOME` if it's set.
