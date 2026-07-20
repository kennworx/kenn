---
id: fnd_3b4a11ad-9a4c-402b-a918-5cdccfcb497f
tags:
- directive
- polarity:dont
parent_ids: []
created_at: 2026-07-06T15:28:30.230042Z
---
kenn reads ALL git metadata in-process via gix through kenn-store/src/git.rs (head_id / tracked_modified / main_worktree / all_worktrees / work_dir) — NEVER Command::new("git") in runtime code. The gix-git-backend migration removed the git-binary dependency (no PATH, no safe.directory, no core.quotepath, no porcelain parsing, no per-call spawn). Add any new git operation to git.rs; tests MAY spawn git only to set up fixture repos. Two hard-won gotchas: (1) gix common_dir() is UNNORMALIZED for a linked worktree (git stores commondir as a relative ../..), so canonicalize BEFORE .parent() or the main-worktree path lands one directory too deep. (2) the staleness dirty set MUST include BOTH staged (HEAD-tree vs index) and unstaged (index vs worktree) changes — the default status() does; a staged change to a clean worktree would otherwise be missed and wrongly skip a reindex.