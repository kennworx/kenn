## Context

The MCP surface mixes three id spaces: snapshot-stable `pub_id`s for
code symbols (`cs:Foo.Bar`), `fnd_…` ids for findings, and code-graph
node ids inside the findings DAG (`<lang>:<pub_id>`). Tools historically
answered an unresolved reference with an empty `items` array — the same
shape a genuine no-match returns.

## Goals / Non-Goals

**Goals:** an unresolved reference is an unambiguous error; list-valued
validators report all bad ids at once.

**Non-Goals:** changing search semantics (empty-on-no-match is correct);
validating code-node references against the live snapshot (see D2).

## Decisions

### D1 — An unresolved reference is an error, not an empty result

A tool given a reference that does not resolve returns `INVALID_INPUT`.
An empty `items` array is reserved for a reference that *resolves* but
has no matches. Tools with an explicit `{found: false}` payload
(`get_symbol`, `get_source`, `get_finding`) already satisfy this — that
shape is unambiguous. Search tools are exempt: an empty result is the
correct answer to a query.

### D2 — Finding ids are validated; code-node references are not

A `fnd_…` id is checked against the findings store — cheap and
definitive. A code-node reference in a finding's `parent_ids` is
**not** validated against the code graph, for two reasons:

- **Durability.** Findings outlive snapshots. A finding may cite a code
  node that is later refactored away; that is *staleness*, surfaced by
  `finding_is_stale`, not an error at write time. Validating code-node
  refs against the current snapshot would reject legitimate provenance
  and would make `find_successors` unable to reach findings about
  refactored-away code.
- **Id space.** Findings-DAG code-node ids are `<lang>:<pub_id>`, not the
  symbol-search `pub_id` form — the symbol-search resolver is the wrong
  tool for them anyway.

So `store_finding` validates only `fnd_…` parents; `find_predecessors` /
`find_successors` validate only a `fnd_…` start id. A code-node start id
is accepted as-is (a code node has no predecessors, and `find_successors`
must still reach findings that cite a now-absent node).

### D3 — No numeric `file_id` on the tool surface

`find_at_location` addresses a file by `file_path` only. A numeric file
`short_id` is assigned per index run and carries no snapshot-stable
meaning: a cached `file_id` could silently resolve to a *different* file
after a re-index. Every other reference on the surface is snapshot-safe
(`pub_id`s are stable, cursors embed the snapshot id), so exposing a raw
`short_id` would be the one staleness hole. `fetch_file_short_id`
resolves a workspace-relative or absolute path — exact match, then a
trailing path-component-suffix match — so the path form is sufficient.
