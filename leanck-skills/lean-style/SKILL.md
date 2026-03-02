---
name: lean-style
description: Dispatch a team of lightweight agents to fix lean style issues.
---

# Applying Style Fixes

This skill is invoked when the user wants to align the specified files or directory
with the repo style guidelines, which are provided in `STYLE.md` (supplementing what
is in `CLAUDE.md`/`AGENTS.md`). 

DO NOT MAKE STYLE EDITS DIRECTLY. Instead, you will dispatch a focused team of 
lightweight and inexpensive `lean-style-worker` agents. These agents will make style 
edits on a specific file. They are explicitly instructed to NOT typecheck to avoid 
transient build failures due to many inflight changes. You must verify that the 
project typechecks after the team is complete.

Provide the agents with:
* Exactly one file to work on.
* The project style guidelines (typically, point them to `STYLE.md`)
* If approrpriate, provide supplemental instructions from user requests or your
  knowledge of the project.

Each `lean-style-worker` agent will report to you (1) the edits it made and (2)
suggestions for project/mathlib-consistent renames for declarations in its file. 

Once the agents ALL complete, do the following:
1. Rebuild the project and fix build errors.
2. Meanwhile, collate the rename suggestions and evaluate them on their merits. Don't
   assess downstream impact or churn; these are concerns the user will evaluate.
3. Write the collated rename suggestions and your evaluation to __RENAMES.md at the repo root.
   Present it to the user for their assessment. DO NOT COMMIT THIS FILE; it is for discussion.
   The user may then instruct you to execute some or all of the renames.
