## MODIFIED Requirements

### Requirement: The store separates committed and derived data into named roots

The store SHALL split its on-disk state into three roots, each
resolved once at `Layout::resolve()`:

- `source_root` — the workspace root (where the source code lives).
- `committed_root` — always `<source_root>/.kenn`, git-tracked,
  not relocatable.
- `derived_root` — gitignored, throwaway, rebuilt by `kenn index`,
  relocatable via `[layout] derived_root` (relative path, absolute
  path, or the keyword `"global"`).

The committed root holds:

- `findings/{id}.md` — finding records (source of truth), markdown with
  immutable YAML frontmatter (`id`, `tags`, `parent_ids`, `created_at`) and a
  prose body.
- `findings/{id}.anchor.jsonl` — the per-finding append-only anchor + liveness
  event log (mutable, mergeable).
- `vectors/code/` — committed code embedding sidecar.
- `vectors/findings/` — committed findings embedding sidecar.
- `.gitignore` — excludes `local/`.

The derived root holds:

- `runs/{id}/` — one directory per index pass (see "runs-centric
  derived state" requirement), including the snapshot-local `overview.md`
  orientation file written by `kenn index`.
- `live` — symlink to the active run.
- `index.lock`, `findings.lock`, `readers/` — store-wide
  bookkeeping. The `embed-locks/` directory is no longer
  required (content-addressed naming + per-writer unique tmp
  filenames replace the per-sidecar advisory lock).

#### Scenario: default layout for a fresh workspace

- **WHEN** `Layout::default_for(<source>)` is called on a workspace
  with no `kenn.toml`
- **THEN** `committed_root` resolves to `<source>/.kenn`
- **AND** `derived_root` resolves to `<source>/.kenn/local`
- **AND** `vectors_root` resolves to `<source>/.kenn/vectors`
- **AND** `code_vectors_dir()` resolves to `<source>/.kenn/vectors/code`
- **AND** `findings_vectors_dir()` resolves to
  `<source>/.kenn/vectors/findings`

#### Scenario: derived_root override relocates only derived state

- **WHEN** `kenn.toml` sets `[layout] derived_root = "global"`
- **THEN** `derived_root` resolves to an XDG cache path keyed by
  the repo id
- **AND** `committed_root` still resolves to `<source>/.kenn`
- **AND** `vectors_root` still resolves to `<source>/.kenn/vectors`
  (the vectors location is independent of `derived_root`)

#### Scenario: a finding's record and anchor log are committed, the snapshot is derived

- **WHEN** a finding is flushed and `kenn index` runs
- **THEN** `findings/{id}.md` and `findings/{id}.anchor.jsonl` are under the
  committed root and git-tracked
- **AND** the run's `overview.md` is under the derived root and gitignored

#### Scenario: paths come only from Layout

- **WHEN** any component needs the path of a finding record, its anchor log, or
  the snapshot overview
- **THEN** it obtains that path from a `Layout` accessor
- **AND** no such path segment is hardcoded outside the layout module
