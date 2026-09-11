# Check PR

The canonical Agent Skill lives at `.agents/skills/check-pr/SKILL.md`
(standard agentskills.io location, vendored verbatim from
https://github.com/greptileai/skills — MIT). This slash command is a thin
pointer so Claude Code's `/check-pr` command finds it.

## Quick reference

```bash
gh pr view --json number -q .number   # auto-detect PR for current branch
# skill walks: detect platform → wait for checks → analyze →
# categorize actionable vs informational → fix → resolve threads
```

## Full instructions

Read `.agents/skills/check-pr/SKILL.md` for the complete workflow — platform
detection (GitHub/GitLab/Perforce), pending-check polling, review-comment
triage, thread resolution via GraphQL/glab, and gotchas. See `AGENTS.md`
"Greptile PR review skills" for requirements (`gh auth login`).
