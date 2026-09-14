---
name: greploop
description: Loop — trigger Greptile review, fix all actionable comments, push, re-review — until 5/5 confidence and zero unresolved comments (max 5 iterations). Use when asked to fully optimize a PR/MR/CL against Greptile's code review standards.
allowed-tools: Bash(gh:*) Bash(glab:*) Bash(git:*) Bash(p4:*) Read Grep Glob Edit Write
---

# greploop — Devin pointer to the canonical Agent Skill

The canonical skill lives at `.agents/skills/greploop/SKILL.md` (the
standard Agent Skills location from agentskills.io, supported by Claude Code,
Cursor, Codex, VS Code, Copilot, Gemini CLI, Goose, OpenHands, and more).
Vendored from https://github.com/greptileai/skills (MIT).
This file exists so Devin's own discovery (`.devin/skills/`) finds it.

## Quick reference

    gh pr comment <PR> --body "@greptile review"   # trigger review
    # skill loops: poll check run → fetch score/comments → fix →
    # resolve threads → push → repeat (until 5/5 + 0 comments, max 5)

## Full instructions

Read `.agents/skills/greploop/SKILL.md` for the complete workflow — platform
detection (GitHub/GitLab/Perforce), Greptile check polling, score parsing
(including edited summary comments), thread resolution, exit conditions, and
the iteration cap. See `AGENTS.md` "Greptile PR review skills" for
requirements (Greptile app installed on the repo + `gh auth login`).
