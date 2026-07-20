## ADDED Requirements

### Requirement: Dangling code classes are reported when a utility allowlist is configured

The `check_css` report SHALL surface a `dangling_class` category — a class-shaped
token *used* in code that matches no class definition in the registry and is not
a known utility. Because an unmatched token deliberately produces no node or edge,
the indexer SHALL persist the filtered set of undefined tokens (file + class name)
when, and only when, a `utility_allowlist` is configured. With no allowlist every
utility is undefined and the category SHALL be inactive (no persistence, no
output), avoiding noise from utility frameworks such as Tailwind.

#### Scenario: A used, undefined, non-utility class is flagged with an allowlist

- **WHEN** `utility_allowlist` is configured
- **AND** a source file uses class `btn-primmary` (a typo) that no stylesheet defines
- **AND** `btn-primmary` is not in the allowlist
- **THEN** `check_css` reports `btn-primmary` under the `dangling_class` category

#### Scenario: No allowlist means the category is inactive

- **WHEN** `utility_allowlist` is empty
- **AND** a source file uses many undefined utility tokens
- **THEN** no undefined tokens are persisted
- **AND** `check_css` emits no `dangling_class` findings
