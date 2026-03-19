# Configuration

`leanck` works without configuration. Every setting is optional; anything you leave out falls back to the built-in default. [`leanck.example.toml`](leanck.example.toml) lists every setting with its default and a short comment, and `leanck config defaults` prints the same thing.

- [Quick start](#quick-start)
- [Files and precedence](#files-and-precedence)
- [Normalization](#normalization)
- [Loogle](#loogle)
- [Builds, diagnostics, and output](#builds-diagnostics-and-output)
- [Client, daemon, and cache](#client-daemon-and-cache)
- [Machine-wide settings](#machine-wide-settings)
- [Runtime directories](#runtime-directories)

## Quick start

```sh
leanck config init          # Guided setup for this project (leanck.toml)
leanck config init --user   # Guided setup for your user defaults
leanck config show          # Print the effective configuration
leanck config sources       # Show which file or flag set each value
leanck config check         # Validate, and report suspicious combinations
leanck config defaults      # Print every default
```

A typical project file:

```toml
version = 1

[normalize]
width = 100
spelling = "off"
exclude = ["Generated/**", "Vendor/**"]

[build]
strict = true

[diagnostics]
limit = 10
severity = ["error", "warning"]
```

### The setup wizard

`leanck config init` asks whether to configure the project or your user defaults, then lets you pick topics: formatting and spelling, license and header format, diagnostics, builds, Loogle, daemon behavior, and the Mathlib cache. User setup also covers the machine-wide daemon limit and updates. When the terminal is wide enough, a side pane shows examples, including before-and-after formatting samples.

| Key | Action |
|---|---|
| Arrows, Enter | Move and accept; Enter in a text field keeps the shown value |
| Space | Toggle a topic or option |
| Ctrl-A / Ctrl-D | Select all / clear in multi-select menus |
| Tab, Shift-Tab | Move focus between the setting and the Previous/Next buttons |
| Ctrl-U, Ctrl-W | Clear the field / delete a word |
| Page Up/Down | Scroll the review screen |
| Esc | Offer to discard the draft |
| Ctrl-C | Quit without saving |

In license-text and header-template fields, type `\n` for a line break and `\\` for a backslash.

The wizard shows existing and inherited values and writes only what you change, so everything else keeps inheriting. Nothing is written until you confirm on the review screen. Comments and unrelated settings in an existing file are preserved.

Destinations:

- Project: `leanck.toml` in the current directory (or `--workspace DIR`). An existing `.leanck.toml` is reused if it's the only project file there.
- User: `$XDG_CONFIG_HOME/leanck/config.toml`, by default `~/.config/leanck/config.toml`.
- Anywhere else: `leanck config init /path/to/file.toml`.

Without a terminal (`--no-interactive`, `--json`, redirected stdin or stderr, or `TERM=dumb`), `init` writes a minimal starter file with commented examples and refuses to overwrite an existing file. `NO_COLOR` disables colors.

## Files and precedence

Later sources override earlier ones:

1. Built-in defaults.
2. User file: `$XDG_CONFIG_HOME/leanck/config.toml`, by default `~/.config/leanck/config.toml`.
3. Project file: the nearest `leanck.toml` or `.leanck.toml`, searching upward from the workspace (or input) directory and stopping at the Lake project or Git root. If both exist in one directory, only `leanck.toml` is read.
4. `LEANCK_CONFIG=FILE`.
5. `--config FILE`.
6. `--set KEY=VALUE`, repeatable and applied in order.
7. Command-line flags and MCP tool parameters.

Merging rules:

- Tables merge key by key, and arrays replace earlier arrays (an empty array included).
- Relative paths inside a config file resolve from that file's directory. `~` and `$VAR` are not expanded.
- Unknown keys, wrong types, malformed TOML, bad URLs or globs, and out-of-range values are errors, not warnings.
- `config show` and `config check` also print nonfatal warnings for settings that have no effect, such as heading rules while `normalize.headings = false`.

A discovered project file cannot set `[instances]` or `[updates]`. These tables are skipped there and `leanck doctor` warns about it. Put them in your user file, or pass them with `--config`, `LEANCK_CONFIG`, or `--set`.

### Command-line flags

Every executable accepts `--config FILE`, `--no-config`, and `--set`, either before or after the subcommand. `--no-config` ignores every config file, `LEANCK_CONFIG` included, but still honors `--set`.

`--set` takes a TOML value, so strings need quotes inside the shell argument:

```sh
leanck --set 'normalize.spelling="british"' normalize Main.lean
leanck build --set 'build.strict=false'
leanck loogle --set 'loogle.endpoints=[]' --constant Nat.add
leanck --no-config --set 'cache.enabled=false' diagnostics Main.lean
```

The environment variables `LOOGLE_URL`, `LEANCKD_BIN`, and `LEANCKD_NO_CACHE_GET` are no longer read. Use `loogle.endpoints`, `client.daemon_bin`, and `cache.enabled` instead.

## Normalization

`leanck normalize [PATH]` (or the standalone `leanck-normalize`) rewrites Lean files in place. `--dry-run` lists the files it would change without writing them, and exits nonzero if any would change; given stdin, it prints nothing and only checks the input.

Top-level switches in `[normalize]`:

| Key | Default | Effect |
|---|---|---|
| `width` | `100` | Comment reflow width and long-attribute threshold. A positive integer or `"unlimited"`; `--width` overrides it. |
| `frontmatter` | `true` | Header, import, and frontmatter ordering rules. |
| `header` | `true` | Format existing headers; generate missing ones from Git history. |
| `sort_imports` | `true` | Import sorting, grouping, and deduplication. |
| `attributes` | `true` | Attribute placement rules. |
| `headings` | `false` | Banner conversion and heading title formatting. |
| `comments` | `true` | All block-comment transformations, spelling included. |
| `comment_reflow` | `true` | Rewrap comment prose to `width`. `false` keeps the existing line breaks. |
| `linkify_urls` | `false` | Turn bare URLs in comments into Markdown links. |
| `spelling` | `"american"` | `"american"`, `"british"`, or `"off"`. |
| `capitalize_after_colon` | `false` | Capitalize the word after a colon in comments. |
| `collapse_blank_lines` | `true` | Apply `rules.whitespace.max_blank_lines`. |
| `exclude` | `[]` | Globs skipped during directory traversal, relative to the input directory. |

Notes:

- `width = "unlimited"` joins soft-wrapped lines into one line per paragraph. To keep the existing wrapping instead, set `comment_reflow = false`.
- `exclude` uses globset syntax (`*`, `?`, `**`). A matching directory excludes its whole subtree. Hidden entries and symlinks are always skipped, and a file named explicitly on the command line is processed even if it matches.
- Only comment prose changes. Code, math, HTML, URLs, and link destinations in comments are left alone, and so is a doc comment whose rewrite Lean would parse differently, or that serves as the expected output of a `#guard_msgs` command.

### Turning rules off locally

Standalone line comments disable rules for a region:

```lean
-- leanck: off
-- Everything here is left verbatim.
-- leanck: on

-- leanck: off spelling,attributes
-- Only spelling and attribute placement are disabled here.
-- leanck: on spelling,attributes
```

Rule names are `frontmatter`, `headings`, `attributes`, `comments`, `spelling`, `comment_reflow`, `capitalize_after_colon`, and `whitespace`. With no names, or with `all`, a directive applies to every rule. Directives don't nest:

- `off NAMES` adds rules to the disabled set.
- `on NAMES` re-enables only those rules, even after a bare `off`.
- A bare `on` re-enables everything.

Frontmatter is processed only above the first directive. Unknown rule names are errors. Set `normalize.rules.directives = false` to ignore all directives.

### Headers and frontmatter

Settings live under `[normalize.rules.frontmatter]`:

| Key | Default | Effect |
|---|---|---|
| `order` | `["header", "module", "imports", "docstring", "section", "namespace"]` | Components to reorder, in this order. |
| `blank_lines` | `1` | Blank lines between components (still capped by `max_blank_lines`). |
| `header_template` | Mathlib style | Template used for generated headers and checked against existing ones. |
| `license_text` | `""` | Text substituted for `{license}`. |
| `normalize_existing` | `true` | Format the delimiters and spacing of an existing header. |
| `invalid_existing` | `"warn"` | What to do with a header that doesn't match the template: `"warn"`, `"ignore"`, or `"replace"`. |
| `ignore_authors` | AI agent identities | Git authors left out of generated headers. |
| `author_map` | `{}` | Git author name → display name for generated headers. |
| `follow_renames` | `true` | Follow renames when reading file history. |
| `history_timeout_ms` | `5000` | Time limit for reading Git history (at most 60000). |
| `module_doc_template` | unset | If set, add a missing module docstring, e.g. `"/-! # About {module} -/"`. |

The default header carries no license notice. Set `license_text` if your project needs one:

```toml
[normalize.rules.frontmatter]
license_text = "Released under Apache 2.0 license as described in the file LICENSE."
header_template = "/-\n{copyright}\n{license}\nAuthors: {authors}\n-/"
```

Template placeholders:

| Placeholder | Value |
|---|---|
| `{copyright}` | One line, e.g. `Copyright (c) 2020–2026 Alice, Bob.` |
| `{authors}` | Author names, sorted and deduplicated |
| `{years}`, `{first_year}`, `{last_year}` | Year range from history |
| `{license}` | `license_text` |
| `{module}` | File stem (for `module_doc_template` only) |

Lines that render empty are dropped. A template must be exactly one Lean block comment, with no comments nested inside it. Unknown placeholders are errors.

How headers are handled:

- A block comment at the very top of the file (not a docstring) counts as the header. Other plain block comments in the frontmatter stay where they are.
- An existing header is compared with the template by structure and fixed text, ignoring layout. Names and dates are not checked against history. With the default `"warn"`, a mismatch produces a `header_template_mismatch` warning, `--dry-run` included.
- A missing header is generated from the file's Git author history, following renames and honoring mailmaps. Committers and `Co-authored-by` trailers are not counted. Nothing is fetched. If history is unavailable (untracked file, shallow clone, timeout, stdin input, or every author ignored), no header is added and a note goes to stderr.
- `ignore_authors` matches exact names, emails, or `Name <email>`, case-insensitively, against both the raw and the mailmapped identity. The default list covers Claude Code, Codex, Cursor, Copilot, Aider, Gemini Code Assist, and Devin. Listing your own entries replaces the defaults; `[]` includes everyone.
- `author_map` keys are exact, case-sensitive names after mailmap resolution. Maps from different files merge by name; `--set 'normalize.rules.frontmatter.author_map={}'` clears inherited entries.
- Existing headers keep their authors and dates unless `invalid_existing = "replace"`.

```toml
[normalize.rules.frontmatter.author_map]
"git-handle" = "Full Name"
```

Ordering:

- Components left out of `order` don't move, and nothing moves past them. For example, `["header", "module", "imports", "docstring"]` leaves `section` and `namespace` commands in place. `[]` disables reordering but still formats headers and imports.
- Only the recognized opening block of the file is reordered. The first declaration or other unrecognized content ends it.

### Headings

Requires `normalize.headings = true`. Settings live under `[normalize.rules.headings]`:

```toml
[normalize.rules.headings]
level = 2                     # Level for converted banners (1–6)
template = "/-! {hashes} {title} -/"
case = "title"                # "title", "sentence", or "off"
small_words = ["a", "an", "and", "of", "the", "to"]  # Replaces the default list

[normalize.rules.headings.replacements]
"Old heading" = "Preferred heading"   # Applied before case formatting
```

Banner comments like the one below are converted to Markdown headings using `level` and `template`:

```
/-
============
My heading
============
-/
```

Existing Markdown headings in comments keep their level, but `replacements` and `case` still apply to their titles. Title case keeps existing capitals and leaves `small_words` lowercase inside a title.

### Spelling

`normalize.spelling` chooses the built-in American or British dictionary, or `"off"` to disable spelling entirely, custom dictionaries included. Settings live under `[normalize.rules.spelling]`:

```toml
[normalize.rules.spelling]
use_builtin = true            # false: use only your own dictionaries
files = ["dictionaries/project.json"]   # JSON objects: {"from": "to"}
protected_words = ["colour", "MyProjectTerm"]
case_sensitive = false        # false: matches any case and preserves it
inflections = true            # Also handle -s, -ed, -ing, ... forms

[normalize.rules.spelling.replacements]
"specialise" = "specialize"
```

Precedence is built-in dictionary, then `files` in order, then `replacements`, then `protected_words`. Chains of replacements resolve to their final word, and cycles are errors.

The built-in dictionaries skip words that are wrong or ambiguous in mathematical writing, such as `gauge` and `analogue` (American) or `program` and `check` (British). `leanck normalize --export-dictionary american` prints the dictionary as applied. To restore a skipped word, add it to `replacements`.

Words that look like Lean identifiers are never changed: dotted (`Colour.red`), primed (`colour'`), subscripted (`colour₁`), or camelCase (`colourIn`).

### Attributes

```toml
[normalize.rules.attributes]
placement = "same_line"   # "same_line", "above", or "off"
above_when_long = false   # With same_line: move above if the line would exceed width

[normalize.rules.attributes.overrides]
ext = "above"             # Per-attribute placement, by bare name
```

Attribute order never changes. Groups that would need reordering, contain conflicting overrides, span several lines, or contain a comment are left alone. Doc comments on the declaration line don't count toward `above_when_long`.

### Imports

```toml
[normalize.rules.imports]
order = "alphabetical"   # or "off" (grouping and deduplication still apply)
public_first = true      # public imports first; meta imports first within each kind
preserve_groups = true   # sort within blank-line-separated groups only
deduplicate = true
```

Only imports in the module header are touched. Sorting never moves an import past a comment. With `max_blank_lines = 0` the groups merge and are sorted as one group.

### Comment delimiters

Each setting is `"compact"` (`/-- Text -/`), `"separate"` (delimiters on their own lines), or `"off"` (leave as is):

```toml
[normalize.rules.comments]
module = "separate"      # /-! module docs
declaration = "compact"  # /-- declaration docs
section = "compact"      # other /-! comments
heading = "compact"      # /-! comments containing only one heading; defaults to section
```

A `/-!` comment that opens with a level-one heading followed by prose counts as a module doc. One that opens with a subheading counts as a section comment. Setting `heading = "compact"` alongside `module = "separate"` and `section = "separate"` keeps lone headings on one line while separating documentation blocks. Delimiters never share a line with a code fence, display math, or a table row.

### Whitespace

```toml
[normalize.rules.whitespace]
max_blank_lines = 1       # 0–254
line_endings = "lf"       # "lf", "crlf", or "off"
final_newline = "ensure"  # "ensure", "remove", or "off"
remove_bom = true         # Strip a leading UTF-8 byte-order mark
```

Blank lines inside code fences, display math, and string literals are kept. Inside doc comments, one blank line always remains between Markdown blocks.

## Loogle

| Key in `[loogle]` | Default | Effect |
|---|---|---|
| `endpoints` | local, then `https://loogle.lean-lang.org/json` | Endpoints tried in order. |
| `timeout_ms` | `15000` | Per-endpoint time limit. |
| `limit` | `10` | Maximum results. |

`endpoints` is the complete fallback chain: a list of local endpoints only never falls back to the public server, and `[]` disables search. URLs must be absolute HTTP(S) with no credentials, query string, or fragment. `--endpoint` replaces the list for one command.

## Builds, diagnostics, and output

| Key | Default | Effect |
|---|---|---|
| `build.warnings` | `false` | Report Lake warnings. |
| `build.strict` | `false` | Fail on warnings, even if Lake succeeds; implies `warnings`. |
| `build.targets` | `[]` | Default targets; `[]` builds the workspace. Targets on the command line replace this list. |
| `build.timeout_ms` | `1800000` | Total build time limit (30 minutes). |
| `build.max_output_bytes` | `8388608` | Output captured per stream (8 MiB). |
| `diagnostics.limit` | `5` | Diagnostics per page. |
| `diagnostics.severity` | `["error"]` | Any of `error`, `warning`, `information`, `hint`. |
| `output.json` | `false` | Default to JSON output for build, live-query, doctor, and Loogle commands. |

CLI flags override these: `--warnings`, `--strict`, `--timeout SECS` (seconds, unlike the config), `--max-output-bytes`, `--limit`, `--severity`, `--all-severities`, `--json`.

## Client, daemon, and cache

Live queries go to one background daemon per worktree. These settings apply when a daemon starts, so restart running daemons (`leanck daemons stop --all`) for changes to take effect.

| Key | Default | Effect |
|---|---|---|
| `daemon.idle_timeout_ms` | `3600000` | Shut down after an idle hour; `0` disables idle shutdown. |
| `daemon.max_inflight` | `8` | Concurrent elaboration-bound requests. |
| `daemon.max_documents` | `128` | Open files kept in memory. |
| `daemon.max_source_bytes` | `67108864` | Total source kept in memory (64 MiB). |
| `daemon.request_timeout_ms` | `300000` | Default time limit for a query. |
| `daemon.elaboration_timeout_ms` | `120000` | Time limit for waiting on elaboration. |
| `daemon.lsp_timeout_ms` | `30000` | Time limit for individual Lean server requests. |
| `daemon.permit_timeout_ms` | `30000` | Wait for a free slot before reporting overload. |
| `daemon.sorry_timeout_ms` | `5000` | Per-file elaboration wait for `sorry` before falling back to a text scan. |
| `cache.enabled` | `true` | Run `lake exe cache get` for Mathlib projects before starting Lean. |
| `cache.timeout_ms` | `1200000` | Cache fetch time limit (20 minutes). |
| `cache.max_output_bytes` | `8388608` | Cache output captured per stream. |
| `client.timeout_ms` | unset | Override the daemon's time limits for every query. |
| `client.transport_timeout_ms` | unset | Socket I/O limit; defaults to the query limit plus 1 second. |
| `client.startup_timeout_ms` | `1800000` | Maximum wait for a cold daemon start (30 minutes). |
| `client.startup_grace_ms` | `30000` | How long to wait if no daemon process appears. |
| `client.probe_timeout_ms` | `2000` | Health check and handshake limit. |
| `client.daemon_bin` | unset | `leanck` executable used to start daemons. |

All durations range from 1 ms to one year. The memory limits are soft: idle files are closed first, and files with pending queries are never closed.

By default the CLI and MCP server start daemons from their own executable. A custom `client.daemon_bin` is invoked as `BIN serve daemon WORKSPACE ...`, so wrappers must accept that.

## Machine-wide settings

These belong in your user file. Project files can't set them.

### Instance limit

```toml
[instances]
limit = 8               # Maximum running daemons on this machine (0 = unlimited, max 4096)
on_limit = "evict_idle" # "evict_idle", "refuse", or "warn"
```

When a new daemon would exceed the limit:

- `"evict_idle"` (default): stop the longest-idle daemon and start the new one. A daemon that has active or queued work is never stopped, so if every daemon is busy, the start fails.
- `"refuse"`: fail with an instance-limit error.
- `"warn"`: start the daemon anyway and print a warning.

```sh
leanck daemons                       # List daemons: uptime, idle time, state, active requests
leanck daemons stop --idle           # Stop every daemon with no active requests
leanck daemons stop --all
leanck daemons stop /path/to/worktree --wait 30
```

`stop` waits up to 15 seconds by default for each daemon to exit. `leanck doctor` shows the current count against the limit. Checking or listing daemons doesn't reset their idle time.

### Updates

```toml
[updates]
check = true                # Check for new releases periodically
check_interval_secs = 86400 # 1 to 31536000 (one year)
auto_install = false        # Install new releases automatically
```

Only installer-managed copies of `leanck` check for updates during ordinary commands. Copies built from source or installed by a package manager check only through `leanck doctor`. Checks are skipped for help, version, configuration commands, and server startup. Each check has a two-second time limit, and a failed check only prints a notice.

`auto_install` installs a new release before a CLI command, but only when no other `leanck` process (MCP server, daemon, or another command) is using the installation. `check = false` turns off checks, notices, and automatic installs. Running `leanck update` explicitly still works.

`leanck update --version TAG` pins a version; with a pin, `doctor` still reports newer releases but nothing installs automatically. `leanck update --version latest` removes the pin. Check state lives in `$XDG_STATE_HOME/leanck/updates`, by default `~/.local/state/leanck/updates`.

## Runtime directories

Daemon sockets, PID files, logs, and locks go in the first usable location:

1. `$XDG_RUNTIME_DIR/leanck` (the base directory must exist, belong to you, and have mode `0700`)
2. `$XDG_CACHE_HOME/leanck`, by default `~/.cache/leanck`
3. `leanck-<uid>` in the system temporary directory
4. `/tmp/leanck-<uid>`

Clients look for running daemons in all of these locations, so a daemon stays where it started even if a higher-priority directory appears later. For clients to find the same daemons, they need the same `XDG_RUNTIME_DIR`, `XDG_CACHE_HOME`, and `TMPDIR`.

Lock files remain on disk after a daemon exits; this is expected and harmless. Don't delete lock files while a daemon is running. Use `leanck daemons stop` instead.

## Upgrading from `leanckd` / `leanck-mcp`

The separate executables have been replaced by `leanck serve daemon WORKSPACE` and `leanck serve mcp`.

- Point MCP configurations at command `leanck` with arguments `["serve", "mcp"]`.
- Point `client.daemon_bin` at `leanck`.
- After upgrading, stop daemons started by the old executables before running queries. These daemons don't appear in `leanck daemons`, but `leanck daemons stop /path/to/worktree` stops them.

`leanck-normalize` is unchanged.
