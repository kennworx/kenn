## Why

Working with Claude produces many small decisions about specific parts of the code — why a function was structured a way, which alternatives were rejected, what tradeoffs were considered. The chosen path lands in git; the reasoning evaporates.

Before we design the ingest pipeline (Lance + embedding + LLM enrichment + MCP) for this data, we want a real corpus to look at. This change is the **capture-only first slice**: wire up Claude Code hooks to a `kenn cc-hook` CLI that appends raw JSONL into the kenn store, and stop. Follow-up changes will add ingest into Lance, verdict/topic enrichment, and MCP read tools — informed by what the raw capture actually looks like in practice.

## What Changes

- Add a `kenn cc-hook <event>` CLI subcommand to kenn-cli, accepting Claude Code hook JSON on stdin and appending a single raw record per call to `history/raw/<session_id>.jsonl` in the kenn store. Supported events: `session-start`, `prompt`, `touch`, `session-end`.
- Add `history/raw/` and `history/ready/` slots to the kenn store layout (siblings of `findings/`). The `session-end` event additionally writes a `history/ready/<session_id>` marker so a future ingest pass can find finished sessions.
- Add a `kenn cc-hook install` subsubcommand that prints the required hook-config snippet for `~/.claude/settings.json` (and optionally writes it).
- Document the trust boundary — raw prompt and response text is stored verbatim — so users opt in informed.
- **Not in this change** (explicit follow-ups): processing raw JSONL into Lance, prompt-text embedding, LLM verdict classification, session-topic generation, the MCP read surface (`conversation_history`, `conversation_search`, `session_summary`), and the system-prompt fragment that drives agent lookup.

## Capabilities

### New Capabilities
- `conversation-history-store`: append-only raw conversation-event store fed by Claude Code hooks via `kenn cc-hook`. This change scopes the capability to the capture surface only; ingest into Lance and downstream enrichment are deferred to follow-up changes that extend this same capability.

### Modified Capabilities
<!-- None. -->

## Impact

- **New kenn CLI subcommand**: `kenn cc-hook ...`. Touches kenn-cli.
- **New storage area**: `history/raw/` and `history/ready/` in the kenn store layout. Coordinated with the active `vector-store-layout-cleanup` change to avoid layout conflict.
- **External dependency**: none — pure file appends, no LLM, no embedding, no network.
- **User-visible setup**: opt-in via a documented hook-config snippet (or `kenn cc-hook install`).
- **Privacy boundary**: raw prompt/response text stored verbatim — same trust boundary as findings.
- **Out of scope for v1**: Lance ingest, LLM enrichment, MCP tools, multi-user sharing tests, Bash-driven file mutations.
