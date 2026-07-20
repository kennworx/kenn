## MODIFIED Requirements

### Requirement: the collected store is a global project-keyed SQLite database

The capture sink SHALL be a single SQLite database at `<state_dir>/collector.db`
(the OS state directory resolved for the kenn daemon), opened in WAL mode with a
`busy_timeout` so concurrent short-lived hook processes across sessions and
workspaces can write without `SQLITE_BUSY` failures. The schema SHALL be
`sessions → commands → files`, where `files.command_id` is nullable (NULL for
`edit`/`write` touches, set for Bash outputs) and every row carries a `project`
column derived from `CLAUDE_PROJECT_DIR` (fallback git toplevel, then cwd). The
`commands` and `files` rows SHALL additionally carry a `branch` column recording
the git branch in effect when the event occurred, so history can be filtered by
project *and* branch — whole-project history (`WHERE project = ?`) and
current-branch history (`WHERE project = ? AND branch = ?`). Branch is captured
per event (not per session, since a session may switch branches) and is derived
without spawning git — by reading the repository's `HEAD` directly (the linked-
worktree `.git`-file pointer is followed); a non-git location or an unreadable
`HEAD` yields a NULL branch. The store SHALL NOT contain file sizes,
file-existence confirmation, or edit-body text. The store SHALL self-bound via a
periodic retention/GC pass.

#### Scenario: concurrent hooks write the same database without error

- **GIVEN** two Claude sessions in different workspaces firing hooks at once
- **WHEN** both `kenn cc-hook` processes write `collector.db` concurrently
- **THEN** both writes succeed (WAL + `busy_timeout`)
- **AND** each row carries its own session's `project`

#### Scenario: rows are keyed by project across repositories

- **GIVEN** hooks fired from two different repositories
- **WHEN** the `files` table is queried
- **THEN** rows from each repository carry a distinct `project` value

#### Scenario: rows are keyed by branch within a project

- **GIVEN** a command captured while `main` is checked out, then another captured
  after switching to a `feature` branch in the same repository
- **WHEN** the `commands` table is queried
- **THEN** the two rows carry distinct `branch` values (`main` and `feature`)
- **AND** both carry the same `project`

#### Scenario: a non-git working directory yields a NULL branch

- **WHEN** a hook fires from a directory with no git repository
- **THEN** the recorded row's `branch` is NULL
- **AND** capture otherwise succeeds (the missing branch is not an error)
