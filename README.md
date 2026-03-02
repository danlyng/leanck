# leanck

Standalone tools for working in Lean 4 projects, available through a CLI and an MCP server. Live Lean queries use a shared background daemon, so no editor needs to be running.

- **Live proof information:** diagnostics, proof goals, declaration lists, and unfinished proofs (`sorry`).
- **Project builds:** run the project's own Lake toolchain and return a compact report, with optional warnings, strict mode, and JSON output.
- **Loogle search:** look up declarations by constant, name substring, subexpression, or conclusion type.
- **Source normalization:** reflow Lean comments, normalize spelling and frontmatter, and sort imports.

## Components

| Directory | Purpose |
|---|---|
| `leanck-cli/` | The `leanck` command-line interface. |
| `leanck-config/` | Shared versioned TOML configuration, discovery, validation, and inspection. |
| `leanck-core/` | Shared identity, runtime paths, typed daemon requests/results, and version 4 protocol data. |
| `leanck-client/` | Workspace-bound daemon client, discovery, startup, validated transport, retry policy, and shared live-query rendering. |
| `leanckd/` | One warm `lake serve` per worktree, shared over a Unix socket. |
| `leanck-lake/` | Independent project-toolchain build, parsing/rendering, Mathlib cache utilities, and source-root discovery. |
| `leanck-mcp/` | MCP server over stdio for live Lean tools, builds, and Loogle. |
| `leanck-loogle/` | Shared structured Loogle search, reusable HTTP client, fallback, and endpoint provenance. |
| `leanck-normalize/` | Pure source normalization and an optional command adapter shared by both binary entry points. |
| `plugin/` | Claude Code and Codex plugin manifests, skills, agents, and hooks. |
| `leanck-skills/`, `leanck-subagents/` | CLI-oriented skills and agent definitions. |

```text
CLI / MCP ── live queries ──► leanck-client ──► leanckd ──► lake serve
          ├─ build ────────► leanck-lake ──► project's lake build
          └─ loogle ───────► leanck-loogle ──► Loogle over HTTP

CLI ── normalize ──► leanck-normalize ──► markdown-reflow (Git dependency)
```

The CLI and MCP server automatically start `leanckd` on the first live query. All clients in a worktree share that daemon. An idle daemon exits after an hour and starts again when needed.

`leanck-core` owns shared identity and typed protocol data and has no dependency on the daemon or frontends. Its optional `wire` feature supplies framing for client and daemon. `leanck-client` owns connection policy and live-query rendering; each operation captures one canonical workspace and receives its own timeout options. Responses must match the dispatched request and its result model. A failed connection can trigger startup and one retry; uncertain writes, reads, or protocol failures are never automatically replayed. Both frontends depend on the shared client and independent utilities. The completed [staged refactor plan](REFACTOR_PLAN.md) links the baseline and every implementation record; [stage 9](docs/refactor/STAGE_9.md) records distribution verification and timing comparisons.

## Build and install

Install Rust 1.89 or newer and a Lean toolchain through elan. Each Lean project supplies its own `lean-toolchain` and Lake configuration.

```sh
cargo build --release --workspace --locked
cargo install --path leanck-cli --locked
cargo install --path leanckd --locked

# Optional standalone normalizer
cargo install --path leanck-normalize --locked
```

For the plugin, `just build` stages `leanck-mcp` and `leanckd` into `plugin/bin/` and installs the CLI and daemon on PATH. See [plugin setup](plugin/README.md). Cargo fetches the Markdown reflow engine from its [Git repository](https://github.com/danlyng/markdown-reflow), pinned to a commit in `leanck-normalize/Cargo.toml`. No sibling checkout is required to build.

## Use

From a Lean project:

```sh
leanck health
leanck diagnostics Main.lean
leanck diagnostics Main.lean --severity error,warning
leanck diagnostics Main.lean --all-severities
leanck goal Main.lean 10 4
leanck sorry .
leanck list Main.lean
leanck list Main.lean --kind theorem,def --kind enum_member
leanck build
leanck loogle --constant Nat.add
leanck normalize Main.lean
```

Run `leanck <command> --help` for options. Restart a file with `leanck restart-file Main.lean`; the hidden `restart_file` CLI alias is removed. MCP still uses `restart_file`, while MCP `sorry` accepts only `path`, not the former `file` alias. Goal coordinates are zero-indexed; columns count UTF-16 code units. `doctor` (alias `health`) checks the workspace and probes an existing daemon without starting it; an idle project is healthy. Use `--verbose` for all checks or `--json` for structured output. MCP `health` starts the daemon if needed and checks reachability.

Diagnostics default to errors only. `--severity` accepts the typed names `error`, `warning`, `information`, and `hint`, with comma-separated values or repeated flags (for example, `--severity error --severity warning`). Use `--all-severities` instead of the old `--severity all`; it cannot be combined with `--severity`. MCP uses a severity object rather than an array; see [current semantics and migration](leanck-mcp/README.md#diagnostic-severity-and-migration).

Declaration filters use typed, canonical snake_case selectors: `--kind theorem,def` or repeated `--kind` flags; invalid names are rejected. Omitting the filter selects all declarations (MCP also accepts `"kind":[]`). `def` and `definition` are synonyms. Lexical Lean kinds take priority over coarse LSP kinds. List JSON now carries a tagged kind, such as `{"source":"lean","kind":"theorem"}` or `{"source":"lsp","kind":"enum_member"}`, instead of a string; human display is unchanged. See [selectors and migration](leanck-mcp/README.md#declaration-kinds-and-migration).

The private daemon protocol is version 4. To migrate from version 3, upgrade CLI, MCP server, and daemon together and explicitly shut down any running older daemon; incompatible peers are rejected, not translated. Sorry quality notes are always included (no `include_notes` parameter), `DiagnosticsResult.source` is a required pinned `String`, and `HealthResult.lifecycle` is required. Typed declaration kinds and diagnostic severity representations are unchanged.

Live operation timeouts cover queueing, document synchronization, elaboration, and Lean requests after startup. Cold daemon startup has its own 30-minute cap; health probes default to a 2-second socket budget. CLI Ctrl-C cancels the current live operation, and MCP cancellation follows the same shared client path. Normal queries check the daemon's protocol version first; an incompatible daemon returns an error and stays running until explicitly shut down. The daemon keeps its lifetime lock file in place and removes socket/PID files on graceful shutdown or failed startup.

Loogle tries the local endpoint at `localhost:8118/loogle/json`, then the main Loogle server. Configure `loogle.endpoints` or use `--endpoint` to select custom endpoints. Both frontends report the endpoint that answered; CLI JSON includes a `provenance` object. Search coverage depends on the server's index. The core tools require no embedding or LLM credentials.

`leanck build --warnings` includes dependency diagnostics and configuration warnings emitted by Lake. `--strict` also makes warnings fail the build, including when Lake exits zero. Builds run independently of the daemon and share a workspace lock with leanck cache fetching. Their default 30-minute budget includes waiting for another build/cache job; override it with `--timeout SECS` (MCP: `timeout_ms`). Capture defaults to 8 MiB per stream; `--max-output-bytes BYTES` (MCP: `max_output_bytes`) raises that limit for large logs. Ctrl-C/MCP cancellation stops the process group and reaps Lake. JSON reports include execution provenance. See [stage 8](docs/refactor/STAGE_8.md) for ownership, coordination scope, and verification.

For Rust callers, `LakeError::kind()` returns `LakeErrorKind` rather than a string; frontend JSON category strings are unchanged. `HealthIssue.level` is typed while retaining its existing JSON strings.

Normalization uses `leanck normalize [OPTIONS] [PATH]` or standalone `leanck-normalize [OPTIONS] [PATH]`. Set width with `--width N`, for example `leanck normalize --width 100 Main.lean`; positional numbers are paths, not widths.

For programmatic normalization, `leanck_normalize::normalize(source, width, options, copyright_year)` returns transformed source and statistics without reading files or the clock. The legacy year argument is retained but no longer supplies attribution. Use `normalize_with_context(source, width, options, rules, context)` with explicit file history and optional compiled dictionaries to create missing headers. Existing headers retain their author, date, and license text by default. `normalize.rules.frontmatter.invalid_existing` controls template mismatches: `"warn"` (default), `"ignore"`, or `"replace"` using the template and author history. New headers use one combined copyright notice and exclude configured `ignore_authors` identities (common AI agents by default). Failures are structured `NormalizeError` variants: `InvalidWidth`, `InvalidConfiguration`, `UnmatchedOpeningDelimiter { kind: CommentKind, line }`, and `Reflow(String)`. Match variants instead of parsing error text; delimiter lines are one-based after frontmatter, attribute, and heading normalization. Disable the crate's default features to omit Clap and command I/O; the default `command` feature provides the installed commands.

The daemon library exposes only its `run` entry point and error/result types; document, state, transport, and query modules stay private. The daemon uses a typed Lean session backed by `lsp-server` message parsing, framing, and request queues. Its adapter keeps cancellation and child-process ownership explicit. Private socket frames use Tokio's length-delimited codec with a symmetric 16 MiB limit. The library assessment and preserved contracts are recorded in [stage 6](docs/refactor/STAGE_6.md).

Live queries pin one source version and Lean session while they run. Edits and explicit restarts wait behind those queries, and recovery isolates old-session notifications. Diagnostic JSON always includes the pinned source so excerpts stay consistent with ranges after disk edits, without a filesystem fallback. Listing and lookup share one declaration catalog and identify heuristic fallbacks. Directory sorry reports scan text; file reports also use diagnostics from their pinned version. Sorry reports always include quality notes. The daemon discovers project source roots through Lake and closes idle documents above its retention targets. See [stage 7](docs/refactor/STAGE_7.md) for behavior, compatibility, and verification.

## Configuration

All executables share optional TOML settings for normalization rules and exclusions, Loogle endpoints, builds, diagnostics, client timeouts, daemon limits, and cache fetching. Use `$XDG_CONFIG_HOME/leanck/config.toml` (default `~/.config/leanck/config.toml`) for personal settings and `leanck.toml` for a project. Explicit `--config FILE`, repeatable `--set KEY=VALUE`, and command flags override defaults; `--no-config` gives a clean invocation.

```sh
leanck config init
leanck config show --json
leanck config sources
leanck config check
leanck normalize --dry-run .
leanck --set 'normalize.spelling="off"' normalize Main.lean
```

See the [configuration reference](docs/CONFIGURATION.md) for precedence, every setting, MCP reload behavior, and daemon restart semantics, or the [complete default template](docs/leanck.example.toml). Existing daemons retain their startup settings. MCP's `configuration` tool inspects its captured settings; `set_workspace` reloads project configuration before switching.

## Development

```sh
cargo test --workspace --locked
just --list

# Stage copied release binaries without installing them
just stage /absolute/staging/directory

# Verify temporary staging against a disposable Lean template
just verify-distribution /absolute/path/to/test-project
```

`test-support/contracts/v4.json` is the authoritative protocol fixture; tests consume it directly rather than patching version 1 fixtures.

Live Lean integration tests are ignored by default. See [development notes](AGENTS.md) for how to run them, and [MCP documentation](leanck-mcp/README.md) for the tool surface.

The distribution check covers configured binaries, sibling and PATH discovery, cold startup, overlapping CLI/MCP requests, MCP workspace switching, and explicit daemon shutdown. On macOS it signs copied executables and verifies their signatures. It never queries or changes the supplied template.
