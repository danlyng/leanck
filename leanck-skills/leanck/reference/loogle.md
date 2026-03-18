# Loogle Search (`leanck loogle`)

`leanck loogle` searches Lean and Mathlib declarations by type signature and name. Unlike the web UI, it does not accept free-form positional query strings—every filter must be passed with an explicit flag (`--constant`, `--substr`, `--subexpr`, or `--conclusion`).

## Choosing the Right Filter

* **`--constant` (by referenced identifier):** Matches declarations whose type mentions the given constant anywhere in hypotheses or the conclusion (e.g., `--constant Real.sin`, `--constant Finset.sum`). Use Lean constant names rather than notation (`HAdd.hAdd` or `Nat.add` rather than `+`).
* **`--substr` (by declaration name fragment):** Case-insensitive substring match against the declaration's identifier (sent to Loogle as `"..."`). Useful when you know Mathlib naming fragments (`mono`, `inj`, `iff`, `le_of_lt`, `continuous`) but not the full name or namespace.
* **`--subexpr` (by type subexpression pattern):** Matches a structural pattern anywhere in the type signature using `_` as wildcards (e.g., `'_ * (_ ^ _)'`).
  * **Nonlinear patterns:** Use named metavariables like `?a` to require the same subexpression in multiple positions (e.g., `'f ?a = g ?a'`).
* **`--conclusion` (by return type pattern):** Matches only the final result type after all hypotheses. 

Always single-quote `--subexpr` and `--conclusion` arguments so the shell does not expand `*`, `?`, parentheses, or pipes.

## Combining Filters and Broadening Searches

* **Repeatable flags with `AND` semantics:** You can pass any flag multiple times and combine different flags in one invocation:
  ```sh
  leanck loogle --constant Filter.Tendsto --constant nhds --substr comp
  ```
  All supplied terms are joined with commas and evaluated with **AND** semantics.
* **Recovering from `0 results`:** Over-constraining a combined query is the most common cause of empty results. When a multi-term query returns nothing:
  1. Drop the most specific filter (usually `--conclusion` or `--subexpr`, which are sensitive to exact binder and coercion structure) and keep `--constant` + `--substr`.
  2. Run separate narrower searches.
  3. Check the `Suggestions:` line in the output—Loogle frequently suggests corrected constant names or alternative filters when a constant is unknown or yields no matches.

## Representation Pitfalls in Type Patterns

* **General vs. specialized types:** Loogle matches syntactic type structure up to definitional reductions it indexes, not arbitrary typeclass specializations. A lemma stated for a general algebraic or topological class (e.g., `Monoid`, `LinearOrder`, `TopologicalSpace`) will not match a `--constant Nat` filter. Prefer searching by operation and name fragment rather than concrete carrier types.
* **Competing abstractions:** Mathlib often has parallel representations (`Finite` vs. `Fintype`, `Continuous` vs. `ContinuousOn` vs. `ContinuousAt`, `Subgroup` vs. `Submodule`, `Set` vs. `Finset`). If one constant returns nothing, try its sibling or a `--substr` fragment.
* **Implicit coercions and notation:** Subexpression and conclusion patterns may fail to match if the library statement uses a coercion, bundled homomorphism (`RingHom`, `ContinuousLinearMap`), or typeclass projection where your pattern uses a bare function.

## Endpoints, Fallback, and Coverage Limits

* **Endpoint chain:** By default, `leanck loogle` tries a local Loogle server (`http://127.0.0.1:8118/loogle/json`) first and falls back to the official `https://loogle.lean-lang.org/json`. The responding endpoint is printed at the bottom of the output (`Endpoint: ...`) and recorded under `provenance` in `--json` mode.
* **When fallback happens:** Fallback only occurs on connection, timeout, HTTP status, or JSON parsing failures. A valid response from an endpoint—including `0 results` or a Loogle query error such as `unknown constant`—is returned immediately without querying later endpoints.
* **Coverage vs. local workspace:** Hosted Loogle endpoints index a single snapshot of Lean core and Mathlib. Unless specifically set up to do so, they do **not** index local project files, non-Mathlib dependencies, or version-specific differences in your project's pinned `lean-toolchain` / `lake-manifest.json`.
  * Use `rg` (or `leanck list`) in the workspace and `.lake/packages/` to search project-local declarations or verify signatures against the exact pinned dependencies.
  * Confirm candidate lemmas with `leanck diagnostics` against the live elaborator before building proof steps around them.
