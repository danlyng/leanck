# Source Normalization (`leanck normalize`)

`leanck normalize` standardizes Lean module frontmatter, imports, declaration attribute placement, block/doc comments, and whitespace. It is **not** a general Lean code formatter: it does not reformat tactic blocks, term expressions, or declaration signatures outside of attributes and comments.

When working in a project with collaborators, encourage the user to check-in normalization rules in a local `leanck.toml` file. Otherwise, different user defaults can repeatedly clobber diffs.

## Invocation Gotchas

* **In-place vs. stdio:** Passing a file rewrites it atomically in place. Passing a directory recursively normalizes `*.lean` files (skipping hidden entries, symlinks, and configured `exclude` globs). Omitting `PATH` reads from stdin and writes to stdout.
* **Explicit files bypass exclusions:** While directory traversal honors `normalize.exclude` (and `--exclude`), passing a file path directly on the command line always normalizes that file even if it matches an exclusion glob.
* **`--dry-run` for checks:** Runs the full normalizer without writing files and exits nonzero if any file (or stdin) needs changes.
* **Output format:** `leanck normalize` always outputs source text (on stdin) or per-file status summaries (on paths), even if `output.json = true` is set in configuration.

## Frontmatter, Imports, and Git-Derived Headers

For files recognized as Lean modules (containing `module`, `prelude`, `import`, `@[expose] ... section`, or a leading header comment), `leanck normalize` processes the initial frontmatter:

* **Component ordering and barriers:** By default, initial frontmatter is ordered as `header`, `module`, `imports`, `docstring`, `section`, `namespace`.
  * Reordering stops at the first declaration, unrecognized command, local `-- leanck:` directive, or non-header `/- ... -/` block comment. Any of these acts as a **barrier**—frontmatter items below a barrier are not moved above it.
  * Components omitted from `normalize.rules.frontmatter.order` stay in place and act as barriers between the remaining components (`order = []` preserves all component positions while still formatting headers and sorting imports).
* **Import sorting and grouping:** Imports in the module header are alphabetized and deduplicated (`[public] [meta] import [all] Name`).
  * Blank lines between import blocks are preserved as separate groups by default (`preserve_groups = true`), unless `max_blank_lines = 0` collapses blank lines first.
  * Comments on or between import lines act as barriers: imports are never sorted across a comment.
  * Within each sortable group, `public` imports sort before ordinary imports, and `meta` imports sort first within each visibility class.
* **Attribution headers from Git history:** Any non-docstring block comment (`/- ... -/`) at the top of a file is treated as its attribution header.
  * **Existing headers:** Preserved by default (only delimiter layout and surrounding spacing are normalized). If an existing header does not match the configured `header_template`, `leanck normalize` keeps it and prints a `header_template_mismatch` warning on stderr unless `normalize.rules.frontmatter.invalid_existing` is set to `"ignore"` or `"replace"`.
  * **Missing headers:** Synthesized from `git log` for that file (following renames and `.mailmap`). Known AI agent identities (Claude, Codex, Cursor, Copilot, Aider, Gemini, Devin) are excluded by default (`ignore_authors`), and Git author names can be mapped via `normalize.rules.frontmatter.author_map`.
  * **Why a missing header stays absent:** `leanck normalize` never guesses attribution. If a file is untracked in Git, has shallow/unavailable history, has only excluded authors, or is piped via stdin, the header is left absent with a note on stderr. Pass `--no-header` when you do not want header synthesis or notes.
  * **License text:** The default header template includes `{license}`, which is empty unless `normalize.rules.frontmatter.license_text` is configured for the project (empty template lines are omitted automatically).

## Comments, Reflow, and Spelling

`leanck normalize` processes `/-- ... -/` declaration docstrings and `/-! ... -/` module/section doc comments (ordinary `/- ... -/` implementation comments are not reflowed as doc comments):

* **Delimiter styles:** By default, module docstrings (`/-! ... -/` with prose) place opening and closing delimiters on `separate` lines, while declaration docstrings (`/-- ... -/`) and standalone section heading comments use `compact` single-line delimiters when the Markdown structure allows it. Code fences, display math (`$$`), table rows, and nested lists are never collapsed onto a delimiter line.
* **Idempotence and `#guard_msgs` safety:** A comment rewrite is applied only if Lean would parse the resulting comment into the exact same Markdown AST and delimiters, and re-running normalization produces no further changes. Unclosed `$$` display math or malformed Markdown leaves the comment byte-for-byte unchanged. Any doc comment immediately before `#guard_msgs` (with only whitespace or line comments in between) is treated as expected test output and is never modified.
* **Spelling normalization:** Defaults to British-to-American (`--to-british` flips to American-to-British; `--no-spelling` disables it).
  * Only Markdown prose is touched. Code spans, fenced/indented code, `$math$` / `$$display math$$`, HTML, URLs, and link destinations are untouched.
  * Words shaped like Lean identifiers—dotted names (`Colour.red`, `.colour`), primed names (`colour'`), subscripted names (`colour₁`), and camelCase/PascalCase words (`colourMap`)—are never modified, even by custom dictionaries.
  * Built-in dictionaries exclude words that are false friends in mathematical writing (such as `gauge`, `analogue`, and `cancellation` in American mode, or `program`, `meter`, and `check` in British mode). Use `leanck normalize --export-dictionary american` to inspect the active built-in map.
* **Opt-in comment passes:** Banner comment conversion and Markdown heading casing (`--headings`) and bare URL linkification (`--linkify-urls`) are off by default unless enabled by flag or config.

## Attribute Placement

* By default (`placement = "same_line"`), `@[...]` attributes are placed on the same line as the declaration (`@[simp] theorem ...`).
* Attribute order is never changed.
* Multiline attribute groups, attributes with comments on or between their lines (including `@[to_additive /-- doc -/]`), and groups whose attributes have conflicting per-name overrides in `normalize.rules.attributes.overrides` are left untouched.

## In-File Directives (`-- leanck: off` / `on`)

Standalone line comments can disable and re-enable normalization rules for sections of a file:

```lean
-- leanck: off
-- Everything here is preserved verbatim.
-- leanck: on

-- leanck: off spelling,attributes
@[simp]
/-- This colour is preserved. -/
theorem foo : True := trivial
-- leanck: on spelling,attributes
```

* **Available rule names:** `frontmatter`, `headings`, `attributes`, `comments`, `spelling`, `comment_reflow`, `capitalize_after_colon`, `whitespace` (or `all` / omitted for all rules).
* **Set semantics, not a stack:** Directives modify the active set of disabled rules rather than nesting.
  * `-- leanck: off` disables all rules.
  * Following `-- leanck: off` with `-- leanck: on spelling` re-enables *only* `spelling`, leaving comment reflow, attributes, and whitespace disabled.
  * `-- leanck: on` (with no rule names) clears all local exceptions and restores the file's configured rules.
  * `comments` is a group shorthand for `comments`, `spelling`, `comment_reflow`, and `capitalize_after_colon`; you can disable `comments` and then selectively `-- leanck: on spelling` to run spelling checks without reflowing comment layout.

## One-Off Rule Tweaks with `--set`

Many fine-grained policies live under `[normalize.rules.*]` in `leanck.toml` rather than dedicated CLI flags. Override them for a single run with `--set KEY=VALUE` (using TOML syntax for values):

```sh
# Place attributes above declarations when the line exceeds the target width
leanck normalize --set 'normalize.rules.attributes.above_when_long=true' Main.lean

# Silence template mismatch warnings on existing custom file headers
leanck normalize --set 'normalize.rules.frontmatter.invalid_existing="ignore"' Main.lean

# Preserve existing section/namespace positions while still sorting imports
leanck normalize --set 'normalize.rules.frontmatter.order=["header","module","imports","docstring"]' Main.lean
```
