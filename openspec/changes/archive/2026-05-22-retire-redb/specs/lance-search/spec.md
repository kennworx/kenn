## REMOVED Requirements

### Requirement: the Lance store is git-committed and merges without a custom driver

**Reason**: The search / knowledge Lance store is no longer git-committed. The completed `incremental-embedding` change made `.kenn/knowledge/` derived and gitignored — the committed artifact is the `.kenn/vectors/` binary sidecar, not a Lance dataset. The code-graph datasets `retire-redb` adds are likewise derived and gitignored. A derived store rebuilt per worktree is never git-merged, so merge-cleanliness is not a property it needs. (The findings store remains a committed Lance dataset, governed by the `findings-store` capability, not this one.)

**Migration**: The committed, merge-clean artifact is the `.kenn/vectors/` embedding sidecar, specified by the `incremental-embedding` capability. Every Lance dataset is rebuilt from source per worktree.

### Requirement: the manifest is written to a committed collision-free path

**Reason**: The custom `CommitHandler` existed only so Lance manifests could be git-committed without sequential-filename collisions. The search / knowledge store is no longer committed, so it uses Lance's default manifest path (`retire-redb` design D7). The handler itself is retained for the still-committed findings store.

**Migration**: None — the derived store uses Lance's default `_versions/<N>.manifest` location.

### Requirement: search indexes are preserved across a merge

**Reason**: Index-preservation-across-merge existed to avoid rebuilding indexes after a `git merge` of a committed store. The search / knowledge store is no longer committed or merged; each run rebuilds it and its indexes from source.

**Migration**: None — indexes are built once per run during the finalize phase.

### Requirement: a single store is written by one writer at a time

**Reason**: This requirement held because the custom `CommitHandler` gave every manifest a unique name, disabling Lance's rename-collision concurrency guard and forcing the store to serialize writers itself. With the custom `CommitHandler` removed (`retire-redb` D7), Lance's default optimistic-concurrency guard is in effect; concurrent appends are resolved by commit-retry, and the ingest phase relies on exactly that (`retire-redb` design D9).

**Migration**: Concurrent writers are handled by Lance's default optimistic-concurrency commit guard — see the `indexing-orchestrator` capability, "Ingesters write records directly to per-language Lance writers".

### Requirement: committed embeddings survive clone and merge without recompute

**Reason**: Superseded by the `incremental-embedding` capability. Embeddings are no longer stored in committed Lance rows; they are committed in the `.kenn/vectors/` sidecar and joined into the derived store's `embedding` column on rebuild. This requirement described the pre-sidecar design and was left stale when `incremental-embedding` shipped without a `lance-search` delta.

**Migration**: See the `incremental-embedding` capability for embedding persistence (the sidecar) and reuse on rebuild.

### Requirement: reconciliation on rebuild reuses unchanged embeddings

**Reason**: Superseded by the `incremental-embedding` capability. Reconciliation now joins sidecar vectors into the rebuilt store by `xxh3-64` fingerprint, rather than reconciling against a committed Lance store by symbol identity. Left stale when `incremental-embedding` shipped without a `lance-search` delta.

**Migration**: See the `incremental-embedding` capability, fingerprint reconciliation at index time.
