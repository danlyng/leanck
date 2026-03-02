# Lean Simplify — Move Catalogue

This is the working reference for the `lean-simplify` skill. Each entry has
a **detection signal** (grep-able where possible), a **before/after**
example, and a **rule** for when the move applies.

**How to use this file:**
1. Read the proof you're simplifying end-to-end.
2. Scan this catalogue and list candidate moves that match. The
   [quick-reference table](#quick-reference-detection-table) at the bottom
   is fast triage; the numbered sections are the details.
3. Apply moves in this order: deletions → extractions → restructures.
4. Diagnose after each edit; full `lake build` only at the end.

The case studies at the top show the source proofs that surfaced each
catalogue entry — useful for calibrating "is this move worth it" by line
delta on a comparable proof.

**Guiding principle — `change`/`show` are a code smell.** Both restate the
goal declaratively rather than driving it forward, so a proof that leans on
them is narrating defeq by hand. Prefer reaching the result imperatively with
`apply`/`simp`/`rw` (and `exact`/`refine`), which transform the goal and
unify up to defeq anyway. Moves #17 and #18 are the deletion mechanics; the
broader read is that *frequent* `change`/`show` — even load-bearing ones —
flags a proof worth restructuring. Keep one only when it sets up a syntactic
`rw`/`simp_rw` match, unfolds a binder the elaborator can't see through, or
*is* the closing `rfl` step.

## Source experiments

### Case A — `optimality_iff_M` (measure-theoretic, 125-line proof body)

`Econlib/MechanismDesign/Persuasion/Moment/Differentiable.lean`. The (⟸)
direction was ~125 lines; the same file contained `optimality_iff_M_with_Sstar`
which duplicated ~30 lines of the (⟸) setup verbatim.

**Outcome.** Net `−59` lines (62 inserted, 121 deleted), plus two private
helpers (~22 lines). One linter warning fixed incidentally.

### Case B — `pooling_fails_intuitive_criterion` (Finset-arithmetic, 40-line proof body)

`Econlib/GameTheory/Signaling/IntuitiveCriterion.lean`. The proof shows that
under a pooling PBE, a profitable deviation contradicts sender optimality.
The 40-line proof body manually argued that a `FinDist` with `pmf m_pool = 1`
must have `pmf m = 0` everywhere else, via 5–6 chained `Finset.single_le_sum`
inequalities + `linarith`.

**Outcome.** Net `−23` lines (17 inserted, 40 deleted). No new helpers.

### Case C — `measurable_max_ray_length` (real-analysis, 173-line proof body)

`Econlib/Math/Analysis/MeasurableCaratheodory.lean`. Measurability of
`y ↦ sSup {t ≥ 0 | z y + t·d y ∈ F y}` via the rational density trick: define
a countable family of indicator-based functions `g : ℚ → Y → ℝ` and identify
the sSup with their iSup. The proof had:
- 12 lines of manual `•`/`+` algebra for the convex-combination identity in
  the segment lemma,
- duplicate `by_cases` blocks proving the same `g q y ≤ b` bound twice,
- 6 lines proving `g 0 y = 0` via a membership case-split when the indicator
  value is itself 0,
- manual `Real.sSup_def + dif_neg` boilerplate (×2) for `sSup` of unbounded
  sets,
- a 3-line "have-rewrite-have" chain where one `▸` would do.

**Outcome.** Net `−40` lines (37 inserted, 77 deleted). No new helpers.

### Case D — `_krAddGroupNorm_eq_zero_of_map_eq_zero'` (measure-theoretic, 287-line proof body)

`Econlib/Math/MeasureTheory/OptimalTransport/KRSignedMeasure.lean`. Shows that
the Kantorovich–Rubinstein norm on signed measures is positive-definite. The
proof bootstraps from a 1-Lipschitz envelope condition to `pos = neg` via
DCT + π-system extension. Issues:

- Manual `lipschitzWith_iff_dist_le_mul` chains (5 lines each) for `-p`,
  `p - c`, `p / K` where Mathlib has `LipschitzWith.neg` or one-liner
  `of_dist_le_mul` calls.
- Four `hint_pos`/`hint_neg`/`hint_pos_K`/`hint_neg_K` integrability helpers
  where one was unused after the integral_neg rewrites no longer needed
  explicit integrability hypotheses; another was dead code.
- 5-line `hp_const` derivation for "LipschitzWith 0 ⟹ constant" doing
  `rw [hK]; simpa; le_antisymm; dist_eq_zero.mp` separately.
- Duplicate DCT applications for `pos` and `neg` (~15 lines), then duplicate
  `integral_indicator` identification (~6 lines).
- Duplicate `hq_p`/`hq_n` integral-scaling blocks (~10 lines) when one
  measure-parameterized helper suffices.

**Outcome.** Net `−69` lines (46 inserted, 115 deleted). No new helpers.

### Case E — redundant `change` sweep across `Econlib/GameTheory` (13 files)

A targeted pass over already-simplified files removing `change` tactics that
restate the goal (or a hypothesis via `change … at h`) into a
*definitionally-equal* form that the **following** defeq-transparent tactic
(`rw`, `exact`, `apply`, `rfl`, `simp`, `congr`, or a term) would accept
anyway. In `Repeated/Basic.lean` a 40-line `change`-scaffolded HEq proof
(`jointBehavior_HEq_atHistory`) collapsed to two `simplexTransport_heq _ _`
lines once the intermediate defeq-massaging `change`s were deleted — the
elaborator unifies the term against the original goal directly.

**Outcome.** Net `−220` lines across 13 files (23 inserted, 243 deleted), the
overwhelming majority pure `change`-line deletions. No statements, signatures,
or used hypotheses changed. Roughly **1 in 3** `change` lines was removable;
the rest were load-bearing (see the caveat in move #18) and stayed.

All proofs continue to type-check; full project build (`lake build`) green;
downstream modules unaffected.

The notes below distill the moves that worked, the detection signals that
surfaced them, and the pitfalls a "simplify" skill should encode.

---

## The moves, ranked by payoff

### 1. Extract duplicated reasoning into private helpers — biggest win

Two proofs in the same file shared the same ~17-line block:

```text
support of π in s.X
    -- IsClosed s.X
    -- IsClosed (Prod.fst ⁻¹' s.X)
    -- (Prod.fst ⁻¹' s.X) ∈ ae π   via prob_compl_eq_zero_iff
    -- support_subset_of_isClosed
    -- rintro x ⟨p, hp, rfl⟩; …
```

Extracted as two small `private lemma`s above the first consumer:

```lean
private lemma feasibleJoint_ae_fst_mem_X        -- 6 lines
private lemma feasibleJoint_proj_support_subset_X  -- 9 lines, calls the first
```

Each call site became a one-liner (`hM.S_eq_support ▸ helper s hpi`).

**Detection signal.** Two `have h_X_closed : IsClosed …` blocks, or any
≥5-line `have` whose body is syntactically near-identical across two proofs.
`grep -A 15 "have h_X_closed"` or equivalent.

**When to extract vs inline.** Extract when the block appears in ≥2 places,
*or* when one place is itself going to be re-used. A single in-line use of a
5-line block is usually fine left in place — the cognitive cost of jumping to
a helper exceeds the gain.

### 2. Detect and delete dead `have`-blocks

`h_pSm_int_of` (8 lines) and its support `h_pSm_p_cont` (2 lines) were
defined but never referenced in the proof body.

**Detection signal.** For each `have h_name :=` in the proof, check
whether `h_name` appears elsewhere in the *same* declaration. The auto-search
is mechanical:

```sh
awk 'NR>=START && NR<=END' file.lean | grep -n 'h_name'
```

If the only hit is the declaration itself, it's dead. (Watch out for haves
used inside `simp only [.., h_name, ..]` or `simpa using h_name` — these
count, even though they're easy to miss visually.)

### 3. Inline single-use intermediate `have`s

Names like `h_slope`, `h_X_closed`, `h_X_meas`, `h_set_eq` that were defined
once and used once on the following line: inline the construction.

**Cost-benefit.** Saves ~2 lines per inline. Hurts readability if the inline
expression is more than ~80 chars or has tactic blocks. Cap: only inline
expressions ≤1 line wrapped.

### 4. Fold `intro … rw … exact` chains into term mode

The original `h_ae_p1_of`:
```lean
intro π' hπ'
rw [MeasureTheory.ae_iff]
have h_pre_meas : MeasurableSet … := …
have h_set_eq : … := rfl
rw [h_set_eq, MeasureTheory.prob_compl_eq_zero_iff h_pre_meas]
exact hπ'.fst_supportsOn
```

Compressed to:
```lean
rw [MeasureTheory.ae_iff]
exact (MeasureTheory.prob_compl_eq_zero_iff
  (s.X_compact.isClosed.preimage continuous_fst).measurableSet).mpr hpi.fst_supportsOn
```

Saves the explicit `h_set_eq : … = … := rfl` because the rewrite-up-to-defeq
goes through on `exact`.

**Detection signal.** A `have h_set_eq : … := rfl` immediately followed by a
`rw [h_set_eq, …]` — the rfl says the two are defeq, so the second `rw`
should accept the goal in the other shape directly.

### 5. Trim redundant `linarith` hints; reach for `calc` only when the chain is the artifact

`linarith` accepts equalities natively — passing `h.le` *and* `h.symm.le`
for the same hypothesis is a tell that the author was unsure of the hint
direction, not a real bidirectional use. Drop the redundant hints first;
that's almost always the right move on its own.

```lean
-- before: redundant bidirectional equality
linarith [h_v_le_int, h_jensen, h_marg.symm.le, h_marg.le]
-- after: equality passed once; linarith handles direction
linarith [h_v_le_int, h_jensen, h_marg]
```

This is a **hygiene move**, not a length-saving one. The line count stays
1; what changes is that the next reader doesn't wonder which direction
of `h_marg` is load-bearing.

**Conversion to `calc` is a separate, stylistic decision.** `calc`
strictly *adds* lines (the example above grows from 1 line to 4). It's
worth it only when the chain itself is the proof's narrative — when
each step is load-bearing, individually commentable, or sequentially
inspectable by a reviewer. Default: keep `linarith`. `linarith` is
strictly more powerful for arithmetic (handles rearrangement, scaling,
and equality-to-inequality automatically). Reach for `calc` only when:

- A reviewer needs to audit individual steps (e.g., the proof appears in a
  load-bearing main theorem where the chain encodes the argument).
- One step in the chain might break in a future Mathlib bump and you want
  the breakage localized.
- The intermediate values appear in user-facing documentation.

If none of those apply, the `linarith` version with trimmed hints is the
right endpoint of this move.

**Detection signal.** `linarith` invocation whose hint list contains the
same hypothesis under `.le` *and* `.symm.le` (or `.ge` & `.le`). This is
the redundant-hint smell. The actual move is to drop one of them — not
to rewrite the proof.

**Caveat.** `linarith` does *not* unfold definitional equalities or peek
through `let`/`set` bindings. If trimming `.symm.le` makes `linarith`
fail, the original author may have included both directions because one
side was a definition the elaborator couldn't see through. In that case,
restore the explicit form or use a `show`/`change` to unfold the binder
first.

### 6. Replace ad-hoc `Finset.single_le_sum` chains with structural lemmas

Case B's original argument for "FinDist with `pmf m_pool = 1` ⟹ `pmf m = 0` for
all `m ≠ m_pool`" was 15 lines of `single_le_sum` + `add_sum_erase` + `linarith`:

```lean
have hle := Finset.single_le_sum … (Finset.mem_univ m)
have hsum := (…).sum_one
have hle2 := Finset.single_le_sum … (Finset.mem_univ m_pool)
have hsplit := Finset.add_sum_erase …
have hm_in_erase : m ∈ Finset.univ.erase m_pool := …
have hle3 := Finset.single_le_sum … hm_in_erase
linarith [(…).nonneg m]
```

The clean version is 4 lines using `Finset.sum_eq_zero_iff_of_nonneg`:

```lean
have h_sum := d.sum_one
rw [← Finset.add_sum_erase _ _ (Finset.mem_univ m_pool), h_pool] at h_sum
exact (Finset.sum_eq_zero_iff_of_nonneg
  (fun i _ => d.nonneg i)).mp (by linarith)
```

Same for the concentration step `expect f = f m_pool`: instead of manually
splitting `Finset.add_sum_erase` and chaining `suffices … = 0 by linarith`,
use `Finset.sum_eq_single m_pool` with the zero-elsewhere hypothesis.

**Detection signal.** Two or more `Finset.single_le_sum` calls in a single
proof, especially followed by `linarith` over their results. This means the
proof is recreating a structural fact about sums of nonnegatives. Search
Mathlib for `sum_eq_zero_iff_of_nonneg`, `sum_eq_single`, `sum_eq_of_ne_zero`,
`Finset.sum_pos_iff_of_nonneg`, etc., before writing the chain.

**General principle.** Whenever a proof's intermediate `have`s look like
"split A into B + C, bound B above and C below, linarith" — there's usually
a one-shot Finset/Mathlib lemma that names the conclusion directly.

### 7. Compress manual `•`/`+` chains into a single `rw` line

Case C's original convex-combination identity was 10 lines:

```lean
have h_comb : z + s • d = (1 - λ) • z + λ • (z + t • d) := by
  rw [smul_add, ← add_assoc]
  have h1 : (1 - λ) • z + λ • z = z := by
    rw [← add_smul]; rw [show (1 - λ) + λ = 1 from h_sum, one_smul]
  rw [show (1 - λ) • z + λ • z + λ • (t • d) = z + λ • (t • d) from by rw [h1]]
  have hλt : λ * t = s := by rw [hλ_def]; exact div_mul_cancel₀ s ht_eq
  rw [smul_smul, hλt]
```

Same identity, two lines:

```lean
have hλt : λ * t = s := by rw [hλ_def]; exact div_mul_cancel₀ s ht_eq
have h_comb : z + s • d = (1 - λ) • z + λ • (z + t • d) := by
  rw [smul_add, ← add_assoc, ← add_smul, sub_add_cancel, one_smul, smul_smul, hλt]
```

The key move: chain the rewrites end-to-end in one `rw` call instead of
interleaving `have` blocks. Each lemma takes the expression one structural
step further; `sub_add_cancel` (`a - b + b = a`) cleans up the
`(1 - λ) + λ = 1` step without needing a separate `h_sum`.

**Detection signal.** A `have h_comb : LHS = RHS := by` whose body contains
≥3 nested `have`s with `rw [show ... from by rw [...]]` patterns. The whole
block is usually one chained `rw` once you list the lemmas in evaluation
order. Cross-check with `module` / `linear_combination` if the expressions
involve mixed `•`/`*`/`+`.

### 8. Use `csSup_of_not_bddAbove` for unbounded `sSup = 0`

Case C had two near-identical 2-line blocks:

```lean
have h_sSup_zero : sSup Gy = 0 := by
  rw [Real.sSup_def]; rw [dif_neg (fun ⟨_, hb⟩ => h_bdd hb)]
have h_iSup_zero : (⨆ q : ℚ, g q y) = 0 := by
  rw [iSup, Real.sSup_def]; exact dif_neg (fun ⟨_, hb⟩ => h_range_unbdd hb)
```

`csSup_of_not_bddAbove : ¬BddAbove s → sSup s = sSup (∅ : Set α)`
(from Mathlib) plus `Real.sSup_empty : sSup (∅ : Set ℝ) = 0` collapse this:

```lean
rw [csSup_of_not_bddAbove h_bdd, iSup, csSup_of_not_bddAbove h_range_unbdd,
  Real.sSup_empty]
```

**Detection signal.** `Real.sSup_def` + `dif_neg` in a tactic block. Indicates
the proof is unfolding the definition of `sSup` to access the
"junk-equals-zero" convention; there's a named lemma for this.

### 9. Term-mode `▸` for "have/rewrite/exact" chains

Case C's range-bound argument:

```lean
have h_q_le : (q : ℝ) ≤ B := by
  have : g q y ∈ Set.range (fun q : ℚ => g q y) := ⟨q, rfl⟩
  have := hB this; rw [h_g] at this; exact this
```

Term-mode equivalent:

```lean
have h_q_le : (q : ℝ) ≤ B := h_g ▸ hB ⟨q, rfl⟩
```

**Detection signal.** A `have := lemma this; rw [eq] at this; exact this`
trailer inside a `by` block. The `▸` operator does the same in term mode.

### 10. Drop case-splits whose outcomes coincide

Case C's `c < 0` branch proved `g 0 y = 0` via:

```lean
have h_q0_nonneg : (0 : ℝ) ≤ ((0 : ℚ) : ℝ) := by rw [Rat.cast_zero]
have h_q0_in : z y + ((0 : ℚ) : ℝ) • d y ∈ F y := by
  rw [Rat.cast_zero, zero_smul, add_zero]; exact hz_in_F y
have h_g0_val : g 0 y = ((0 : ℚ) : ℝ) := h_g_eval_in 0 y h_q0_nonneg h_q0_in
have h_g0 : g 0 y = 0 := by rw [h_g0_val, Rat.cast_zero]
```

But `g q y = (indicator s (fun _ => (q : ℝ))) y`. For `q = 0`, the indicator
*value* is `0` regardless of the case-split — both branches return `0`. So:

```lean
have h_g0 : g 0 y = 0 := by
  change {y | _}.indicator (fun _ => ((0 : ℚ) : ℝ)) y = 0
  exact Set.indicator_apply_eq_zero.mpr (fun _ => by norm_num)
```

**Detection signal.** A multi-line proof of `f x = c` where `c` is the
identity element (0, 1, ∅, etc.) and the proof case-splits on a predicate
that doesn't affect the conclusion. Check whether one of the cases trivially
collapses to the other and if so, find the unconditional lemma.

### 11. Extract duplicate `by_cases` inside the same proof

Case C had this block twice (lines 12 apart):

```lean
by_cases h_cond : (0 : ℝ) ≤ (q : ℝ) ∧ z y + (q : ℝ) • d y ∈ F y
· rw [h_g_eval_in q y h_cond.1 h_cond.2]
  exact hb_ub (q : ℝ) ⟨h_cond.1, h_cond.2⟩
· rw [h_g_eval_out q y h_cond]; exact hb_nonneg
```

Both proved `g q y ≤ b`. Hoisted as a shared `have h_g_le_b : ∀ q, g q y ≤ b`
*above* the two use-sites, both consumers shrink to one line.

**Detection signal.** Two `by_cases` blocks with the same first-line predicate
within the same proof. This is the in-proof analogue of "extract a helper
lemma" from move #1, but stays inside the proof (no new declaration).

### 12. Replace manual `lipschitzWith_iff_dist_le_mul` chains with named Mathlib lemmas

Case D's original proof of `LipschitzWith 1 (fun ω => -p ω)` was 5 lines:

```lean
have hneg_lip : LipschitzWith 1 (fun ω => -p ω) := by
  rw [lipschitzWith_iff_dist_le_mul]
  intro a b
  have h := (lipschitzWith_iff_dist_le_mul.mp hp_lip) a b
  simpa [Real.dist_eq, abs_sub_comm] using h
```

Replaced by Mathlib's `LipschitzWith.neg` — a one-liner:

```lean
hp_lip.neg
```

Same compression applies to `p - c` and `p / K` derivations. For variants
without a direct Mathlib name, `LipschitzWith.of_dist_le_mul` + a one-line
`simpa` collapses the proof to 2–3 lines.

**Detection signal.** Any `have h_lip : LipschitzWith K (fun … => …)` whose
body opens with `rw [lipschitzWith_iff_dist_le_mul]; intro a b`. Check for
named Mathlib lemmas first: `LipschitzWith.neg`, `LipschitzWith.add`,
`LipschitzWith.const_mul`, `Isometry.lipschitz.comp`. If none fit, use
`LipschitzWith.of_dist_le_mul fun a b => by simpa [...] using hp.dist_le_mul a b`.

### 13. Drop integrability hypotheses that the lemma doesn't need

Case D had:

```lean
have hpos_int : MeasureTheory.Integrable p pos := hint_pos p hp_lip
have hneg_int : MeasureTheory.Integrable p neg := hint_neg p hp_lip
rw [MeasureTheory.integral_neg, MeasureTheory.integral_neg] at hupper_neg
```

`MeasureTheory.integral_neg : ∫ x, -f x ∂μ = -∫ x, f x ∂μ` holds
unconditionally — the Bochner convention "non-integrable returns 0" makes
both sides equal 0 when `f` is not integrable, so no integrability
hypothesis is needed. The two `have`s were dead code.

**Detection signal.** A `have h_int : Integrable f μ` whose only consumer is
a rewrite by a lemma you suspect is unconditional. Check the lemma's signature
on Mathlib — `integral_neg`, `integral_const`, `integral_const_mul` all hold
without integrability. By contrast, `integral_add`, `integral_sub`,
`integral_mono_ae` *do* need integrability of each summand.

### 14. Parameterize duplicated `pos`/`neg` blocks by measure

Case D had two near-identical DCT applications and two near-identical
indicator-integration blocks, one for `pos` and one for `neg`. Each pair
collapsed to a single `∀ μ : Measure Ω, [IsFiniteMeasure μ], …` helper that
both call sites then invoked:

```lean
have h_tendsto : ∀ (μ : Measure Ω) [IsFiniteMeasure μ],
    Tendsto (fun n => ∫ ω, g n ω ∂μ) atTop (𝓝 (μ.real F)) := by
  intro μ _
  …
-- both pos and neg use h_tendsto pos / h_tendsto neg
```

When the consumed lemma takes a typeclass instance like `[IsFiniteMeasure μ]`,
the typeclass arrow must come *after* the explicit binder: `∀ (μ : …)
[IsFiniteMeasure μ], …`, not `∀ μ : …, [IsFiniteMeasure μ] → …` (the latter
fails to unify the instance metavariable).

**Detection signal.** Two `have h_*_pos` / `have h_*_neg` blocks (or any
mirror pair `_left`/`_right`, `_lo`/`_hi`) whose bodies differ only by which
of two values they take. Extract a `∀ x, …` quantifier and call it twice.

### 15. Strip `let` / `set` bindings whose `_def` equation is unused

Case D had `set g : ℚ → Y → ℝ := ... with hg_def` but `hg_def` was never
referenced. `set` without `with` (or `let` for a non-definitional binder) is
just as effective and clearer.

**Detection signal.** A `set x := … with hx_def` where `hx_def` does not
appear in the rest of the proof. Drop the `with` clause.

### 16. Watch for `← div_eq_inv_mul` in `simp_rw` chains

After `simp_rw [div_eq_inv_mul, integral_const_mul]`, the goal often has the
form `μ.real F * c⁻¹` (or similar) — and Lean's normal form for division
auto-closes it without the reverse `← div_eq_inv_mul` step that "looks
needed" to a reader.

**Detection signal.** "No goals to be solved" error on a `← X` step at the
end of a `simp_rw` chain. Drop that step; the prior rewrite already closed
the goal.

### 17. Fix `show … ; unfold/change` redundancy

```lean
show ∫ p, v p.1 ∂pi.toMeasure = momentPrimal s v   -- linter warns: changes goal
unfold momentPrimal
```

Either:
- delete the `show` (since `unfold` produces the same effect), or
- replace with `change` (per `feedback_show_vs_change`).

**Detection signal.** `linter.style.show` warning at a `show` immediately
followed by `unfold` or another goal-shape-changing tactic. The `show` is
purely cosmetic and clashes with the linter's contract.

### 18. Delete redundant `change` lines (defeq restatements)

The single highest-frequency move on already-tidied files. A `change <expr>`
(or `change <expr> at h`) that restates the goal/hypothesis into a form
**definitionally equal** to what it already was is dead scaffolding whenever
the *next* tactic is defeq-transparent. `rw`, `exact`, `apply`, `rfl`, `simp`,
`congr`, and term-mode elaboration all unify up to defeq, so they accept the
original goal shape without the intermediate restatement.

```lean
-- before (Repeated/Basic.lean): change pins a defeq form rw accepts anyway
have hm : (R.toGameTree.nodeKind h).movesAt i := hex
change (R.toGameTree.nodeKind h).iChoiceTypeAt' i = R.stage.Action i
rw [NodeKind.iChoiceTypeAt'_eq_iChoiceTypeAt _ _ hm]
change R.stage.Action (R.stageJointNode.iPosition i hm) = R.stage.Action i
exact congrArg R.stage.Action (R.stageJointNode.iPosition_player i hm)
```

```lean
-- after: the second change was defeq scaffolding; exact elaborates against the goal directly
have hm : (R.toGameTree.nodeKind h).movesAt i := hex
change (R.toGameTree.nodeKind h).iChoiceTypeAt' i = R.stage.Action i
rw [NodeKind.iChoiceTypeAt'_eq_iChoiceTypeAt _ _ hm]
exact congrArg R.stage.Action (R.stageJointNode.iPosition_player i hm)
```

The largest wins come when a long proof is *entirely* defeq narration: every
`change` is just unfolding-by-hand for the reader, and the closing
`exact`/`simpa` would have unified anyway. `jointBehavior_HEq_atHistory`
dropped from 40 lines to 2 (`simplexTransport_heq _ _` twice) this way.
`change … at h` followed by `exact`/`HEq.trans`/`eq_of_heq h` is the
hypothesis-side analogue and removes identically.

**Detection signal.** A line `change <expr>` or `change <expr> at h`
(grep `^\s*change `) immediately followed by `rw`/`exact`/`apply`/`rfl`/
`simp`/`congr`/`simpa` or term mode. The mechanical test is uniform and
fully reversible: **delete the line, re-run `leanck:diagnostics` on the
declaration; if it stays green the `change` was dead.** Because it's a pure
deletion with an immediate local check, sweep every `change` in the file
this way — keep only the ones whose deletion breaks elaboration.

**Caveat — when `change` is load-bearing (keep it).** Deleting it will fail
the diagnostics check in exactly these cases, so the test catches them, but
recognize them up front to save round-trips:

1. **It sets up a syntactic `rw`/`simp_rw` match.** The following `rw [lem]`
   needs `lem`'s LHS to appear *syntactically* in the goal; `change` massages
   the goal into that shape. `rw` matches up to syntactic pattern, **not**
   defeq, so here the `change` is required. (Distinct from `exact`, which is
   defeq-transparent — that's why `change` before `exact` is usually dead but
   `change` before `rw` often isn't.)
2. **It exposes a binder the elaborator can't see through** — unfolding a
   `let`/`set`-bound symbol or a non-reducible `def` so a later term typechecks
   (see the "set binders block term-mode unfolding" pitfall).
3. **It *is* the proof step** — a `change` to a defeq goal that is then closed
   by `rfl` (or left as the final goal) is doing the reduction itself; the
   `rfl` has nothing else to chew on.
4. **Two consecutive `change`s where the first is a stepping stone** to the
   second (the elaborator can't jump directly). Delete the *second*
   (redundant) one, not the first.

Never batch-delete all `change` lines blind — that conflates the dead ones
with cases 1–4. One deletion + one diagnostic per occurrence.

### 19. Close HEq/cast eliminator leaf lemmas with `aesop`

A helper lemma that eliminates a type equality or `HEq` — quantified over the
equality/`HEq`, then proved by `rintro … rfl …` (substituting the equality)
followed by `cases`/`subst` on the remaining `HEq`s and a closing `rfl` — is
what `aesop` closes by default. First observed in Kuhn's theorem: `h_be_subst`,
`fun_HEq`, `aux_val` in `Kuhn/Converse.lean` and the byte-identical `σ_heq`,
`fun_HEq`, `aux` in `Kuhn/Forward.lean` (the same congruence helpers are
copy-pasted across both files).

```lean
-- before (3 lines): manual dependent elimination
have fun_HEq : ∀ {T₁ T₂ : Type u} (h_T : T₁ = T₂)
    (f₁ : T₁ → ℝ) (f₂ : T₂ → ℝ) (h_f : HEq f₁ f₂)
    (c₁ : T₁) (c₂ : T₂) (h_c : HEq c₁ c₂), HEq (f₁ c₁) (f₂ c₂) := by
  rintro T₁ T₂ rfl f₁ f₂ h_f c₁ c₂ h_c
  cases h_f; cases h_c; rfl
```

```lean
-- after (0 body lines): aesop does the rintro/cases/rfl
have fun_HEq : ∀ {T₁ T₂ : Type u} (h_T : T₁ = T₂)
    (f₁ : T₁ → ℝ) (f₂ : T₂ → ℝ) (h_f : HEq f₁ f₂)
    (c₁ : T₁) (c₂ : T₂) (h_c : HEq c₁ c₂), HEq (f₁ c₁) (f₂ c₂) := by aesop
```

**Detection signal.** A `have`/`lemma` whose goal is an `HEq …` (or `_ ▸ _ = _`)
under `∀` binders that include a type equality `(h : a = b)` or an `HEq`, proved
by a body of shape `rintro … rfl …; (cases|subst) …; rfl` (grep
`rintro .* rfl`, `cases h.*;\s*rfl`, `subst .*;\s*cases`). Replace the body with
`by aesop` and re-diagnose.

**Interaction with move #1 (extract duplicates).** These congruence helpers are
often *also* duplicated across files (`fun_HEq`/`aux_val` appear verbatim in two
Kuhn files). Once each is a one-line `by aesop`, the duplication is negligible —
prefer aesop-inlining at each site over extracting a shared helper.

**Caveat.** `aesop` will not discover instance equalities on its own. When the
`rfl`-substituted binder leaves two *duplicate instance arguments* (e.g. two
`Fintype T` once `T₁ = T₂` is closed), keep the manual bridge —
`have hft : ft₁ = ft₂ := Subsingleton.elim _ _` — and let `aesop` finish; only
the trailing `subst hft; cases h_s; rfl` collapses.

**Delta note.** Per occurrence this saves 1–3 body lines; its value is recurrence
(6+ sites in one subsystem) and robustness — it replaces fragile hand-written
dependent elimination that breaks under refactors. Do **not** generalize it to
the surrounding orchestration: `aesop` closes the cast-*bookkeeping leaves*, not
the algebraic spine (telescoping / `set`+`rw`+`ring` bodies) they feed. The unit
of judgment is the leaf goal — *try* `aesop` on it rather than predicting its
reach from the enclosing proof's difficulty.

---

## Pitfalls (write these into the skill's no-go list)

### Unused hypotheses are not always dead code

Case B's signature includes `h_off`, `h_not_dom`, `h_others_dom`, `hic` — the
proof references *none* of them. The naive read says "drop them from the
signature." Don't.

The theorem is named `pooling_fails_intuitive_criterion`. The four unused
hypotheses are **documentation as types**: they pin the theorem to a specific
narrative about the intuitive criterion, even though the actual contradiction
follows from sender optimality alone. The IC story is told by the *signature*;
the proof just dispatches the core implication.

**Rule.** Hypotheses that are unused by the proof but plausibly *describe the
context* of the theorem (named with `h_*` prefix, narratively coherent with
the theorem name) are not dead. Leave them. Unused-variable warnings on such
hypotheses are pre-existing — verify with `git stash && lake env lean …`
before treating them as new.

**Counter-rule.** Hypotheses that are unused *and* whose name has no
narrative connection (random `K`, leftover `hmeas` from an earlier draft) are
dead. Confirm by checking whether the theorem still makes sense without them.
If in doubt, ask the user.

### `set` binders block term-mode unfolding

After `set g := <expr> with hg_def`, a term like `g q y` does *not* auto-unfold
to `<expr> q y` in term mode. Case C tried:

```lean
have h_g_eval_in : ∀ q y, … → g q y = (q : ℝ) :=
  fun _ _ hq h_in => Set.indicator_of_mem ⟨hq, h_in⟩ _
```

and got `Invalid ⟨...⟩ notation: The expected type of this term could not be
determined` — Lean couldn't see that the membership target was the set inside
`g`. Two fixes:

1. **Use `(s := …)` named arguments** to pin the implicit set explicitly:
   ```lean
   fun q y hq h_in =>
     Set.indicator_of_mem (s := {y | … }) ⟨hq, h_in⟩ _
   ```
2. **Switch to tactic mode with `change`** to force the unfolding:
   ```lean
   by intro q y hq h_in
      change _ ; exact Set.indicator_of_mem ⟨hq, h_in⟩ _
   ```

Detection signal: term-mode `Invalid ⟨...⟩` or `Type mismatch` referring to
the `let`-bound symbol in the expected type.

### `Real.sSup_def` + `dif_neg` is a code smell

The lookup chain `Real.sSup_def; dif_neg ⟨_, hb⟩ => …` unpacks the
"junk-equals-zero" convention manually. Mathlib already names it:
`csSup_of_not_bddAbove`. Plus `Real.sSup_empty : sSup (∅ : Set ℝ) = 0` for
the final step. If you see this pattern in a proof, the lemma is named.

### File-local identifier conventions

`Differentiable.lean` reserves the identifier `π` (matrix multiplication
notation) and uses `pi` for joints. My first draft of the helpers used `π`
as a binder name and hit `unexpected token 'π'`. The convention was stated in
the file's header docstring, but I didn't read it before naming binders.

**Mitigation.** Before introducing new binders in a refactor, grep the file
for `naming` / `reserved` / `convention` lines, or sample 2–3 existing
binders to match the pattern.

### `unusedSectionVars` on extracted helpers

A small helper whose body only touches a few of the `variable` instances
will get an `unusedSectionVars` linter warning. Two options:

1. `set_option linter.unusedSectionVars false in` before the helper. Quick,
   local, no scope churn.
2. `omit [Foo] [Bar] in` before the helper. Cleaner — actually drops the
   unneeded instances — but invasive if the list is long.

For one-off helpers, (1) is fine. For a polished library, (2).

### `csSup` / `le_csSup` placeholder failure

```lean
refine le_csSup ⟨_, ?_⟩ ⟨pi, hpi, rfl⟩
```

The first `_` is the upper-bound *value*, not its proof. Lean can't infer
the value from later context because nothing else mentions it. Concrete
value required:

```lean
refine le_csSup ⟨∫ ω, pS v S (s.m ω) ∂s.prior.toMeasure, ?_⟩ ⟨pi, hpi, rfl⟩
```

**General rule.** A `_` in a structure literal works only if some other
elaboration step pins the type and value. If the value is the *content* of
the witness, you must supply it.

### Pre-existing warnings drift into the diff

After a refactor that shrinks a file, line numbers shift. A linter warning
that was on (old) line 273 may now appear at line 239 — looking like a new
issue. Check `git blame` (or the diff) before acting.

### Stale leanck diagnostics after a large rewrite

After Case B's edit, the leanck server reported a `show` warning at a line
number that no longer existed in the file (the warning was from the pre-edit
state). The fix is `leanck restart-file <file>`. Detection signal: a diagnostic's
line number is past the file's last line, or the highlighted text on that
line doesn't match the warning description.

---

## Quick-reference detection table

| Signal | Move |
|---|---|
| Verbatim ≥5-line `have`-block in ≥2 proofs | Extract `private lemma`. |
| `have h := …` where `h` is not referenced again | Delete. |
| `intro x; rw [eq]; exact h` after `set_eq … := rfl` | Drop the rfl and fold into term mode. |
| `linarith [.le, .symm.le, …]` mixing equality both ways | Drop the redundant `.symm.le` (linarith handles equalities); `calc` only if the chain is load-bearing narrative. |
| `show G; unfold f` with `linter.style.show` warning | Delete `show` or use `change`. |
| `change <expr>` / `change … at h` before `rw`/`exact`/`apply`/`rfl`/`simp`/`congr`/term | Delete + re-diagnose; keep only if it sets up a `rw` match, exposes a binder, or *is* the `rfl` step (move #18). |
| `have x := …` where `x` is used exactly once on next line | Inline (if ≤1 wrapped line). |
| `MeasureTheory.ae_iff + prob_compl_eq_zero_iff` chain repeated | Hoist into helper / search Mathlib first. |
| Long `have h_X_closed : IsClosed s.X := s.X_compact.isClosed` repeated | Inline at every site (1 char savings each) or extract once. |
| ≥2 `Finset.single_le_sum` calls + `linarith` in same block | Search for `sum_eq_zero_iff_of_nonneg` / `sum_eq_single` / structural sum lemma. |
| Unused hypothesis with narratively-coherent name | Leave it (documentation as types). |
| `Finset.add_sum_erase _ _ (mem_univ i)` + manual `add_zero` after rewriting | Use `Finset.sum_eq_single i` directly. |
| Diagnostic's line number past file's EOF | Stale leanck state — `leanck restart-file <file>` and re-query. |
| `Real.sSup_def` + `dif_neg` in tactic block | Replace with `csSup_of_not_bddAbove` + `Real.sSup_empty`. |
| Manual `•`/`+` algebra with ≥3 nested `have`s | Try one chained `rw [smul_add, ← add_assoc, ← add_smul, …]`. |
| `have := lemma this; rw [eq] at this; exact this` | Term-mode `eq ▸ lemma <args>`. |
| Two `by_cases h : P` blocks with identical predicate inside one proof | Extract a shared `have` above both. |
| Indicator's value function is `(fun _ => 0)` or a constant equal to 0 | Skip the membership case-split; use `Set.indicator_apply_eq_zero`. |
| `Invalid ⟨...⟩` term-mode error mentioning a `set`-bound symbol | Use `(s := …)` named argument or switch to tactic mode + `change`. |
| `set ... with hg_def` where `hg_def` never appears later | Drop the `with` clause. |
| `have h_lip : LipschitzWith K _` body opens `rw [lipschitzWith_iff_dist_le_mul]` | Try `LipschitzWith.neg`/`.add`/`.const_mul` or `of_dist_le_mul fun a b => by simpa using hp.dist_le_mul a b`. |
| `have h_int : Integrable f μ` consumed only by `integral_neg`/`integral_const`/`integral_const_mul` | Drop — those lemmas are unconditional. |
| Two `have h_*_pos`/`have h_*_neg` blocks identical except for measure | Parameterize over `∀ μ, [IsFiniteMeasure μ], …` (instance binder *after* explicit). |
| "No goals" on a `← X` at end of `simp_rw` chain | Drop that step; the prior rewrite already closed it. |
| 4 integrability helpers `hint_pos`, `hint_neg`, `hint_pos_K`, `hint_neg_K` | Audit each — the K=1 case may be subsumed by the K variant, and one or both may be dead. |
| `LipschitzWith K f` with `K = 0` ⟹ `f` constant | `fun a b => dist_eq_zero.mp (le_antisymm (by simpa [hK] using hf.dist_le_mul a b) dist_nonneg)`. |
| `have`/`lemma` with `HEq`/`▸` goal proved by `rintro … rfl; cases …; rfl` | Replace body with `by aesop` (keep a `Subsingleton.elim` instance bridge if duplicate instances remain; move #19). |
