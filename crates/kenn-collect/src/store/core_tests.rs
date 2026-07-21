use std::sync::Arc;
use std::sync::Barrier;
use std::thread;

use tempfile::tempdir;

use super::*;

fn fresh() -> (tempfile::TempDir, Store) {
    let dir = tempdir().expect("tempdir");
    let store = Store::open_at(&dir.path().join("collector.db")).expect("open");
    (dir, store)
}

/// Write a `.git/HEAD` under `dir` with the given contents, returning `dir`.
fn write_head(dir: &Path, head: &str) {
    let git = dir.join(".git");
    std::fs::create_dir_all(&git).expect("mkdir .git");
    std::fs::write(git.join("HEAD"), head).expect("write HEAD");
}

#[test]
fn derive_branch_reads_head_ref() {
    let dir = tempdir().expect("tempdir");
    write_head(dir.path(), "ref: refs/heads/feature/x\n");
    assert_eq!(
        derive_branch(dir.path()).as_deref(),
        Some("feature/x"),
        "branch comes from refs/heads/<name>"
    );
}

#[test]
fn derive_branch_detached_head_is_short_sha() {
    let dir = tempdir().expect("tempdir");
    write_head(dir.path(), "0123456789abcdef0123456789abcdef01234567\n");
    assert_eq!(derive_branch(dir.path()).as_deref(), Some("0123456789ab"));
}

#[test]
fn derive_branch_none_outside_git() {
    let dir = tempdir().expect("tempdir");
    assert_eq!(derive_branch(dir.path()), None);
}

#[test]
fn derive_branch_follows_worktree_gitdir_file() {
    // Linked worktree: `.git` is a file pointing at the real gitdir, where
    // HEAD lives.
    let dir = tempdir().expect("tempdir");
    let real_gitdir = dir.path().join("realgit");
    std::fs::create_dir_all(&real_gitdir).expect("mkdir gitdir");
    std::fs::write(real_gitdir.join("HEAD"), "ref: refs/heads/wt\n").expect("HEAD");
    std::fs::write(
        dir.path().join(".git"),
        format!("gitdir: {}\n", real_gitdir.display()),
    )
    .expect("write .git file");
    assert_eq!(derive_branch(dir.path()).as_deref(), Some("wt"));
}

#[test]
fn insert_command_and_file_record_the_branch() {
    let (gitdir, store) = fresh();
    // Put the working dir in a repo on branch `main`.
    write_head(gitdir.path(), "ref: refs/heads/main\n");
    let cwd = gitdir.path().to_string_lossy().into_owned();
    store.upsert_session("s", &cwd, 100).expect("sess");
    let cid = store
        .insert_command("s", Some("t1"), "ls > out.log", Some("ls"), &cwd, 110)
        .expect("cmd");
    store
        .insert_file(
            "s",
            Some(cid),
            &cwd,
            "out.log",
            FileChannel::Redirect,
            Some(">"),
            true,
            110,
        )
        .expect("file");
    let cmd_branch: Option<String> = store
        .conn
        .query_row("SELECT branch FROM commands WHERE id = ?1", [cid], |r| {
            r.get(0)
        })
        .expect("row");
    let file_branch: Option<String> = store
        .conn
        .query_row(
            "SELECT branch FROM files WHERE command_id = ?1",
            [cid],
            |r| r.get(0),
        )
        .expect("row");
    assert_eq!(cmd_branch.as_deref(), Some("main"));
    assert_eq!(file_branch.as_deref(), Some("main"));
}

/// `(source, transcript_path, os_user, tmux_pane, tmux_socket)`.
type SessionMetaRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn read_session_meta(store: &Store, id: &str) -> SessionMetaRow {
    store
        .conn
        .query_row(
            "SELECT source, transcript_path, os_user, tmux_pane, tmux_socket
                   FROM sessions WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .expect("row")
}

#[test]
fn start_session_records_all_metadata() {
    let (_dir, store) = fresh();
    let meta = SessionMeta {
        source: Some("resume".into()),
        transcript_path: Some("/t/conv.jsonl".into()),
        os_user: Some("ada".into()),
        tmux_pane: Some("%5".into()),
        tmux_socket: Some("/tmp/tmux-1000/default".into()),
    };
    store.start_session("s", "/cwd", &meta, 100).expect("start");
    let (source, transcript, user, pane, socket) = read_session_meta(&store, "s");
    assert_eq!(source.as_deref(), Some("resume"));
    assert_eq!(transcript.as_deref(), Some("/t/conv.jsonl"));
    assert_eq!(user.as_deref(), Some("ada"));
    assert_eq!(pane.as_deref(), Some("%5"));
    assert_eq!(socket.as_deref(), Some("/tmp/tmux-1000/default"));
}

#[test]
fn start_session_backfills_then_does_not_clobber() {
    let (_dir, store) = fresh();
    // A non-start hook created the row first → metadata NULL.
    store.upsert_session("s", "/cwd", 100).expect("ensure");
    assert_eq!(read_session_meta(&store, "s").0, None);

    // SessionStart backfills.
    let meta = SessionMeta {
        source: Some("startup".into()),
        tmux_pane: Some("%7".into()),
        ..SessionMeta::default()
    };
    store.start_session("s", "/cwd", &meta, 110).expect("start");
    let after = read_session_meta(&store, "s");
    assert_eq!(after.0.as_deref(), Some("startup"));
    assert_eq!(after.3.as_deref(), Some("%7"));

    // A second SessionStart with empty metadata must NOT null out the
    // populated fields (COALESCE keeps the existing value).
    store
        .start_session("s", "/cwd", &SessionMeta::default(), 120)
        .expect("start again");
    let final_meta = read_session_meta(&store, "s");
    assert_eq!(final_meta.0.as_deref(), Some("startup"), "source preserved");
    assert_eq!(final_meta.3.as_deref(), Some("%7"), "tmux_pane preserved");
    let last_seen: i64 = store
        .conn
        .query_row(
            "SELECT last_seen_at FROM sessions WHERE id = 's'",
            [],
            |r| r.get(0),
        )
        .expect("row");
    assert_eq!(last_seen, 120, "last_seen_at still advances");
}

#[test]
fn opens_in_wal_mode() {
    let (_dir, store) = fresh();
    let mode: String = store
        .conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .expect("pragma");
    assert_eq!(mode.to_lowercase(), "wal");
}

#[test]
fn schema_is_idempotent() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("collector.db");
    let s1 = Store::open_at(&path).expect("open1");
    drop(s1);
    let _s2 = Store::open_at(&path).expect("open2");
}

#[test]
fn upsert_session_inserts_then_updates() {
    let (_dir, store) = fresh();
    store.upsert_session("s1", "/tmp/x", 100).expect("ins");
    let (started, last, ended): (i64, i64, Option<i64>) = store
        .conn
        .query_row(
            "SELECT started_at, last_seen_at, ended_at FROM sessions WHERE id = 's1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("row");
    assert_eq!(started, 100);
    assert_eq!(last, 100);
    assert!(ended.is_none());

    store.end_session("s1", 150).expect("end");
    store.upsert_session("s1", "/tmp/x", 200).expect("re-enter");
    let (started2, last2, ended2): (i64, i64, Option<i64>) = store
        .conn
        .query_row(
            "SELECT started_at, last_seen_at, ended_at FROM sessions WHERE id = 's1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("row");
    assert_eq!(started2, 100, "started_at must not change on re-entry");
    assert_eq!(last2, 200);
    assert!(ended2.is_none(), "re-entry clears ended_at");
}

#[test]
fn insert_command_then_finish_transitions_running_state() {
    let (_dir, store) = fresh();
    store.upsert_session("s1", "/tmp/x", 100).expect("sess");
    let id = store
        .insert_command("s1", Some("tu1"), "ls", Some("ls"), "/tmp/x", 110)
        .expect("ins");
    assert!(id > 0);

    // Running: finished_at IS NULL.
    let finished: Option<i64> = store
        .conn
        .query_row(
            "SELECT finished_at FROM commands WHERE tool_use_id = 'tu1'",
            [],
            |r| r.get(0),
        )
        .expect("row");
    assert!(finished.is_none());

    store.finish_command("tu1", 0, 120).expect("finish");
    let (finished2, exit): (Option<i64>, Option<i64>) = store
        .conn
        .query_row(
            "SELECT finished_at, exit_code FROM commands WHERE tool_use_id = 'tu1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert_eq!(finished2, Some(120));
    assert_eq!(exit, Some(0));
}

#[test]
fn insert_file_for_bash_output_sets_command_id() {
    let (_dir, store) = fresh();
    store.upsert_session("s1", "/tmp/x", 100).expect("sess");
    let cid = store
        .insert_command("s1", Some("tu1"), "x > a.log", Some("x"), "/tmp/x", 110)
        .expect("cmd");
    store
        .insert_file(
            "s1",
            Some(cid),
            "/tmp/x",
            "/tmp/x/a.log",
            FileChannel::Redirect,
            Some(">"),
            true,
            111,
        )
        .expect("file");
    let (command_id, channel, op, resolved): (Option<i64>, String, Option<String>, i64) = store
        .conn
        .query_row(
            "SELECT command_id, channel, op, resolved FROM files WHERE path = '/tmp/x/a.log'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .expect("row");
    assert_eq!(command_id, Some(cid));
    assert_eq!(channel, "redirect");
    assert_eq!(op.as_deref(), Some(">"));
    assert_eq!(resolved, 1);
}

#[test]
fn insert_file_for_edit_touch_has_null_command_id() {
    let (_dir, store) = fresh();
    store.upsert_session("s1", "/tmp/x", 100).expect("sess");
    store
        .insert_file(
            "s1",
            None,
            "/tmp/x",
            "/abs/foo.rs",
            FileChannel::Edit,
            None,
            true,
            111,
        )
        .expect("file");
    let (command_id, channel): (Option<i64>, String) = store
        .conn
        .query_row(
            "SELECT command_id, channel FROM files WHERE path = '/abs/foo.rs'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert!(command_id.is_none());
    assert_eq!(channel, "edit");
}

#[test]
fn set_last_prompt_and_end_session() {
    let (_dir, store) = fresh();
    store.upsert_session("s1", "/tmp/x", 100).expect("sess");
    store.set_last_prompt("s1", "do the thing", 150).expect("p");
    let (prompt, last): (String, i64) = store
        .conn
        .query_row(
            "SELECT last_prompt, last_seen_at FROM sessions WHERE id = 's1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert_eq!(prompt, "do the thing");
    assert_eq!(last, 150);

    store.end_session("s1", 200).expect("end");
    let ended: Option<i64> = store
        .conn
        .query_row("SELECT ended_at FROM sessions WHERE id = 's1'", [], |r| {
            r.get(0)
        })
        .expect("row");
    assert_eq!(ended, Some(200));
}

#[test]
fn derive_project_uses_git_root_path() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path().join("myproj");
    std::fs::create_dir_all(root.join(".git")).expect("mkdir .git");
    let sub = root.join("crates").join("inner");
    std::fs::create_dir_all(&sub).expect("mkdir sub");
    assert_eq!(derive_project(&sub), root.to_string_lossy());
}

#[test]
fn derive_project_falls_back_to_cwd() {
    let dir = tempdir().expect("tempdir");
    let leaf = dir.path().join("lonely");
    std::fs::create_dir_all(&leaf).expect("mkdir");
    assert_eq!(derive_project(&leaf), leaf.to_string_lossy());
}

#[test]
fn set_status_logs_transition_and_updates_session() {
    let (_dir, store) = fresh();
    store.upsert_session("s1", "/tmp/x", 100).expect("sess");
    store
        .set_status("s1", AgentStatus::NeedsPermission, Some("grant?"), 150)
        .expect("set_status");

    // The session row carries the live status.
    let (status, status_at): (String, i64) = store
        .conn
        .query_row(
            "SELECT status, status_at FROM sessions WHERE id = 's1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("session row");
    assert_eq!(status, "needs_permission");
    assert_eq!(status_at, 150);

    // The transition log has a matching row with the detail.
    let (log_status, detail, t): (String, Option<String>, i64) = store
        .conn
        .query_row(
            "SELECT status, detail, t FROM session_status WHERE session_id = 's1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("session_status row");
    assert_eq!(log_status, "needs_permission");
    assert_eq!(detail.as_deref(), Some("grant?"));
    assert_eq!(t, 150);
}

#[test]
fn bump_subagents_increments_decrements_and_clamps_at_zero() {
    let (_dir, store) = fresh();
    store.upsert_session("s1", "/tmp/x", 100).expect("sess");
    let count = |store: &Store| -> i64 {
        store
            .conn
            .query_row(
                "SELECT active_subagents FROM sessions WHERE id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("row")
    };

    assert_eq!(count(&store), 0, "defaults to 0");
    store.bump_subagents("s1", 1, 110).expect("bump +1");
    assert_eq!(count(&store), 1);
    store.bump_subagents("s1", -1, 120).expect("bump -1");
    assert_eq!(count(&store), 0, "+1 then -1 returns to 0");
    // An extra decrement from 0 must clamp, not go negative.
    store.bump_subagents("s1", -1, 130).expect("bump -1 again");
    assert_eq!(count(&store), 0, "clamps at 0");
}

#[test]
fn concurrent_writers_one_wal_db_do_not_error() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("collector.db");
    {
        let store = Store::open_at(&path).expect("open");
        store.upsert_session("a", "/a", 100).expect("a");
        store.upsert_session("b", "/b", 100).expect("b");
    }
    let n_per_thread: usize = 50;
    let barrier = Arc::new(Barrier::new(2));
    let path_a = path.clone();
    let path_b = path.clone();
    let ba = Arc::clone(&barrier);
    let bb = Arc::clone(&barrier);

    let ta = thread::spawn(move || {
        let store = Store::open_at(&path_a).expect("open a");
        ba.wait();
        for i in 0..n_per_thread {
            let tu = format!("a-{i}");
            store
                .insert_command("a", Some(&tu), "cmd a", Some("cmd"), "/a", 200)
                .expect("ins a");
        }
    });
    let tb = thread::spawn(move || {
        let store = Store::open_at(&path_b).expect("open b");
        bb.wait();
        for i in 0..n_per_thread {
            let tu = format!("b-{i}");
            store
                .insert_command("b", Some(&tu), "cmd b", Some("cmd"), "/b", 200)
                .expect("ins b");
        }
    });
    ta.join().expect("join a");
    tb.join().expect("join b");

    let store = Store::open_at(&path).expect("reopen");
    let total: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0))
        .expect("cnt");
    let expected = i64::try_from(n_per_thread * 2).expect("fits");
    assert_eq!(total, expected);
}
