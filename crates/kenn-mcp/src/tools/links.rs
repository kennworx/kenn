//! `check_links` — the markdown link-health report (index-markdown task 7.1).
//!
//! The read path for the `link_grade` edge column: lists markdown links that are
//! not `exact` — drifted (path/qualifier stale), fuzzy, ambiguous (one of
//! several kept candidates), or dangling (written but unresolved) — with the
//! linking section and the resolved (or written) target. File-target links
//! (`links_to_file`) render the code file path; the file/symbol id collision
//! never bites because the store hydrates them by edge kind.
//!
//! Output is bounded: at most `limit` rows are returned (with the full matching
//! `total` so the caller sees truncation), optionally filtered to specific
//! grades — a rotted corpus must not flood the response.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{McpError, McpErrorCode};
use crate::tools::support::internal;
use crate::tools::ServerState;

/// Default / maximum number of links returned in one call.
const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1000;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct CheckLinksArgs {
    /// Restrict to these grades: `drifted`, `fuzzy`, `ambiguous`, `dangling`.
    /// Omit for every non-exact link.
    #[serde(default)]
    pub grade: Option<Vec<String>>,
    /// Max links returned (default 100, max 1000). `total` is always the full
    /// matching count regardless of this cap.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// One problematic markdown link.
#[derive(Debug, Serialize)]
pub struct LinkDiagnostic {
    /// Public id of the linking markdown section/document.
    pub src: String,
    /// `path#L<line>` of the linking section, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Edge relation: `links_to`, `embeds`, or `links_to_file`.
    pub kind: String,
    /// Link grade: `drifted`, `fuzzy`, `ambiguous`, or `dangling`.
    pub grade: String,
    /// The resolved target — a markdown/code symbol id or a code file path —
    /// or, for a dangling link, the written-but-unresolved target.
    pub target: String,
}

/// The `check_links` response.
#[derive(Debug, Serialize)]
pub struct CheckLinksResponse {
    /// Full count of matching (non-exact, grade-filtered) links.
    pub total: u64,
    /// Number actually returned in `links` (≤ `total`, ≤ `limit`).
    pub returned: usize,
    /// True when `total > returned` — refine `grade` or raise `limit` to see more.
    pub truncated: bool,
    pub links: Vec<LinkDiagnostic>,
}

/// Prefix of an unresolved (dangling) link's stub `pub_id`; the suffix is the
/// target the author wrote, shell-safe-escaped (the `pub_id` is a shell token).
const UNRESOLVED_PREFIX: &str = "md:@unresolved/";

/// Map a grade name to its stored discriminant, erroring on an unknown name so
/// a typo'd filter fails loudly rather than silently matching nothing.
fn grade_code(name: &str) -> Result<u8, McpError> {
    match name {
        "drifted" => Ok(1),
        "fuzzy" => Ok(2),
        "ambiguous" => Ok(3),
        "dangling" => Ok(4),
        other => Err(McpError::new(
            McpErrorCode::InvalidInput,
            format!("check_links: unknown grade `{other}` — use drifted|fuzzy|ambiguous|dangling"),
        )),
    }
}

/// List the non-exact markdown links in the current index, bounded + filtered.
pub async fn check_links(
    state: &ServerState,
    args: &CheckLinksArgs,
) -> Result<CheckLinksResponse, McpError> {
    let limit = args.limit.map_or(DEFAULT_LIMIT, |l| l.clamp(1, MAX_LIMIT));
    let grade_codes = match &args.grade {
        Some(names) => Some(
            names
                .iter()
                .map(|n| grade_code(n))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        None => None,
    };
    state
        .with_db(|h| async move {
            let (rows, total) = h
                .read
                .scan_link_diagnostics(grade_codes, limit)
                .await
                .map_err(internal)?;
            let returned = rows.len();
            let links: Vec<LinkDiagnostic> = rows
                .into_iter()
                .map(|r| LinkDiagnostic {
                    src: r.src_pub_id,
                    location: r.location,
                    kind: r.kind,
                    grade: r.grade,
                    // A dangling target is a stub id; surface the written target.
                    target: r
                        .target
                        .strip_prefix(UNRESOLVED_PREFIX)
                        .map_or_else(|| r.target.clone(), ToString::to_string),
                })
                .collect();
            Ok(CheckLinksResponse {
                total,
                returned,
                truncated: total > returned as u64,
                links,
            })
        })
        .await
}
