# leanck

Standalone tools for working in Lean 4 projects, available through a CLI and
an MCP server. Live Lean queries use a shared background daemon, so no editor
needs to be running.

- **Live proof information:** diagnostics, proof goals, declaration lists,
  and unfinished proofs (`sorry`).
- **Project builds:** run the project's own Lake toolchain and return a compact
  report, with optional warnings, strict mode, and JSON output.
- **Loogle search:** look up declarations by constant, name substring,
  subexpression, or conclusion type.
- **Source normalization:** reflow Lean comments, normalize spelling and
  frontmatter, and sort imports.

## Components

| Directory | Purpose |
|---|---|
| `leanck-cli/` | The `leanck` command-line interface. |
| `leanckd/` | One warm `lake serve` per worktree, shared over a Unix socket. |
| `leanck-mcp/` | MCP server over stdio for live Lean tools, builds, and Loogle. |
| `leanck-loogle/` | Shared HTTP client for Loogle. |
| `leanck-normalize/` | Normalizer library and standalone binary; also backs `leanck normalize`. |
| `plugin/` | Claude Code and Codex plugin manifests, skills, agents, and hooks. |
| `leanck-skills/`, `leanck-subagents/` | CLI-oriented skills and agent definitions. |

```text
CLI / MCP ── live queries ──► leanckd ──► lake serve
          ├─ build ────────► project's lake build
          └─ loogle ───────► Loogle over HTTP

CLI ── normalize ──► leanck-normalize ──► markdown-reflow (Git dependency)
```

The CLI and MCP server automatically start `leanckd` on the first live query.
All clients in a worktree share that daemon. An idle daemon exits after an
hour and starts again when needed.

## Build and install

Install Rust and a Lean toolchain through elan. Each Lean project supplies
its own `lean-toolchain` and Lake configuration.

```sh
cargo build --release --workspace --locked
cargo install --path leanck-cli --locked
cargo install --path leanckd --locked

# Optional standalone normalizer
cargo install --path leanck-normalize --locked
```

For the plugin, `just build` stages `leanck-mcp` and `leanckd` into `plugin/bin/`
and installs the CLI and daemon on PATH. See [plugin setup](plugin/README.md).
Cargo fetches the Markdown reflow engine from its
[Git repository](https://github.com/danlyng/markdown-reflow), pinned to a commit
in `leanck-normalize/Cargo.toml`. No sibling checkout is required to build.

## Use

From a Lean project:

```sh
leanck health
leanck diagnostics Main.lean
leanck goal Main.lean 10 4
leanck sorry .
leanck list Main.lean
leanck build
leanck loogle --constant Nat.add
leanck normalize Main.lean
```

Run `leanck <command> --help` for options. Goal coordinates are zero-indexed.
`doctor` (alias `health`) checks the workspace and probes an existing daemon
without starting it; an idle project is healthy. Use `--verbose` for all checks
or `--json` for structured output. MCP `health` starts the daemon if needed
and checks reachability.

Loogle tries the local endpoint at `localhost:8118`, then the self-hosted
server, then the main Loogle server. `LOOGLE_URL` prepends a custom endpoint.
Corpus coverage depends on the endpoint; the CLI warns when falling back
from the local server. The core tools require no embedding or LLM credentials.

## Development

```sh
cargo test --workspace --locked
just --list
```

Live Lean integration tests are ignored by default. See [development notes](CLAUDE.md)
for how to run them, and [MCP documentation](leanck-mcp/README.md) for the tool surface.
