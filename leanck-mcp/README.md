# leanck-mcp

An MCP server over stdio for standalone Lean 4 tooling.

| Tool | Purpose |
|---|---|
| `diagnostics` | Wait for elaboration and report file or declaration diagnostics. |
| `goal` | Inspect the proof goal at a zero-indexed file position. |
| `sorry` | Find unfinished proofs under a file or directory. |
| `list` | List declarations in a file. |
| `restart_file` | Restart a file's elaborator worker. |
| `build` | Run the project's `lake build`; optionally report warnings or JSON. |
| `loogle` | Search declarations by constant, substring, expression, or conclusion. |
| `health` | Check daemon reachability; compact text or `{ok, lsp, issues}` JSON. |
| `set_workspace` | Switch subsequent calls to another Lake project. |

Live queries go over a Unix socket to `leanckd`, which keeps one warm
`lake serve` per worktree. Builds run the project's own Lake toolchain.
Loogle uses HTTP, trying local, self-hosted, and main endpoints in that order;
`LOOGLE_URL` prepends an override.

## CLI

```sh
leanck-mcp [--workspace <PATH>]
```

The workspace defaults to the nearest ancestor of the current directory with
`lakefile.lean`, `lakefile.toml`, or `lean-toolchain`. `set_workspace` validates
those same markers before switching and returns `{ok, workspace, socket_path}`.
No project-name field, database, or API credentials are required.

## Daemon discovery

The CLI and MCP server derive the same socket path for a worktree (see
`leanckd/src/paths.rs`). On first use, MCP connects or starts `leanckd`, looking
for `LEANCKD_BIN`, then a sibling executable, then PATH. The daemon exits after
an hour idle. MCP `health` also starts the daemon when necessary; CLI `doctor`
only probes an existing daemon.

## Build

```sh
cargo build --release -p leanck-mcp -p leanckd --locked
target/release/leanck-mcp --help
```

The plugin normally launches this server using its MCP configuration.
Stdout is reserved for the MCP protocol; logs go to stderr.
