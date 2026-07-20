## ADDED Requirements

### Requirement: the MCP server exposes unified search over code and findings

The server SHALL expose a `semantic_search` tool that ranks results by BM25 and that can be scoped to code, to findings, or to both. It SHALL also expose `get_source` over the code graph, and `get_finding` and `search_findings` over the findings store.

Vector / semantic ranking is **deferred**: it depends on the `embedding-producer` change. Until that lands, `semantic_search` is lexical (BM25) — at which point this requirement becomes hybrid BM25 + vector. Code-graph node, caller, and callee reads are already served by the existing `get_symbol` / `list_callers` / `list_callees` tools and SHALL NOT be re-exposed under new names.

#### Scenario: a query spanning code and findings returns both

- **WHEN** `semantic_search` is called with a query and a scope covering code and findings
- **THEN** the ranked result includes matching code symbols and matching findings

#### Scenario: a finding is retrievable by id

- **WHEN** `get_finding` is called with a known finding id
- **THEN** the server returns that finding's text, tags, and `parent_ids`

### Requirement: the MCP server exposes finding writes with provenance

The server SHALL expose `store_finding`, accepting `text`, `parent_ids`, and `tags`, and returning the new finding's id together with any semantically near existing findings. It SHALL expose `merge_findings`, which synthesizes a new finding from given finding ids and records those ids as parents.

#### Scenario: store_finding returns id and near-duplicates

- **WHEN** `store_finding` is called and a semantically similar finding already exists
- **THEN** the response contains the new finding's id
- **AND** the response lists the similar prior finding

#### Scenario: merge_findings records its inputs as parents

- **WHEN** `merge_findings` is called with two finding ids
- **THEN** a new finding is created whose `parent_ids` include both inputs

### Requirement: the MCP server exposes derivation-DAG traversal

The server SHALL expose `find_predecessors` and `find_successors`, walking the `parent_ids` edges of the unified ID space so a caller can trace a finding back to the code or earlier findings it was derived from.

#### Scenario: provenance is walkable to source

- **GIVEN** a finding derived from another finding that cites a code-graph node
- **WHEN** `find_predecessors` is walked transitively from the finding
- **THEN** the walk reaches the originating code-graph node

### Requirement: the MCP server runs no model and performs no task analysis

The server SHALL expose only primitive capabilities — graph reads, finding reads and writes, DAG traversal. It SHALL NOT host an embedding or language model, and SHALL NOT expose a tool that interprets a task, plans work, or slices work for subagents. Slicing and dispatch are the calling agent's responsibility.

#### Scenario: no planning or slicing tool is offered

- **WHEN** the server's tool list is enumerated
- **THEN** it contains search, graph-read, finding-read, finding-write, and DAG tools
- **AND** it contains no tool that analyzes a task or produces a work plan

### Requirement: a system-prompt fragment drives finding accumulation

The change SHALL ship a system-prompt fragment instructing an agent to search existing findings before re-investigating and to store a finding at a stable conclusion. The fragment SHALL be installable alongside the MCP server so findings accumulate as a byproduct of normal agent work, independent of the orchestrator in use.

#### Scenario: the fragment is available on install

- **WHEN** the MCP server's knowledge layer is installed
- **THEN** the system-prompt fragment is provided as an installable asset
- **AND** it directs the agent to both query and store findings

### Requirement: the subagent-as-extractor pattern is documented

The change SHALL document the subagent-as-extractor dispatch pattern: a main agent orients with search and graph reads, slices the task, fans out subagents that each investigate through the MCP surface and record findings, and synthesizes the returned finding ids. The documentation SHALL state that coordination is through the findings store and returned ids, not ad-hoc file passing.

#### Scenario: the dispatch pattern is described for implementers and agents

- **WHEN** the knowledge-layer documentation is read
- **THEN** it describes the orient → slice → fan-out → record → synthesize flow
- **AND** it states that subagents coordinate via stored findings and returned ids
