## ADDED Requirements

### Requirement: Producer registration is identical across all index entry paths

The set of producers enabled for an index run SHALL be derived from a **single
source of truth**, so that every enabled producer runs regardless of which entry
path (CLI `kenn index` or the workflow/MCP `index_workspace`) triggers the run.
Adding, removing, or configuring a producer SHALL take effect on all entry paths
from one edit; no entry path may register a different producer set than another
for the same config.

#### Scenario: an enabled producer runs on both entry paths

- **GIVEN** a config with `[language.markdown] enabled = true`
- **WHEN** an index run is triggered via the CLI **and** via the MCP/workflow path
- **THEN** both runs register the markdown producer and produce markdown nodes

#### Scenario: adding a producer cannot drift between paths

- **WHEN** a new producer is added to the index driver configuration
- **THEN** it is registered from the single shared configuration function used by
  every entry path, so no path silently omits it
