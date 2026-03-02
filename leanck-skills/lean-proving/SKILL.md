---
name: "lean-proving"
description: "Lean 4 theorem proving workflow. Use this skill when working with Lean proofs."
allowed-tools: Bash(leanck*)
---

# Theorem Proving in Lean4

This skill helps you prove theorems in Lean4. The guiding principle is **fast feedback**: rather than one-shot a complicated proof, incrementally build it up by stating smaller lemmas and discharging them with realtime checks from the Lean elaborator. Every step you take should be checked against the elaborator before the next step.

You have been provided the `leanck` CLI, which exposes several subcommands that will be useful to you. The most important is `leanck diagnostics`, which provides real-time elaboration feedback. Use these tools early and often to maximize your effectiveness.

## Before You Start

* Make sure that the `leanck` CLI is available and working by running `leanck health`. If the binary is not available, STOP and inform the user. Otherwise, surface the `health` warnings (if any) and continue.
* Sketch the mathematics of the proof before formalizing. Make sure you understand *precisely* the claim you are trying to prove and ensure that the shape of the statement is semantically accurate. Most lean4 errors are SEMANTIC (theorems that suggest one conclusion but actually prove a weaker one). Avoid this mistake up front by pinning down the statement shape.
* Search for existing infrastructure and declarations. Especially when working in Econlib, there is a large library of existing mathematical and economic infrastructure already available. Be sure you are not duplicating effort by searching the repository carefully for intermediate lemmas or mathematical facts that will be useful. 

## Workflow

1. **Search before you write.** Use `leanck loogle` for queries by constant,
   name substring, subexpression, or conclusion type. Use `grep` for discovery
   in project and Mathlib source. Inspect each candidate's full signature.
2. **State the theorem first.** Write the theorem signature and confirm the statement is faithful to the request. Make sure:
  * The theorem is *complete*. If asked to prove an iff, ensure both directions are present. If a uniqueness claim is made, ensure that you demonstrate uniqueness, not just existence. 
  * The theorem is *semantically correct*. If a hostile reviewer read the name and docstring of the theorem, would they agree it has been formalized in full?
  * The theorem is *not vacuous*. The hypotheses should be jointly satisfiable.
  * If you are asked to prove a named, well-known theorem, be especially careful that your encoding correctly reflects the *entire* theorem.
3. **Write the scaffold**. Compose the proof skeleton as a sequence of `have ... := by sorry` and sublemmas. The skeleton tells you whether the *decomposition* is sound before you commit time to the leaves. If the skeleton's final step doesn't close the goal even with all `have`s admitted, the decomposition is wrong — fix that before going deeper.
4. **Discharge `sorry`s in order of risk.** Pick the hardest-looking sublemma first. If that one turns out to be false or unprovable as stated, it us better to discover that early.
5. Once you are done, **confirm the full build** using `leanck build`, then run the [post-flight checklist](#post-flight-checklist).

## Guidelines

### Use `leanck` as the builder and typechecking tool
`leanck` provides an agent-optimized interface to `lake` and `lean`. DO NOT use `lake build` or `lake env lean`; prefer `leanck diagnostics` for incremental typechecking, `leanck goal` for inspecting goal state, and `leanck build` for project-wide builds. Each `leanck` command and subcommand has a help page you can consult by passing `--help`.

Diagnostics default to errors only. Use `leanck diagnostics <file> --severity error,warning` to include warnings, or repeat `--severity` for each name. Accepted names are `error`, `warning`, `information`, and `hint`. Use `--all-severities` for all severities instead of the old `--severity all`; it cannot be combined with `--severity`.

### Write good proof scaffolds
Proof scaffolds are intended to ensure that the pieces of the intended proof strategy compose as you expect. Scaffolds should be detailed and not handwave steps: It is better to catch issues early than have to work around them once tactics are being written.

When writing scaffolds, first write the headline theorem with a series of `have` statements with sorry bodies. Then write the necessary lemmas, also with `sorry` bodies. Declarations (theorems, lemmas, defs, structures, etc) should be named according to Mathlib convention; avoid eponymous names unless it is well-justified.

### Write object-centered docstrings

Docstrings must never emphasize implementation decisions. Explain the mathematical object and its
economics: what the parameters represent, what the fields or expressions mean, and the assumptions,
quantifiers, and sign conventions needed to interpret the definition or result. For example, an
environment's docstring should identify agents, type spaces, outcomes, valuations, and utility—not
lead with the absence of a bundled prior.

Apply this rule to both declaration and module docstrings. Write for users, not as a development log:
docstrings must contain no migration history. Keep API organization, representation choices, and
proof-engineering rationale in design documents or ordinary implementation comments, not docstrings.
State mathematical scope qualifications where needed, but do not substitute them for an
explanation of the object. Document economic meaning only where it applies; do not invent an economic
interpretation for general mathematical infrastructure.

### Use subagents
Use the host's available subagents for bounded proof tasks, with explicit file
ownership and self-contained briefs.

* `lean-expert` (Traditional subagent): Opus-tier Lean 4 formalization specialist. Use when a proof needs Mathlib-fluent intuition (measure theory, functional analysis, intricate `simp`/rewrite chains, deep type-class plumbing) or is expected to be intricate.
* `lean-grind` (Traditional subagent): Cost-optimized formalization agent. Use to discharge tedious or routine proofs that do not require deep insight but that would be time-consuming and error-prone to do on your own.

### Parallelize work
To increase efficiency, you should parallelize work as much as possible. Agents must work on disjoint files; dispatch them in worktrees to avoid transient typecheck errors.

### Anti-patterns

- **Don't batch many tactic edits then check.** One edit, one diagnostic, one goal check. 
- **Don't add or change hypotheses without user confirmation.** Hypotheses should be added or changed only after the user has confirmed they are correct.
- **Don't weaken the conclusion without user confirmation.** The conclusion should be weakened only after the user has agreed that the change is necessary.
- 
## Post-flight checklist
Once you are satisfied that the proof is complete, you should:
- Run `leanck build` to verify that the project builds after your changes.
- Run `leanck normalize` on touched files; this will automatically handle formatting issues like comment line width, heading formats, and correct frontmatter.
- Audit declaration and module docstrings against the object-centered documentation rule above.
- Load the `lean-style` skill and execute its workflow; use one `lean-style-worker` per file.
- Rerun `leanck build` to verify that the project still builds.
