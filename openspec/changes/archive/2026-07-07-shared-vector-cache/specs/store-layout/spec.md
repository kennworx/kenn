## MODIFIED Requirements

### Requirement: Vectors location is independently configurable

The committed vectors root SHALL be relocatable via the `[vectors] location`
config setting, which accepts: a **relative path**, an **absolute path**, or the
keyword `"global"` (an XDG cache path keyed by the repo id).

A relative path SHALL resolve against the **main worktree** (git root), not the
per-worktree `source_root`, so that every worktree of one repository resolves a
relative location to the same directory. The main worktree is discovered via the
in-process git backend (`git::main_worktree`); outside a git working tree the
relative path SHALL resolve against `source_root` (prior behavior). When
`source_root` *is* the main worktree, its own path spelling is preserved so
resolved layouts stay byte-identical to the pre-change default.

When set, every sidecar directory — the per-generation namespaces and the
legacy flat `code/`/`findings/` dirs — SHALL resolve under the new location.

The default — when `[vectors] location` is unset — SHALL be the git-root-relative
shared vectors subdir, so linked worktrees share a vector cache out of the box;
outside a git tree it SHALL remain `<committed_root>/vectors` (prior behavior).

#### Scenario: a relative location is shared across worktrees

- **GIVEN** two worktrees of one repository, both with `[vectors] location = "vectors"`
- **WHEN** each resolves its layout
- **THEN** both resolve `vectors_root` to the same `<main-worktree>/vectors`

#### Scenario: relative location outside a git tree keeps prior behavior

- **WHEN** a relative `[vectors] location` is set in a non-git directory
- **THEN** it resolves against `source_root`, unchanged

#### Scenario: default location is shared across worktrees

- **GIVEN** a linked worktree with no `[vectors] location` set
- **WHEN** it resolves its layout
- **THEN** `vectors_root` resolves under the main worktree's shared vectors subdir
- **AND** a second worktree resolves to the same directory

#### Scenario: absolute override still moves both sidecars verbatim

- **WHEN** `[vectors] location = "/mnt/shared/kenn-vectors"`
- **THEN** `vectors_root` resolves to `/mnt/shared/kenn-vectors`
- **AND** `code_vectors_dir()` / `findings_vectors_dir()` resolve under it
