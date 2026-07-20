## 1. Pool-backed reader in kenn-store

- [x] 1.1 Add `async-sqlite` (0.6, `default-features=false,
      features=["bundled"]`) to `kenn-store` deps so it unifies with the
      workspace `rusqlite 0.40` (one shared bundled build — confirmed via
      `cargo tree`).
- [x] 1.2 Add a pool-backed reader: `SqliteReader` holds an async-sqlite `Pool`
      opened read-only over `code.db` (main) with `vector.db` ATTACHed `AS vec`
      (`SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_URI`), `vec0` registered, and a busy
      timeout, applied per connection via `conn_for_each`. The query methods
      became a sync `SqliteConnRef<'a>` over the pooled `&Connection`; **no SQL
      changed** — `code.db`/`vector.db` share no table names, so bare names
      resolve across the attach (derisk-verified). `findings.db` is NOT attached
      to the reader pool: findings tools use the `FindingsStore` plus a
      code-graph resolver (a `code.db` query), so the reader pool stays
      code-only (revises design D2).
- [x] 1.3 No resident bulk-scan projection exists in the reader (it is lazy /
      per-query), so there was nothing to keep off the pool.

## 2. Wire the pool into the ReaderBinding hot path

- [x] 2.1 `ReaderBinding` already wraps `DbReader`, which now holds the `Pool`
      (opened once in `open_reader`, i.e. at snapshot bind in
      `open_ready_if_live` and the reload path). Dropping the binding drops the
      pool.
- [x] 2.2 `ready_view_or_err` no longer opens a connection per call:
      `DbReader::connect()` is a cheap pool-handle clone (`DbConn` is now a
      `DbReader` alias). `ReadyView` carries it.
- [x] 2.3 `with_db` (incl. the `count_table` empty-snapshot gate) and
      `with_findings_read`/`with_findings_write` run their reads through the
      pool automatically — every `Reader` method dispatches via
      `Pool::conn_and_then`. Helper ergonomics unchanged.
- [x] 2.4 The findings writer keeps its own lifecycle (unchanged); no read-only
      ATTACH into the reader pool was needed (see 1.2).

## 3. Preserve the wire contract

- [x] 3.1 Payloads and error forms unchanged: the existing
      `get_workspace_overview` / `INDEX_UNAVAILABLE` / `EMPTY_SNAPSHOT`
      regression tests pass untouched (kenn-mcp suite green).

## 4. Tests

- [x] 4.1 Reader unit tests migrated to async + the `Reader` trait, so they now
      exercise the **pool dispatch path** end-to-end (17 tests green).
- [x] 4.2 The blended/tiered/find-similar tests exercise the cross-attach
      (`code.db` + `vector.db`) query path through pooled connections.

## 5. Verification

- [x] 5.1 `cargo clippy --workspace --all-targets` clean.
- [x] 5.2 `cargo test -p kenn-mcp` and `cargo test -p kenn-store` green.
- [x] 5.3 `just crap-ci` passes (no regressions, no new over-threshold).
- [x] 5.4 `cargo fmt --all` run as the final step.
