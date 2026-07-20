//! Retention / garbage collection (design §D9).
//!
//! 30-day retention with lazy GC: a hook triggers [`Store::maybe_lazy_gc`],
//! which runs the sweep at most once per 24h (tracked via `meta.last_gc_at`).
//! Ported from periClaude's GC against the `sessions → commands → files`
//! schema.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use crate::store::Store;

/// 30 days in seconds — the retention window for GC.
const RETENTION_SECS: i64 = 30 * 24 * 60 * 60;

/// 24 hours in seconds — the threshold for lazy GC.
const LAZY_GC_THRESHOLD_SECS: i64 = 24 * 60 * 60;

impl Store {
    /// Delete data older than 30 days. Running commands (and their files) are
    /// never pruned. Updates `meta.last_gc_at` to `now`.
    pub fn gc(&mut self, now: i64) -> Result<()> {
        let cutoff = now - RETENTION_SECS;
        let tx = self.conn.transaction()?;
        // Files attached to old finished commands cascade by command_id.
        tx.execute(
            "DELETE FROM files
              WHERE command_id IN (
                  SELECT id FROM commands
                   WHERE started_at < ?1 AND finished_at IS NOT NULL
              )",
            params![cutoff],
        )?;
        tx.execute(
            "DELETE FROM commands
              WHERE started_at < ?1 AND finished_at IS NOT NULL",
            params![cutoff],
        )?;
        // Command-less (Edit/Write) file rows older than cutoff.
        tx.execute(
            "DELETE FROM files
              WHERE command_id IS NULL AND t < ?1",
            params![cutoff],
        )?;
        // Sessions older than cutoff with no remaining commands or files.
        tx.execute(
            "DELETE FROM sessions
              WHERE last_seen_at < ?1
                AND id NOT IN (SELECT DISTINCT session_id FROM commands)
                AND id NOT IN (SELECT DISTINCT session_id FROM files)",
            params![cutoff],
        )?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('last_gc_at', ?1)
             ON CONFLICT(key) DO UPDATE SET value = ?1",
            params![now.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Run GC if `meta.last_gc_at` is missing or older than 24h. Returns
    /// `true` if GC ran.
    pub fn maybe_lazy_gc(&mut self, now: i64) -> Result<bool> {
        let last: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_gc_at'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let should_run = match last.as_deref().and_then(|s| s.parse::<i64>().ok()) {
            None => true,
            Some(ts) => now.saturating_sub(ts) > LAZY_GC_THRESHOLD_SECS,
        };
        if should_run {
            self.gc(now)?;
        }
        Ok(should_run)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::store::FileChannel;

    fn fresh() -> (tempfile::TempDir, Store) {
        let dir = tempdir().expect("tempdir");
        let store = Store::open_at(&dir.path().join("collector.db")).expect("open");
        (dir, store)
    }

    #[test]
    fn gc_prunes_old_finished_keeps_running() {
        let (_dir, mut store) = fresh();
        let now: i64 = 100 * 24 * 60 * 60; // day 100
        let old: i64 = now - 40 * 24 * 60 * 60; // 40 days ago
        let recent: i64 = now - 5 * 24 * 60 * 60; // 5 days ago

        store.upsert_session("old", "/o", old).expect("o");
        store.upsert_session("recent", "/r", recent).expect("r");

        // old finished command + its file — prune both.
        let cid_old_fin = store
            .insert_command("old", Some("a"), "x", Some("x"), "/o", old)
            .expect("a");
        store.finish_command("a", 0, old + 1).expect("fa");
        store
            .insert_file(
                "old",
                Some(cid_old_fin),
                "/o",
                "/o/x.log",
                FileChannel::Redirect,
                Some(">"),
                true,
                old,
            )
            .expect("file");

        // old running command — keep.
        let _cid_old_run = store
            .insert_command("old", Some("b"), "y", Some("y"), "/o", old)
            .expect("b");

        // recent command — keep.
        let _cid_recent = store
            .insert_command("recent", Some("c"), "z", Some("z"), "/r", recent)
            .expect("c");

        store.gc(now).expect("gc");

        let cmd_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
            .expect("cnt");
        assert_eq!(
            cmd_count, 2,
            "old finished pruned, old running + recent kept"
        );

        let file_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .expect("cnt");
        assert_eq!(file_count, 0, "file cascaded with old finished command");

        let last_gc: String = store
            .conn
            .query_row("SELECT value FROM meta WHERE key='last_gc_at'", [], |r| {
                r.get(0)
            })
            .expect("meta");
        assert_eq!(last_gc, now.to_string());
    }

    #[test]
    fn gc_prunes_empty_old_session() {
        let (_dir, mut store) = fresh();
        let now: i64 = 100 * 24 * 60 * 60;
        let old: i64 = now - 40 * 24 * 60 * 60;
        store.upsert_session("ghost", "/g", old).expect("g");
        store.gc(now).expect("gc");
        let n: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .expect("cnt");
        assert_eq!(n, 0);
    }

    #[test]
    fn gc_prunes_old_command_less_file() {
        let (_dir, mut store) = fresh();
        let now: i64 = 100 * 24 * 60 * 60;
        let old: i64 = now - 40 * 24 * 60 * 60;
        store.upsert_session("s", "/s", old).expect("s");
        store
            .insert_file(
                "s",
                None,
                "/s",
                "/s/edit.rs",
                FileChannel::Edit,
                None,
                true,
                old,
            )
            .expect("file");
        store.gc(now).expect("gc");
        let n: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .expect("cnt");
        assert_eq!(n, 0, "old command-less file pruned");
    }

    #[test]
    fn maybe_lazy_gc_runs_first_time_and_skips_when_fresh() {
        let (_dir, mut store) = fresh();
        let now: i64 = 1_000_000;
        assert!(store.maybe_lazy_gc(now).expect("first"));
        assert!(!store.maybe_lazy_gc(now + 60).expect("soon"));
        let later = now + LAZY_GC_THRESHOLD_SECS + 1;
        assert!(store.maybe_lazy_gc(later).expect("later"));
    }
}
