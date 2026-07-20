## ADDED Requirements

### Requirement: Parameter-bound batch writes

The rust-side `SurrealdbSink::write_batch` SHALL pass record data to
`SurrealDB` via bound query parameters, not as inline `SurrealQL`
literals. Each non-empty record table per batch SHALL bind to a
named parameter and reference it by name in the query string.

The query template per batch SHALL be assembled from at most one
statement per non-empty section:

- `INSERT INTO <table> $<table>` for files, packages, symbols,
  symbol_docs, defs
- `INSERT RELATION INTO <kind> $b_<kind>` for each non-empty edge
  kind (one statement per kind, not per edge)

Record fields SHALL be passed as a `Value::Array` of `Value::Object`
entries bound to the matching parameter. The implementation MUST
NOT format individual record fields into the query string.

#### Scenario: One bound array per non-empty table

- **WHEN** a batch contains symbols and edges
- **THEN** the assembled query MUST contain exactly one
  `INSERT INTO symbols $syms` statement
- **AND** at most one `INSERT RELATION INTO <kind> $b_<kind>` per
  edge kind present in the batch
- **AND** records MUST be passed via `.bind((name, value))` calls
  on the query, not embedded as string literals

#### Scenario: Empty sections produce no statements

- **WHEN** a batch contains only edges (no record-table data)
- **THEN** the query MUST contain only `INSERT RELATION INTO …`
  statements, no record-table `INSERT INTO …` statements

### Requirement: Edge tables retain `TYPE RELATION` semantics

Edge tables SHALL remain `TYPE RELATION`. Switching edge writes
from `RELATE` statements to `INSERT RELATION INTO` statements
MUST NOT change the on-disk row layout, the relation type, or the
read-side query shape. Read queries that use record-link field
traversal (`SELECT VALUE in.short_id FROM <kind> WHERE
out = symbols:N`) MUST continue to work without modification.

#### Scenario: Read-side parity after switching emission shape

- **GIVEN** the same wire stream ingested by the previous (`RELATE`)
  and the current (`INSERT RELATION INTO`) emission paths
- **THEN** the resulting snapshot row counts (documents, symbols,
  edges) MUST be identical
- **AND** read-side traversals via `in`/`out` field links MUST
  return the same results

### Requirement: Bit-equivalent snapshot output

The bound-parameter ingest path MUST produce snapshots that are
bit-equivalent (modulo non-deterministic content like timestamps)
to the prior inline-string path for the same input wire stream.

#### Scenario: Snapshot stats match across emission paths

- **GIVEN** a JSONL fixture replayed through the rust ingestion
  pipeline
- **WHEN** the bound-parameter sink writes the snapshot
- **THEN** documents, symbols, definitions, and edges counts in
  `kenn status` MUST match the counts produced by the prior
  inline-string sink for the same input

### Requirement: Threaded JSONL ingest

The rust ingestion pipeline SHALL spawn a dedicated reader thread
that drains the producer's stdout pipe into a bounded in-memory
channel; the worker (calling) thread parses each line and pushes
records to the sink.

This decouples pipe-drain from sink-write so the producer (e.g.
`kenn-dotnet`) does not stall on its own stdout-write lock when
the consumer is busy with a batch flush.

#### Scenario: Reader thread drains pipe during sink flush

- **WHEN** the sink is mid-flush and would otherwise block stdout
  reads
- **THEN** the reader thread continues to drain the pipe into the
  bounded channel until the channel reaches capacity
- **AND** the producer is not stalled by sink-flush latency until
  the channel itself fills
