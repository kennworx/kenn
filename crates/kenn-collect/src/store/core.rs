use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::schema::SCHEMA;

/// How long a writer waits on a locked DB before giving up (`SQLITE_BUSY`).
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum number of parent directories to walk when deriving the project from
/// a `.git` directory.
const PROJECT_WALK_LIMIT: usize = 32;

/// One parsed output destination of a Bash command (redirect or `tee` arg).
/// Produced by [`crate::parser`]; declared here because the store is the only
/// consumer.
#[derive(Debug, Clone)]
pub struct Output {
    pub path: String,
    pub kind: OutputKind,
    pub fd: Option<i32>,
    pub op: Option<String>,
    pub resolved: bool,
    pub literal_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Redirect,
    Tee,
}

impl From<OutputKind> for FileChannel {
    fn from(k: OutputKind) -> Self {
        match k {
            OutputKind::Redirect => FileChannel::Redirect,
            OutputKind::Tee => FileChannel::Tee,
        }
    }
}

/// The `channel` of a `files` row: how the agent touched the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChannel {
    Edit,
    Write,
    Redirect,
    Tee,
}

impl FileChannel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Write => "write",
            Self::Redirect => "redirect",
            Self::Tee => "tee",
        }
    }
}

/// The live status of a session's main agent, inferred from lifecycle hooks
/// (design D1): `Working` at `UserPromptSubmit`, `Idle` at `Stop`, and
/// `NeedsInput` / `NeedsPermission` at `Notification`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Working,
    Idle,
    NeedsInput,
    NeedsPermission,
}

impl AgentStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Idle => "idle",
            Self::NeedsInput => "needs_input",
            Self::NeedsPermission => "needs_permission",
        }
    }
}

/// Session metadata captured at `SessionStart` (design D1, D2). Each field is
/// `None` when the payload or the hook environment doesn't provide it (e.g. the
/// tmux fields are `None` when the session is not running inside tmux).
#[derive(Debug, Clone, Default)]
pub struct SessionMeta {
    /// The `SessionStart` start reason: `startup` / `resume` / `clear` / `compact`.
    pub source: Option<String>,
    /// Path to the session's conversation transcript JSONL.
    pub transcript_path: Option<String>,
    /// The OS `$USER` the session runs as.
    pub os_user: Option<String>,
    /// The tmux pane id (`$TMUX_PANE`, e.g. `%5`) — a `tmux switch-client` target.
    pub tmux_pane: Option<String>,
    /// The tmux socket path (the field of `$TMUX` before the first `,`).
    pub tmux_socket: Option<String>,
}

/// A handle to the collector database.
pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    /// Open (or create) the store at `<state_dir>/collector.db`. The state dir
    /// is [`collector_state_dir`] (`KENN_STATE_DIR` override → per-OS state
    /// dir).
    pub fn open() -> Result<Self> {
        let dir = collector_state_dir()?;
        ensure_dir(&dir)?;
        Self::open_at(&dir.join("collector.db"))
    }

    /// Open (or create) the store at an explicit path. Used by tests and by
    /// callers that have already resolved the state dir.
    pub fn open_at(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening sqlite at {}", path.display()))?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // WAL must be set before schema init so the first transaction lands in
        // WAL.
        let mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            anyhow::bail!("failed to enable WAL mode (got {mode})");
        }
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA)?;
        Ok(())
    }

    /// Insert a new session row, or update `last_seen_at` and clear `ended_at`
    /// if the id already exists. `project` is derived from `cwd`.
    pub fn upsert_session(&self, id: &str, cwd: &str, now: i64) -> Result<()> {
        let project = derive_project(Path::new(cwd));
        self.conn.execute(
            "INSERT INTO sessions (id, project, cwd, started_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 last_seen_at = ?4,
                 ended_at     = NULL",
            params![id, project, cwd, now],
        )?;
        Ok(())
    }

    /// Insert a session row with its `SessionStart` metadata, or backfill an
    /// existing one (design D3). `project` is derived from `cwd`. On conflict it
    /// bumps `last_seen_at`, clears `ended_at`, and `COALESCE`-fills each
    /// metadata column — so a row a prior `upsert_session` created with NULLs is
    /// enriched, and a repeated `SessionStart` never overwrites a populated
    /// field with NULL (existing value wins).
    pub fn start_session(&self, id: &str, cwd: &str, meta: &SessionMeta, now: i64) -> Result<()> {
        let project = derive_project(Path::new(cwd));
        self.conn.execute(
            "INSERT INTO sessions
                (id, project, cwd, source, transcript_path, os_user, tmux_pane,
                 tmux_socket, started_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT(id) DO UPDATE SET
                 last_seen_at    = ?9,
                 ended_at        = NULL,
                 source          = COALESCE(source, excluded.source),
                 transcript_path = COALESCE(transcript_path, excluded.transcript_path),
                 os_user         = COALESCE(os_user, excluded.os_user),
                 tmux_pane       = COALESCE(tmux_pane, excluded.tmux_pane),
                 tmux_socket     = COALESCE(tmux_socket, excluded.tmux_socket)",
            params![
                id,
                project,
                cwd,
                meta.source,
                meta.transcript_path,
                meta.os_user,
                meta.tmux_pane,
                meta.tmux_socket,
                now,
            ],
        )?;
        Ok(())
    }

    /// Record the last user prompt for a session and bump `last_seen_at`.
    pub fn set_last_prompt(&self, session_id: &str, prompt: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions
                SET last_prompt = ?2, last_seen_at = ?3
              WHERE id = ?1",
            params![session_id, prompt, now],
        )?;
        Ok(())
    }

    /// Set the session's current agent status (design D3): append a
    /// `session_status` transition row AND stamp the live `status` / `status_at`
    /// columns on `sessions`. `detail` carries the raw `Notification` message
    /// when present. Bumps `last_seen_at`.
    pub fn set_status(
        &self,
        session_id: &str,
        status: AgentStatus,
        detail: Option<&str>,
        now: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO session_status (session_id, status, detail, t)
             VALUES (?1, ?2, ?3, ?4)",
            params![session_id, status.as_str(), detail, now],
        )?;
        self.conn.execute(
            "UPDATE sessions
                SET status = ?2, status_at = ?3, last_seen_at = ?3
              WHERE id = ?1",
            params![session_id, status.as_str(), now],
        )?;
        Ok(())
    }

    /// Adjust the session's active-subagent count by `delta` (design D2),
    /// clamped at 0 so a missed spawn hook can't drive it negative. Bumps
    /// `last_seen_at`.
    pub fn bump_subagents(&self, session_id: &str, delta: i64, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions
                SET active_subagents = MAX(0, active_subagents + ?2),
                    last_seen_at = ?3
              WHERE id = ?1",
            params![session_id, delta, now],
        )?;
        Ok(())
    }

    /// Mark a session ended (sets `ended_at` and bumps `last_seen_at`).
    pub fn end_session(&self, session_id: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions
                SET ended_at = ?2, last_seen_at = ?2
              WHERE id = ?1",
            params![session_id, now],
        )?;
        Ok(())
    }

    /// Insert a new command row (`PreToolUse`): `started_at` set, `finished_at`
    /// NULL. Returns the new `id`.
    pub fn insert_command(
        &self,
        session_id: &str,
        tool_use_id: Option<&str>,
        cmd_text: &str,
        signature: Option<&str>,
        cwd: &str,
        now: i64,
    ) -> Result<i64> {
        let branch = derive_branch(Path::new(cwd));
        self.conn.execute(
            "INSERT INTO commands
                (session_id, tool_use_id, cmd_text, signature, cwd, branch, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                session_id,
                tool_use_id,
                cmd_text,
                signature,
                cwd,
                branch,
                now
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Mark the command with this `tool_use_id` as finished (`PostToolUse`).
    pub fn finish_command(&self, tool_use_id: &str, exit_code: i32, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE commands
                SET finished_at = ?2, exit_code = ?3
              WHERE tool_use_id = ?1",
            params![tool_use_id, now, exit_code],
        )?;
        Ok(())
    }

    /// Insert a single `files` row. Used for both Edit/Write touches
    /// (`command_id` NULL) and Bash outputs (`command_id` set). `project` is
    /// derived from `cwd`.
    #[expect(
        clippy::too_many_arguments,
        reason = "file rows have many independent columns; bundling into a struct just to satisfy clippy hurts the single call site"
    )]
    pub fn insert_file(
        &self,
        session_id: &str,
        command_id: Option<i64>,
        cwd: &str,
        path: &str,
        channel: FileChannel,
        op: Option<&str>,
        resolved: bool,
        now: i64,
    ) -> Result<()> {
        let project = derive_project(Path::new(cwd));
        let branch = derive_branch(Path::new(cwd));
        self.conn.execute(
            "INSERT INTO files
                (session_id, command_id, project, branch, path, channel, op, resolved, t)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session_id,
                command_id,
                project,
                branch,
                path,
                channel.as_str(),
                op,
                i32::from(resolved),
                now,
            ],
        )?;
        Ok(())
    }
}

/// Resolve the collector's state directory via `kenn_server::paths::state_dir`
/// (which honors the `KENN_STATE_DIR` override used by hermetic tests). Both
/// the `collector.db` and the `cc-hook.log` diagnostic file live here.
pub fn collector_state_dir() -> Result<std::path::PathBuf> {
    kenn_server::paths::state_dir().context("resolving state dir")
}

/// Create `dir` (and parents) with mode 0700 on Unix. Idempotent.
fn ensure_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        return Ok(());
    }
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    Ok(())
}

/// Walk up from `cwd` looking for a `.git` entry. The path of the directory
/// containing `.git` is the project. Falls back to `cwd` itself.
fn derive_project(cwd: &Path) -> String {
    let mut cur: Option<&Path> = Some(cwd);
    for _ in 0..PROJECT_WALK_LIMIT {
        let Some(p) = cur else { break };
        if p.join(".git").exists() {
            return p.to_string_lossy().into_owned();
        }
        cur = p.parent();
    }
    cwd.to_string_lossy().into_owned()
}

/// The git branch in effect at `cwd`, or `None` outside a repo / on an
/// unreadable `HEAD`. Reads `<gitdir>/HEAD` directly — no `git` subprocess, so
/// it stays within the hook latency budget (design D2). `ref: refs/heads/<name>`
/// yields `<name>`; a detached HEAD (raw SHA) yields its short form.
fn derive_branch(cwd: &Path) -> Option<String> {
    let gitdir = find_gitdir(cwd)?;
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref: ") {
        let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        return (!name.is_empty()).then(|| name.to_owned());
    }
    // Detached HEAD: a raw object id. Surface a short form.
    let is_sha = head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit());
    is_sha.then(|| head.chars().take(12).collect())
}

/// Resolve the git directory for `cwd`: walk up for a `.git` entry, following
/// the `gitdir: <path>` pointer when `.git` is a file (a linked worktree).
fn find_gitdir(cwd: &Path) -> Option<std::path::PathBuf> {
    let mut cur: Option<&Path> = Some(cwd);
    for _ in 0..PROJECT_WALK_LIMIT {
        let p = cur?;
        let dot_git = p.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if dot_git.is_file() {
            let contents = std::fs::read_to_string(&dot_git).ok()?;
            let target = contents.trim().strip_prefix("gitdir:")?.trim();
            let target = Path::new(target);
            return Some(if target.is_absolute() {
                target.to_path_buf()
            } else {
                p.join(target)
            });
        }
        cur = p.parent();
    }
    None
}

#[cfg(test)]
mod tests;
