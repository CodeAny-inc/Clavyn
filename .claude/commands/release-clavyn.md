# Release Clavyn

The canonical Agent Skill lives at `.agents/skills/release-clavyn/SKILL.md`
(standard agentskills.io location). This slash command is a thin pointer so
Claude Code's `/release-clavyn` command finds it.

## Quick reference

```bash
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
```

## Full instructions

Read `.agents/skills/release-clavyn/SKILL.md` for the complete workflow —
changelog format rules, CI-vs-local release path selection, the moving-main
rebase dance, post-release verification, and gotchas.
