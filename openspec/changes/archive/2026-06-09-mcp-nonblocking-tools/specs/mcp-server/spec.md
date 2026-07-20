## ADDED Requirements

### Requirement: Concurrent tool calls do not serialize behind a lock

No MCP tool handler SHALL hold a lock across an `.await` of slow or
unbounded work such that other concurrent tool calls block behind it.
Read-only tools that need only shared access MUST NOT acquire an
exclusive lock. In particular, the findings store SHALL permit concurrent
read tools (search, get, DAG walks) to proceed in parallel; only mutating
tools serialize.

Every tool handler (except `wait_for_index`, whose blocking is bounded by
its own timeout) SHALL return within a small bounded latency budget on a
Ready server, or with a fast error — never an unbounded wait that also
stalls other tools.

#### Scenario: A slow/long findings read does not stall a concurrent read

- **GIVEN** one findings read tool is executing
- **WHEN** a second findings read tool is called concurrently
- **THEN** the second call proceeds in parallel and is not blocked for the
  full duration of the first

#### Scenario: Only wait_for_index may exceed the budget

- **WHEN** any tool other than `wait_for_index` is called on a Ready
  server
- **THEN** it returns within the bounded budget (or with a fast error),
  not after an unbounded wait

### Requirement: Tool calls are observable via tracing spans and metrics

Each tool call SHALL be wrapped in a `tracing` span carrying the tool
name, whose open→close duration is the call's latency, so a slow or
stuck call is diagnosable from the observability stack — not a profiler.
The same dispatch boundary SHALL emit metrics through a facade (a call
counter and a duration histogram, keyed by tool) so a backend exporter
can be wired without changing the instrumentation. All observability
output goes to stderr or an exporter — **never stdout**, which the stdio
transport reserves for JSON-RPC.

#### Scenario: A completed tool call has a span with name and duration

- **WHEN** a `tools/call` completes
- **THEN** a tracing span identifies the tool by name and its close
  event carries the elapsed duration
- **AND** nothing is written to stdout

#### Scenario: Metrics facade is in place for a future exporter

- **WHEN** a tool call completes
- **THEN** a counter and a duration histogram are recorded for that tool
  through the metrics facade (a no-op until an exporter is installed)
