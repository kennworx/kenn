## 1. Implement `NormalizeDocXml` (prose extraction)

- [x] 1.1 Replace the no-op `NormalizeDocXml` in
      `indexers/kenn-dotnet/src/Indexing/IndexerCore.cs` with an `XDocument`-based
      walk that returns plain prose: text of `summary`/`remarks`/`returns`/
      `value`/`example`/`param`/`typeparam`, `see cref`/`paramref` → bare name,
      `<c>`/`<code>` inner text, `<member>` envelope + all tags stripped,
      entities decoded, whitespace collapsed (design D2/D4/D5).
- [x] 1.2 `<inheritdoc/>`-only (no inline prose) → `null` (treated as
      undocumented; design D3).
- [x] 1.3 Malformed/unexpected XML → `null` (never leak raw markup; design D4).

## 2. Verification

- [x] 2.1 Unit tests in `indexers/kenn-dotnet.tests`: `<summary>` extracted to
      prose; `<param>`/`<returns>` rendered; `see cref` → name; `<inheritdoc/>`
      → null; entity decode (`&lt;` → `<`); malformed → null.
- [x] 2.2 Rebuild the sidecar; reindex the C# corpus; confirm `symbol_docs.doc`
      rows are prose (no `<member`/`<summary>` substrings) and `doc_fts` no
      longer matches the tokens `member` / `summary`.
- [x] 2.3 Re-run `composed_spike` on the cleaned C# corpus with the now-valid
      prose gold; record G2 sig+doc vs doc-only (this is the measurement the
      `doc-only-embeddings` recipe decision is waiting on).
- [x] 2.4 .NET build + tests green (`dotnet test indexers/kenn-dotnet.tests`).
