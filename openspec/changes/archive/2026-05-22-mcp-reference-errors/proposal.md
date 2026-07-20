## Why

The MCP tool surface returned a **silently-empty result** when handed a
reference that does not exist — a non-existent symbol `pub_id`, an
unindexed file, an unknown finding id. An empty `items` array is
ambiguous: the agent cannot tell *"this symbol genuinely has no
callers"* from *"I mis-typed the id."* That ambiguity burns agent turns
and hides mistakes instead of surfacing them.

This change makes an unresolved reference a **clear error**. It documents
behavior that has already shipped — the implementation landed as direct
edits; this change retro-captures it into the specs so they describe the
real surface.

## What Changes

- Every MCP tool that takes an entity reference SHALL return an
  `INVALID_INPUT` error when that reference does not resolve, instead of
  an empty success payload. An empty `items` array is reserved for a
  reference that resolves but has no matches (a real symbol with no
  callers). Tools that already return an explicit `{found: false}`
  (`get_symbol`, `get_source`, `get_finding`) are unambiguous and keep
  that. Search tools are exempt — an empty result is the right answer to
  a query that matched nothing.
- `find_at_location` addresses its file by `file_path` — a
  workspace-relative *or* absolute path. No numeric `file_id` is exposed:
  a per-run `short_id` carries no snapshot-stable meaning, so it would be
  a silent staleness hazard.
- `store_finding` and `merge_findings` validate their id-list inputs and
  report **every** unresolved id in one error, not just the first.
- `find_predecessors` / `find_successors` reject an unknown `fnd_…` start
  id; code-node references stay deliberately loose (see design D2).

## Capabilities

### Modified Capabilities

- `mcp-server`: adds the cross-cutting rule that an unresolved entity
  reference is an `INVALID_INPUT` error rather than an empty result, and
  documents `find_at_location`'s `file_path` argument.
- `findings-mcp`: `store_finding` / `merge_findings` validate their input
  ids (reporting all unresolved ones at once); `find_predecessors` /
  `find_successors` validate the `fnd_…` start id.

## Impact

- **Code**: `crates/kenn-mcp/src/tools.rs`, `src/server.rs`;
  `crates/kenn-store` `fetch_file_short_id` (relative + absolute path
  resolution). Already shipped — this change documents landed behavior.
- **Protocol**: no version change; the observable difference is an error
  envelope where there was an empty-list success.
- **Out of scope**: the retire-redb storage rework (separate, archived
  change); search-tool semantics (empty-on-no-match is correct).
