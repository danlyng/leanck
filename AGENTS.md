# CLAUDE.md

## Project

leanck provides standalone Lean 4 tooling through a CLI and an MCP server.
Read `README.md` and `plugin/README.md` for setup and the component map.

## Build and test

- `just build` stages the MCP server and daemon into `plugin/bin/` and installs
  the CLI and daemon on PATH.
- `cargo build --release --workspace --locked` builds every binary.
- `cargo test --workspace --locked` runs the Rust suite.
- `just build-mcp` / `just build-leanckd` stage individual binaries. On macOS,
  copies must be re-signed ad hoc; the recipes already do this.
- Live tests need Lake. Set `LEANCKD_TEST_PROJECT` to an absolute path to a
  disposable copy of `leanck-mcp/test-project`, then run
  `cargo test -p leanckd --test live_smoke -- --ignored` and
  `cargo test -p leanckd --test socket_smoke -- --ignored` sequentially.
  The MCP client test is `cargo test -p leanck-mcp --test daemon_e2e -- --ignored`;
  also set `LEANCKD_BIN` to an absolute path to the built daemon.

## Source map

- `leanck-cli/src/cli.rs`: user command surface. Live queries use
  `leanckd::client`; builds use `leanckd::build`; `doctor` is a non-spawning probe.
- `leanck-mcp/src/mcp.rs`: MCP tool definitions. `daemon.rs` wraps the shared
  client and holds the current workspace. `set_workspace` needs only a Lake marker.
- `leanckd`: shared client, Unix socket protocol, Lean server lifecycle,
  elaboration state, queries, build runner, and compact renderers.
- `leanck-loogle`: shared Loogle client; local endpoint first, then self-hosted,
  then main. `LOOGLE_URL` prepends an override. Local corpus coverage may differ.
- `leanck-normalize`: library and standalone normalizer; also backs
  `leanck normalize`.
- `markdown-reflow`: Git dependency from `https://github.com/danlyng/markdown-reflow`,
  pinned by revision in `leanck-normalize/Cargo.toml`. Update it with
  `cargo add -p leanck-normalize markdown-reflow --git https://github.com/danlyng/markdown-reflow.git --rev <COMMIT>`
  and rerun the normalizer tests.
- `plugin/`: plugin manifests, MCP configs, skills, agents, and hooks.
- `leanck-skills/`, `leanck-subagents/`: CLI-oriented workflow definitions.

## Runtime

The daemon warms the project before binding its socket. For Mathlib projects,
it attempts `lake exe cache get` first (best effort; `LEANCKD_NO_CACHE_GET`
skips it). The shared client waits while the daemon is alive, up to its startup
cap. Socket, pid, and lock paths are derived per worktree.

Builds use the project's own `lake` on PATH, preserving toolchain compatibility
with the Lean language server. Parsing and rendering are shared in `leanckd::build`.

The daemon is the single shared owner of the Lean server, which is what fixes the multi-agent deadlocks the HTTP-to-VSCode path had: every LSP request is timeout-bounded and `$/cancelRequest`-cancellable; agents waiting on the same file share one elaboration wait (`tokio::Notify` per URI); restarts are serialized per URI; a semaphore caps in-flight elaboration work; and all in-memory state sits behind `std::sync::Mutex` so a guard can never span an `.await` (a compile error). Socket path / pid / lock are derived per-worktree in `leanckd/src/paths.rs`; the wire protocol (`proto.rs`) is shared with the `leanck-mcp` client.

## Packaging

`plugin/bin/` contains staged executables, not source. MCP discovers the daemon
via `LEANCKD_BIN`, a sibling executable, or PATH. The core has no credential or
database bootstrap step. Normalization is available through the installed CLI.
