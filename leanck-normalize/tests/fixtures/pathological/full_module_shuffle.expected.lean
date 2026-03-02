/-
Copyright (c) 1999 Pathological Tests.
-/
module

public import Algebra.Basic
public import Data.List
import Alpha
import Zeta

/-!
# color center

this module doc has a terribly long sentence with color and
organisation details: It should be capitalized after the colon and
wrapped at a narrow width.

* a bullet with a continuation line that has extra content about
  marginalizing cases and favoring stable indentation and this
  continuation should stay visually attached to the bullet even after
  wrapping.
-/

@[expose] public noncomputable section

namespace Weird

@[simp] lemma colour_lemma : True := by
  trivial

def x := 1

/-- a very long compact comment describing behavior: It favors color,
organize and marginalized examples while preserving a literal delimiter
like /- inside the text and it should not lose the closing delimiter. -/
theorem t : True := by
  trivial
