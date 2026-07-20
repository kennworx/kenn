## Why

C# documentation is emitted **raw, as XML doc-comment markup**. The .NET sidecar
calls Roslyn's `GetDocumentationCommentXml()` and passes it through a hook named
`NormalizeDocXml` — but that hook is a **no-op**:

```csharp
// indexers/kenn-dotnet/src/Indexing/IndexerCore.cs
private static string? NormalizeDocXml(string? xml) =>
    string.IsNullOrWhiteSpace(xml) ? null : xml;   // returns the raw XML unchanged
```

So every documented C# symbol streams (and is stored) as:

```
<member name="P:Acme.Market.BaseAsset">
    <summary>
    Base asset name, example - in btc_usdt this is btc.
    </summary>
</member>
```

All 6,653 documented symbols in a measured C# corpus carry this markup. It
contaminates everything downstream:

- **`doc_fts` (porter) indexes the markup** — `member`, `summary`, and the
  fully-qualified `name="…"` tokens become searchable noise that dilutes BM25.
- **The vector arm embeds the markup** — `sig\ndoc` (and a future doc-only
  recipe) push XML tags and a repeated FQN into the embedding, away from prose.
- **It corrupts doc-derived evaluation.** Two embedding spikes disagreed wildly
  on C# (`recipe_spike` doc-only **+8%**, `composed_spike` **−62%**) precisely
  because one used a cleaned-prose query and the other used the raw
  `<member name=FQN>` first line. Neither C# recipe number is trustworthy until
  the producer emits clean prose.

This is a C#/.NET-only problem: the TypeScript sidecar emits JSDoc prose and the
SCIP path (Rust/Python) emits markdown — neither carries the XML doc envelope.
The fix is to make the already-present `NormalizeDocXml` hook actually normalize.

## What Changes

- **Implement `NormalizeDocXml`** in the .NET sidecar: parse the XML doc comment
  and emit plain prose — keep the human text of `<summary>`, `<remarks>`,
  `<param>`, `<returns>`, `<value>`, `<example>`; render `<see cref="…"/>` /
  `<paramref name="…"/>` as their bare names; strip the `<member>` envelope and
  all tags; decode XML entities; collapse whitespace.
- A doc whose only content is `<inheritdoc/>` (no inline prose) normalizes to
  **empty** → the symbol is treated as undocumented (inherited-doc resolution is
  out of scope).
- Fixing it at the producer means the clean prose is what the Rust ingest stores
  in `symbol_docs.doc`, indexes in `doc_fts`, and embeds — one clean source of
  truth, no language-aware stripping needed on the Rust side.

## Capabilities

### Modified Capabilities

- `dotnet-stream-indexer`: the `doc` field of an emitted symbol frame is plain
  documentation prose, not raw XML doc-comment markup.

## Impact

- **Search:** `doc_fts` stops matching `member`/`summary`/FQN markup tokens;
  conceptual doc search on C# improves.
- **Embeddings:** vectors carry prose, not XML — a prerequisite for any valid
  doc-vs-`sig+doc` recipe comparison on C# (unblocks `doc-only-embeddings`).
- **Re-measurement:** after cleaning, the `composed_spike` C# G2 numbers can be
  re-run against a valid prose gold to actually decide the embedding recipe.
- **Code:** `indexers/kenn-dotnet/src/Indexing/IndexerCore.cs` (`NormalizeDocXml`)
  + `indexers/kenn-dotnet.tests`.
- **Reindex required** to take effect (no migration of existing snapshots).
