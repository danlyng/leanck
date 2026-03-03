/-
Copyright (c) 1999 Pathological Tests.
-/
module

import Zeta
public import Algebra.Basic

import Alpha
public import Data.List

@[expose] public noncomputable section

namespace Weird

/-!
# colour centre

this module doc has a terribly long sentence with colour and organisation details: it should be capitalised after the colon and wrapped at a narrow width.

* a bullet with a continuation line that has extra content about marginalising cases and favouring stable indentation
  and this continuation should stay visually attached to the bullet even after wrapping.
-/

@[simp]
lemma colour_lemma : True := by
  trivial

def x := 1

/-- a very long compact comment describing behaviour: it favours colour, organise and marginalised examples while preserving a literal delimiter like /- inside the text and it should not lose the closing delimiter. -/
theorem t : True := by
  trivial
