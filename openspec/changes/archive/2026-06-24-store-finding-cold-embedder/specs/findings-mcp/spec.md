## MODIFIED Requirements

### Requirement: the MCP server exposes finding writes with provenance

The server SHALL expose `store_finding`, accepting `text`, `parent_ids`, `tags`,
and an optional `anchors` list (file or directory paths), and returning the new
finding's id together with any semantically near existing findings. When
`anchors` are supplied, the server SHALL record an initial `attach` event for
each in the new finding's `<id>.anchor.jsonl`, so a directive is created and
anchored in one call. The near-duplicate probe is **advisory**: it pre-embeds the
text to find similar prior findings, but when the embedder is cold the server
SHALL still write the finding and return its id with an empty `similar` list,
rather than failing the write — matching `find_directives`' non-blocking degrade,
so the first write against a freshly-indexed repo (whose embeddings are not yet
built) succeeds. It SHALL expose `merge_findings`, which synthesizes a new
finding from given finding ids and records those ids as parents.

Both SHALL validate their id inputs before writing. A `fnd_…` id that names no
existing finding SHALL fail the call with `INVALID_INPUT`, and the error SHALL
list **every** unresolved id, not only the first, so the caller corrects them in
one round-trip. `merge_findings` inputs are findings, so every input id is
checked. `store_finding`'s `parent_ids` mix finding ids and code-graph node ids;
only the `fnd_…` ones are checked — a code-node reference is best-effort
provenance whose later resolvability is reported by finding staleness, not
enforced at write time.

#### Scenario: store_finding returns id and near-duplicates

- **WHEN** `store_finding` is called and a semantically similar finding already
  exists
- **THEN** the response contains the new finding's id
- **AND** the response lists the similar prior finding

#### Scenario: store_finding succeeds while the embedder is cold

- **WHEN** `store_finding` is called before the embedder has warmed (a freshly
  indexed repo with no embeddings yet)
- **THEN** the finding is written and its id is returned
- **AND** the `similar` list is empty rather than the call failing

#### Scenario: store_finding anchors the new finding in one call

- **WHEN** `store_finding` is called with an `anchors` list
- **THEN** the new finding's `<id>.anchor.jsonl` records an `attach` for each
  anchor

#### Scenario: merge_findings records its inputs as parents

- **WHEN** `merge_findings` is called with two finding ids
- **THEN** a new finding is created whose `parent_ids` include both inputs

#### Scenario: unknown finding inputs are rejected, all at once

- **WHEN** `store_finding` or `merge_findings` is called with two `fnd_…` ids
  that name no existing finding
- **THEN** the response is an `INVALID_INPUT` error
- **AND** the error message names both unresolved ids
