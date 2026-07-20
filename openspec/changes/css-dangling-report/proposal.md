## Why

> **Status: DEFERRED / split out of `index-css`.** Captures the one `check_css`
> category that needs a new persistence path, so `index-css` could archive with
> its 32 substantive tasks done. Not scheduled.

`check_css` reports two hygiene categories today — `orphan_class` (defined, zero
usages) and `orphan_stylesheet` (no used selectors, not imported). Both report
over data **already in the graph**. The third category from the `index-css`
design — **dangling code class** (a class *used* in code with no definition,
and not a known utility) — cannot, because of a deliberate design choice:

> an unmatched class token produces **no edge and no node** (so Tailwind
> utilities, which have no definition, don't inflate the graph).

So the undefined tokens that *are* the dangling candidates are computed during
indexing (`UsageScan.undefined` in `crates/kenn-indexer/src/css/usage.rs`) and
then **discarded** — only a count survives in `CssUsageCounts.undefined`, and
even that is dropped in the pipeline. There is nothing in the store for
`check_css` to read.

Reporting dangling classes therefore requires *persisting* the undefined tokens
— but only the genuinely-dangling ones. Without a utility allowlist, every
Tailwind utility is "undefined" and the report is pure noise. Hence the task's
gate: **flag only when an allowlist is configured.** With an allowlist, the
indexer's `is_utility` filter (currently hardcoded `|_| false`) removes the
utilities, leaving real typos / missing styles.

## What Changes

- Add a **`utility_allowlist`** field to `CssConfig`
  (`crates/kenn-config/src/language/css.rs`) — a list/glob of known utility
  class names (e.g. Tailwind prefixes). Empty by default → category inactive.
- Wire the allowlist into the usage resolver: replace the hardcoded
  `is_utility = |_| false` (`crates/kenn-indexer/src/css/ingest.rs:210`) with a
  matcher built from config.
- **Persist** the filtered `UsageScan.undefined` tokens (file + class name)
  during indexing — a new lightweight store table, written only when an
  allowlist is configured (empty otherwise, so no cost in the common case).
- Add a reader query + a `dangling_class` category to `check_css`
  (`scan_css_health`, `CssHealthCounts`, the MCP `want()` validator), mirroring
  the existing two categories.
- Tests: dangling flagged only with an allowlist present; not flagged without.

## Capabilities

### Modified Capabilities

- `css-usage-graph`: `check_css` gains a third hygiene category,
  `dangling_class`, gated on a configured `utility_allowlist`.

## Impact

- **Schema:** one new table for persisted undefined tokens (additive).
- **Config:** new optional `utility_allowlist` field; absent → category off.
- **Indexing:** a reindex is needed to populate the table; MCP reload to serve.
- **Scope:** ~150–200 lines across config, indexer, store, and the MCP tool.
