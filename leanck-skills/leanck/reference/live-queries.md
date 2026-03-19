# Live Elaboration Queries (`diagnostics`, `goal`, `sorry`, `list`, `restart-file`)

Live query commands talk to the workspace's background `leanck serve daemon` (which manages a single warm `lake serve` process per worktree). Editing a file on disk automatically syncs the updated text to the daemon.

## `leanck goal`: Positions and UTF-16 Columns

* **0-indexed lines and columns:** Both `<LINE>` and `<CHARACTER>` are zero-based (line 0 is the first line of the file; character 0 is the first column). Note that diagnostic output and `sorry` note lines are displayed 1-based for humans, so subtract 1 when copying a line number into `leanck goal`.
* **Columns count UTF-16 code units (LSP standard):**
  * ASCII characters count as `1`.
  * Most Lean mathematical symbols in the Basic Multilingual Plane (`∀`, `∃`, `→`, `↔`, `≤`, `≥`, `≠`, `∈`, `⊆`, `∪`, `∩`, `⟨`, `⟩`, `⊢`, `λ`, `α`, `β`, `ℝ`, `ℕ`, `ℤ`, `ℚ`, `ℂ`) are 3 bytes in UTF-8, **1 Unicode scalar value**, and **1 UTF-16 code unit**. Counting UTF-8 bytes on a line containing these symbols will overshoot the column.
  * Characters outside the BMP—such as mathematical alphanumeric symbols (`𝔸`, `𝔽`, `𝕏`, `𝒜`, `\mathcal`, non-BMP blackboard bold)—are **2 UTF-16 code units** (a surrogate pair).
* **Before vs. after a tactic:**
  * Placing `<CHARACTER>` at the start of a tactic keyword inspects the goal state **before** that tactic executes.
  * Placing `<CHARACTER>` at the end of the tactic line (or at the indentation column of the next tactic) inspects the residual goal state **after** the tactic executes.

## `leanck diagnostics`: Scope, Severities, and Notes

* **Declaration scoping (`--ident`):**
  * `--ident <NAME>` resolves the declaration (accepting either the short name or a namespace-qualified name; ambiguous short names require qualification) and filters diagnostics to that declaration's source range.
  * **What `--ident` hides:** Errors outside the target declaration's range—such as broken `import`s, `open`/`variable` errors, or earlier broken helper lemmas—are excluded from the filtered output. Run an unscoped `leanck diagnostics <FILE>` to verify the whole file is error-free.
* **Severities and pagination:**
  * Defaults to `--severity error` and `--limit 5`.
  * Even when filtering to errors only, `diagnostics` attaches a `notes` entry (`file contains sorry at line ...` or `declaration contains sorry at line ...`) whenever an in-scope `sorry` is present, and enriches each error diagnostic with the conclusions of open goals at the error's start position.
  * To inspect linter warnings, unused variables, or raw `declaration uses sorry` warnings, pass `--severity error,warning` or `--all-severities` (`--severity` accepts `error`, `warning`, `information`, `hint`; `--all-severities` cannot be combined with `--severity`).
  * When `total` exceeds the returned page (`limit`, default 5), use `--offset` and `--limit` to page through remaining diagnostics.

## `leanck sorry`: Elaborated vs. Fast Lexical Scans

* **Default mode (elaborated + lexical):** For both single files and directories, `leanck sorry <PATH>` elaborates `.lean` files through the daemon (waiting up to `daemon.sorry_timeout_ms`, 5s per file by default, before falling back to lexical results for that file) and combines two signals:
  1. **Declaration diagnostics (`declaration` kind):** Lean's `"declaration uses 'sorry'"` warning, which catches `sorry`s generated inside macros or tactics that never spell `sorry` in source text. Because Lean suppresses the `declaration uses sorry` warning on declarations that fail with other errors, `leanck sorry` adds a note whenever error diagnostics are present in the file.
  2. **Source tokens (`tactic` kind):** Lexical occurrences of `sorry`, `admit`, `stop`, and `sorryAx` outside comments and string literals.
* **Fast mode (`--fast`):** Runs the lexical scanner directly in the client without starting or querying the daemon. The client also falls back to this scan automatically if daemon startup or the query times out.
* **Red-flag notes:** Both modes always report quality notes for constructs in the file that can bypass or silence `sorry` checks:
  * `axiom` declarations
  * `warn.sorry` option changes (which can silence Lean's sorry warning)
  * `#guard_msgs` (which absorbs warnings on the following command)
  * Native evaluation (`native_decide`, `+native`, `Lean.ofReduceBool`, `Lean.ofReduceNat`, ` Lean.trustCompiler`)
  * `debug.skipKernelTC`
* **Directory traversal:** Recursive directory scans automatically skip `.git`, `.lake`, `build`, `target`, and `node_modules`.

## `leanck list`: Declaration Kinds and Fallbacks

* `leanck list <FILE>` combines LSP document symbols with a lexical Lean declaration scanner so Lean-specific kinds (`theorem`, `lemma`, `def`, `abbrev`, `instance`, `structure`, `class`, `inductive`, `opaque`, `axiom`) are distinguished rather than collapsed into coarse LSP `method`/`function`/`constant` kinds.
* `--kind` accepts comma-separated or repeated `snake_case` selectors (e.g., `--kind theorem,lemma,def`). Multiple kinds combine with **OR** semantics; omitting `--kind` lists all declarations.
* If the file is still elaborating or LSP symbols time out, `list` and `--ident` lookup fall back to lexical declaration discovery and surface a note indicating that ranges are approximate.

## Cross-File Edits and `leanck restart-file`

You should only rarely need to explicitly `leanck restart-file`. `leanck` automatically reloads dependencies when changes are detected. If a manual restart is necessary, remember to restart the file reporting the stale imports: if `Bar.lean` imports `Foo.lean`, restart `Bar.lean`.

* **Same-file edits:** Saving changes to `Foo.lean` and running `leanck diagnostics Foo.lean` or `leanck goal Foo.lean ...` automatically sends the new file contents to the daemon and waits for re-elaboration. You do not need `restart-file` for ordinary single-file iteration.
* **Upstream dependency edits:** Live queries also check imported source files, including transitive imports, for modification times newer than the queried file's last synchronization. When stale dependencies are detected, leanck closes and reopens the queried file with dependency building enabled. If project source roots could not be resolved, it reopens conservatively and reports a quality note. Ordinary cross-file iteration therefore usually needs no manual restart.
* **When to force a restart:** Use `leanck restart-file <FILE>` if Lean still reports `Imports are out of date`, or if dependency changes escaped the source freshness check (for example, replaced build artifacts or source changes with preserved timestamps).
* **Suspected stale or stuck elaboration:** A restart forces the file to close and reopen within the existing Lean server and can help recover its elaboration state. A tactic or query timeout alone does not imply that a restart is needed or that restarting will fix it.
* **Completion and concurrency:** Restart waits for active queries on the file to release their document leases. A successful return confirms that the file was reopened with dependency building enabled; it does not wait for elaboration to finish. Run `leanck diagnostics <FILE>` afterward to wait for results.
