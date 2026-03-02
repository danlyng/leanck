# Finding existing Lean declarations

Look for a theorem that closes the goal before decomposing the proof. Search
for definitions, high-level results, and individual proof steps separately.

## Loogle

Use structured inputs to `leanck:loogle`:

- `constant`: declarations occurring in the type, such as `Real.sin`.
- `substr`: fragments of declaration names, such as `continuous`.
- `subexpr`: expressions with holes, such as `_ * (_ ^ _)`.
- `conclusion`: result-type patterns; the tool adds `|-` when omitted.

The CLI equivalents are `leanck loogle --constant Real.sin`, `--substr`,
`--subexpr`, and `--conclusion`. Multiple terms are combined with AND semantics;
try separate searches when a combined query returns nothing.

## Source search

Use `rg` in project source and `.lake/packages/mathlib/Mathlib` to locate
relevant namespaces, declarations, and examples of their use. Start with a
known identifier or mathematical term, then inspect neighboring declarations.
Read the full signature and surrounding assumptions rather than trusting a name.

## Candidate selection

Check types (`Finite` versus `Fintype`), hypotheses, quantifiers, and conclusion
direction. Prefer a general theorem whose specialization solves the goal.
Verify the application with Lean diagnostics before building further proof
steps on it. HTTP search coverage depends on the responding Loogle server;
local source and the elaborator are the reference for the project's pinned version.
