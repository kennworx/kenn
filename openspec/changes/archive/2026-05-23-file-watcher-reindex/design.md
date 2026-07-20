## Context

`index-store-staleness` shipped explicit invocation and the git-aware
skip; the file-watcher signal it also specified was deferred. With
`mcp-background-reindex` now shipped — the MCP server owns
`spawn_background_reindex`, the snapshot poll task, and the `ArcSwap`
hot-reload — the watcher's natural home is no longer a separate
process. It becomes an MCP feature that *triggers* the existing
reindex machinery.

The git-aware (and tree-fingerprint) staleness key is still the
load-bearing safety net: the watcher only triggers a run; whether
work actually happens is decided by the staleness key and the
one-writer flock. The watcher stays dumb and cheap.

## Goals / Non-Goals

**Goals:**
- An in-process file watcher inside `kenn mcp` that triggers
  `spawn_background_reindex` on a debounced window
- Agent-controllable via `watch_start` / `watch_stop` tools; idempotent
  start
- An optional `mcp.watch_on = true` to start the watcher at server boot
- A single MCP notification on every reader swap — agent sees fresh
  data regardless of who caused the reindex

**Non-Goals:**
- A `kenn watch` CLI subprocess (rejected — folds into MCP instead)
- A `Clock` abstraction trait — tokio's paused-time test mode covers
  the testing need
- Incremental / partial reindex — a trigger is always a full reindex
- Cross-machine or network filesystem support

## Decisions

### D1. Watcher lives in `kenn-mcp`, not in a separate process

The MCP server is already the long-running process that owns the
indexed snapshot. Putting the watcher in the same process collapses
three pieces — watcher, trigger, swap — into one address space and
removes the inter-process polling that a separate `kenn watch` would
have produced. `notify` is added to `kenn-mcp`'s deps unconditionally;
whether the watcher *runs* is a runtime decision (`watch_on` /
`watch_start`).

### D2. Event source: the `notify` crate

`notify` is the standard cross-platform Rust filesystem-watch crate
(inotify / FSEvents / ReadDirectoryChangesW). Used as
`RecommendedWatcher` in `RecursiveMode::Recursive` rooted at the
workspace, so platform debouncing / coalescing is in play before our
own window even starts.

All `notify` event kinds whose paths survive the filter contribute to
the debounce window — `Create`, `Modify`, `Remove`, `Rename`. A
deletion can drop symbols just as a save can add them, so the watcher
treats them identically; the staleness key decides whether the
resulting reindex does real work.

Symlink traversal: `notify`'s symlink behavior is platform-dependent
and not adjusted here. A symlink that points outside the workspace
into a watched subtree may produce events for paths outside the
workspace; those are filtered out by the exclude check below (which
operates on workspace-relative paths). Symlinks pointing *into* the
workspace from outside are not observed at all.

**Known limitation: nested git checkouts are not specially handled.**
If a separate git checkout lives at a child path (e.g., `subprojects/foo/`
with its own `.git`), its source files produce events like any other
directory. The `.git/` and `.kenn/` subdirectories are still skipped
(via `WORKSPACE_SKIP_DIRS`), so the noisy parts are filtered, but the
source files of a nested checkout will trigger reindex windows. Users
who want a nested checkout ignored can add it to `[exclude] globs`.
Auto-detecting nested checkouts would require a stat-per-event walk
upward looking for `.git`; deferred until a real user hits it.

### D3. Filtering before debouncing

An event survives to the debouncer only if its path:
- has an extension in `Language::extensions()` for any indexed
  language (see D3a), AND
- is not inside any directory in `WORKSPACE_SKIP_DIRS` (see D3b;
  includes `.git` and `.kenn`), AND
- does not match any user-supplied `[exclude] globs`.

Filtering happens before debouncing so build-output bursts never even
start a window.

### D3a. Extension source: a new `Language::extensions()`

`kenn-model::Language` currently exposes `prefix()` and `db_name()`
but no file-extension mapping — extensions are an external concept
(the per-language drivers consume JSONL, not source). The watcher
needs them, and giving them a home on the closed `Language` enum
keeps the mapping in one place:

```rust
pub enum ProjectFile {
    /// Match by extension (no leading dot), e.g. `MyApp.csproj`.
    Extension(&'static str),
    /// Match by full filename, e.g. `Cargo.toml`.
    Filename(&'static str),
}

impl Language {
    /// Source-file extensions only.
    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Csharp     => &["cs"],
            Self::TypeScript => &["ts", "tsx", "mts", "cts"],
            Self::Rust       => &["rs"],
            Self::Go         => &["go"],
            Self::Python     => &["py", "pyi"],
        }
    }

    /// Project / dependency files — extension or full-filename matchers.
    pub const fn project_files(self) -> &'static [ProjectFile] {
        use ProjectFile::{Extension, Filename};
        match self {
            Self::Csharp     => &[Extension("csproj"), Extension("sln")],
            Self::TypeScript => &[Filename("tsconfig.json"), Filename("package.json")],
            Self::Rust       => &[Filename("Cargo.toml")],
            Self::Go         => &[Filename("go.mod"), Filename("go.sum")],
            Self::Python     => &[Filename("pyproject.toml"), Filename("requirements.txt")],
        }
    }
}
```

The watcher unions `extensions()` and `project_files()` across all
`Language` variants. An event survives if its path extension is in the
extension union, OR a `project_files()` entry matches: `Extension(e)`
matches `path.extension() == e`; `Filename(n)` matches
`path.file_name() == n`.

The watcher unions extensions across all `Language` variants. (Once
per-language enable flags are honored, this can be narrowed to the
configured set; for now, watching all of them is harmless because the
trigger is staleness-gated anyway.)

### D3b. Shared skip-dir constant

`staleness.rs` already has `FINGERPRINT_SKIP_DIRS = ["node_modules",
"target", "bin", "obj", ".git", ".kenn"]`. The watcher needs the same
set, and the two must stay in sync — a file the fingerprint walk
ignores must also be a file the watcher ignores, or the watcher will
trigger a reindex that the staleness key immediately skips.

Promote the constant to a shared `WORKSPACE_SKIP_DIRS` in
`kenn-store` (or `kenn-config`), and have both `staleness.rs` and the
watcher import it. The watcher applies it *in addition to* the user's
`[exclude] globs`, not as a substitute — a first-run user with no
`kenn.toml` still gets `target/` ignored.

### D4. Debounce: tokio `sleep_until`, no `Clock` trait

Each surviving event resets a deadline of `mcp.watch_debounce_ms`
(default 30 s) into the future. The debounce task uses
`tokio::time::sleep_until(deadline)`; when it elapses with no further
events, it fires once and calls `spawn_background_reindex`.

Tests use `#[tokio::test(start_paused = true)]` + `tokio::time::advance`
to step the virtual clock — no abstraction, no injection, no extra
crate. `tokio = { workspace = true, features = ["test-util"] }` in
`[dev-dependencies]` enables the feature only for test builds.

### D5. Trigger: `spawn_background_reindex`, not a CLI subprocess

At debounce expiry the watcher calls the existing
`spawn_background_reindex` in-process. The staleness key and the
one-writer flock still gate real work; the watcher never bypasses
either gate. If a background reindex is already running, the call is
a no-op (the existing tool path handles this).

**Redundant triggers are acceptable.** Race: an agent's `reindex`
tool call and a watcher debounce expiry can both fire in the same
window. Whichever loses the race meets either an in-progress reindex
(no-op) or a just-completed snapshot whose key matches (skipped by
the staleness check). No data corruption, no extra work past one
staleness-key comparison. The watcher does not attempt to coordinate
with other reindex sources.

### D6. Agent surface: two idempotent tools + status field

- `watch_start` — start the watcher; idempotent. Returns:

  ```rust
  #[derive(Serialize, schemars::JsonSchema)]
  struct WatchStartResult {
      /// True if this call started a new watcher.
      /// False if a watcher was already running and this call was a no-op.
      started: bool,
      /// Debounce window in milliseconds (from `mcp.watch_debounce_ms`).
      debounce_ms: u64,
  }
  ```

  `started` lets a defensive agent (one that calls `watch_start` on
  every connect) tell "I just brought it up" from "it was already
  up." `debounce_ms` echoes the effective config so the agent can
  surface the cadence without an extra `get_index_status` call. No
  `session_id`: there is one server and one watcher; `watch_stop`
  takes no arg.

- `watch_stop` — abort the debounce task, drop the `notify` watcher.

- `get_index_status` — gains a `watcher` field with a `WatcherState`
  enum (snake_case on the wire):

  ```rust
  #[derive(Serialize, schemars::JsonSchema)]
  #[serde(rename_all = "snake_case")]
  enum WatcherState {
      /// Not running.
      Off,
      /// Running, no event has landed inside the debounce window.
      Idle,
      /// Running, an event has landed and a trigger is scheduled.
      Debouncing,
  }
  ```

  The field is always present (no `Option`) so the agent's MCP client
  sees an exhaustive enum constraint in the JSON Schema, not an
  optional string.

`watch_stop` followed by `watch_start` is the supported way to
"restart" the watcher; there is no implicit auto-restart even when
`watch_on = true` (boot-time default is not a supervisor — explicit
stop wins).

### D6a. Lifecycle-state preconditions

`watch_start` is only meaningful when the server is `Ready` — there
is no served snapshot to keep fresh in `Indexing`, and `Failed` means
no snapshot at all. The tool SHALL error when invoked outside
`Ready`. The agent's recovery is to poll `get_index_status` until
the state changes, then retry `watch_start`. (Queuing a deferred
start across state transitions would add a state machine for no
real benefit — the agent already polls.)

`watch_stop` SHALL be permitted in any state (idempotent: stopping a
not-running watcher is a no-op success).

### D6b. `notify` initialization failure

Creating a `notify::RecommendedWatcher` can fail (permission denied,
inotify handle limit, unsupported platform). Two surfaces:

- **`watch_start` call:** returns an MCP error with the underlying
  message. The watcher state remains `Off`. The agent can surface
  the error to the user and decide whether to retry.

- **Boot-time `watch_on = true`:** the server SHALL log a `warn!`
  with the error and proceed to `Ready` without the watcher.
  Bringing down a working MCP server because an optional convenience
  failed would be the wrong tradeoff. `get_index_status` will report
  `watcher: "off"` and the agent (or user via `watch_start`) can
  attempt to start it later.

### D7. Code-update notification on every reader swap

The `ArcSwap` reader swap (already implemented in `poll_once`) is the
single moment where the served snapshot changes. On every successful
swap, the poll task SHALL emit `notifications/message`:

```json
{ "level": "info",
  "data": { "event": "code_updated",
            "message": "Code updated at 2026-05-23T14:23:05Z" } }
```

The event name is consumer-oriented: from the agent's perspective the
relevant fact is that the indexed code it queries against has been
updated. (The internal mechanism — a snapshot swap on an `ArcSwap` —
is implementation noise to the agent.) `snapshot_id` is intentionally
omitted: it's an opaque 12-char hex id with no agent-side use.

Edge case: this fires whenever `live` advances, including a manual
`reindex` on unchanged code (the staleness gate runs on cold-start
only; the background reindex tool always writes a new snapshot). In
that case "code updated" is slightly misleading but harmless — the
agent re-queries and sees the same answers.

This converges all reindex sources — watcher trigger, agent's
`reindex` tool, external `kenn index`, another MCP instance's reindex —
onto a single notification kind. The agent doesn't need to know who
caused the refresh; it just needs to know to re-query.

Reuses the existing `spawn_notification_pump` channel and the existing
peer-gone handling (`debug!` log when no client is connected).

**Notification latency.** The notification fires from `poll_once`,
which ticks roughly every 3 s. So a swap notification can lag the
actual reindex completion by up to one poll interval. This is fine
for "data is fresh" semantics — agents query on demand, not in
real-time response to notifications — but is documented here so
nobody reads `notifications/message` as a real-time signal.

### D8. Config moves to `[mcp]`

The file-watcher is no longer a *staleness* signal — it is an MCP
feature that calls the staleness machinery. The config moves
accordingly. The starter `kenn.toml` written by `kenn init` SHALL
include the following block verbatim:

```toml
[mcp]
# Start the in-process file watcher at server boot. When false, agents
# can start it on demand via the `watch_start` tool.
watch_on = false

# Debounce window for the file watcher: collapse edit bursts within
# this many milliseconds of inactivity into a single reindex trigger.
watch_debounce_ms = 30000
```

The legacy `staleness.file_watcher` and
`staleness.file_watcher_debounce_ms` fields are removed (no production
users — the wiring never shipped). This is a one-line break for any
hand-written `kenn.toml` that set them; the migration is renaming the
section.

## Risks / Trade-offs

- **[Risk] Rebase or branch-switch rewrites many files fast.** → The
  debounce collapses the burst; the staleness key is the final gate
  before any real cost.
- **[Risk] `notify` semantics differ across platforms (FSEvents
  coalescing vs inotify per-event).** → Debouncing absorbs most of
  the difference; filtering is path-based, not event-kind-based.
- **[Risk] An agent that never calls `watch_start` and a user who
  doesn't set `watch_on = true` sees no improvement.** → Documented;
  defensive `watch_start` on session connect is the recommended client
  pattern.
- **[Trade-off] `notify` is an unconditional dep, not feature-gated.** →
  Simpler build matrix and uniform binary; the runtime cost when the
  watcher is off is the cost of an unused dep at link time.

## Open Questions

(None outstanding — all four questions from the prior iteration were
resolved in favour of the MCP-tool design.)
