# leanck — the Lean infoview for agents

`leanck` provides utilities for accelerating agentic Lean 4 development. The motivation is a faster feedback loop: agents write Lean and can immediately inspect proof states and file diagnostics without needing `lake env lean` or running a full `lake build` on every edit. If you've noticed that your editor's infoview gives you feedback far faster than your agents are able to get it, `leanck` is for you.

`leanck` is also attentive to minimizing token usage. Agents can request diagnostics for specific declarations and all output is limited to essential information by default (for example, linter warnings are suppressed by default with the expectation that they will be cheap to clean up later).

It exposes the same capabilities through a command-line interface (`leanck`) and a Model Context Protocol server (`leanck serve mcp`), along with skills, subagents, and hooks packaged as a plugin for Claude Code and Codex.

## Features

- **Live proof information:** Query file or declaration diagnostics (`leanck diagnostics`), inspect tactic goals (`leanck goal`), list declarations (`leanck list`), and locate unfinished proofs (`leanck sorry`) against a warm Lean elaborator.
- **Token-efficient builds:** Run `leanck build` to wrap `lake build` in a compact pass/fail summary that strips progress noise and surfaces only the failing modules and errors (with optional `--warnings` and `--strict` modes).
- **Loogle search:** Search Lean and Mathlib declarations by constant, name substring, subexpression, or conclusion pattern via `leanck loogle` (with configurable custom endpoints).
- **Source normalization:** Format comments, reflow Markdown prose, sort imports, standardize attribute placement, and maintain file headers and frontmatter with `leanck normalize`.

## Quickstart

Install prebuilt `leanck` (macOS or Linux, Apple Silicon/ARM64 or x86-64):

```bash
curl -fsSL https://lean4.party/leanck-install.sh | sh
```

Both installation modes prompt for shell completions before final review. The default is the current shell from `SHELL` when supported (Bash, Zsh, Fish, PowerShell, or Elvish), or `none` when the shell is unknown or unsupported. Saved completion choices and explicit flags override that default. Plugin installation also asks whether to add `leanck` to PATH, defaulting to yes, and confirms the shell profile to update. Use `--no-add-to-path` to default that prompt to no or disable the change in automation.

The installer welcomes you with the release version and detected coding agents, then offers **Plugin (recommended)** and **Standalone**, with Plugin selected by default. Use the arrow keys and Enter to select. Plugin mode installs the Claude Code and Codex plugin forms under `~/.local/share/leanck`, including skills, agents, hooks, and MCP, and registers the plugin with detected hosts. The executable goes in `~/.local/share/leanck/bin/leanck`; Codex's MCP configuration pins its full path. Codex custom agents live in `~/.local/share/leanck/agents/` and are linked into `~/.codex/agents/leanck/` (or `$CODEX_HOME/agents/leanck/` when `CODEX_HOME` is set). Plugin mode skips the standalone component prompts.

The final prompt is “Installing `leanck` as a plugin. Continue?” with **Cancel**, **Continue**, and **Edit advanced config...**; Cancel is selected by default. The advanced option opens an inline form for plugin and executable directories, skill and agent locations, shell completions, adding `leanck` to PATH, and plugin registration. Plugin skills and Claude agents are marked `[bundled with plugin]`; Codex agents show their installation directory. Add additional skill, Claude agent, or Codex agent locations in the form; clear a location to remove it from the draft. Shell and shell profile fields appear only when **Add leanck to PATH** is enabled. Installer state and command timeout remain configurable through flags. Up and Down select a field. Enter cycles settings with fixed choices, including yes/no switches, hosts, completion shells, and the PATH shell; Left and Right cycle backward and forward. Enter on a path starts text editing. During a text edit, Enter saves the field to the form’s draft and Escape discards that edit. The cursor appears only while editing. Save changes applies the draft and returns to confirmation. Cancel discards the draft; if any settings differ from their original values, it first asks whether to Keep editing or Discard changes, with Keep editing selected by default. Destination checks and the installation plan are refreshed after changes. The installer verifies the release archive's SHA-256 checksum before approval; installation and host registration happen afterward. It requires neither Rust nor `sudo`; interactive prompts use Bash. Live queries still need your project's Lean toolchain and Lake, usually installed through elan.

Terminal output is colorized by default: prompts are bold cyan, selected menu options are bold green, and typed input uses the terminal’s default color. `--no-color`, nonempty `NO_COLOR`, and `TERM=dumb` disable color; redirected streams use plain text. Prompts read from the controlling terminal so the piped quickstart remains interactive. Without a terminal, pass `--yes` to approve the selected settings or `--dry-run` to preview them. `--force` permits backing up and replacing conflicts and does not imply approval. Use `--json` for a structured result or error (`--json --yes` to install), `--quiet` to suppress routine output, and `-v`/`-vv` for detail.

Choose additional components with flags passed after `sh -s --`:

```sh
# Standalone CLI plus skills for both hosts
curl -fsSL https://lean4.party/leanck-install.sh | sh -s -- --standalone --skills both

# CLI plus the full Claude Code plugin (skills, agents, MCP, hooks)
curl -fsSL https://lean4.party/leanck-install.sh | sh -s -- --plugin claude

# CLI plus the Codex plugin and custom agents
curl -fsSL https://lean4.party/leanck-install.sh | sh -s -- --plugin codex

# CLI, both plugins, and Codex custom agents
curl -fsSL https://lean4.party/leanck-install.sh | sh -s -- --all

# Validate the release and preview a full installation
curl -fsSL https://lean4.party/leanck-install.sh | sh -s -- --all --dry-run

# Noninteractive installation with explicit approval
curl -fsSL https://lean4.party/leanck-install.sh | sh -s -- --yes --plugin codex

# Only skills, using an existing CLI; pin a published release
curl -fsSL https://lean4.party/leanck-install.sh | sh -s -- --no-cli --skills codex --version v0.30.0
```

Use `--standalone` for the standalone CLI and optional skills and custom agents. `--skills` and `--agents` also select standalone mode unless `--plugin` or `--all` explicitly selects plugin mode. Each host flag accepts `none`, `claude`, `codex`, or `both`; repeated flags combine hosts, and `none` clears the selection. `--plugin none` selects standalone mode. Plugin mode includes all skills and agents automatically. `--plugin HOST` chooses which hosts to register; `--all` registers both. Registration requires the corresponding host CLI, and plugin installation requires `python3` for configuration and hooks. If no host is detected, plugin mode still installs both forms for later registration. Restart the host afterward and review the Codex hook with `/hooks`.

| Component | Default installation location |
|---|---|
| Plugin executable | `~/.local/share/leanck/bin/leanck` (`--bin-dir DIR` overrides) |
| Standalone executable | `~/.local/bin/leanck` (`--bin-dir DIR` overrides) |
| Claude standalone skills / agents | `~/.claude/skills/`, `~/.claude/agents/` (honors `CLAUDE_CONFIG_DIR`) |
| Codex standalone skills | `~/.agents/skills/` |
| Codex plugin agents | `~/.local/share/leanck/agents/`, linked from `~/.codex/agents/leanck/` (honors `CODEX_HOME`) |
| Codex standalone agents | `~/.codex/agents/leanck/` (honors `CODEX_HOME`) |
| Plugin marketplace and bundle | `~/.local/share/leanck/bundle/` (honors `XDG_DATA_HOME`; `--data-dir DIR` overrides the `leanck/` directory) |
| Installer choices and backups | `~/.local/state/leanck/installer/` (honors `XDG_STATE_HOME`; `--state-dir DIR` overrides) |
| Fish completions | `~/.config/fish/completions/leanck.fish` |
| Bash completions | `~/.local/share/bash-completion/completions/leanck` |
| Zsh completions | `~/.local/share/zsh/site-functions/_leanck` |
| PowerShell completions | `~/.config/powershell/Completions/leanck.ps1` |
| Elvish completions | `~/.config/elvish/lib/leanck.elv` |

Codex agent definitions use the `leanck/` subdirectory in both modes by default; advanced config or `--codex-agents-dir DIR` overrides the agent destination. Additional locations can also be selected with repeatable `--extra-skills-dir DIR`, `--extra-claude-agents-dir DIR`, and `--extra-codex-agents-dir DIR` flags. The installer honors [`CODEX_HOME`](https://learn.chatgpt.com/docs/config-file/config-advanced#config-and-state-locations), which defaults to `~/.codex`; `--codex-dir DIR` explicitly overrides that location. Unchanged installer-managed definitions from the former flat `agents/` layout are backed up and migrated into `agents/leanck/`. Unrelated files and local edits are preserved.

Plugin remains the default on each run. Standalone mode reuses previous component and destination choices; command-line flags override them. A pinned `--version` remains pinned until you choose `latest`. After upgrading, run `leanck daemons stop --all` to retire daemons using the previous executable. Existing files from another source and locally edited managed components require a keep/replace/cancel choice, or `--force` in automation. Older managed copies without content fingerprints are treated as conflicts. Replacements retain numbered backups with a `manifest.tsv` mapping original paths to backup paths. File installation failures restore prior files; host plugin registration can leave partial external changes, which the installer reports so the same selections can be rerun.

Shell completions support Bash, Zsh, Fish, PowerShell, and Elvish; select a shell in advanced config or with `--completions SHELL`, or disable them with `--completions none`. Completion directories honor `XDG_CONFIG_HOME` or `XDG_DATA_HOME` as appropriate; `--completions-dir DIR` overrides the selected destination, and `--fish-completions-dir DIR` sets the Fish default. Bash and Fish use their standard completion directories. For Zsh, include the completion directory in `fpath` before calling `compinit`; for PowerShell, load the generated script from your profile; for Elvish, load it with `use leanck`. These completion choices do not edit startup files. Completion defaults follow the detected supported shell, with saved choices and explicit flags taking precedence. Plugin installs offer PATH setup by default; standalone installs keep the existing opt-in prompt or `--add-to-path`; `--verbose` includes the exact profile and command after advanced configuration, and the installer preserves the rest of the profile. Use `--shell fish|bash|zsh` and `--path-file FILE` to customize it. Shell startup files are otherwise untouched. `--no-add-to-path` disables a remembered profile update, and `--cli` enables the executable after a previous `--no-cli` selection.

Use `--help` for all options, including an HTTPS release mirror via `--release-url`, configurable download and command timeouts via `--timeout SECONDS`, host-directory overrides, and `--no-pager` for long plans.

`--dry-run` checks prerequisites, downloads and verifies the release in a temporary directory, checks destination conflicts, and prints the files and plugin commands that would be installed or run. It leaves installation directories, existing files, and host configuration unchanged, including when combined with `--force`. Temporary downloads are removed on exit. It does not execute downloaded binaries or host CLIs, so binary execution and host plugin-command compatibility are checked only during a real installation.

If the archive or checksum returns HTTP 404 (for example, before the first release is published), `--dry-run` continues with an explicitly unverified preview using the expected skill and agent names. Destination checks still apply, but release contents, checksum, and version cannot be verified. Other download errors and available-but-invalid releases still fail; real installations always fail on missing assets.

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

## Uninstall

Run the standalone [`uninstall.sh`](uninstall.sh) from a checkout or extracted release archive:

```bash
sh ./uninstall.sh --dry-run
sh ./uninstall.sh
```

The script previews removals and asks for confirmation; use `--yes` for automation. It stops leanck daemons, unregisters recorded Claude Code and Codex plugins and the `leanck-installer` marketplace, removes unchanged installed executables, completions, skills, and agents, and removes managed PATH blocks from shell profiles. It preserves unrelated files, user configuration, Lean projects, caches, and installer backups. Edited components are kept and produce a nonzero exit status; their records remain so you can restore the original contents or remove them manually and rerun. Host-command failures retain installation files and remaining records for a retry. Dry runs execute neither leanck nor host CLIs.

Installer records default to `~/.local/state/leanck/installer`, honoring `XDG_STATE_HOME`; use `--state-dir DIR` if installation used a custom state directory. New installs retain a cumulative inventory of custom and previous destinations. For older installations, the script discovers managed components from saved selections and ownership markers; add `--plugin claude`, `--plugin codex`, or `--plugin both` for plugin registrations, which older installers did not record. Installs made directly with `cargo install` are removed with `cargo uninstall leanck-cli`.

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

`leanck` runs a background daemon (`leanck serve daemon`) per Lean worktree that manages a single persistent `lake serve` process over a local Unix socket. When an agent runs a live query through the `leanck` CLI or MCP server, the client connects to the workspace's daemon (starting it automatically on first use, including fetching the Mathlib cache when applicable) and queries the running Lean server.

Runtime sockets prefer `$XDG_RUNTIME_DIR/leanck`, then the user cache directory, then private per-user temporary directories. Leanck validates socket-path length, directory access, locking, and socket support before starting Lean. Existing daemons remain discoverable in older locations, and fallback daemons count toward the instance limit. See [runtime directories](docs/CONFIGURATION.md#runtime-directories) for the full selection and fallback rules.

This mirrors how an editor uses the Lean LSP: in both cases, the server process keeps imported `.olean` artifacts and elaborated file states warm in memory, re-elaborating only what changed instead of re-initializing the environment from scratch on each invocation. Because `leanckd` owns and coordinates access to the single `lake serve` instance, multiple concurrent agents and CLI invocations can query the same workspace safely without editor dependencies or multi-client deadlocks. An idle daemon shuts down automatically after an hour. To bound memory, at most `instances.limit` daemons (8 by default) run on a machine; when the limit is reached, starting a daemon for another worktree first stops the longest-idle one. The daemon is asked to stop only if it is still idle, so a daemon that has accepted work in the meantime stays up. `leanck daemons` lists and stops them.

By default, `leanck` also enables Lake's local artifact cache (`LAKE_ARTIFACT_CACHE=true`), allowing worktrees and projects on the same machine to reuse compiled build outputs instead of rebuilding dependencies from scratch (disable with `cache.shared = false`).

```text
CLI (leanck) / MCP (leanck serve mcp)
  ├── live queries ──► leanck-client ──► leanck serve daemon (per worktree) ──► lake serve
  ├── build ─────────► leanck-lake ────► project's lake build
  ├── loogle ────────► leanck-loogle ──► Loogle HTTP endpoint(s)
  └── normalize ─────► leanck-normalize
```

The server modes also support explicit launches: `leanck serve mcp [--workspace PATH]` runs MCP over stdio, and `leanck serve daemon WORKTREE` runs a workspace daemon in the foreground. Normal queries start the daemon automatically.

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

- The `leanck` executable, including CLI, MCP, and daemon modes
- Skills for theorem proving (`lean-proving`), proof simplification (`lean-simplify`), and style checking (`lean-style`)
- Subagents (`lean-expert`, `lean-grind`, `lean-style-worker`)
- A `PostToolUse` hook for `push_neg` migration hints

See [`packaging/plugin/README.md`](packaging/plugin/README.md) for setup instructions in Claude Code and Codex, and [`crates/leanck-mcp/README.md`](crates/leanck-mcp/README.md) for the MCP tool reference.

## Installer and release packaging

Build and install from a local checkout with the same plugin/standalone configuration flow:

```sh
sh ./install.sh --from-repo .
```

The installer reads the checkout’s version, skill names, and agent names before configuration. After Continue, it builds the native CLI with Cargo’s locked release profile, packages the local integrations, and installs the selected components. Plugin mode includes both host forms and registers detected hosts; `--standalone` selects standalone components. Local installation requires Python 3.11+ and Rust/Cargo on PATH; `--no-cli` with standalone skills or agents skips compilation. Builds reuse the checkout’s `target/` cache and may fetch missing Cargo dependencies; no published release is downloaded. Use `--timeout 1800` for a slow build. Local installs use the checkout’s version and preserve any remembered release pin; `--version` cannot be combined with `--from-repo`.

For an entirely offline preview without compiling or executing checkout code:

```sh
sh ./install.sh --from-repo . --dry-run
```

This walks through configuration, conflict choices, review, and the approval prompt. Choosing Edit advanced config... opens the editable configuration; choosing Continue only completes the preview. It reads skill and agent names and the CLI version from the source checkout, creates a temporary inventory, and removes it on exit. It needs no build, release archive, network access, or installed agent host. Installation directories, shell profiles, host configuration, and saved installer choices are unchanged. The preview explicitly labels release artifacts as unverified.

Dry runs are interactive when a terminal is available. Add `--yes` to skip prompts, or use `--json` for a noninteractive preview. Without `--from-repo`, dry runs continue to download and verify release archives.

[`install.sh`](install.sh) is the standalone POSIX shell installer to serve as `https://lean4.party/leanck-install.sh`. Its default download source is `https://github.com/danlyng/leanck/releases`; it selects the latest published release unless `--version` pins a tag. Archives and checksums come from the same release, so checksum verification detects corruption but does not provide an independent signature.

The [`Release` workflow](.github/workflows/release.yml) builds native macOS ARM64/x86-64 and static musl Linux ARM64/x86-64 archives. A `v<VERSION>` tag matching `leanck --version` creates a **draft** GitHub release with all archives, their `.sha256` files, `install.sh`, and `uninstall.sh`. Both scripts are included in each archive. A manual workflow run only builds downloadable CI artifacts. To enable the public curl command, publish a reviewed draft release and serve this repository's `install.sh` at the `lean4.party` URL; neither happens as part of local packaging.

To package an already-built native executable locally (Python 3.11+):

```sh
python3 packaging/package_release.py --bin-dir target/release --output dist \
  --target aarch64-apple-darwin --version v0.30.0
```

Supported target names are `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-musl`, and `x86_64-unknown-linux-musl`. Each archive is named `leanck-<TARGET>.tar.gz` and contains a `leanck/` root with the plugin, local marketplace catalogs, and release marker. Release packaging sets the generated plugin versions to the release version. `just test-installer` exercises the actual packager and piped installer with temporary homes, offline downloads, and host CLI stubs. If your default Python is older than 3.11, specify a newer interpreter, for example `just test-installer python3.13`.

## Repository layout

| Path | Description |
|---|---|
| `crates/leanck-cli/` | The `leanck` command-line interface. |
| `crates/leanck-mcp/` | Stdio MCP server (`leanck serve mcp`) exposing live Lean queries, builds, and Loogle. |
| `crates/leanckd/` | Per-worktree daemon library (`leanck serve daemon`) managing `lake serve` over a Unix socket. |
| `crates/leanck-client/` | Shared workspace-bound daemon client, auto-start, transport, and output rendering. |
| `crates/leanck-core/` | Shared workspace identity, socket/lock paths, and wire protocol types. |
| `crates/leanck-lake/` | Bounded `lake build` runner, output parser, and Mathlib cache coordination. |
| `crates/leanck-loogle/` | Structured Loogle client with endpoint fallback. |
| `crates/leanck-normalize/` | Lean source and comment normalizer (`leanck normalize` / `leanck-normalize`). |
| `crates/leanck-config/` | Shared versioned TOML configuration loader and validator. |
| `packaging/plugin/` | Plugin manifests and MCP configs for Claude Code and Codex. |
| `leanck-skills/`, `leanck-subagents/`, `leanck-hooks/` | Agent skills, subagent definitions, and hooks bundled into the plugin. |

[^0]: Relax, we use XDG_CONFIG_HOME if it's set
