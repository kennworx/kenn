## 1. Config

- [ ] 1.1 Add `utility_allowlist: Vec<String>` to `CssConfig`
  (`crates/kenn-config/src/language/css.rs`), default empty. → verify:
  round-trips through config load; default is `[]`.

## 2. Indexer — persist undefined tokens

- [ ] 2.1 Build an `is_utility` matcher from `utility_allowlist` and pass it
  into `resolve_usages` (replace the hardcoded `|_| false` at
  `crates/kenn-indexer/src/css/ingest.rs:210`). → verify: allowlisted token is
  excluded from `UsageScan.undefined`.
- [ ] 2.2 Persist filtered `UsageScan.undefined` (file + class name) via the
  pipeline sink — only when an allowlist is configured. New store table. →
  verify: undefined non-utility tokens land in the store; none persisted when
  allowlist empty.

## 3. Store — reader + types

- [ ] 3.1 Add a `dangling_class` predicate/query to `scan_css_health`
  (`crates/kenn-store/src/db/sqlite/reader/css_health.rs`) and a
  `dangling_classes` count to `CssHealthCounts`. → verify: reader returns
  dangling rows with location.

## 4. MCP tool

- [ ] 4.1 Accept `"dangling_class"` in the `want()` category validator
  (`crates/kenn-mcp/src/tools/css.rs`); thread the new category through the
  response. → verify: tool returns dangling findings; unknown category still
  errors.

## 5. Verification

- [ ] 5.1 Dangling flagged only when allowlist present; not flagged without
  (gate behavior). → verify: two-case test mirroring the `usage_mining_on`
  gate test in `crates/kenn-mcp/tests/check_css.rs`.
