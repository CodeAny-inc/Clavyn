# CLI Review

The canonical Agent Skill lives at `.agents/skills/cli-review/SKILL.md`
(standard agentskills.io location, vendored from
https://github.com/greptileai/skills — MIT). This slash command is a thin
pointer so Claude Code's `/cli-review` command finds it.

## Quick reference

```bash
command -v greptile        # CLI installed? (npm i -g greptile)
greptile whoami            # authenticated? (greptile login)
greptile review --json     # run the review, summarize findings
```

## Full instructions

Read `.agents/skills/cli-review/SKILL.md` for the complete workflow — repo
context check, CLI install/auth, JSON output parsing, and the `--agent`
fallback. See `AGENTS.md` "Greptile PR review skills" for requirements.
