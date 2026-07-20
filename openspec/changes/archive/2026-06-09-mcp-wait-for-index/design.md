## Context

`kenn mcp` binds stdio immediately and indexes in the background. The
lifecycle is `Indexing → Ready` (cold start) / `Failed`, with `Ready`
non-terminal (a background reindex may run while serving the current
snapshot). Data tools fail fast with `INDEX_UNAVAILABLE` while not
`Ready`; `get_index_status` is the one tool that always succeeds.

Two observed failure modes motivated this change:

1. An agent's first orientation call races indexing. There is no way to
   *wait* — the agent must re-poll `get_index_status` by hand or give up.
2. The server can serve an **empty `Ready` snapshot**. Because cold-start
   skips to any retained snapshot whose `StalenessKey` matches, a
   zero-symbol snapshot published under the workspace's key (e.g. a prior
   index run where the language server failed to launch) is served as a
   settled `Ready`. The agent sees `symbol_count: 0` / a
   `not-initialized` hint and concludes "the index isn't built."

## Goals / Non-Goals

**Goals:**
- A blocking `wait_for_index` tool with a bounded timeout, so an agent
  can wait for the index to settle instead of polling or bailing.
- Cold start does not present an empty/stale snapshot as a settled
  `Ready` when the config expects symbols — it re-indexes once.
- Keep all existing data tools fail-fast; do not introduce hidden
  blocking anywhere else.

**Non-Goals:**
- No change to the staleness key algorithm or the snapshot format.
- No retry/backoff machinery beyond a single cold-start re-index.
- Not fixing *why* an indexer subprocess might yield zero symbols — only
  not serving that result as a clean skippable snapshot.
- No change to the `Failed`-state recovery contract (already covered by
  `index_unavailable_failed`).

## Decisions

### D1: A dedicated `wait_for_index` tool, not a `wait_ms` param on `get_index_status`

A separate tool keeps `get_index_status` a pure, instant snapshot (its
contract — "returned without delay (< 100ms)" — stays intact) and makes
the blocking behavior discoverable in `tools/list`. The wait tool reuses
the `IndexStatus` payload builder so the two never drift.

*Alternative considered:* overload `get_index_status` with an optional
`wait_ms`. Rejected — it muddies a tool whose spec promises immediacy and
forces every caller to reason about a blocking branch.

### D2: Settle predicate = `Ready && !reindex_in_progress`, or `Failed`

The tool waits while *unsettled*: `Indexing`, or `Ready` with a
background reindex in flight. This single predicate covers both the
cold-start case (wait for first `Ready`) and the empty-snapshot-being-
repopulated case (wait for the background reindex to swap in a populated
snapshot). `Failed` is terminal, so it returns immediately.

### D3: Poll, don't condvar

The handler loops: read the lifecycle state under the lock, drop the
lock, and `tokio::sleep` a short interval (~250 ms) until settled or the
deadline passes. Polling is simple and correct; the lock is never held
across a sleep, so concurrent dispatch is unaffected. A `Notify`-based
wakeup would be marginally tighter but adds wiring to every transition
site for no agent-visible benefit at these timescales.

This is an *explicitly* blocking, opt-in tool with a hard-capped
timeout — distinct from the rule that hot-path data handlers must never
block on daemon/model startup. The data tools stay fail-fast.

### D4: Cold-start hardening lives in the startup decision, gated on "config expects symbols"

After the freshness check selects a snapshot to skip to, if that
snapshot has zero symbols **and** at least one language is enabled, the
server re-indexes instead of serving it. The "≥1 language enabled" gate
is what prevents a re-index loop: a no-`kenn.toml` / all-disabled
workspace does not expect symbols, so its empty snapshot is served as
`Ready` with the existing config-hint and never triggers a re-index.

The re-index runs at most once per cold start (one startup decision per
process launch), so even a language-enabled-but-genuinely-empty
workspace re-indexes once and then settles `Ready` with the
`configured-but-empty` hint — no within-session loop.

*Alternative considered:* "re-index on any empty snapshot." Rejected —
it would re-index a legitimately empty workspace on every launch and
provides no benefit there.

*Alternative considered:* route a zero-symbol-under-enabled-config run to
`Failed`. Rejected for this change — `Failed` is for pipeline errors; an
empty result is not strictly an error, and the `configured-but-empty`
hint already guides the operator to `kenn status` / `report.json`.

## Risks / Trade-offs

- [A genuinely empty but language-enabled workspace re-indexes on every
  cold start] → The re-index is cheap when there is nothing to index, and
  it correctly picks up files that appeared since the last launch. Bounded
  to once per process start; never loops within a session.
- [`wait_for_index` holds an agent turn for up to the timeout] → Hard cap
  (120 s) plus a sane default (30 s); `timed_out` lets the agent decide
  whether to keep waiting. The lifecycle lock is never held across the
  wait, so other dispatch is unaffected.
- [Polling granularity (~250 ms) adds latency to the settle detection] →
  Negligible against indexing timescales; the tradeoff buys simplicity
  over threading a notifier through every transition.
- [Determining "zero symbols" at startup costs a count on the candidate
  snapshot] → A single cheap count against the already-open reader on the
  skip path; only runs at cold start, not per tool call.

## Open Questions

- None blocking. The exact poll interval and timeout default/cap are
  implementation constants chosen above (250 ms / 30 s / 120 s) and can
  be tuned without a spec change.
