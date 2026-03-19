# Configuration

`leanck` (including `serve mcp` and `serve daemon`) and `leanck-normalize` share versioned TOML configuration through the `leanck-config` crate. If no config file is supplied, the default config is used. You can inspect the [complete default template](leanck.example.toml) by running `leanck config defaults`.

## Getting started

Run `leanck config init` in a terminal for guided setup. First choose settings for this project or user defaults for all projects, then select the topics you want to customize. The wizard covers formatting and spelling, default license text and header format, diagnostic severities and page size, build warnings and timeouts, Loogle endpoints and results, daemon concurrency and idle shutdown, and Mathlib cache fetching. User setup also offers the machine-wide daemon limit, behavior at that limit, and release checks and automatic updates. Use arrow keys to navigate, Space to toggle topics, and Enter to accept a value. Up and Down select options. Previous and Next buttons move between setting screens; Tab and Shift-Tab cycle focus between the setting and these buttons. Press Enter or Space to activate a focused button. Enter on a setting also advances. Going back preserves entered values and selections, including across sections. From the first setting, Previous returns to topic selection; from topic selection, Previous returns to the scope choice when available. Next validates text before advancing. The first screen disables Previous. The review screen has a Confirm button instead of Next, and saving requires explicit confirmation. Ctrl-A selects all items in a multi-select menu; Ctrl-D clears those selections. Esc asks whether to discard the draft, with Keep editing selected by default. Ctrl-C exits without saving.

When the terminal has enough width and height, a pane on the right shows examples for the current setting, including before-and-after formatting samples. The wizard switches to a single column in smaller windows and adapts when you resize the terminal. Examples use subdued explanations and manually highlighted Lean code. `NO_COLOR` disables these colors. Text fields show the current value: Enter keeps it, typing or pasting replaces it, and Left/Right, Ctrl-B/F, or Home/End allows editing in place. Arrow keys in a focused field only move its cursor. While a navigation button is focused, Left and Right move focus between the buttons without activating them. Ctrl-U clears a field and Ctrl-W deletes a word. License text and header format fields use `\n` for line breaks and `\\` for a literal backslash. Header templates support the same placeholders as the configuration file and are validated before advancing. Menus use arrows and letter shortcuts without a filter. Use Page Up and Page Down to scroll a long review before saving.

The wizard shows the destination and existing values, including inherited user defaults for a project. Unchanged settings remain omitted so they continue to inherit. Review changes before selecting Save, or choose Edit choices to revisit them. Existing files retain their comments and unrelated settings. Saving replaces the file atomically; a file changed by another process during setup must be reopened. Running daemons need to be restarted for daemon configuration changes to take effect.

Project setup uses `leanck.toml` in the current directory, reusing `.leanck.toml` if that is the only existing project file there. Use `leanck config init .leanck.toml` to choose a hidden file explicitly, or specify another destination with `leanck config init /path/to/config.toml`. `--workspace DIR` selects the directory for the default project destination. `leanck config init --user` goes directly to user setup at `$XDG_CONFIG_HOME/leanck/config.toml`, or `~/.config/leanck/config.toml` by default, and creates its parent directories when saved. User setup reads the user file without importing the current project's settings.

With `--no-interactive`, `--json`, redirected input or stderr, or `TERM=dumb`, initialization writes a minimal starter file and refuses to overwrite an existing file. The starter sets only `version = 1` and includes commented examples for normalization width, spelling, header format, and license text, plus a reference to the full example configuration. Uncomment settings to override them; omitted settings inherit from user configuration and built-in defaults. `--json` prints a structured creation result and never prompts. Redirecting stdout alone still allows prompts on stderr. `NO_COLOR` disables prompt colors. Use `leanck config defaults` to print the complete defaults.

A small project configuration might contain:

```toml
version = 1

[normalize]
width = 100
header = false
spelling = "off"
exclude = ["Generated/**", "Vendor/**"]

[loogle]
endpoints = ["https://loogle.lean-lang.org/json"]
timeout_ms = 5000
limit = 20

[build]
strict = true

[diagnostics]
limit = 10
severity = ["error", "warning"]
```

You can inspect and validate your config:

```sh
leanck config show # Prints the active config
leanck config sources # Shows the sources of the active config
leanck config check # Validates the active config
leanck config show --workspace /path/to/project
leanck --config /path/to/extra.toml config check
```

`show` prints effective TOML.

## Discovery and precedence

Settings are resolved in this order:

1. Built-in defaults.
2. User configuration at `$XDG_CONFIG_HOME/leanck/config.toml`, falling back to `$HOME/.config/leanck/config.toml`.
3. The nearest `leanck.toml` or `.leanck.toml` starting at the selected workspace or input directory. Discovery stops at the first Lake project or Git root; if both filenames exist in the same directory, only `leanck.toml` is loaded.
4. The file specified in `LEANCK_CONFIG`.
5. The file specified in `--config FILE`.
6. Repeatable `--set KEY=VALUE` overrides, in command-line order.

The machine-wide `[instances]` and `[updates]` tables are ignored in a discovered `leanck.toml` or `.leanck.toml`, including any invalid values or unknown keys within that section. Other project settings are still loaded and validated. Doctor and MCP health checks report a nonfatal warning identifying the project file and user configuration path; routine commands remain quiet. Machine-wide settings may come from any other layer, including either project filename passed explicitly with `--config` or `LEANCK_CONFIG`.

## Updates

```toml
[updates]
check = true
check_interval_secs = 86400
automatic = false
```

`check` enables foreground release checks and notices. `check_interval_secs` accepts 1 through 31536000 seconds (one year). `automatic` opts installer-managed releases into installation before eligible CLI operations when the installation is unused. `check = false` disables opportunistic checks, notices, and automatic installation; an explicit `leanck update --check` or `leanck update` is still a direct request.

CLI doctor and MCP doctor/health retain available-version notices between checks and include structured `updates` status in JSON. Doctor remains non-spawning and never installs. Ordinary managed CLI invocations check when due; help, version, completion generation, configuration commands, daemon startup, and MCP startup are excluded. Source/package-manager copies do not check during ordinary operations. Both kinds can check through doctor. Release-check failures are informational and preserve earlier available-version notices.

Check state defaults to `$XDG_STATE_HOME/leanck/updates`, or `~/.local/state/leanck/updates`. Each release source has a separate directory containing a `last-check` timestamp, `check.json` release metadata and diagnostics, and a stable `check.lock`. Attempts are stamped before HTTP, so failures are throttled too. Metadata and timestamps are atomically replaced. Opportunistic checks have a two-second total deadline. Unwritable state is reported rather than causing repeated unthrottled requests. Missing, corrupt, and future timestamps are handled conservatively.

Version pins and archive mirrors belong to the installation receipt. A pin permits doctor to announce a newer version while preventing automatic installation. `leanck update --version latest` explicitly unpins; `--version TAG` records an exact pin. Stable checks exclude semantic prereleases and use GitHub's latest published release. Archive and checksum downloads use one exact release tag. HTTPS is required for sources, with HTTP allowed only on loopback for local testing. Checksums provide integrity against the published release, not an independent publisher signature.

Each managed CLI operation, MCP server, and daemon retains a shared installation lease. Install, native update, and uninstall share an exclusive maintenance lock. Automatic updates defer while any client holds a lease; they do not evict or interrupt work. Installation bookkeeping uses the installer's state directory, including custom directories recorded in the executable's marker. Never unlink held maintenance locks. See the README's Updates section for preview, adoption, recovery, and host-registration commands.

Tables merge by field. Arrays replace earlier arrays, including empty arrays. Unknown keys, unsupported schema versions, malformed TOML, invalid types, invalid URLs/globs, and invalid resource limits are errors.

Paths in the configuration files are relative to its parent directory. All other paths are relative to the process's current directory.

All executables accept `--config`, `--no-config`, and `--set`. The flags can appear before or after subcommands. `--no-config` ignores all configuration files, including `LEANCK_CONFIG`, while still accepting `--set` and ordinary command flags. It conflicts with `--config`. Ordinary runtime environment behavior such as `PATH`, logging filters, and XDG runtime/cache directories remains available. The `LEANCK_CONFIG` selector remains supported; `--config FILE` takes precedence over it. Setting-specific environment overrides (`LOOGLE_URL`, `LEANCKD_BIN`, and `LEANCKD_NO_CACHE_GET`) are no longer read. Use `loogle.endpoints`, `client.daemon_bin`, and `cache.enabled` in TOML or with `--set` instead.

`--set` values use TOML syntax; strings need TOML quotes inside the shell argument:

```sh
leanck --set 'normalize.spelling="british"' normalize Main.lean
leanck normalize --set 'normalize.header=false' Main.lean
leanck build --set 'build.strict=false'
leanck loogle --set 'loogle.endpoints=[]' --constant Nat.add
leanck --no-config --set 'cache.enabled=false' diagnostics Main.lean
```

## Runtime directories

Runtime socket, PID, log, and lock filenames use a 12-character lowercase hexadecimal worktree key, generated directly with a 6-byte (48-bit) BLAKE2b digest of the canonical workspace path.

New daemons prefer `$XDG_RUNTIME_DIR/leanck`, then `$XDG_CACHE_HOME/leanck` (default `$HOME/.cache/leanck`), then a private `leanck-<uid>` directory under the system temporary directory, and finally `/tmp/leanck-<uid>` if the configured temporary path is unusable. Relative XDG values are ignored. The XDG runtime base must already exist, belong to the current user, and have mode `0700`; leanck creates its own subdirectory, not the login session's runtime base.

Before starting Lean, leanck validates the complete socket pathname against the platform's byte limit, directory ownership and permissions, file creation, exclusive locking, and Unix socket binding. New directories use mode `0700`. Invalid candidates are skipped; if none work, the error lists the attempted locations and reasons. A client passes the selected directory explicitly to the daemon and uses that directory for its retry.

Discovery checks existing daemons across candidate locations, including the former direct system-temporary location. A live daemon keeps its location even when a higher-priority directory becomes available. Lifetime locks remain in place and record the selected directory, allowing clients to find a daemon through an older coordination location. Do not delete held lock files. Read-only commands do not create runtime directories or start daemons.

Slot inventory, admission, and idle eviction cover all available candidate directories. Admission locks are acquired in canonical order before counting held slots across directories, so a fallback does not create a separate instance budget. New temporary-fallback daemons always hold a slot. As with other environment-based paths, clients must share the relevant XDG and temporary-directory environment to discover the same inventory.

## Nonfatal warnings

`config show` and `config check` will report configuration warnings. Warnings cover combinations of options that may interact in surprising ways, like specifying customized heading rules while heading normalization are disabled.

## Normalization

| Key in `[normalize]` | Default | Meaning |
|---|---|---|
| `width` | `100` | Positive comment-reflow width; overridden by `--width`. |
| `frontmatter` | `true` | Enable frontmatter transformations. |
| `header` | `true` | Normalize an existing header, or create a missing header from file history. |
| `sort_imports` | `true` | Enable import ordering, grouping, and deduplication policies. |
| `attributes` | `true` | Apply configured declaration attribute placement. |
| `headings` | `false` | Convert banners and format Markdown heading titles. |
| `comments` | `true` | Enable block-comment transformations. |
| `comment_reflow` | `true` | Reflow comment text to `width`. When `false`, preserve paragraph line breaks; delimiter layout and other comment rules still apply. |
| `linkify_urls` | `false` | Convert bare comment URLs to Markdown links. Without `comment_reflow`, only the link syntax is inserted. |
| `spelling` | `"american"` | `"american"`, `"british"`, or `"off"`. |
| `capitalize_after_colon` | `false` | Capitalize words after a colon in comments. |
| `collapse_blank_lines` | `true` | Collapse consecutive blank lines. |
| `exclude` | `[]` | Recursive traversal exclusion globs, relative to the input directory. |

`exclude` uses globset syntax, including `*`, `?`, and `**`; use `Generated/**` to skip a subtree. Matching a directory excludes its entire subtree. Hidden entries and symlinks are always skipped during recursive traversal. An explicitly named file is processed even if it matches an exclusion.

`--dry-run` performs the same transformations and reports files that would change, without writing. It exits nonzero if changes are needed or processing fails. With stdin, normal operation writes transformed source; `--dry-run` emits no source and tests whether the input is already normalized. Normalization retains its text/source output regardless of `output.json`.

The pure `leanck_normalize::normalize` API remains independent of files, environment, and clock. The command adapter's `run` uses built-in rules, and `run_with_settings` accepts an explicitly resolved `Normalize` configuration. Executable entry points load configuration and resolve width before calling the adapter. Library-only builds with default features disabled do not pull in the configuration loader or command dependencies.

### Attribution headers and frontmatter

Header normalization adds absent headers and formats existing headers. A non-docstring block comment that opens the file, regardless of its content, is treated as a header; a plain block comment anywhere else in the frontmatter is a reordering barrier and is never moved. When a header exists, by default only its delimiters and the whitespace next to them are formatted, and the indentation of its inner lines is kept. Set `normalize.rules.frontmatter.normalize_existing = false` to preserve the existing header's internal formatting too.

By default, existing headers that do not match `header_template` emit a `header_template_mismatch` warning with the source line, including in `--dry-run` mode. To disable the warning, set `invalid_existing = "ignore"`; to replace nonmatching headers, set `invalid_existing = "replace"`.

The check compares structure and fixed text (including configured `license_text`), treats layout whitespace as insignificant, and accepts existing names and dates without comparing them to Git history.

When a header is generated, the command adapter reads that file's Git author history. It follows renames by default and honors Git mailmaps. Common AI agent authors are ignored by default; set `normalize.rules.frontmatter.ignore_authors` to exclude authors from newly generated headers (see below). **Note that the default generated header does not include a license notice.** Configure `license_text` to include project-specific license text in newly created headers.

`normalize.rules.frontmatter.ignore_authors` excludes authors from newly generated headers and their year ranges. Entries are exact names, email addresses, or `Name <email>` signatures, matched ASCII case-insensitively after trimming surrounding whitespace; there are no wildcards or substring matches. Both the raw Git identity and the mailmapped identity are checked. The defaults exclude known Claude Code, Codex, Cursor, GitHub Copilot, Aider, Gemini Code Assist, and Devin identities; the commented example config lists every email. Set `ignore_authors = []` to include everyone, or replace the array with your own identities. Retained existing headers keep their authors; explicitly replaced headers use the filtered history. The pure API applies the same exclusions to explicitly supplied file history.

Attribution uses Git author fields, not committers or `Co-authored-by` trailers.

`normalize.rules.frontmatter.author_map` maps exact, case-sensitive Git author names to display strings in generated or explicitly replaced headers. Matching uses names after Git mailmap resolution and author exclusions. Each name is mapped once; unmapped names stay unchanged. Both `{authors}` and `{copyright}` use the mapped strings, deduplicated and sorted, while year ranges retain all included contributions. Retained existing headers are unchanged. Keys and values must be nonempty single-line strings without control characters; values cannot contain Lean comment delimiters. The pure API applies the same mapping to supplied history.

```toml
[normalize.rules.frontmatter.author_map]
"git-handle" = "Full Name"
"Old Git Name" = "Full Name"
```

The map defaults to empty and merges by name across configuration files. Use `--set 'normalize.rules.frontmatter.author_map={}'` to clear inherited mappings, or supply a complete inline TOML table through `--set` to replace them.

Default identities are based on the tools' public signatures and bot accounts, including [Codex's signature](https://github.com/openai/codex/blob/main/codex-rs/git-utils/src/baseline.rs), [Aider's attribution implementation](https://github.com/Aider-AI/aider/blob/main/aider/repo.py), and GitHub's public account IDs for [Claude](https://api.github.com/users/claude%5Bbot%5D), [Cursor](https://api.github.com/users/cursor%5Bbot%5D), [Copilot](https://api.github.com/users/copilot%5Bbot%5D), [Gemini Code Assist](https://api.github.com/users/gemini-code-assist%5Bbot%5D), and [Devin](https://api.github.com/users/devin-ai-integration%5Bbot%5D). Email matching covers model-specific display names without excluding similarly named humans.

Git history is read only, with one shared five-second budget for repository inspection and the file log, and a 4 MiB capture limit. `history_timeout_ms` changes the budget up to 60 seconds; `follow_renames = false` limits history to the current path. Untracked files, shallow histories, unavailable Git, failed or timed-out history reads, and histories with every author excluded leave the header absent and emit a note on stderr. No history is fetched. When reading from stdin, git integration is disabled. Directory runs print a note that repeats for several files once, with the number of files; `--verbose` lists each file. `--dry-run` uses the same history and reports a required header without writing it.

```toml
[normalize.rules.frontmatter]
order = ["header", "module", "imports", "docstring", "section", "namespace"]
blank_lines = 1
normalize_existing = true
invalid_existing = "warn" # replace, warn, or ignore
history_timeout_ms = 5000
follow_renames = true
license_text = "Project-specific license text."
header_template = "/-\n{copyright}\n{license}\nAuthors: {authors}\n-/"
# Optional: synthesize a missing module docstring using the input file's stem.
module_doc_template = "/-! # About {module} -/"
```

Header placeholders are `{copyright}` (one line, for example `Copyright (c) 2020–2026 Alice, Bob.`), `{authors}`, `{years}` (overall year range), `{first_year}`, `{last_year}`, and `{license}`. Empty template lines disappear. Module-doc templates accept `{module}`, the input filename without its extension. Templates must contain one outer Lean comment, with no nested comment delimiters; unknown placeholders are errors. License text and replacements cannot inject comment delimiters. There is no static copyright-year setting: generated dates come from history, and existing dates are retained unless the header is explicitly replaced.

`order` lists the components to reorder, with no duplicates. Omitted components retain their position relative to all other blocks and act as barriers: included components reorder only within the runs between them. For example, `order = ["header", "module", "imports", "docstring"]` pins any existing section and namespace commands. `order = []` preserves every component's position while still allowing header formatting and import-content policies. Omitted components that are absent from the source create no barrier. This replaces the former `section_relocation` / `namespace_relocation` settings and `--no-section-relocation` / `--no-namespace-relocation` flags.

When included, the header must be first; the module declaration must precede imports and scopes, and imports must precede namespace/section scopes when both are included. Only recognized initial frontmatter is reordered: a body declaration or other unrecognized content is a barrier. Missing module, import, section, and namespace commands are ignored. `blank_lines` sets the spacing between components; note that the global blank-line cap overrides this setting. 

### Headings

```toml
[normalize]
headings = true

[normalize.rules.headings]
level = 2
case = "title" # off, title, or sentence
small_words = ["a", "an", "and", "of", "the", "to"]
template = "/-! {hashes} {title} -/"

[normalize.rules.headings.replacements]
"Old heading" = "Preferred heading"
```

Enabling headings converts recognized banners headings like
```
/-
============
My heading
============
-/
```
and formats existing Markdown heading titles inside comments. `level` (1–6) and `template` control converted banner headings; existing heading levels are preserved. `{hashes}` is the configured number of `#` characters. Title casing preserves existing capitalization and uses `small_words` for interior words. Exact title replacements run before case formatting. Delimited code and math inside headings are protected from prose transformations.

### Dictionaries

`leanck` includes two built-in directional dictionaries converting from American to British English and vice versa. Users can export a copy with `leanck normalize --export-dictionary american` or `leanck-normalize --export-dictionary british`. Export writes a plain JSON word-to-replacement object to stdout. The source attribution and license are recorded in the normalizer README. The bundled maps are applied minus documented exclusion lists in `crates/leanck-normalize/src/spelling.rs`: entries that are wrong or ambiguous in mathematical prose, such as `gauge`, `analogue`, and `cancellation` in American mode or `program`, `meter`, and `check` in British mode, are skipped together with their inflections. `--export-dictionary` prints the filtered map that is applied, and `normalize.rules.spelling.replacements` can restore an excluded entry.

```toml
[normalize.rules.spelling]
use_builtin = true
files = ["dictionaries/project.json", "dictionaries/overrides.json"]
protected_words = ["colour", "MyProjectTerm"]
case_sensitive = false
inflections = true

[normalize.rules.spelling.replacements]
"specialise" = "specialize"
```

Dictionary files are JSON objects such as `{"from":"to"}`.

Precedence is built-in map, files in order, inline replacements, then protected words. `use_builtin = false` replaces the built-in vocabulary entirely. Case-insensitive matching preserves the matched word's case; case-sensitive matching uses exact replacement text. Inflection handling can be disabled independently. Ambiguous case-insensitive keys and invalid entries are rejected. Replacement chains resolve during dictionary compilation; cycles are errors. When spelling is enabled, all dictionary files are loaded and compiled before any source file is rewritten. Disabling spelling or comment transformations avoids dictionary file access.

Spelling and capitalization apply only to Markdown prose. Code spans, fenced and indented code, math, HTML, and link destinations are protected; link labels remain prose. Bare URL text is protected too. Words written like Lean names are never corrected, including by custom dictionaries: dotted names (`Colour.red`, `.colour`), primed names (`colour'`), subscripted names (`colour₁`), and words with internal capitals (`colourIn`). Sentence punctuation, possessives, quotes, and all-capital words do not protect a word. The `normalize.spelling = "off"` master setting disables spelling substitutions, including custom maps.

### Attributes, imports, and comment delimiters

```toml
[normalize.rules.attributes]
placement = "same_line" # above or off
above_when_long = false

[normalize.rules.attributes.overrides]
ext = "above"
simp = "same_line"

[normalize.rules.imports]
order = "alphabetical" # or off
public_first = true
preserve_groups = true
deduplicate = true

[normalize.rules.comments]
module = "separate"
declaration = "compact"
section = "compact" # each accepts compact, separate, or off
```

Attribute placement applies to all recognized declaration attributes. Overrides use bare attribute names. `above_when_long` places attributes above a declaration when the combined line exceeds the configured width; a doc comment on the declaration line, and the code after it, do not count toward the width, since comment reflow wraps the comment separately and moves that code to its last line; a plain `/- -/` comment counts like code. Attribute order is preserved. Mixed placements that would require reordering attributes, groups containing conflicting per-name policies, multiline groups, and intervening comments are left intact; a comment anywhere on the attribute lines counts, including a doc comment inside a group such as `@[to_additive /-- doc -/]`. Moved attributes take the declaration's indentation: they join its line or sit above it at its column. `off` leaves placement untouched unless a per-name override selects another placement. The `normalize.attributes` master switch disables all placement rules, including overrides.

Imports are recognized only in Lean's module header, after an optional `module` or `prelude` command; an import after the first other command is left alone. Every `[public] [meta] import [all] Name` form sorts and deduplicates as an import. Import sorting respects blank-line groups by default and never sorts across comments or lines holding a comment. Public imports sort first within each group when requested, and `meta` imports sort before other imports within the public and the ordinary imports. When blank lines are collapsed to `max_blank_lines = 0`, the groups merge, so they are sorted as one group. Deduplication removes duplicate import lines within a sortable group, ignoring differences in whitespace; it preserves distinctions between public and ordinary imports. `order = "off"` skips sorting but still permits configured grouping and deduplication; the `sort_imports` switch disables all these transformations.

Delimiter styles independently control module docs, declaration docs, and section comments. Standalone Markdown heading comments (including level one) are classified as section comments; a level-one module title followed by prose is interpreted as a module doc. `off` retains the original opening/closing delimiter placement while allowing enabled prose transformations. Compact layout still separates delimiters from Markdown table rows when required to retain table structure, and code fences, display-math fences, and table rows never share a line with a delimiter in any layout. Compact layout moves the first line onto the opening-delimiter line only when that keeps the Markdown structure, for example not for a list item with nested children.

Comment bodies are read as Lean reads them, with Lean's own removal of leading spaces (`String.removeLeadingSpaces`): only the common indentation of the later lines is removed, so a doc comment whose only content is indented is a paragraph, not code. Continuation lines of a comment that opens mid-line, such as `@[simp, to_additive /-- doc -/]` or `library_note "x" /-- doc -/`, use the indentation of the line holding the opener. A doc comment whose layout Lean would read as different Markdown than its rewrite, or whose rewrite would change the comment delimiters Lean sees, is left unchanged, as is any comment the Markdown parser cannot read. A doc comment followed by `#guard_msgs`, with only whitespace and line comments such as directives between them, is that command's expected output and is never modified. Inline math (`$...$`) counts its own width when wrapping, and display math (`$$`) keeps its lines; an unclosed `$$` leaves the comment unchanged.

### Whitespace and local exceptions

```toml
[normalize.rules]
directives = true

[normalize.rules.whitespace]
max_blank_lines = 1
line_endings = "lf" # off retains the input convention; crlf converts to CRLF
final_newline = "ensure" # off leaves it unchanged; remove strips it
remove_bom = true # false retains a leading UTF-8 byte-order mark
```

Blank-line counts range from 0 to 254. Blank lines inside code fences, display math, and Lean literals are protected. Inside doc comments one blank line always remains between Markdown blocks, even with `max_blank_lines = 0`, and the code, math, and HTML that Lean reads there are left alone; lines inside doc comments that hold only spaces or tabs are not emptied. Only spaces and tabs make a line blank. Blank lines are collapsed before comments are rewritten, and when collapsing is enabled, blank lines at the end of the file are removed. By default, active regions use LF line endings, the file ends with a newline, and a leading UTF-8 byte-order mark is removed. Set `remove_bom = false` to retain the mark; marks elsewhere in the source are never removed. The `collapse_blank_lines` switch disables blank-line collapsing only. Local whitespace-disable directives protect their regions; a first-line `-- leanck: off whitespace` (or `off` for all rules) also protects a leading BOM, even if whitespace processing resumes later.

Standalone Lean line comments can delimit local exceptions:

```lean
-- leanck: off
-- This region is kept verbatim.
-- leanck: on

-- leanck: off spelling,attributes
-- Only the named rules are disabled here.
-- leanck: on spelling,attributes

-- leanck: off attributes,headings
-- Attribute placement and heading formatting are disabled.
-- leanck: on headings
-- Headings resume; attributes remain disabled.
-- leanck: off spelling
-- Spelling and attributes are now disabled.
-- leanck: on
-- All rules resume their configured behavior.
```

Names are `frontmatter`, `headings`, `attributes`, `comments`, `spelling`, `comment_reflow`, `capitalize_after_colon`, and `whitespace`. Omitting names or including `all` selects all rules. Directives update a set, not a nesting stack: repeated `off` is idempotent. Named `off` commands accumulate; named `on` commands restore only the selected rules, even after a nameless `off`. For example, `off`, then `on spelling,attributes`, enables spelling and attribute placement while leaving headings, comment layout/reflow, capitalization, and whitespace changes disabled. `on` without names resets all local exceptions. The `comments` directive groups comment layout, spelling, reflow, and capitalization; a later named `on spelling` can restore just spelling without changing comment layout. Directives inside strings or block comments are ignored. Unknown rule names are errors. Directives form structural barriers, so frontmatter is processed only in the initial segment before the first directive. Enabled segments use the configured line-ending policy; fully disabled regions retain their bytes. Final-newline policy applies only when whitespace remains enabled at the end of the file.

## Loogle

| Key in `[loogle]` | Default | Meaning |
|---|---|---|
| `endpoints` | local, main | Ordered HTTP(S) JSON endpoint URLs; see the complete template for exact URLs. |
| `timeout_ms` | `15000` | Request budget for each endpoint, including the response body. |
| `limit` | `10` | Maximum hits returned; zero is permitted. |

The list is the complete fallback chain, so a local-only list never falls back to an external endpoint. `[]` disables searches with a “no endpoints configured” error. Empty queries are rejected. URLs must be absolute HTTP(S) URLs without credentials, query strings, or fragments. Duplicate URLs are deduplicated when creating the search client. Results retain endpoint provenance; custom file/flag URLs use the `configured` source tag.

## Builds, diagnostics, and output

| Section/key | Default | Meaning |
|---|---|---|
| `build.warnings` | `false` | Include Lake warnings. |
| `build.strict` | `false` | Fail on recognized warnings; implies reporting them. |
| `build.targets` | `[]` | Default Lake build targets. Explicit CLI targets replace them. |
| `build.timeout_ms` | `1800000` | Total build budget, including lock acquisition. |
| `build.max_output_bytes` | `8388608` | Positive per-stream capture limit (8 MiB). |
| `diagnostics.limit` | `5` | Default diagnostic page size; zero is permitted. |
| `diagnostics.severity` | `["error"]` | Selection from `error`, `warning`, `information`, `hint`; `[]` selects none. |
| `output.json` | `false` | JSON by default for CLI build/live-query/doctor/Loogle commands and MCP build/health/diagnostics tools. |

Builds use the project's Lake toolchain and the existing shared execution lock. CLI `--timeout` is in seconds; configuration and MCP timeouts are in milliseconds. `--warnings`, `--strict`, `--max-output-bytes`, `--limit`, `--severity`, `--all-severities`, and `--json` override applicable defaults. For all configured severities, list the four names, or use CLI `--all-severities` / MCP `{"mode":"all"}`. 

## Client, daemon, and cache

| Section/key | Default | Meaning |
|---|---|---|
| `client.daemon_bin` | unset | Explicit `leanck` executable to invoke with `serve daemon`; otherwise use the host-provided executable, then sibling/`PATH` discovery. |
| `client.timeout_ms` | unset | Default explicit live-query budget; like request `timeout_ms`, also overrides per-stage wait caps. |
| `client.transport_timeout_ms` | unset | Independent socket I/O budget; normally operation budget plus 1 second, or probe budget for control operations. |
| `client.startup_timeout_ms` | `1800000` | Absolute cold-start wait cap. |
| `client.startup_grace_ms` | `30000` | Grace window while no live daemon PID is visible. |
| `client.probe_timeout_ms` | `2000` | Health/control and compatibility handshake budget. |
| `daemon.idle_timeout_ms` | `3600000` | Idle shutdown threshold; `0` disables idle shutdown. |
| `daemon.max_inflight` | `8` | Maximum concurrent elaboration-blocking requests, from 1 through 65536. |
| `daemon.max_documents` | `128` | Maximum open documents kept in memory before evicting idle ones. |
| `daemon.max_source_bytes` | `67108864` | Maximum total source bytes kept in memory (64 MiB). |
| `daemon.request_timeout_ms` | `300000` | Default whole-operation budget when the request has no explicit timeout. |
| `daemon.elaboration_timeout_ms` | `120000` | Default elaboration wait cap. |
| `daemon.lsp_timeout_ms` | `30000` | Default query LSP cap, including declaration discovery and goal enrichment. |
| `daemon.permit_timeout_ms` | `30000` | Admission wait before reporting overload, bounded by the request deadline. |
| `daemon.sorry_timeout_ms` | `5000` | Per-file sorry elaboration gate before text fallback, including directory checks. |
| `cache.enabled` | `true` | Attempt Mathlib cache fetching before Lean startup. |
| `cache.timeout_ms` | `1200000` | Cache-fetch budget, including execution-lock waiting. |
| `cache.max_output_bytes` | `8388608` | Positive per-stream cache output capture limit. |
| `instances.limit` | `8` | Maximum running `leanckd` daemons on this machine, at most 4096; `0` disables the limit. |
| `instances.on_limit` | `"evict_idle"` | At the limit: `"evict_idle"` stops the longest-idle daemon and then starts, `"refuse"` fails, `"warn"` starts anyway with a warning. |

Durations must be between 1 millisecond and one year (31,536,000,000 ms), except `daemon.idle_timeout_ms`, which also accepts `0`. `daemon.max_documents` and `daemon.max_source_bytes` are soft limits: when the daemon exceeds either threshold, it closes and evicts least-recently-used idle documents, but it never evicts documents with active or queued queries.

Set `client.daemon_bin` to select an explicit `leanck` executable. The CLI and MCP server otherwise launch their own executable; embedded library callers check for a sibling `leanck` and then PATH. Every selected executable is invoked with `serve daemon WORKSPACE`; custom wrappers must accept that prefix. Set `cache.enabled=false` to skip cache fetching. Directly launched `leanck serve daemon WORKSPACE` loads settings for that workspace. Auto-start passes resolved daemon/cache settings to the child with configuration-file discovery disabled, preventing it from resolving a different configuration from the parent's working directory.

Daemon settings apply at startup. An existing daemon keeps its settings until shut down.

### Instance limit

`[instances]` is machine-wide: every daemon started by any worktree counts against one limit. Each daemon holds an exclusive lock on a numbered `leanck-slot-N.lock` file in its selected runtime directory for its whole life; admission counts held slots across the candidate directories described above. The file records its pid, canonical worktree, socket, and start time; the lock is released when the process exits, including on a crash, so stale files are harmless. A daemon refused by the limit exits with status 75. Daemons do not stop each other: under `evict_idle`, the client that is starting a daemon stops the longest-idle ready daemons with no active requests, as many as one more daemon needs, and retries. Clients starting the same worktree at once take turns, so only one of them makes room. It asks that daemon to stop only if it is still idle, and the daemon decides atomically with accepting work, so a daemon that has accepted a request since the listing keeps running and the client tries the next candidate. Listing or probing a daemon does not reset its idle time. When every daemon is busy, or under `refuse`, the client reports an instance-limit error. Set the limit in the user configuration, an explicit `--config`/`LEANCK_CONFIG` file, or `--set`; auto-start forwards it to the daemon. Older daemons that never acquired a slot cannot be counted retroactively; stop and restart them after upgrading. Every newly started daemon, including a temporary fallback, participates in accounting.

Inspect and free slots with `leanck daemons`:

```sh
leanck daemons # List running daemons with uptime, idle time, state, and active requests
leanck daemons stop --idle # Stop every ready daemon with no active requests; one that accepts work first keeps running
leanck daemons stop --all # Stop every daemon that is serving its socket
leanck daemons stop /path/to/worktree --wait 30
```

`stop` asks each daemon to shut down and waits (15 seconds by default) for it to exit, so its slot is free when the command returns. Both subcommands accept `--json`. `leanck doctor` reports the count against the limit and warns when it is reached. The CLI, MCP server, and daemon ship in one executable. After upgrading it, explicitly stop older running daemons before issuing queries; wire-version checks still apply. Auto-started daemons use `leanckd` as their command-line name (`argv[0]`); the OS executable name can remain `leanck`. Eviction and listing use slot records, process start identities, and sockets, never process names.

## Unified executable migration

The former `leanck-mcp` and `leanckd` executables are replaced by `leanck serve mcp` and `leanck serve daemon WORKSPACE`. Update MCP configurations to use command `leanck` and arguments `["serve", "mcp"]`. Update `client.daemon_bin` to point to the unified executable, and update custom daemon wrappers to accept `serve daemon` before the workspace argument. No legacy executable launchers are installed. The standalone `leanck-normalize` executable is unchanged. Existing cache and temporary runtime locations remain discoverable. Slot records and the wire protocol are unchanged; stop older daemons explicitly rather than deleting their locks.
