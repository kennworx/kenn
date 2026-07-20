//! Directive tools: `find_directives` (path-anchored retrieval),
//! `check_anchors` (report unresolved anchors), and `record_anchor`
//! (append an `attach` / `rename` / `detach` event to a finding's anchor log).

use kenn_store::{AnchorEvent, Timestamp};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{McpError, McpErrorCode};
use crate::types::{ListResponse, RankedFindingView, TOP_K_MATERIALIZE};

use super::{db_to_mcp, embed_query, finding_to_view, internal, ServerState};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct FindDirectivesArgs {
    /// Changed file and/or directory paths (e.g. from `git diff --staged`).
    /// A directive anchored to a file matches that file; a directive anchored
    /// to a directory matches any path beneath it.
    pub paths: Vec<String>,
    /// Optional natural-language description of the change. When supplied it
    /// enables the semantic leg (body-vector proximity); omit for anchor-only
    /// retrieval. Ignored when the embedder is still warming up.
    #[serde(default)]
    pub query: Option<String>,
}

/// Retrieve directives/guides relevant to the given changed paths, ranked by
/// anchor match and recency-weighted liveness (plus body-vector proximity when
/// `query` is supplied). Superseded/tombstoned findings are excluded and each
/// hit carries a `stale` flag. Degrades to anchor-only when the embedder is
/// cold — directives work before the index is built.
pub async fn find_directives(
    state: &ServerState,
    args: &FindDirectivesArgs,
) -> Result<ListResponse<RankedFindingView>, McpError> {
    if args.paths.is_empty() {
        return Err(McpError::new(
            McpErrorCode::InvalidInput,
            "find_directives: empty paths",
        ));
    }
    let paths = args.paths.clone();
    // The semantic leg is optional: a cold embedder degrades to anchor-only
    // rather than erroring.
    let query_vec = match args.query.as_deref() {
        Some(q) if !q.is_empty() => match embed_query(q).await {
            Ok(v) => v,
            Err(e) if e.code == McpErrorCode::EmbedderStarting => None,
            Err(e) => return Err(e),
        },
        _ => None,
    };
    let now = Timestamp::now();
    state
        .with_findings_read(|h, store| {
            Box::pin(async move {
                let resolver = h.read.code_node_resolver().await.map_err(internal)?;
                let hits = store
                    .find_directives(
                        &paths,
                        query_vec.as_deref(),
                        now,
                        TOP_K_MATERIALIZE as usize,
                        &resolver,
                    )
                    .await
                    .map_err(db_to_mcp)?;
                Ok(ListResponse {
                    items: hits
                        .into_iter()
                        .map(|hit| RankedFindingView {
                            finding: finding_to_view(hit.finding, hit.stale, hit.drifted),
                            score: f64::from(hit.score),
                        })
                        .collect(),
                    next: None,
                })
            })
        })
        .await
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct CheckAnchorsArgs {}

/// One finding with anchors that no longer resolve on disk (broken) or whose
/// anchored files changed content since attach (drifted).
#[derive(Debug, Serialize)]
pub struct AnchorEntry {
    pub finding_id: String,
    pub anchors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CheckAnchorsResponse {
    /// Findings whose anchored file/dir paths no longer exist — repair each
    /// with a `record_anchor` `rename`/`detach` before committing.
    pub broken: Vec<AnchorEntry>,
    /// Findings whose anchored **files** still exist but changed content since
    /// the finding was anchored — re-read each before relying on it (a stale
    /// directive), then re-`attach` to refresh its sha.
    pub drifted: Vec<AnchorEntry>,
}

/// Report committed findings whose anchors no longer resolve on disk, so a
/// `rename`/`detach` can repair them before a commit. v1 anchors are file/dir
/// paths, checked against the workspace — no index needed.
pub async fn check_anchors(
    state: &ServerState,
    _args: &CheckAnchorsArgs,
) -> Result<CheckAnchorsResponse, McpError> {
    state
        .with_findings_read(|_h, store| {
            Box::pin(async move {
                let health = store.check_anchors().await.map_err(db_to_mcp)?;
                Ok(CheckAnchorsResponse {
                    broken: health
                        .broken
                        .into_iter()
                        .map(|b| AnchorEntry {
                            finding_id: b.finding_id,
                            anchors: b.anchors,
                        })
                        .collect(),
                    drifted: health
                        .drifted
                        .into_iter()
                        .map(|d| AnchorEntry {
                            finding_id: d.finding_id,
                            anchors: d.anchors,
                        })
                        .collect(),
                })
            })
        })
        .await
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct RecordAnchorArgs {
    /// The finding (`fnd_…`) whose anchor log to append to.
    pub finding_id: String,
    /// The event kind: `attach` (apply to / re-confirm a path — the liveness
    /// signal), `detach` (no longer applies), or `rename` (a path moved).
    pub op: String,
    /// The path, for `attach` / `detach`.
    #[serde(default)]
    pub anchor: Option<String>,
    /// The old path, for `rename`.
    #[serde(default)]
    pub from: Option<String>,
    /// The new path, for `rename`.
    #[serde(default)]
    pub to: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RecordAnchorResponse {
    pub recorded: bool,
}

/// Append an anchor event to a finding's `<id>.anchor.jsonl` log. A repeat
/// `attach` to a path already in the set is the liveness signal.
pub async fn record_anchor(
    state: &ServerState,
    args: &RecordAnchorArgs,
) -> Result<RecordAnchorResponse, McpError> {
    if args.finding_id.is_empty() {
        return Err(McpError::new(
            McpErrorCode::InvalidInput,
            "record_anchor: empty finding_id",
        ));
    }
    let ts = Timestamp::now();
    let need = |field: &str, v: &Option<String>| -> Result<String, McpError> {
        v.clone().filter(|s| !s.is_empty()).ok_or_else(|| {
            McpError::new(
                McpErrorCode::InvalidInput,
                format!("record_anchor: `{}` op needs `{field}`", args.op),
            )
        })
    };
    let event = match args.op.as_str() {
        "attach" => {
            let anchor = need("anchor", &args.anchor)?;
            // Hash the live file at attach time so later drift can be detected;
            // a directory or unreadable path records no sha (→ live).
            let sha = kenn_store::file_content_sha(&state.source_root().join(&anchor));
            AnchorEvent::Attach { anchor, ts, sha }
        }
        "detach" => AnchorEvent::Detach {
            anchor: need("anchor", &args.anchor)?,
            ts,
        },
        "rename" => AnchorEvent::Rename {
            from: need("from", &args.from)?,
            to: need("to", &args.to)?,
            ts,
        },
        other => {
            return Err(McpError::new(
                McpErrorCode::InvalidInput,
                format!("record_anchor: unknown op `{other}` (attach|rename|detach)"),
            ))
        }
    };
    let finding_id = args.finding_id.clone();
    state
        .with_findings_write(|_h, store| {
            Box::pin(async move {
                // Reject an unknown finding id rather than create an orphan
                // `<id>.anchor.jsonl` for a finding that does not exist.
                if store
                    .get_finding(&finding_id)
                    .await
                    .map_err(internal)?
                    .is_none()
                {
                    return Err(McpError::new(
                        McpErrorCode::InvalidInput,
                        format!("record_anchor: no finding with id `{finding_id}`"),
                    ));
                }
                store
                    .record_anchor_event(&finding_id, &event)
                    .await
                    .map_err(internal)?;
                Ok(RecordAnchorResponse { recorded: true })
            })
        })
        .await
}
