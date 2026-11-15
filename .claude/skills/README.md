# Vendored skills

These skills are copied from [baiirun/skills](https://github.com/baiirun/skills)
so every Claude Code session that clones this repo has them without a fetch
step.

- Source commit: `2fbca5ffd2c29fb2e3e4de615bdb808bed9b7916` (2026-11-10)
- Copied: `code-quality`, every skill its router names (`skeleton-first`,
  `modeling-data`, `designing-structure`, `handling-failure`, `sharing-state`,
  `changing-existing-code`, `testing-and-verifying`, `documenting-code`,
  `no-comments`, `typescript-best-practices`), `feature-contract-docs` (named
  by `documenting-code`), and `principles/`.
- Each skill reaches the principles through a `principles -> ../principles`
  symlink, as upstream does.

Files here are unedited copies. Chimi-specific rules about how to use them,
such as skipping the approval gates, live in
[docs/process/procedure.md](../../docs/process/procedure.md).

## Re-sync

```sh
git clone https://github.com/baiirun/skills /tmp/skills
for d in code-quality principles skeleton-first modeling-data designing-structure \
  handling-failure sharing-state changing-existing-code testing-and-verifying \
  documenting-code no-comments feature-contract-docs typescript-best-practices; do
  rm -rf .claude/skills/$d && cp -a /tmp/skills/$d .claude/skills/
done
git -C /tmp/skills rev-parse HEAD   # update the source commit above
```

If upstream's router starts naming another skill, add it to the list.
