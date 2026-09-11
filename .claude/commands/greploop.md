# Greploop

The canonical Agent Skill lives at `.agents/skills/greploop/SKILL.md`
(standard agentskills.io location, vendored verbatim from
https://github.com/greptileai/skills — MIT). This slash command is a thin
pointer so Claude Code's `/greploop` command finds it.

## Quick reference

```bash
gh pr comment <PR> --body "@greptile review"   # trigger review
# skill loops: poll check run → fetch score/comments → fix →
# resolve threads → push → repeat (until 5/5 + 0 comments, max 5)
```

## Full instructions

Read `.agents/skills/greploop/SKILL.md` for the complete workflow — platform
detection (GitHub/GitLab/Perforce), Greptile check polling, score parsing
(including edited summary comments), thread resolution, exit conditions, and
the iteration cap. See `AGENTS.md` "Greptile PR review skills" for
requirements (Greptile app installed on the repo + `gh auth login`).
