//! Pilot: validate `async-sqlite`'s `Pool` as the kenn-mcp read path, replacing
//! the synchronous `with_db` (`Connection::open_with_flags` + query on a tokio
//! worker thread). A `Pool` is N background-thread rusqlite `Client`s behind a
//! round-robin counter; `pool.conn(f).await` ships the closure to a worker
//! thread over a channel and awaits the reply — the blocking `SQLite` call never
//! runs on a runtime worker, so a tool can't stall one or blow the latency
//! budget (the `mcp-offload-blocking-storage` goal, achieved structurally
//! rather than via per-call `spawn_blocking`).
//!
//! Three risky unknowns from the design discussion, validated here against
//! throwaway DBs that mirror the real `vector.db` / `findings.db` schemas:
//!
//!   P1  vec0 (`sqlite-vec`) works through pool-opened connections. The
//!       extension is registered process-globally via `sqlite3_auto_extension`;
//!       this confirms it applies to connections opened on async-sqlite's
//!       background threads, not just the main thread.
//!   P2  `findings.db` (a separate-lifetime, WAL, append-only store) can be
//!       attached read-only to every pooled reader connection, queried across
//!       the attach boundary, and reflects a live writer's committed appends.
//!       Also probes the read-only-WAL gotcha (writer gone) + `immutable=1`.
//!   P3  Concurrent `pool.conn` calls actually parallelize across the worker
//!       connections — the whole point of moving off the serialized path.
//!
//! Run: `cargo run -p kenn-store --example async_sqlite_spike`

use std::path::Path;
use std::time::Instant;

use async_sqlite::{ClientBuilder, JournalMode, PoolBuilder};
use rusqlite::{params, Connection, OpenFlags};

const DIM: usize = 768;
const VEC_ROWS: usize = 2_000;
const POOL_CONNS: usize = 4;
const CTE_N: i64 = 2_000_000;

// ---- narrowing casts: spike-local, values are small and non-negative ----
#[expect(clippy::cast_possible_wrap, reason = "spike row counts are small")]
fn i64c(v: usize) -> i64 {
    v as i64
}
fn unit(bits: u64) -> f32 {
    let m = u16::try_from(bits & 0xFFFF).unwrap_or(0);
    f32::from(m) / f32::from(u16::MAX) - 0.5
}

fn register_vec() {
    #[expect(
        unsafe_code,
        clippy::missing_transmute_annotations,
        clippy::multiple_unsafe_ops_per_block,
        reason = "FFI registration copied from kenn-store's register.rs"
    )]
    // SAFETY: `sqlite3_vec_init` is a valid C-ABI extension entry point of the
    // shape `sqlite3_auto_extension` expects; registered before any vec0 open.
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }
}

/// Deterministic L2-normalized pseudo-vector for a seed (no `Math.random`).
fn embed_for(seed: usize) -> Vec<f32> {
    let mut s = (seed as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(1);
    let mut v = vec![0f32; DIM];
    for x in &mut v {
        s = s
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *x = unit(s >> 16);
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    for x in &mut v {
        *x /= norm;
    }
    v
}

fn vec_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Build a throwaway `vector.db` mirroring the real `knowledge` + `vec_knowledge`
/// schema (synchronously — this is the indexer's job, not the read path).
fn build_vector_db(path: &Path) -> anyhow::Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE knowledge(
           rowid INTEGER PRIMARY KEY, id INTEGER NOT NULL,
           pub_id TEXT NOT NULL, name_text TEXT NOT NULL);
         CREATE VIRTUAL TABLE vec_knowledge USING vec0(embedding float[768] distance_metric=cosine);",
    )?;
    let tx = conn.unchecked_transaction()?;
    for i in 1..=VEC_ROWS {
        tx.execute(
            "INSERT INTO knowledge(rowid,id,pub_id,name_text) VALUES(?,?,?,?)",
            params![
                i64c(i),
                i64c(i),
                format!("sym::{i}"),
                format!("symbol number {i}")
            ],
        )?;
        tx.execute(
            "INSERT INTO vec_knowledge(rowid, embedding) VALUES(?,?)",
            params![i64c(i), vec_bytes(&embed_for(i))],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Build a WAL `findings.db` mirroring the persistent findings index and return
/// the live writer `Client` (kept open: a separate-lifetime writer the readers
/// attach to read-only).
async fn build_findings_writer(path: &Path) -> anyhow::Result<async_sqlite::Client> {
    let writer = ClientBuilder::new()
        .path(path)
        .journal_mode(JournalMode::Wal)
        .open()
        .await?;
    writer
        .conn(|c| {
            c.execute_batch(
                "CREATE TABLE findings(
                   id TEXT PRIMARY KEY, text TEXT NOT NULL,
                   superseded INTEGER NOT NULL DEFAULT 0, tombstoned INTEGER NOT NULL DEFAULT 0);
                 CREATE VIRTUAL TABLE f USING fts5(id UNINDEXED, text);",
            )?;
            for (id, text) in [
                ("fnd_a", "cancel an order and refund the buyer"),
                ("fnd_b", "render the dashboard widget chart"),
                ("fnd_c", "retry the upload on transient failure"),
            ] {
                c.execute(
                    "INSERT INTO findings(id,text) VALUES(?,?)",
                    params![id, text],
                )?;
                c.execute("INSERT INTO f(id,text) VALUES(?,?)", params![id, text])?;
            }
            Ok(())
        })
        .await?;
    Ok(writer)
}

fn step(pass: bool, label: &str, detail: &str) {
    let tag = if pass { "PASS" } else { "FAIL" };
    println!("[{tag}] {label}\n       {detail}");
}

fn info(label: &str, detail: &str) {
    println!("[INFO] {label}\n       {detail}");
}

#[expect(clippy::too_many_lines, reason = "linear top-to-bottom pilot script")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    register_vec();
    let dir = tempfile::TempDir::new()?;
    let vector_db = dir.path().join("vector.db");
    let findings_db = dir.path().join("findings.db");

    build_vector_db(&vector_db)?;
    let writer = build_findings_writer(&findings_db).await?;
    println!(
        "built vector.db ({VEC_ROWS} rows, {DIM}-d) + findings.db (WAL, live writer)\n\
         opening read-only Pool: {POOL_CONNS} background-thread connections\n"
    );

    // Read-only pool over vector.db. URI flag lets ATTACH interpret `file:?mode=ro`.
    let pool = PoolBuilder::new()
        .path(&vector_db)
        .flags(OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)
        .num_conns(POOL_CONNS)
        .open()
        .await?;

    // ---- P1: vec0 KNN through a pool (background-thread) connection ----
    {
        let q = vec_bytes(&embed_for(7));
        let hits: Vec<(i64, f64)> = pool
            .conn(move |c| {
                let mut s = c.prepare(
                    "SELECT k.id, vk.distance
                       FROM vec_knowledge vk JOIN knowledge k ON k.rowid = vk.rowid
                      WHERE vk.embedding MATCH ?1 AND vk.k = ?2 ORDER BY distance",
                )?;
                let out: rusqlite::Result<Vec<(i64, f64)>> = s
                    .query_map(params![q, 5i64], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect();
                out
            })
            .await?;
        let nearest = hits.first().copied().unwrap_or((-1, -1.0));
        step(
            nearest.0 == 7 && hits.len() == 5,
            "P1 vec0 through pool connection",
            &format!(
                "auto-extension applied on a worker-thread conn; 5 KNN hits, nearest id={} dist={:.4} (self-match expected)",
                nearest.0, nearest.1
            ),
        );
    }

    // ---- P2: ATTACH findings.db read-only to every pooled connection ----
    let attach = format!(
        "ATTACH DATABASE 'file:{}?mode=ro' AS fnd",
        findings_db.display()
    );
    {
        let results = pool.conn_for_each(move |c| c.execute_batch(&attach)).await;
        let ok = results.iter().all(Result::is_ok);
        let detail = if ok {
            format!("{POOL_CONNS}/{POOL_CONNS} connections attached findings.db read-only (writer live)")
        } else {
            format!(
                "attach errors: {:?}",
                results
                    .iter()
                    .filter_map(|r| r.as_ref().err())
                    .collect::<Vec<_>>()
            )
        };
        step(ok, "P2a read-only WAL ATTACH (writer live)", &detail);
    }

    // ---- P2: cross-attach FTS query, lifecycle-filtered ----
    {
        let hits: Vec<String> = pool
            .conn(|c| {
                let mut s = c.prepare(
                    "SELECT f.id FROM fnd.f f JOIN fnd.findings d ON d.id = f.id
                      WHERE f MATCH ?1 AND d.superseded = 0 AND d.tombstoned = 0
                      ORDER BY bm25(f) LIMIT 10",
                )?;
                let out: rusqlite::Result<Vec<String>> =
                    s.query_map(params!["widget"], |r| r.get(0))?.collect();
                out
            })
            .await?;
        step(
            hits == ["fnd_b"],
            "P2b cross-db FTS over attached findings.db",
            &format!("`widget` MATCH on fnd.f joined to fnd.findings → {hits:?}"),
        );
    }

    // ---- P2: live writer append is visible to the read-only attached readers ----
    {
        writer
            .conn(|c| {
                c.execute(
                    "INSERT INTO findings(id,text) VALUES('fnd_d','expedite the widget refund')",
                    [],
                )?;
                c.execute(
                    "INSERT INTO f(id,text) VALUES('fnd_d','expedite the widget refund')",
                    [],
                )?;
                Ok(())
            })
            .await?;
        let hits: Vec<String> = pool
            .conn(|c| {
                let mut s =
                    c.prepare("SELECT f.id FROM fnd.f f WHERE f MATCH ?1 ORDER BY bm25(f)")?;
                let out: rusqlite::Result<Vec<String>> =
                    s.query_map(params!["widget"], |r| r.get(0))?.collect();
                out
            })
            .await?;
        step(
            hits.contains(&"fnd_d".to_owned()),
            "P2c append-after-attach visible to RO readers",
            &format!("writer appended fnd_d in WAL; readers now see {hits:?}"),
        );
    }

    // ---- P3: concurrent pool.conn calls parallelize across worker conns ----
    {
        fn heavy(c: &Connection) -> rusqlite::Result<i64> {
            c.query_row(
                "WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM cnt WHERE x < ?1)
                 SELECT count(*) FROM cnt",
                params![CTE_N],
                |r| r.get(0),
            )
        }
        let t0 = Instant::now();
        pool.conn(heavy).await?;
        let single = t0.elapsed();

        let t1 = Instant::now();
        let handles: Vec<_> = (0..POOL_CONNS)
            .map(|_| {
                let p = pool.clone();
                tokio::spawn(async move { p.conn(heavy).await })
            })
            .collect();
        for h in handles {
            h.await??;
        }
        let parallel = t1.elapsed();
        #[expect(clippy::cast_precision_loss, reason = "POOL_CONNS is a small constant")]
        let speedup = single.as_secs_f64() * POOL_CONNS as f64 / parallel.as_secs_f64();
        step(
            speedup > 1.8,
            "P3 concurrent reads parallelize",
            &format!(
                "1 query {single:.2?}; {POOL_CONNS} concurrent {parallel:.2?} → {speedup:.1}x speedup (serialized would be ~1x)"
            ),
        );
    }

    // ---- P2 gotcha: read-only WAL with the writer gone ----
    writer.close().await?;
    {
        let direct = PoolBuilder::new()
            .path(&findings_db)
            .flags(OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)
            .num_conns(1)
            .open()
            .await;
        match direct {
            Ok(p) => {
                let n: Result<i64, _> = p
                    .conn(|c| c.query_row("SELECT count(*) FROM findings", [], |r| r.get(0)))
                    .await;
                match n {
                    Ok(n) => info(
                        "P2d RO open of WAL db, writer gone",
                        &format!("clean close checkpointed the WAL; RO open succeeded, {n} rows readable"),
                    ),
                    Err(e) => info(
                        "P2d RO open of WAL db, writer gone",
                        &format!("opened but query failed: {e} — would need immutable=1"),
                    ),
                }
            }
            Err(e) => info(
                "P2d RO open of WAL db, writer gone",
                &format!("RO open failed (the WAL gotcha): {e}\n       workaround: ATTACH 'file:...?immutable=1' or open read-write"),
            ),
        }

        // immutable=1 path: treat the file as never-changing, bypassing shm/WAL.
        // This reads ONLY checkpointed data — un-checkpointed WAL frames are
        // invisible. Here findings.db's WAL is still pinned by the pool's
        // attachments, so the writer's close did not checkpoint: even the
        // schema lives in the WAL and immutable=1 sees an empty main file.
        let imm = PoolBuilder::new()
            .path(&vector_db)
            .flags(OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)
            .num_conns(1)
            .open()
            .await?;
        let attach_imm = format!(
            "ATTACH DATABASE 'file:{}?immutable=1' AS imm",
            findings_db.display()
        );
        let r = imm
            .conn(move |c| {
                c.execute_batch(&attach_imm)?;
                c.query_row("SELECT count(*) FROM imm.findings", [], |r| {
                    r.get::<_, i64>(0)
                })
            })
            .await;
        match r {
            Ok(n) => info(
                "P2e immutable=1 ATTACH",
                &format!("read {n} checkpointed rows; only valid after a wal_checkpoint(TRUNCATE)"),
            ),
            Err(e) => info(
                "P2e immutable=1 ATTACH bypasses the WAL",
                &format!(
                    "{e} — expected: immutable=1 ignores un-checkpointed frames.\n       \
                     Not our path: the findings writer stays live, so readers use the P2a RO attach."
                ),
            ),
        }
    }

    pool.close().await?;
    println!("\ndone.");
    Ok(())
}
