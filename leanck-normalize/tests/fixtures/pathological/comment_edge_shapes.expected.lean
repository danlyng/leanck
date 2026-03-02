namespace Scratch

  /-- a compact indented comment with very uneven
  spacing, colors, centers, and a trailing code-looking
  token such as `foo : bar` that should not be
  capitalized inside backticks. -/
  def indented := 10

/-- A paragraph with an inner delimiter /-- nested
looking start and a closing-looking - / near miss token
written as prose.

* first bullet talks about organized colors and has
  enough prose to wrap over multiple physical lines
  continuation text is indented more deeply and should
  remain attached to the bullet.

Another paragraph after far too many blank lines: it
should be capitalized and separated by just one blank
line. -/

def y := 20

/-!
A later module-style doc comment in a snippet is still
the first module doc block here, so its delimiters
should stay on their own lines.
-/
