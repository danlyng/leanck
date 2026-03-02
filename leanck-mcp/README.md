# leanck-mcp

An MCP server over stdio for standalone Lean 4 tooling.

| Tool | Purpose |
|---|---|
| `diagnostics` | Wait for elaboration and report file or declaration diagnostics. |
| `goal` | Inspect the proof goal at a zero-indexed file position. |
| `sorry` | Find unfinished proofs under a file or directory. |
| `list` | List declarations in a file. |
| `restart_file` | Restart a file's elaborator worker. |
| `build` | Run the project's `lake build`; optional targets, warnings/strict policy, JSON, timeout_ms, and max_output_bytes. |
| `loogle` | Search declarations by constant, substring, expression, or conclusion. |
| `health` | Check daemon reachability; compact text or `{ok, lsp, issues}` JSON. |
| `set_workspace` | Switch subsequent calls to another Lake project. |

Live queries go over a Unix socket to `leanckd`, which keeps one warm `lake serve` per worktree. Builds run the project's own Lake toolchain. Loogle uses HTTP, trying the local endpoint and then the main endpoint. Configure `loogle.endpoints` to select custom endpoints.

Live operations pin their source version and session. `DiagnosticsResult.source` is a required pinned `String`; diagnostic excerpts never fall back to the filesystem. Restarts wait for active queries. Directory `sorry` is a text scan; file `sorry` also uses matching elaboration diagnostics. Sorry reports always include quality notes without an `include_notes` opt-in; list/reference fallbacks also expose quality notes. The `sorry` tool accepts `path` only; the former `file` alias is removed. MCP file restarts remain `restart_file`; the CLI uses `leanck restart-file <file>`, with no `restart_file` alias. Goal lines and columns are zero-indexed; columns count UTF-16 code units.

## Diagnostic severity and migration

The `diagnostics` tool's optional `severity` field is an object:

| Selection | `severity` value |
|---|---|
| Errors only (default) | Omit the field, or use `{"mode":"only","severities":["error"]}`. |
| Selected severities | `{"mode":"only","severities":["error","warning"]}` |
| All severities | `{"mode":"all"}` |
| No diagnostics | `{"mode":"only","severities":[]}` |

The accepted names are `error`, `warning`, `information`, and `hint`. An explicit empty selection means none, not all and not the error-only default.

For example, call `diagnostics` with:

```json
{"file":"Main.lean","severity":{"mode":"only","severities":["error","warning"]}}
```

Migrate old MCP severity arrays to the `only` object; replace the old `all` selector with `{"mode":"all"}` and use `information` instead of `info`. In the CLI, use `leanck diagnostics Main.lean --severity error,warning` or repeated flags (`--severity error --severity warning`). Replace `--severity all` with `--all-severities`; the two flags cannot be combined. Omitting both CLI flags selects errors only.

Severity selection and diagnostic severity JSON representations are unchanged by the declaration-kind redesign.

## Declaration kinds and migration

The `list` tool accepts a typed `kind` array. Core uses `ListParams.kind: Option<Vec<KindSelector>>`; the CLI and MCP schema expose the same closed vocabulary. Omission or `[]` selects all declarations; nonempty selections combine with OR. Invalid names are rejected rather than silently returning no matches.

```json
{"file":"Main.lean","kind":["theorem","def","enum_member","type_parameter"]}
```

CLI equivalent (comma-separated or repeated flags):

```sh
leanck list Main.lean --kind theorem,def --kind enum_member,type_parameter
```

Accepted selectors are case-sensitive, canonical snake_case:

- Lean: `theorem`, `lemma`, `def`, `definition`, `abbrev`, `instance`, `structure`, `class`, `inductive`, `opaque`, `axiom`; `section` selects unrefined LSP namespaces.
- LSP: `file`, `module`, `namespace`, `package`, `class`, `method`, `property`, `field`, `constructor`, `enum`, `interface`, `function`, `variable`, `constant`, `string`, `number`, `boolean`, `array`, `object`, `key`, `null`, `enum_member`, `struct`, `event`, `operator`, `type_parameter`, `unknown`.

Lexical Lean kinds take priority and match exactly, except that `def` and `definition` are synonyms. LSP selectors do not match entries refined to Lean lexical kinds: `method` does not select a refined `theorem`, nor `function` a refined `def`. The shared selector `class` matches either Lean `class` or unrefined LSP `class`. Only unrefined LSP entries use these coarse fallbacks:

| Selector | Unrefined LSP kind(s) |
|---|---|
| `theorem`, `lemma`, `instance` | `method` |
| `def`, `definition`, `abbrev` | `method`, `function` |
| `structure` | `class`, `struct` |
| `inductive` | `enum` |
| `axiom`, `opaque` | `constant` |
| `section` | `namespace` |

`ListedSymbol.kind` is now a source-tagged `DeclarationKind`, not a string. The JSON field changes from `"kind":"theorem"` to `"kind":{"source":"lean","kind":"theorem"}`; an unrefined LSP kind is `"kind":{"source":"lsp","kind":"enum_member"}`. Rust callers match `DeclarationKind::Lean(LeanDeclarationKind)` or `DeclarationKind::Lsp(LspSymbolKind)`. Human display is unchanged, including `enumMember` and `typeParameter` labels; request selectors and JSON use `enum_member` and `type_parameter` instead.

Migrate string-based result consumers to the tagged object and replace camelCase selectors with snake_case. The private core/daemon protocol is version 4. Upgrade the CLI, MCP server, and daemon together; explicitly shut down any running older daemon before issuing live queries with the new clients. Version negotiation rejects incompatible peers rather than translating or replaying requests.

Other type changes do not change frontend JSON strings: `LakeError::kind()` returns `LakeErrorKind` with the same serialized categories, and `HealthIssue.level` is typed with the same serialized levels. Normalization failures use structured `NormalizeError` variants; see the [library notes](../README.md#use).

## Protocol 4 migration

When upgrading from protocol 3, remove `SorryParams.include_notes`: quality notes are always included. `DiagnosticsResult.source` is now a mandatory pinned `String`, and `HealthResult.lifecycle` is mandatory. Consumers must not rely on missing fields or filesystem excerpt fallbacks. Typed declaration kinds and diagnostic severity representations are unchanged. The authoritative contract fixture is `test-support/contracts/v4.json`, consumed directly rather than patched from version 1 fixtures.

## CLI

```sh
leanck-mcp [--workspace <PATH>]
```

The workspace defaults to the nearest ancestor of the current directory with `lakefile.lean`, `lakefile.toml`, or `lean-toolchain`. `set_workspace` validates those same markers before switching and returns `{ok, workspace, socket_path}`. Selection does not start Lean. Each operation captures its workspace once; switching affects subsequent calls, while previously started calls keep their original workspace and daemon. No project-name field, database, or API credentials are required.

## Daemon discovery

The CLI and MCP server derive the same socket path for a worktree (see `leanck-core/src/paths.rs`). On first use, MCP connects or starts `leanckd`, using `client.daemon_bin` when configured, then sibling/PATH discovery otherwise. The daemon exits after an hour idle. MCP `health` also starts the daemon when necessary; CLI `doctor` only probes an existing daemon.

Builds remain independent of the daemon and share a workspace lock with leanck cache fetching. The default budget is 30 minutes including queueing; capture defaults to 8 MiB per stream. MCP cancellation stops and reaps Lake. JSON build reports include execution provenance; strict mode rejects dependency and configuration warnings even when Lake exits zero. See [stage 8](../docs/refactor/STAGE_8.md).

Operation timeout_ms values cover queueing and execution after cold startup; the shared client gives startup a separate 30-minute cap. Defaults are 300 seconds per operation, 120 seconds for elaboration, 30 seconds for direct LSP calls, and 5 seconds for the single-file sorry gate. Explicit values override the relevant wait cap within the remaining operation deadline. Socket health/probes default to a separate 2-second transport budget. MCP cancellation uses the same client policy as CLI cancellation. Wire version 4 is negotiated before live query dispatch; incompatible peers return an error without replaying the query.

## Build

```sh
cargo build --release -p leanck-mcp -p leanckd --locked
target/release/leanck-mcp --help
```

The plugin normally launches this server using its MCP configuration. Stdout is reserved for the MCP protocol; logs go to stderr.

Use `just verify-distribution /absolute/path/to/test-project` from the repository root to test temporary release staging, executable discovery, shared CLI/MCP use, workspace switching, and daemon cleanup.

## Shared runtime configuration

The executables load user/project TOML configuration and accept `--config FILE`, `--no-config`, and repeatable `--set KEY=VALUE`. See the [configuration reference](../docs/CONFIGURATION.md) for all settings, precedence, and lifecycle behavior. Use `leanck config show --json` to inspect effective settings and `leanck config check` to validate them.

The read-only `configuration` tool includes structured configuration warnings alongside the selected workspace and settings. It does not inspect source files, read dictionaries, or start Lean. Live timeout warnings account for per-request timeout overrides.
