## Why

`incremental-embedding` established the rule: vectors are committed only as the
`fingerprint -> vector` sidecar; Lance stores are derived and gitignored.
Findings break that rule. `.kenn/findings/` is a committed Lance dataset that
carries an `embedding` column — vectors in a binary store in git, exactly what
the sidecar exists to avoid. And the authored finding records have no textual
form: a finding can only be read back by opening the binary dataset, and
diffing or reviewing one in a PR is impossible.

This change brings findings under the same model — the no-binary-Lance-in-git
decision applied uniformly.

## What Changes

- **Finding records become committed text.** Each finding is written to
  `.kenn/findings/<id>.json` — one file per finding, the `fnd_<uuid>` id giving
  a unique, conflict-free filename (an append-log). These JSON records are the
  source of truth.
- **The findings Lance dataset becomes derived.** It is rebuilt from the JSON
  records and relocated under the gitignored `.kenn/local/`. The git-merge
  reconciliation machinery (`reconcile_after_merge`) is removed — git merges the
  immutable JSON records directly, with no binary union to heal.
- **Finding embeddings move to a sidecar.** A finding's embedding is keyed by
  `fingerprint(finding.text)` in a dedicated findings sidecar
  (`.kenn/findings/vectors/`) that reuses the `incremental-embedding` sidecar
  format — reconciled when the Lance store is rebuilt, embedded at flush, never
  carried in a committed Lance column. It is a *separate* sidecar from the code
  one, not a shared directory (see design D3).
- `FindingsStore::flush` writes the JSON records first (source of truth), then
  derives the Lance store and reconciles / embeds.

## Capabilities

### Modified Capabilities

- `findings-store`: finding records persist as committed per-finding JSON
  files; the Lance dataset and finding embeddings become derived — the Lance
  store is rebuilt from the records, embeddings ride the `incremental-embedding`
  vector sidecar.

## Impact

- **Committed artifact changes:** `.kenn/findings/<id>.json` records replace the
  committed `.kenn/findings/` Lance dataset.
- **`.gitignore`:** the derived findings Lance store (relocated under
  `.kenn/local/`) is gitignored; the `*.json` records are tracked.
- **`FindingsStore`:** `open` rebuilds the Lance store from the records; `flush`
  writes records first; `reconcile_after_merge` and its merge-union machinery
  are removed.
- **Depends on `incremental-embedding`** for the sidecar *format* — the `KVS1`
  segments, `manifest.toml`, int8 quantization, and compaction — reused at a
  separate directory.
- **Embeddings:** finding vectors leave the committed Lance column for the int8
  sidecar — consistent with code symbols, and dedup'd by content fingerprint.
