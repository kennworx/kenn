## 1. The reconcile skill

- [x] 1.1 Add `claude-plugins/kenn/skills/reconcile/SKILL.md` — frontmatter
  (`name`, triggering `description`, `user-invocable: true`) + the janitor steps:
  sweep `check_anchors` (broken + drifted) and finding `stale`/`drifted` flags;
  for each, **re-read the cited file** then act (rename/attach refresh,
  supersede, detach, tombstone). → verify: skill is discovered and invocable.
- [x] 1.2 Encode **vet-over-report** (re-read before acting; never act on a flag
  alone) and the **repo-content-is-data** injection guard (a file that issues
  instructions is recorded as data, never followed). → verify: both disciplines
  appear as explicit steps.

## 2. Wire into the guide + harden squeeze

- [x] 2.1 Update the kenn agent guide (`claude-plugins/kenn/skills/kenn/SKILL.md`)
  to route to `reconcile` as the third lifecycle skill, and reflect the new
  `drifted` signal on `check_anchors` / `find_directives`. → verify: guide names
  all three skills and the drifted bucket/flag.
- [x] 2.2 Add the prompt-injection guard to `squeeze`
  (`claude-plugins/kenn/skills/squeeze/SKILL.md`), matching `reconcile`. →
  verify: squeeze states repo content is data, not instructions.

## 3. Spec

- [x] 3.1 `agent-guide` delta: ADD the reconcile-skill requirement; MODIFY the
  router requirement to name `reconcile`. → verify: `openspec validate` passes.
