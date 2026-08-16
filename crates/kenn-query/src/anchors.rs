//! Directive tools: `find_directives` (path-anchored retrieval),
//! `check_anchors` (report unresolved anchors), and `record_anchor`
//! (append an `attach` / `rename` / `detach` event to a finding's anchor log).

use kenn_store::{AnchorEvent, Outcome, Timestamp};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{QueryError, QueryErrorCode};
use crate::types::{ListResponse, RankedFindingView, TOP_K_MATERIALIZE};

use crate::ctx::QueryCtx;
use crate::{db_to_mcp, embed_query, finding_to_view, internal};

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
    ctx: &QueryCtx<'_>,
    args: &FindDirectivesArgs,
) -> Result<ListResponse<RankedFindingView>, QueryError> {
    if args.paths.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "find_directives: empty paths",
        ));
    }
    let paths = args.paths.clone();
    // The semantic leg is optional: a cold embedder degrades to anchor-only
    // rather than erroring.
    let query_vec = match args.query.as_deref() {
        Some(q) if !q.is_empty() => match embed_query(q).await {
            Ok(v) => v,
            Err(e) if e.code == QueryErrorCode::EmbedderStarting => None,
            Err(e) => return Err(e),
        },
        _ => None,
    };
    let now = Timestamp::now();
    let store = ctx.findings_read().await?;
    let resolver = ctx.read.code_node_resolver().await.map_err(internal)?;
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
    ///
    /// Rules only. A rule survives edits to the file it is anchored to, so
    /// drift here is usually incidental and clearing it is routine.
    pub drifted: Vec<AnchorEntry>,
    /// **Claims** — findings asserting something about the current state of the
    /// code (a bug, a limitation, deferred work, a fix) — whose anchored content
    /// changed since the claim was recorded, so the assertion may no longer be
    /// true.
    ///
    /// Unlike `drifted`, this is not a routine re-read: whoever changed the code
    /// had no reason to look for a finding describing it, so a claim can quietly
    /// stop being true while still being served as fact. Re-verify against the
    /// current code and record the outcome. Do NOT clear these by re-attaching —
    /// `attach` means "this applied to my change" and asserts nothing about
    /// whether the claim holds.
    pub unverified: Vec<AnchorEntry>,
}

/// Report committed findings whose anchors no longer resolve on disk, so a
/// `rename`/`detach` can repair them before a commit. v1 anchors are file/dir
/// paths, checked against the workspace — no index needed.
pub async fn check_anchors(
    ctx: &QueryCtx<'_>,
    _args: &CheckAnchorsArgs,
) -> Result<CheckAnchorsResponse, QueryError> {
    let store = ctx.findings_read().await?;
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
        unverified: health
            .unverified
            .into_iter()
            .map(|u| AnchorEntry {
                finding_id: u.finding_id,
                anchors: u.anchors,
            })
            .collect(),
    })
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
    ctx: &QueryCtx<'_>,
    args: &RecordAnchorArgs,
) -> Result<RecordAnchorResponse, QueryError> {
    if args.finding_id.is_empty() {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            "record_anchor: empty finding_id",
        ));
    }
    let event = anchor_event(args, Timestamp::now(), &ctx.source_root)?;
    let finding_id = args.finding_id.clone();
    let store = ctx.findings_write().await?;
    record_event(&store, &finding_id, event).await
}

/// Build the anchor event named by `args.op`.
///
/// Split from `record_anchor` for the CRAP gate: the op match is six arms and
/// three of them validate their own required field, which is most of the
/// function's branches. Lifting it out leaves `record_anchor` as validate,
/// build, write.
fn anchor_event(
    args: &RecordAnchorArgs,
    ts: Timestamp,
    source_root: &std::path::Path,
) -> Result<AnchorEvent, QueryError> {
    let need = |field: &str, v: &Option<String>| -> Result<String, QueryError> {
        v.clone().filter(|s| !s.is_empty()).ok_or_else(|| {
            QueryError::new(
                QueryErrorCode::InvalidInput,
                format!("record_anchor: `{}` op needs `{field}`", args.op),
            )
        })
    };
    Ok(match args.op.as_str() {
        "attach" => {
            let anchor = need("anchor", &args.anchor)?;
            // Hash the live file at attach time so later drift can be detected;
            // a directory or unreadable path records no sha (→ live).
            let sha = kenn_store::file_content_sha(&source_root.join(&anchor));
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
        // Verification outcomes. Separate ops from `attach` on purpose: `attach`
        // says "this applied to my change" and the pre-commit ritual writes it
        // in bulk, so letting it double as verification would declare a store's
        // worth of claims re-read without anyone reading one.
        "verified" | "stale" | "partial" => {
            let anchor = need("anchor", &args.anchor)?;
            let sha = kenn_store::file_content_sha(&source_root.join(&anchor));
            let outcome = match args.op.as_str() {
                "verified" => Outcome::StillTrue,
                "stale" => Outcome::NoLongerTrue,
                _ => Outcome::PartlyTrue,
            };
            AnchorEvent::Verify {
                anchor,
                ts,
                sha,
                outcome,
            }
        }
        other => {
            return Err(QueryError::new(
                QueryErrorCode::InvalidInput,
                format!(
                    "record_anchor: unknown op `{other}` \
                     (attach|rename|detach|verified|stale|partial)"
                ),
            ))
        }
    })
}

/// Append the event to the finding's log, refusing an unknown finding id.
async fn record_event(
    store: &kenn_store::FindingsStore,
    finding_id: &str,
    event: AnchorEvent,
) -> Result<RecordAnchorResponse, QueryError> {
    // Reject an unknown finding id rather than create an orphan
    // `<id>.anchor.jsonl` for a finding that does not exist.
    if store
        .get_finding(finding_id)
        .await
        .map_err(internal)?
        .is_none()
    {
        return Err(QueryError::new(
            QueryErrorCode::InvalidInput,
            format!("record_anchor: no finding with id `{finding_id}`"),
        ));
    }
    store
        .record_anchor_event(finding_id, &event)
        .await
        .map_err(internal)?;
    Ok(RecordAnchorResponse { recorded: true })
}
