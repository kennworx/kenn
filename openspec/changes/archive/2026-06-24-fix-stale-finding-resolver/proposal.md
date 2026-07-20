## Why

Surfaced dogfooding `reconcile` on a large repo: a finding citing a **present**
symbol (a C# `cs:…Response` type, confirmed live by `find_symbol`) came
back flagged `stale`. Root cause: the read-time staleness resolver
(`code_node_resolver` in `crates/kenn-store/src/db/sqlite/reader/fetch.rs`) built
its id set as `format!("{language}:{pub_id}")` — but the `pub_id` column **already
is** the canonical code-node id and carries the language short-code
(`cs:Ns.Type`, `rs:foo::bar`). So it produced doubled ids (`csharp:cs:…`,
`rust:rs:…`) that never match the canonical id a finding stores in `parent_ids`.

Effect: `finding_is_stale` returned true for **every** finding with a code-graph
`parent_id`, in every language — the `stale` flag on `search_findings`,
`find_directives`, and `semantic_search` was firing unconditionally on
code-cited findings. Read-time only (no persisted corruption), but the feature was
effectively non-functional. The unit tests missed it because they use mock
resolvers; the real SQL-backed resolver was never exercised against real pub_ids.

This violated the existing `findings-store` requirement "staleness is computed at
read time" (a finding whose evidence node still exists SHALL be returned without a
stale flag).

## What Changes

- `code_node_resolver` keys on the `pub_id` column directly (`SELECT pub_id FROM
  symbols`), dropping the erroneous `{language}:` prefix — so the resolver set
  matches the canonical ids findings cite.
- Add a regression test that builds the real resolver from a seeded symbols table
  and asserts the canonical id resolves while the language-doubled form does not.

## Capabilities

### Modified Capabilities

- `findings-store`: the read-time staleness resolver SHALL key on the canonical
  code-node id (`pub_id`), so a finding citing a present symbol is live.

## Impact

- **Bugfix, read-time only** — no schema, no migration, no persisted state. A
  one-query change plus a regression test. Restores spec'd staleness behavior for
  all code-anchored findings.
