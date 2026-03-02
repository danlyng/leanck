# leanck plugin

Lean 4 proof tools for Claude Code and Codex. The plugin exposes diagnostics, proof goals, unfinished proofs, declaration lists, builds, Loogle search, health checks, and workspace switching through `leanck-mcp`.

## Build

From the repository root:

```sh
just build
```

This stages `leanck-mcp` and `leanckd` into `plugin/bin/` and installs `leanck` and `leanckd` on PATH. The CLI includes `leanck normalize`, used by the style workflow. A Lean toolchain and Lake must be available through elan in the project you work on.

The MCP server automatically starts one shared `leanckd` daemon per worktree. The daemon runs `lake serve` itself, so no editor or extension is required. The core tools need no API credentials or database setup. Live queries pin their source version; recovery rejects old-session notifications. Directory sorry scans use text, while file scans also use matching diagnostics. Sorry reports always include quality notes, without an `include_notes` parameter. Diagnostic results require pinned source and never use a filesystem excerpt fallback; daemon health requires lifecycle state. MCP `sorry` accepts `path` only, not the former `file` alias. MCP keeps `restart_file`; CLI restarts use `leanck restart-file <file>` without an underscore alias. Tool output identifies heuristic fallbacks.

For temporary staging without installing on PATH, run `just stage /absolute/staging/directory`. It copies `leanck`, `leanck-mcp`, and `leanckd` together and signs them ad hoc on macOS. `just verify-distribution /absolute/path/to/test-project` checks that layout using disposable projects. The plugin ships no replacement Lake.

## Configuration

- `.claude-plugin/plugin.json` and `.mcp.json` configure the Claude Code plugin.
- `.codex-plugin/plugin.json` and `.mcp.codex.json` configure the Codex plugin.
- `skills/`, `agents/`, and `hooks/` contain the proof and style workflows.

To launch Claude Code with the checkout, run from your Lean project:

```sh
claude --plugin-dir /path/to/leanck/plugin
```

MCP accepts an optional `--workspace <PATH>`; otherwise it detects the project from its working directory. Use `set_workspace` to switch projects during a session. `client.daemon_bin` can select the daemon executable; otherwise MCP finds it beside its own binary or on PATH. Workspace selection alone does not start Lean; ongoing calls retain their captured workspace. MCP `health` starts the daemon if needed, while CLI `doctor` probes without spawning. Goal positions are zero-indexed with UTF-16 columns; operation deadlines start after cold startup, and cancellation goes through the shared client.

MCP diagnostics default to errors only. Set `severity` to `{"mode":"only","severities":["error","warning"]}` for selected severities or `{"mode":"all"}` for all. Valid names are `error`, `warning`, `information`, and `hint`; `{"mode":"only","severities":[]}` selects none. Legacy severity arrays are no longer accepted. The private daemon protocol is version 4; when migrating from version 3, upgrade clients and daemon together and explicitly shut down any running older daemon. See [severity migration](../leanck-mcp/README.md#diagnostic-severity-and-migration).

MCP `list` accepts a typed `kind` array, for example `{"file":"Main.lean","kind":["theorem","def","enum_member"]}`. Names are canonical snake_case (`type_parameter`, not `typeParameter`); invalid selectors are rejected. Omission or `[]` selects all; `def`/`definition` are synonyms. Lexical Lean kinds take priority, with coarse LSP matching only for unrefined entries. Structured list results use tagged kinds such as `{"source":"lean","kind":"theorem"}` or `{"source":"lsp","kind":"enum_member"}` instead of strings; human display is unchanged. See [matching rules and migration](../leanck-mcp/README.md#declaration-kinds-and-migration).

Builds use the shared Lake utility without starting a daemon. They coordinate with leanck cache fetching, have a default 30-minute budget and 8 MiB per-stream capture limit, and honor MCP cancellation. Build tools accept targets, timeout_ms, max_output_bytes, warnings/strict policy, and JSON provenance.

## Search and formatting

`loogle` queries a Loogle HTTP server by constant, name substring, subexpression, or conclusion type. It tries the local endpoint, then the main server. Configure `loogle.endpoints` to select custom endpoints. Search coverage depends on which server answers; tool output reports the actual endpoint.

Source cleanup uses `leanck normalize [OPTIONS] [PATH]`, for example `leanck normalize --width 100 Main.lean`. Width is set only with `--width N`; a numeric positional argument is a path. The standalone command has the same syntax: `leanck-normalize [OPTIONS] [PATH]`. Install it with `cargo install --path leanck-normalize --locked` from the repository root.

See the [MCP tool reference](../leanck-mcp/README.md) and the [workspace README](../README.md) for more details.

## Shared runtime configuration

The executables load user/project TOML configuration and accept `--config FILE`, `--no-config`, and repeatable `--set KEY=VALUE`. See the [configuration reference](../docs/CONFIGURATION.md) for all settings, precedence, and lifecycle behavior. Use `leanck config show --json` to inspect effective settings and `leanck config check` to validate them.
