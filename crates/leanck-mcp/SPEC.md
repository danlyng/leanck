# Problem Statement
Writing Lean4 proofs with agentic coding tools requires a tight feedback loop between the agent and the Lean proof state. IDEs expose this proof feedback with sub-ms reactivity by communicating changes to the language server as they occur and exposing the results incrementally—thus one can use the lean infoview to inspect the proof state at a given line (such as variables and hypotheses in scope and the current goals).

## vLSP sketch
We propose a virtual language server client that reacts to file changes that agents can query to get diagnostics.

Interface:
- By Claude Code hook, structured patches are asynchronously provided to the virtual client on `Edit` tool use (see @patch.rs) - the virtual client sends `textDocument/didChange` with the diff
    - Note: Because this is vulnerable to desync by changes outside of the Edit tool, on every diagnostic request the LSP/virtual client compares the hash of the file on disc to the file in LSP memory. If the hashes do not match, then send the entire file contents as a `didChange` (this is faster than `didClose, didOpen`)
- The LSP streams diagnostics to the virtual client (this is a lean LSP extension; see below).
- The agent queries the virtual client for diagnostics at a specified identifier (declaration name, filename, or module name). The vClient either responds with the diagnostic or **blocks** until it is available.

## Symbol resolution
The `/diagnostics` endpoint takes:

```
GET /diagnostics
  ?ident=<name|path|module>
  &file=<path or file:// URI>   # optional — scopes declaration lookup to this file
  &namespace=<dotted prefix>    # optional — prepended to unqualified idents
  &timeout_ms=<ms>              # optional — wait budget for elaboration
```

`ident` is resolved in this order:

1. **Path/module**. If `ident` ends in `.lean`, contains `/`, or starts with `.`, it's treated as a path. Capitalised dotted names (`Foo.Bar`) are tried as module paths (`Foo/Bar.lean`). A hit returns the whole file with no range filter.
2. **Declaration name**. If `file` is supplied, the resolver opens that file and queries `textDocument/documentSymbol` plus an out-of-band Lean parser (`scripts/parse_decls.lean` run via `lake env lean --run`) for that file only. Without `file`, it walks every `.lean` under the project root and does the same per file until it finds a match.
3. **Namespace context**. If `namespace` is set and `ident` is unqualified (no `.` and no `_root_.` prefix), the resolver tries `<namespace>.<ident>` before falling back to the bare name. Suffix-matching on dotted full-names also lets bare `bar` resolve to `Foo.bar`.

### Why two sources for symbol info

The LSP's `documentSymbol` reflects Lean's elaborator: it knows namespaces, syntax extensions when elaboration succeeds, and gives the most accurate ranges for working code. It can miss declarations when elaboration of the body fails (e.g., Mathlib's `lemma` syntax extension drops out under errors). The parser fallback runs Lean's parser directly with no elaboration, so it still sees the declaration head even when the body is broken; it also tracks `namespace`/`end` blocks to emit fully-qualified `fullName`s.

### Why ranges are extended past the header

Both LSP `documentSymbol` and the raw parser frequently report ranges that stop at the declaration *header* (e.g., right after `:= by`), especially for tactic-block bodies. Filtering diagnostics by the header range alone drops every error and `unsolved goals` notification inside the proof body. The resolver therefore extends each symbol's range to the line just before the next top-level declaration (or end-of-file), matching the behavior of `lean-tool`'s `_with_decl_body_range`.

### Response shape

```jsonc
{
  "uri": "file:///.../Basic.lean",
  "range": { "start": {"line":0,"character":0}, "end": {"line":8,"character":4294967295} },
  "symbol": "Foo.hello_world",   // omitted when ident resolved to a whole file/module
  "version": 1,
  "diagnostics": [ /* lsp_types::Diagnostic */ ]
}
```

`range.end.character` is `u32::MAX` to indicate "to the end of that line" — it's used only for client-side overlap filtering, never sent back to the server.

## Caveats
The lean LSP intentionally violates the spec in some ways.
### All files must be explicitly opened
What the client must do is open every file it asks about. The protocol overview is explicit (src/Lean/Server/ProtocolOverview.lean:60-65): "The Lean language server requires all files for which it receives requests and notifications to be opened by a textDocument/didOpen notification first… It will discard notifications and error requests for closed files. This is in violation with LSP."

### Diagnostics are incrementally streamed
There is a Lean-specific extension to textDocument/publishDiagnostics that lets the server stream diagnostics out as the file is elaborated instead of waiting until the whole file is done.

Protocol side — the params get one extra field (src/Lean/Data/Lsp/Diagnostics.lean:159-171):

{ "uri": "...", "version": N,
  "isIncremental": true,      // Lean-specific; absent or false = standard LSP semantics
  "diagnostics": [ ... ] }

Semantics (also stated in the protocol overview, src/Lean/Server/ProtocolOverview.lean:157):
- isIncremental: false (or absent) → standard LSP: the new array replaces what the client has for that document version.
- isIncremental: true → the new array should be appended to the diagnostics the client already has for that same document version.

Clients have to opt in. The server only emits isIncremental at all if the client set LeanClientCapabilities.incrementalDiagnosticSupport in its initialize params. Otherwise the field is omitted and the server falls back to non-incremental sends (src/Lean/Server/FileWorker/Utils.lean:130-159).
