---
name: leanck
description: "Reference for using leanck in Lean 4 projects: live diagnostics, proof goals, declaration search, builds, and source normalization."
---

# leanck

`leanck` is a CLI for Lean 4 projects that provides live elaboration feedback through a background Lean server. `leanck` provides feedback faster and more token-efficiently than the equivalent `lake` tools. It uses the project's toolchain and Lake. Live queries start the workspace daemon automatically, so the first query can take longer while the project warms up; be patient!

## Quickstart

- Run commands from the Lake project directory. 
- Check an entire file for errors: `leanck diagnostics relative/path/to/File.lean`
- Check a specific declaration: `leanck diagnostics File.lean --ident my_theorem`
- Inspect the goal at a 0-indexed (line, column) position: `leanck goal Main.lean 10 4`
- Find `sorry`s in a file or directory: `leanck sorry Module/`

Pass file arguments are parsed relative to the workspace. Command-specific options are available through `leanck <command> --help`.

| Command | Purpose |
|---|---|
| `leanck doctor` | Check workspace and daemon status without starting Lean. |
| `leanck diagnostics Main.lean` | Wait for elaboration and report file errors. |
| `leanck diagnostics Main.lean --ident my_theorem` | Restrict diagnostics to one declaration. |
| `leanck goal Main.lean 10 4` | Inspect the proof goal at a position. Lines and columns are zero-indexed; columns count UTF-16 code units. |
| `leanck sorry .` | Find unfinished proofs in a file or directory. Directory scans are text-only. |
| `leanck list Main.lean --kind theorem,def` | List declarations, optionally filtered by kind. |
| `leanck restart-file Main.lean` | Restart elaboration for one file. |
| `leanck build` | Run `lake build` with compact output; optional positional arguments select build targets. |
| `leanck loogle --constant Nat.add --conclusion '_ <= _'` | Search declarations through Loogle. Terms combine with AND semantics; coverage depends on the responding endpoint. |
| `leanck normalize Main.lean` | Rewrite comments, imports, attributes, and frontmatter in place. `--dry-run` reports needed changes without writing. |

## Command information

Additional reference information is available in `reference/`:

- [Loogle search (`leanck loogle`)](reference/loogle.md): query filters, pattern syntax, `AND` semantics, representation pitfalls, and endpoint coverage.
- [Source normalization (`leanck normalize`)](reference/normalize.md): frontmatter ordering, Git-derived headers, comment reflow/spelling guards, local `-- leanck:` directives, and `--set` overrides.

Diagnostics default to errors only and at most five results. `--severity error,warning` includes warnings; `--all-severities` includes every severity. `--limit` and `--offset` paginate diagnostics. Builds accept `--warnings` to show warnings and `--strict` to treat warnings as failures.

Loogle also accepts `--substr` for name fragments and `--subexpr` for expression patterns. Quote patterns to protect them from shell expansion. `--limit N` changes the default ten results.

Live queries, builds, and Loogle support `--json` for structured output. Live queries and builds accept `--timeout SECS`. `leanck daemons` lists background daemons; `leanck daemons stop --idle` stops idle instances.
