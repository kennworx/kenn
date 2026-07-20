//! `SQLite` schema for the collector store (design §D5, §D6).
//!
//! `sessions → commands → files`, with `files.command_id` nullable so one
//! table holds both Bash outputs (`command_id` set) and Edit/Write touches
//! (`command_id` NULL). `project` is carried on every row; `commands`/`files`
//! also carry the event-time git `branch`. No `confirmed_at` / `size_bytes` —
//! the collector never reads the filesystem (§D6).

/// The full schema, applied idempotently on every open. `channel ∈
/// {edit, write, redirect, tee}`.
pub(crate) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS sessions (
        id               TEXT PRIMARY KEY,
        project          TEXT,
        cwd              TEXT NOT NULL,
        source           TEXT,
        transcript_path  TEXT,
        os_user          TEXT,
        tmux_pane        TEXT,
        tmux_socket      TEXT,
        started_at       INTEGER NOT NULL,
        last_seen_at     INTEGER NOT NULL,
        last_prompt      TEXT,
        status           TEXT,
        status_at        INTEGER,
        active_subagents INTEGER NOT NULL DEFAULT 0,
        ended_at         INTEGER
    );

    CREATE TABLE IF NOT EXISTS session_status (
        id            INTEGER PRIMARY KEY,
        session_id    TEXT NOT NULL REFERENCES sessions(id),
        status        TEXT NOT NULL,
        detail        TEXT,
        t             INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS session_status_session_time
        ON session_status(session_id, t);

    CREATE TABLE IF NOT EXISTS commands (
        id            INTEGER PRIMARY KEY,
        session_id    TEXT NOT NULL REFERENCES sessions(id),
        tool_use_id   TEXT,
        cmd_text      TEXT NOT NULL,
        signature     TEXT,
        cwd           TEXT NOT NULL,
        branch        TEXT,
        started_at    INTEGER NOT NULL,
        finished_at   INTEGER,
        exit_code     INTEGER
    );
    CREATE INDEX IF NOT EXISTS commands_session_time
        ON commands(session_id, started_at);
    CREATE INDEX IF NOT EXISTS commands_running
        ON commands(session_id) WHERE finished_at IS NULL;
    CREATE INDEX IF NOT EXISTS commands_tool_use
        ON commands(tool_use_id);

    CREATE TABLE IF NOT EXISTS files (
        id            INTEGER PRIMARY KEY,
        session_id    TEXT NOT NULL REFERENCES sessions(id),
        command_id    INTEGER REFERENCES commands(id),
        project       TEXT,
        branch        TEXT,
        path          TEXT NOT NULL,
        channel       TEXT NOT NULL,
        op            TEXT,
        resolved      INTEGER NOT NULL,
        t             INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS files_command ON files(command_id);
    CREATE INDEX IF NOT EXISTS files_session ON files(session_id);

    CREATE TABLE IF NOT EXISTS meta (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
";
