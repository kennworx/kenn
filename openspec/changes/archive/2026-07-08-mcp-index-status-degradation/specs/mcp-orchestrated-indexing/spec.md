## ADDED Requirements

### Requirement: Index status reports the served snapshot's degraded-run summary

`get_index_status` SHALL report whether the **served snapshot** was built
from a degraded run (`wait_for_index` returns the same payload):
the aggregate run status recorded in the snapshot's metadata
(`"success" | "partial" | "failed"`), the bounded failed-project attribution
list with its true total count, and the bounded status-neutral warning list
with its true total count. When the run was clean — `success` with no
warnings — these fields SHALL be omitted, leaving the payload unchanged.

The summary SHALL be parsed from the snapshot's persisted metadata **once per
reader binding** (cold start, recovery, and every snapshot rotation) and
served from that cached state — never a store open or metadata read on the
status call path. A snapshot without parseable metadata (pre-reporting era)
SHALL yield no summary, not an error.

Degradation SHALL be reported, not escalated: a `partial` snapshot still
serves, and the `state` field continues to reflect the lifecycle/embed stage.

#### Scenario: a partial run's failures are visible to the agent

- **GIVEN** an index run where one language sidecar failed (e.g. C# msbuild)
- **WHEN** the resulting snapshot is served and `get_index_status` is called
- **THEN** the payload carries `run_status: "partial"` and the failed-project
  attribution naming that language
- **AND** `failed_count` is the true total (bounded list length + overflow)
- **AND** `state` still reflects the embed stage (the graph serves)

#### Scenario: producer warnings surface without changing the state

- **GIVEN** a successful run that recorded status-neutral warnings (e.g.
  stale index-store units kept on a trusted read)
- **WHEN** `get_index_status` is called
- **THEN** the payload carries the warning list and `warning_count`
- **AND** `run_status` is `"success"`

#### Scenario: a clean run leaves the payload unchanged

- **GIVEN** a snapshot whose run succeeded with no warnings
- **WHEN** `get_index_status` is called
- **THEN** none of the degraded-run fields are present

#### Scenario: the summary tracks snapshot rotation from cached state

- **GIVEN** a served `partial` snapshot and a subsequent clean reindex
- **WHEN** the `live` pointer flips and the reader swaps
- **THEN** the next `get_index_status` reflects the new snapshot's clean
  summary
- **AND** no metadata read happens on the status call path itself
