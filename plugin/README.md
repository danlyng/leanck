# leanck plugin

Lean 4 proof tools for Claude Code and Codex. The plugin exposes diagnostics,
proof goals, unfinished proofs, declaration lists, builds, Loogle search,
health checks, and workspace switching through `leanck-mcp`.

## Build

From the repository root:

```sh
just build
```

This stages `leanck-mcp` and `leanckd` into `plugin/bin/` and installs `leanck`
and `leanckd` on PATH. The CLI includes `leanck normalize`, used by the style
workflow. A Lean toolchain and Lake must be available through elan in the
project you work on.

The MCP server automatically starts one shared `leanckd` daemon per worktree.
The daemon runs `lake serve` itself, so no editor or extension is required.
The core tools need no API credentials or database setup.

## Configuration

- `.claude-plugin/plugin.json` and `.mcp.json` configure the Claude Code plugin.
- `.codex-plugin/plugin.json` and `.mcp.codex.json` configure the Codex plugin.
- `skills/`, `agents/`, and `hooks/` contain the proof and style workflows.

To launch Claude Code with the checkout, run from your Lean project:

```sh
claude --plugin-dir /path/to/leanck/plugin
```

MCP accepts an optional `--workspace <PATH>`; otherwise it detects the project
from its working directory. Use `set_workspace` to switch projects during a
session. `LEANCKD_BIN` can override the daemon executable; otherwise MCP finds
it beside its own binary or on PATH.

## Search and formatting

`loogle` queries a Loogle HTTP server by constant, name substring,
subexpression, or conclusion type. It tries the local endpoint, then the
self-hosted endpoint, then the main server. `LOOGLE_URL` prepends a custom
endpoint. Search coverage depends on which server answers.

Source cleanup is available through `leanck normalize <FILE_OR_DIR>`.
The separate `leanck-normalize` binary can also be installed with
`cargo install --path leanck-normalize --locked` from the repository root.

See the [MCP tool reference](../leanck-mcp/README.md) and the
[workspace README](../README.md) for more details.
