---
name: verify-clavyn
description: Drive, inspect, and verify the Clavyn Tauri app's UI with a composable CLI over CDP/Playwright. Use after any frontend change to confirm behavior, capture evidence, and navigate features.
allowed-tools: Bash(node:*) Bash(npm:*) Bash(npx:*) Read Grep Glob
---

# verify-clavyn — Devin pointer to the canonical Agent Skill

The canonical skill lives at `.agents/skills/verify-clavyn/SKILL.md` (the
standard Agent Skills location from agentskills.io, supported by Claude Code,
Cursor, Codex, VS Code, Copilot, Gemini CLI, Goose, OpenHands, and more).
This file exists so Devin's own discovery (`.devin/skills/`) finds it; this
file is a condensed pointer to the canonical instructions.

## Quick reference

    node scripts/verify/control-clavyn.mjs doctor --pretty   # always start here
    node scripts/verify/control-clavyn.mjs home
    node scripts/verify/control-clavyn.mjs navigate <view>
    node scripts/verify/control-clavyn.mjs snapshot --pretty
    node scripts/verify/control-clavyn.mjs screenshot /tmp/proof.png
    node scripts/verify/control-clavyn.mjs console --pretty
    node scripts/verify/control-clavyn.mjs stop

## Full instructions

Read `.agents/skills/verify-clavyn/SKILL.md` for the complete workflow,
examples, gotchas, and rules. Read `scripts/verify/features/README.md` before
navigating — it catalogs every view and the exact CLI commands to drive it.
Full CLI docs: `scripts/verify/README.md`.
