---
name: check-pr
description: Check a PR/MR/CL for unresolved review comments, failing status checks, and incomplete description, then fix and resolve. Use when asked to check a PR/MR/CL, address review feedback, or prepare a change for submission.
allowed-tools: Bash(gh:*) Bash(glab:*) Bash(git:*) Bash(p4:*) Read Grep Glob Edit Write
---

# check-pr — Devin pointer to the canonical Agent Skill

The canonical skill lives at `.agents/skills/check-pr/SKILL.md` (the
standard Agent Skills location from agentskills.io, supported by Claude Code,
Cursor, Codex, VS Code, Copilot, Gemini CLI, Goose, OpenHands, and more).
Vendored from https://github.com/greptileai/skills (MIT).
This file exists so Devin's own discovery (`.devin/skills/`) finds it.

## Quick reference

    gh pr view --json number -q .number   # auto-detect PR for current branch
    # skill walks: detect platform → wait for checks → analyze →
    # categorize actionable vs informational → fix → resolve threads

## Full instructions

Read `.agents/skills/check-pr/SKILL.md` for the complete workflow — platform
detection (GitHub/GitLab/Perforce), pending-check polling, review-comment
triage, thread resolution via GraphQL/glab, and gotchas. See `AGENTS.md`
"Greptile PR review skills" for requirements (`gh auth login`).
