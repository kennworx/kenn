# mcp-server

## ADDED Requirements

### Requirement: Empty-snapshot tools point at config, not silent empty results

When the published snapshot has zero symbols (e.g., a fresh workspace where no language is enabled, or every enabled language found nothing to index), every MCP tool that returns symbol or finding data (e.g., `search_symbols`, `find_symbol`, `find_at_location`, `list_callers`, `list_callees`, `list_usages`, `list_implementers`, `list_overrides`, `list_correspondences`, `list_in_scope`, `list_module_files`, `list_imports`, `get_symbol`, `get_source`, `find_similar`, `search_findings`, `semantic_search`) SHALL return a structured error rather than a silent empty array. Tools that are NOT subject to this rule: the carve-outs `get_index_status` and `get_workspace_overview`, plus all MCP protocol primitives (`initialize`, `tools/list`, `tools/call` dispatch, `notifications/*`) — those continue to operate per their existing contracts. The error is the empty-snapshot dual of the existing *An unresolved entity reference is an error, not an empty result* requirement.

The error SHALL reuse JSON-RPC code `-32002` (the same code kenn-mcp uses today for `IndexUnavailable`/`EmbedderStarting`, since "the index exists but has no data to serve you" belongs to the same service-unavailable family). On the wire, kenn-mcp's existing error envelope places the per-error string code under `data.kenn_subcode` (injected by the server wire layer) and the per-error classifier payload under `data.data`. For an empty-snapshot error this materialises as `data.kenn_subcode = "EMPTY_SNAPSHOT"` plus `data.data = { kind, enabled_languages }`. Agents branch on `data.kenn_subcode` for the error class and on `data.data.kind` / `data.data.enabled_languages` for the classifier — without parsing the human-readable `message`:

- **config-disabled**: every `[language.*].enabled` is `false` in the workspace's `kenn.toml`. `enabled_languages` is the empty array. `message` MUST reference `kenn.toml` and list the strings `csharp`, `rust`, `typescript`, `python` verbatim.
- **configured-but-empty**: at least one language is enabled but the snapshot still has zero symbols. `enabled_languages` lists the enabled language identifiers using the canonical lowercase serialization (`csharp`, `rust`, `typescript`, `python`). `message` MUST identify the case as configured-but-empty AND name the enabled language(s); it MAY include a most-common-cause hint (e.g., "no `.py` files were found"), but the implementation is NOT required to diagnose the actual cause — an honest "snapshot is empty, reason unclear" message naming the enabled language(s) is compliant.

The workspace whose `kenn.toml` is consulted MUST be the workspace resolved by the existing *Workspace resolution follows a five-step priority chain* requirement — not `cwd` — so worktree-bound MCP sessions see the right config.

`get_workspace_overview` MUST succeed in both cases (the empty state itself is information) and its response struct SHALL grow an optional `config_hint` field of shape `{ kind: "config-disabled" | "configured-but-empty", enabled_languages: [..] }`, present only when the snapshot has zero symbols and absent (or `null`) on healthy snapshots.

#### Scenario: MCP query against config-disabled empty snapshot

- **WHEN** `kenn mcp` serves a snapshot with `symbols=0` AND every `[language.*].enabled` is false in `kenn.toml`
- **AND** the agent calls `search_symbols("anything")` (or any other data-returning tool listed in this requirement)
- **THEN** the tool MUST return a structured JSON-RPC error with `code = -32002`, `data.kenn_subcode = "EMPTY_SNAPSHOT"`, and `data.data = { kind: "config-disabled", enabled_languages: [] }`
- **AND** the error `message` MUST reference `kenn.toml` and list the strings `csharp`, `rust`, `typescript`, `python`
- **AND** the error MUST NOT be a generic empty-results array

#### Scenario: MCP query against configured-but-empty snapshot

- **WHEN** the snapshot has `symbols=0` AND `[language.python].enabled = true` (and no other language enabled)
- **AND** the agent calls `find_symbol("Foo")`
- **THEN** the tool MUST return a structured JSON-RPC error with `code = -32002`, `data.kenn_subcode = "EMPTY_SNAPSHOT"`, and `data.data = { kind: "configured-but-empty", enabled_languages: ["python"] }`
- **AND** the error `message` MUST identify the case AND name Python as the enabled language
- **AND** the implementation MAY but is NOT required to add a "no `.py` files" diagnostic — an honest "reason unclear" message is compliant

#### Scenario: get_workspace_overview surfaces config state on empty snapshots

- **WHEN** the snapshot has `symbols=0`
- **THEN** `get_workspace_overview` MUST return successfully
- **AND** the response MUST include a `config_hint` field of shape `{ kind, enabled_languages }` populated per the classification above

#### Scenario: get_workspace_overview omits config_hint on healthy snapshots

- **WHEN** the snapshot has `symbols > 0`
- **THEN** `get_workspace_overview` MUST return successfully
- **AND** the response MUST either omit `config_hint` or set it to `null`

#### Scenario: get_index_status remains the lifecycle-only probe

- **WHEN** the snapshot has `symbols=0` for any reason
- **THEN** `get_index_status` MUST still respond per its existing contract (lifecycle state, snapshot id, indexed_at)
- **AND** MUST NOT return a config-hint error — config diagnosis is the responsibility of the read tools and `get_workspace_overview`
