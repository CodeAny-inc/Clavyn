# Verify Clavyn UI

The canonical Agent Skill lives at `.agents/skills/verify-clavyn/SKILL.md`
(standard agentskills.io location). This slash command is a thin pointer so
Claude Code's `/verify-clavyn` command finds it.

## Quick reference

```bash
node scripts/verify/control-clavyn.mjs doctor --pretty   # always start here
node scripts/verify/control-clavyn.mjs home
node scripts/verify/control-clavyn.mjs navigate <view>
node scripts/verify/control-clavyn.mjs snapshot --pretty
node scripts/verify/control-clavyn.mjs screenshot /tmp/proof.png
node scripts/verify/control-clavyn.mjs console --pretty
node scripts/verify/control-clavyn.mjs stop
```

## Full instructions

Read `.agents/skills/verify-clavyn/SKILL.md` for the complete workflow,
examples, gotchas, and rules. Read `scripts/verify/features/README.md` before
navigating — it catalogs every view and the exact CLI commands to drive it.
Full CLI docs: `scripts/verify/README.md`.
