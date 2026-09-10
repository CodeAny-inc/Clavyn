---
name: release-clavyn
description: Cut a Clavyn release end-to-end — CHANGELOG.md, version bump, verification, tag, and signed multi-platform GitHub release via CI. Use whenever asked to release, ship, tag, or write release notes for Clavyn.
allowed-tools: Bash(git:*) Bash(gh:*) Bash(cargo:*) Bash(npm:*) Bash(npx:*) Bash(node:*) Read Grep Glob Write Edit
---

# release-clavyn — Devin pointer to the canonical Agent Skill

The canonical skill lives at `.agents/skills/release-clavyn/SKILL.md` (the
standard Agent Skills location from agentskills.io, supported by Claude Code,
Cursor, Codex, VS Code, Copilot, Gemini CLI, Goose, OpenHands, and more).
This file exists so Devin's own discovery (`.devin/skills/`) finds it.

## Quick reference

    git log --oneline <prev-tag>..origin/main   # survey what's shipping
    # write the CHANGELOG.md entry (Keep a Changelog format)
    ./scripts/version.sh 0.1.2-alpha.4          # bump all three manifests
    cargo check --workspace
    cd desktop && npx vue-tsc --noEmit && npx vitest run
    node scripts/verify/check-comments.mjs
    git commit -m "Release v0.1.2-alpha.4" && git push origin main
    git tag v0.1.2-alpha.4 && git push origin v0.1.2-alpha.4
    gh run watch <run-id> --exit-status         # CI builds + signs + publishes
    gh release edit v0.1.2-alpha.4 --notes "..."  # replace placeholder notes

## Full instructions

Read `.agents/skills/release-clavyn/SKILL.md` for the complete workflow —
changelog format rules, CI-vs-local release path selection, the moving-main
rebase dance, post-release verification, and gotchas.
