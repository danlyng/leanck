---
name: "lean-simplify"
description: "Aggressively simplify and restructure a Lean 4 proof without changing its statement or used hypotheses. Use when the user asks to shorten, clean up, refactor, or de-duplicate an existing proof. Not for original proof writing — use lean-proving for that."
allowed-tools: mcp__leanck__* mcp__plugin_leanck_leanck__*
---

# Lean Simplify

This skill tightens an existing, type-checking Lean 4 proof. The
constraints are non-negotiable:

1. **The theorem statement does not change.** Same name, same binders, same
   conclusion, same instance arguments.
2. **Hypotheses that the proof actually uses do not change.** Renaming,
   reordering, or weakening a used hypothesis is out of scope. (Hypotheses
   that are unused *by the proof* may sometimes be removed — see
   [Unused hypotheses](#unused-hypotheses) below for the test.)
3. **The proof must continue to type-check at every step.** No
   intermediate broken state.

The work product is a smaller, flatter, more idiomatic proof body. The
input/output contract from the rest of the codebase's perspective is
unchanged.

## Prerequisites

Before starting, confirm the workspace is operational:

* `leanck:health` returns `{ok: true}`. If not, the Lean server daemon
  couldn't start — it auto-starts on first use, so this usually means the
  project doesn't build. Stop and confirm the project is a valid Lake project
  (`lake build` succeeds).
* The target file builds **clean** (no errors). Run
  `leanck diagnostics <file> --severity error` (errors are also the default).
  If there are errors, refuse — this is
  the wrong skill. The user wants `lean-proving` to *fix* the proof first.
* Capture the pre-existing warning count so post-edit drift can be
  distinguished from new issues. Either `lake env lean <file> 2>&1 | grep -c
  warning` or `git stash && lake env lean <file> 2>&1 | grep -c warning && git
  stash pop` if you've already started editing.

`--severity` accepts `error`, `warning`, `information`, and `hint`, with
comma-separated values or repeated flags (for example, `--severity error
--severity warning`). Use `--all-severities` for all severities, not the old
`--severity all`; it cannot be combined with `--severity`.

## Scoping the work

Before any edit, ask the user one targeted scope question if their request
left it open:

> **In-place only**, **extract obvious duplicates** (recommended for proofs
> with mirrored blocks across multiple theorems in the same file), or
> **aggressive helper extraction** (pull every multi-line `have`-block into
> its own lemma)?

Default to "extract obvious duplicates" — it's the highest-leverage move
across the case studies, but only when the same reasoning appears in ≥2
places. A single inline 5-line block is usually best left in place.

## Workflow

### 1. Read the proof end-to-end first

Don't start editing. Read the whole proof body once and list candidate moves
using the catalogue in `reference/moves-catalog.md`. Each candidate should
be a tuple of `(detection signal, move, expected line delta)`. If the list
is empty, tell the user the proof is already tight and stop.

### 2. Plan move order

- Apply moves that **delete** before moves that **restructure**: dead
  `have`-blocks, unused hypotheses, redundant `show`/`change` lines first.
  The redundant-`change` sweep (catalogue move #18) is usually the
  single highest-yield deletion pass on an already-tidied file — `change`
  lines that restate the goal to a defeq form the next tactic accepts
  anyway. Delete each, re-diagnose, keep only the load-bearing ones.
- Apply **extract-helper** moves before tightening their consumers: the
  consumers depend on the helpers existing.
- Apply **named-Mathlib-lemma swaps** in bulk where possible (a single edit
  can replace 4 manual Lipschitz derivations).

Use `TaskCreate` for ≥3 moves. Marking each `in_progress`/`completed` keeps
the user informed and prevents drift.

### 3. Edit one move at a time, diagnose after each

After each edit, run
`leanck diagnostics <file> --ident <declaration> --severity error,warning`.
Compare against the pre-edit warning count. If the count went up, locate the new warning and either:
- Fix it (e.g., `show` → `change`, `simp_rw` cleanup), or
- Confirm it's actually pre-existing (line numbers shift after deletions —
  use `git stash && lake env lean <file>` as ground truth).

If diagnostics report a stale state (e.g., a warning at a line past the
current EOF, or `Cannot process request to closed file`), call
`leanck restart-file <file>` and re-query.

### 4. Bulk verify

Once all moves are applied:

1. `leanck diagnostics <file> --severity error,warning` (no `--ident`
   filter). Count must equal the pre-edit baseline (modulo
   warnings you've intentionally fixed, e.g. `linter.style.show`).
2. `lake build <module>` for the touched module. Must be green.
3. `lake build` for the full project. **This step is essential.** Helper
   extractions and namespace-level edits can shadow downstream consumers in
   ways the per-module build doesn't catch.

### 5. Report

End with a short summary:
- One sentence on what changed (e.g., "Extracted `feasibleJoint_ae_fst_mem_X`
  helper, consolidated K-Lipschitz integrability with the 1-Lipschitz case,
  replaced manual Lipschitz derivations with `LipschitzWith.neg`/`.dist_le_mul`").
- `git diff --stat <file>`: insertion/deletion counts and net line delta.
- Per-move bullet list (this can be brief — one line each).

## Move catalogue

The full catalogue lives in [`reference/moves-catalog.md`](reference/moves-catalog.md).
Read it before planning moves. It has 19 entries indexed by detection signal,
each with a before/after example, and a quick-reference table at the bottom.

The move families:

- **Extract duplicated reasoning** as a private helper or measure-parameterized
  `∀` quantifier. Highest payoff when the same block appears in ≥2 proofs in
  the same file.
- **Detect and delete dead `have`-blocks** (defined but never referenced
  later in the proof body).
- **Delete redundant `change` lines** — defeq restatements of the goal (or a
  hypothesis via `change … at h`) that the following `rw`/`exact`/`apply`/
  `rfl`/`simp`/`congr`/term step accepts unchanged. Highest-frequency move on
  already-tidied files; delete each and re-diagnose. (Move #18.)
- **Find the named Mathlib lemma** that replaces a hand-rolled chain.
  Specific signals: `Real.sSup_def + dif_neg`, ad-hoc `Finset.single_le_sum`
  chains, manual `lipschitzWith_iff_dist_le_mul` derivations.
- **Compress tactic chains to term mode**: `▸` for "have/rewrite/exact", a
  single chained `rw [...]` for nested `have h := show ... from by rw [...]`
  algebra blocks.
- **Eliminate redundant case-splits** where both branches produce the same
  result (e.g., indicator with constant-zero value, `K = 0` Lipschitz
  branches).
- **Discharge HEq/cast eliminator leaf lemmas with `aesop`** — a `∀`-over-a-
  type-equality/`HEq` helper proved by `rintro … rfl; cases …; rfl` collapses
  to `by aesop` (move #19). Applies to the cast-bookkeeping *leaves*, not the
  algebraic spine that consumes them.
- **Linter-driven fixups**: `show` → `change` (`linter.style.show`),
  unused `with hx_def` clauses on `set`, drop `← X` from `simp_rw` chains
  that already closed.

### Guiding principle: `change`/`show` are a code smell

Frequent `change`/`show` is a smell, not just the occasional redundant line.
Both restate the goal **declaratively** — they assert what the goal already
is (up to defeq) instead of driving it toward the conclusion. A proof
littered with them is narrating defeq by hand. Prefer reaching the result
**imperatively**: `apply`, `simp`, and `rw` (plus `exact`/`refine`) transform
the goal forward, and they already unify up to defeq, so the restatement is
usually dead scaffolding (moves #17, #18). When you find a `change`/`show`,
the default question is "does an `apply`/`simp`/`rw` get me to the same place
without it?" — not "is this particular one redundant?".

Keep a `change`/`show` only when it is genuinely load-bearing: setting up a
*syntactic* `rw`/`simp_rw` match, unfolding a binder the elaborator can't see
through, or *being* the `rfl`/closing step. If most of a proof's `change`s
survive that test, that itself is the signal the proof is fighting defeq —
flag it, even when you can't delete the lines outright.

## Maintaining the catalogue

As you work through real simplification passes, you may surface patterns
that aren't in the catalogue yet. The catalogue is only useful if every
entry is **actually applicable to future proofs**. A bad entry — a
one-off observation dressed up as a pattern — wastes the time of every
future reader who scans for matches.

Before adding an entry, every one of the following must be true:

### Admission criteria (all required)

1. **You've seen this pattern in ≥2 distinct proofs.** Not "I bet this
   would come up again" — actually observed twice. A single occurrence is
   an anecdote; two is the start of a pattern. If you've only seen it
   once, note it in the session log and wait. If you don't see it again,
   it wasn't a pattern.

2. **The detection signal is mechanical.** A reader scanning the proof
   should be able to spot it without elaborating the file. Acceptable:
   "≥2 `Finset.single_le_sum` calls in the same block", "`Real.sSup_def`
   followed by `dif_neg`", "tactic block opens with `rw
   [lipschitzWith_iff_dist_le_mul]; intro a b`". Not acceptable: "the
   proof has redundant reasoning", "the convex combination is
   unnecessarily verbose", "this could be tighter".

3. **The replacement is concrete and named.** Either a specific Mathlib
   lemma (`csSup_of_not_bddAbove`, `LipschitzWith.neg`,
   `Finset.sum_eq_zero_iff_of_nonneg`), a specific tactic shape
   (`LipschitzWith.of_dist_le_mul fun a b => by simpa ... using ...`), or
   a specific structural transform (`have x := ...; rw [eq] at x; exact x`
   → `eq ▸ ...`). If the "replacement" is a vague direction ("use
   Mathlib's structural lemmas"), the entry is not actionable.

4. **The line delta is ≥3 lines per occurrence,** or the move eliminates
   a recurring linter warning, or the move enables further moves down the
   chain. A move that trades a 1-line `have` for a 1-line term-mode
   expression is not a pattern — it's stylistic taste, which doesn't
   belong in the catalogue.

5. **The move is unconditional, or the conditions are checkable from the
   text.** A move that works "usually but not when `f` is non-integrable"
   is dangerous unless the catalogue entry tells the reader exactly how
   to check for the bad case. If the check requires running the
   elaborator, the move belongs in a tactic, not the catalogue.

6. **Applying the move requires no mathematical insight.** If picking the
   right Mathlib lemma requires understanding the proof's mathematical
   content (rather than just its syntactic shape), the move isn't
   mechanical and shouldn't be catalogued — it's a one-off insight.

### Template for a new entry

When the criteria are met, add a numbered entry to the "moves" section
of `reference/moves-catalog.md` and a row to the quick-reference table.
Use this template:

```markdown
### N. Short imperative title (≤8 words)

One or two sentences describing the pattern in prose. Reference the
case study where you first observed it.

```lean
-- before (cite line count)
<exact code, ≥3 lines, copied from a real proof>
```

```lean
-- after (cite line count)
<exact replacement, with the same semantics>
```

**Detection signal.** A specific grep-able pattern, ideally with the
literal token(s) to search for.

**Caveat (if any).** When the move *doesn't* apply, with a concrete
test the reader can run.
```

Then add a row to the quick-reference table at the bottom of the file:

```
| <detection signal in 5–10 words> | <move in 5–10 words>. |
```

### Anti-patterns of bad catalogue entries

- **"This proof had redundant `have`s."** No detection signal, no
  replacement, vacuous. Don't add it.
- **"Use `simp` more aggressively."** Stylistic, not mechanical. `simp`
  changes proof terms and can break elaboration time — there's no
  unconditional rule.
- **"Refactor for readability."** Readability is not a pattern.
- **"The original proof used X but Y is shorter."** If Y is shorter
  *only* in this proof's specific context, it's not a pattern. Wait for
  a second occurrence before claiming it.
- **Entries that contradict existing entries.** If the catalogue says
  "extract duplicated reasoning into helpers" and your new entry says
  "inline helpers used in only one place", you don't have a contradiction
  — you have two consistent guidelines for different situations. But if
  the contradiction is real (e.g., one entry says "always use `calc`",
  another says "always use `linarith`"), audit and reconcile.

### When to consult the user before adding

Always show the user the proposed entry before committing it. Two
specific cases warrant an explicit confirmation:

- **The pattern came from a single proof.** Even if you think it's
  obviously a pattern, the "≥2 occurrences" rule is the strongest
  defense against catalogue rot. Ask the user whether they've seen it
  elsewhere.
- **The replacement involves a Mathlib lemma you just learned about.**
  Confirm with the user that the lemma's preconditions match the cases
  the catalogue entry will apply to.

If the user disagrees that something is a pattern, *don't add it*. A
shorter, sharper catalogue beats a long one with weak entries.

## Anti-patterns

- **Don't change the theorem statement.** Not the name, not the binders, not
  the conclusion, not the typeclass arguments. If you think the statement is
  wrong, that's a different task — stop and tell the user.
- **Don't replace tactic style with semantically different tactics.**
  Swapping `simp` for `decide` can change elaboration time, proof term
  shape, and even success/failure when terms aren't fully reducible. The
  goal is *clarity*, not minimal-character golf.
- **Don't add `set_option …` to suppress *new* warnings.** Only to suppress
  warnings *created by your own extraction* (e.g., `linter.unusedSectionVars`
  on a private helper) and even then only after considering whether to
  refactor the variable scope.
- **Don't inline a helper that's used in ≥2 places.** That's the opposite of
  the simplification the user asked for.
- **Don't rename hypotheses or `have`s for "consistency."** Out of scope.
  Renaming is a separate request.
- **Don't run `lake build` after every edit.** Use `leanck:diagnostics`
  scoped to the declaration for fast feedback; reserve `lake build` for the
  end of the pass.
- **Don't trust a single per-module build.** Helper-extraction edits can
  shadow downstream consumers — the full-project `lake build` is the only
  ground truth.

## Unused hypotheses

A hypothesis the proof body never references is *not* automatically dead
code. Two cases:

### 1. Documentation as types

If the hypothesis name aligns with the theorem name or its docstring (e.g.,
`hic : passesIntuitiveCriterion pbe` in `pooling_fails_intuitive_criterion`),
the hypothesis is documenting the *context* in which the theorem applies —
it pins the theorem to a specific narrative even though the proof core
doesn't need it.

**Leave these alone.** The pre-existing unused-variable warning is part of
the file's design. Silence the warning with by preceding the declaration with an 
underscore and leave a comment documenting WHY the variable is unused if not
already present.

### 2. Genuine dead code

If the hypothesis name has no narrative connection to the theorem name and
no docstring mentions it (e.g., a leftover `hmeas` from an earlier draft),
it may genuinely be dead. **Ask the user before dropping it.** Removing a
hypothesis changes the signature, and callers compiled against the longer
signature will break.
