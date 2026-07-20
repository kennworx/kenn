//! Hot-path latency benchmark for the collector (task 5.3 / design D8).
//!
//! Measures the in-process work a single `PreToolUse(Bash)` hook does — open
//! the collector DB, parse the command AST, insert the command + its output
//! files — per invocation, against a *growing* DB (each iteration opens fresh
//! and inserts, exactly as a real short-lived hook process does). Process spawn
//! itself is fixed overhead the prior JSONL hook also paid and is out of scope.
//!
//! `#[ignore]` so it never runs in the normal suite; record numbers with:
//!   cargo test -p kenn-collect --test latency -- --ignored --nocapture

use std::path::Path;
use std::time::Instant;

use kenn_collect::store::{FileChannel, Store};

#[test]
#[ignore = "latency benchmark; run explicitly with --ignored --nocapture"]
fn pretool_bash_hot_path_p95() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("collector.db");
    let base = dir.path();
    let cmd = "cargo test --workspace 2>&1 | tee ./tmp/test.log";

    // Warm-up: prime the page cache, the SQLite file, and the schema.
    for _ in 0..50 {
        one_hook(&db, base, cmd);
    }

    let iters = 2000;
    let mut samples_us: Vec<u128> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        one_hook(&db, base, cmd);
        samples_us.push(t.elapsed().as_micros());
    }
    samples_us.sort_unstable();
    // Integer percentile (per-mille) — no float casts.
    let p = |permille: usize| {
        samples_us[(samples_us.len() * permille / 1000).min(samples_us.len() - 1)]
    };
    let p50 = p(500);
    let p95 = p(950);
    let p99 = p(990);
    let max = *samples_us.last().unwrap();
    println!(
        "pretool-bash hot path over {iters} iters: p50={p50}us p95={p95}us p99={p99}us max={max}us"
    );

    // Budget: design D8 says the added parse+insert work is sub-millisecond,
    // well within the ≤5ms hook budget. Gate the controllable work at 5ms p95.
    assert!(
        p95 < 5_000,
        "p95 {p95}us exceeds the 5ms hook budget for parse+open+insert"
    );
}

/// One hook's worth of work: open fresh, parse, insert the command + its files.
fn one_hook(db: &Path, base: &Path, cmd: &str) {
    let store = Store::open_at(db).expect("open");
    store
        .upsert_session("bench-sess", &base.display().to_string(), 0)
        .expect("session");
    let parsed = kenn_collect::parse(cmd, base).expect("parse");
    let cid = store
        .insert_command(
            "bench-sess",
            Some("tu-bench"),
            cmd,
            parsed.signature.as_deref(),
            &base.display().to_string(),
            0,
        )
        .expect("insert_command");
    for o in &parsed.outputs {
        let channel: FileChannel = o.kind.into();
        store
            .insert_file(
                "bench-sess",
                Some(cid),
                &base.display().to_string(),
                &o.path,
                channel,
                o.op.as_deref(),
                o.resolved,
                0,
            )
            .expect("insert_file");
    }
}
