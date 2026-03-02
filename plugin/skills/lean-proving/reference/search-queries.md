# Writing search queries — reference

How to get the *right* Mathlib/project declaration into the top few
`leanck:search` hits. Distilled from retrieval practice for Lean 4 premise
search.

## Look for a closing theorem first

Before you decompose a goal into steps, spend a query or two hunting for a
**high-level theorem that directly implies the conclusion**. Mathlib is full
of powerful generalizations ("a PID is a UFD", "a Noetherian domain where
every height-1 prime is principal is a UFD") that collapse a multi-step
argument into a single `exact`/`apply`. Only decompose once you're convinced
no such bridge exists — reconstructing a proof Mathlib already has is wasted
effort.

## Query in three registers

For one goal the useful declarations fall into three kinds. Search for each
explicitly rather than firing one generic query:

1. **Definitions / structures** named in the statement — so you use them with
   their exact Mathlib meaning. Phrase as "the definition of …".
2. **High-level theorems** that bridge hypotheses to conclusion in one or two
   steps (see above).
3. **Step-wise lemmas** — the specific tools for each inference, once you have
   decomposed.

## Write detailed natural-language queries

`leanck:search` is semantic + lexical, and the top hits are cross-encoder
reranked — so precise, content-bearing queries pay off. Name the object and
state its content:

- Definition: *"The definition of the norm on ℤ√d: for n = x + y·√d, the norm
  is n.re² − d·n.im²."*
- Theorem: state the name if you suspect one — *"Krull's principal ideal
  theorem: in a Noetherian ring, a prime ideal minimal over a nonzero
  principal ideal has height 1."*

Vague queries ("how do I use the norm here?") retrieve poorly. If you know the
identifier, you can also pass it directly — the index covers both prose and
names — or use `leanck:loogle` for an exact constant / subexpression / type
shape.

## Mathlib is Bourbaki-style

It holds general, reusable results, not theorems about specific constants. For
a goal about a particular object (`Zsqrtd (-5)`, "a group of order 336"),
**don't** search for a theorem about that object — search for the *general
tool* you'll instantiate (the definition of the field norm, the Sylow
theorems, a counting lemma). For goals that are really a calculation, search
for the calculation tactic/lemma, not a packaged result.

## Filter hits by type signature

Search ranks by relevance, not type-correctness; the reranker improves
precision but does **not** type-check. Before you rely on a hit, read its
**signature** and confirm it matches the objects you actually have:

- Watch close-but-wrong types: `ℕ` vs `ℤ`, `Fintype` vs `Finite`,
  `Ideal R` vs `Submodule R R`, `Polynomial R` vs `MvPolynomial σ R`.
- A hit whose *name* sounds right but whose *types* don't match is a miss —
  discard it.

Prefer keeping **nothing** over a marginally-relevant lemma. A wrong "bridge"
lemma sends the whole proof down a dead end; it is cheaper to decompose
further and search again than to build on a mismatch.

## When one query isn't enough

For surveys, side-by-side candidate comparison, or coverage maps, delegate to
`leanck-agent explore` (see [`lean-explore.md`](lean-explore.md)) instead of
firing many `leanck:search` calls inline.
