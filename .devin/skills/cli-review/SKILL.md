---
name: cli-review
description: Run a Greptile CLI review from the current local checkout and summarize findings. Use when asked for Greptile feedback before opening a PR, outside a hosted PR review flow, or directly from a local checkout.
allowed-tools: Bash(git:*) Bash(greptile:*) Bash(command:*) Bash(curl:*) Bash(npm:*) Read Grep Glob
---

# cli-review — Devin pointer to the canonical Agent Skill

The canonical skill lives at `.agents/skills/cli-review/SKILL.md` (the
standard Agent Skills location from agentskills.io, supported by Claude Code,
Cursor, Codex, VS Code, Copilot, Gemini CLI, Goose, OpenHands, and more).
Vendored verbatim from https://github.com/greptileai/skills (MIT).
This file exists so Devin's own discovery (`.devin/skills/`) finds it.

## Quick reference

    command -v greptile        # CLI installed? (npm i -g greptile)
    greptile whoami            # authenticated? (greptile login)
    greptile review --json     # run the review, summarize findings

## Full instructions

Read `.agents/skills/cli-review/SKILL.md` for the complete workflow — repo
context check, CLI install/auth, JSON output parsing, and the `--agent`
fallback. See `AGENTS.md` "Greptile PR review skills" for requirements.
