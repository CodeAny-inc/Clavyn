# Check Comments

The canonical Agent Skill lives at `.agents/skills/check-comments/SKILL.md`
(standard agentskills.io location). This slash command is a thin pointer so
Claude Code's `/check-comments` command finds it.

## Quick reference

```bash
node scripts/verify/check-comments.mjs --pretty     # scan tracked source
node scripts/verify/check-comments.mjs --staged     # only staged files
node scripts/verify/check-comments.mjs <path>       # scan a path
```

## Full instructions

Read `.agents/skills/check-comments/SKILL.md` for the full rule, the fix
workflow, escape hatches, and gotchas. See `AGENTS.md` "Comment hygiene
(enforced)" for the policy. Fix every finding before committing.
