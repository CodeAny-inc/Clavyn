---
name: check-comments
description: Scan Clavyn source for history/attribution comments that should be documentation instead. Use after writing or editing code to catch comments that narrate history or attribute work and fix them before committing.
allowed-tools: Bash(node:*) Bash(npm:*) Bash(npx:*) Read Grep Glob Edit
---

# check-comments — Devin pointer to the canonical Agent Skill

The canonical skill lives at `.agents/skills/check-comments/SKILL.md` (the
standard Agent Skills location from agentskills.io). This file exists so
Devin's own discovery (`.devin/skills/`) finds it.

## Quick reference

    node scripts/verify/check-comments.mjs --pretty     # scan tracked source
    node scripts/verify/check-comments.mjs --staged     # only staged files
    node scripts/verify/check-comments.mjs <path>       # scan a path

## Full instructions

Read `.agents/skills/check-comments/SKILL.md` for the full rule, the fix
workflow, escape hatches, and gotchas. See `AGENTS.md` "Comment hygiene
(enforced)" for the policy. Fix every finding before committing.
