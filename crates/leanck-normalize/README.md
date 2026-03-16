# leanck-normalize

Normalize Lean source from stdin, or rewrite a file or directory in place:

```sh
leanck-normalize [OPTIONS] [PATH]
leanck-normalize --width 80 < Main.lean > Normalized.lean
leanck-normalize -w 80 Main.lean
leanck-normalize --width 80 ./MyProject
```

Omitting `PATH` reads stdin and writes the normalized source to stdout. A file
path is rewritten in place; a directory is traversed recursively, selecting
`*.lean` files unless `--all-files` is supplied. Hidden entries and symlinks in
directories are skipped. Use `--verbose` for per-file updates.

The target comment width defaults to 100 and must be greater than zero. Set it
only with `--width` or `-w`; positional widths are no longer accepted. Numeric
names such as `80` are always paths, not widths. For example, replace the old
`leanck-normalize 80 Main.lean` with `leanck-normalize --width 80 Main.lean`.
The same arguments work with `leanck normalize`. Run `--help` for transformation
flags.

## Acknowledgments

The UK/US English replacement dictionaries are provided by Marcus Deacey (https://github.com/mdeacey/english-spelling-mapper) under the license below. Some entries that are wrong or ambiguous in mathematical prose are excluded when the maps are applied and exported; the exclusion lists are in `src/spelling.rs`.
```
MIT License

Copyright (c) 2024 Marcus Deacey

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Shared runtime configuration

The executables load user/project TOML configuration and accept `--config FILE`, `--no-config`, and repeatable `--set KEY=VALUE`. See the [configuration reference](../docs/CONFIGURATION.md) for all settings, precedence, and lifecycle behavior. Use `leanck config show --json` to inspect effective settings and `leanck config check` to validate them.

## History and style policies

Missing attribution headers on named Lean files are derived from file-specific Git author history, following renames and mailmaps. Existing headers retain their author, year, and license text by default. `normalize.rules.frontmatter.invalid_existing` selects `"warn"` (default), `"ignore"`, or `"replace"` for template mismatches. Replacement uses the template and author history, retaining the old header with a warning if history is unavailable. Untracked files, shallow histories, stdin, and unavailable history never receive a guessed identity or license. Set the new-header license text explicitly for your project.

Structured policies under `normalize.rules` configure frontmatter ordering and templates, heading text/case, attribute placement and per-name overrides, custom dictionaries, import groups, comment delimiters, whitespace, and local exception directives. `--export-dictionary american` or `--export-dictionary british` prints an editable built-in dictionary. Repeat `--dictionary FILE` for custom JSON maps. See the shared configuration reference for all policies and examples.

`normalize.rules.frontmatter.order` may list any subset of frontmatter components. Omitted components stay in place as barriers; `[]` preserves all positions. Use this instead of the removed relocation settings and flags. Header formatting and import-content policies remain independent of component order.

The pure `normalize_with_context` API accepts explicit history and compiled dictionary resources. Schema-only builds retain no configuration loader or command adapter.

Project configuration may be named `leanck.toml` or `.leanck.toml`; `leanck.toml` wins when both exist in the same directory. The `normalize.rules.frontmatter.author_map` table maps exact, case-sensitive Git names after mailmap resolution to display strings in generated or explicitly replaced headers. Exclusions apply first; unmapped names and retained existing headers stay unchanged. For example, `author_map = { "git-handle" = "Full Name" }` inside `[normalize.rules.frontmatter]` changes both the author list and copyright attribution. The pure API applies the same mapping to supplied history.

Normalization returns nonfatal diagnostics in `NormalizeResult.warnings`; command entry points print warnings on stderr. Config inspection exposes warnings that do not depend on source content. URL linkification works independently of comment wrapping: without reflow it only inserts `[url](url)` around bare URLs and leaves the rest of the comment text unchanged.

A comment rewrite is kept only when it reads as the same Markdown and keeps the Lean comment delimiters, and only once rewriting it again changes nothing; otherwise the comment is left byte-identical. A panic while normalizing a file in directory mode is reported as an error for that file, which is left unchanged, and the remaining files are still processed. Files are rewritten through a temporary sibling and a rename, keeping permissions and following symbolic links. A file with other hard links, a file whose owner or group a new file would not get, or a file in a directory that cannot be written is overwritten in place instead; extended attributes and access control lists are not carried over by a rename.
