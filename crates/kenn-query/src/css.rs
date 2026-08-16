//! `check_css` — the dead-CSS report (index-css Group 9).
//!
//! The read path for the stylesheet graph: lists classes nothing uses
//! (`orphan_class`) and stylesheets nothing imports whose selectors are unused
//! (`orphan_stylesheet`). Orphan-class detection needs class-usage mining to
//! have run (a configured `[language.css] usage_sources`); when it hasn't, every
//! class would look unused, so that category is skipped and a `note` says so.
//!
//! Output is bounded: at most `limit` rows (classes first), with the full
//! per-category `total` so the caller sees truncation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ctx::QueryCtx;
use crate::error::{QueryError, QueryErrorCode};
use crate::support::internal;

/// Default / maximum number of findings returned in one call.
const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1000;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct CheckCssArgs {
    /// Restrict to these categories: `orphan_class`, `orphan_stylesheet`.
    /// Omit for both.
    #[serde(default)]
    pub category: Option<Vec<String>>,
    /// Max findings returned (default 100, max 1000). `total` is always the full
    /// matching count regardless of this cap.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// One dead-CSS finding.
#[derive(Debug, Serialize)]
pub struct CssDiagnostic {
    /// `orphan_class` or `orphan_stylesheet`.
    pub category: String,
    /// Public id of the class or stylesheet `module` node.
    pub pub_id: String,
    /// `path#L<line>` (class) or `path` (stylesheet), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

/// The `check_css` response.
#[derive(Debug, Serialize)]
pub struct CheckCssResponse {
    /// Full count of matching findings across the requested categories.
    pub total: u64,
    /// Number actually returned in `findings` (≤ `total`, ≤ `limit`).
    pub returned: usize,
    /// True when `total > returned` — refine `category` or raise `limit`.
    pub truncated: bool,
    /// Set when orphan-class detection was skipped because usage mining is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub findings: Vec<CssDiagnostic>,
}

/// Validate a category name, erroring on an unknown one so a typo fails loudly.
fn want(categories: Option<&Vec<String>>) -> Result<(bool, bool), QueryError> {
    let Some(names) = categories else {
        return Ok((true, true));
    };
    let (mut classes, mut sheets) = (false, false);
    for name in names {
        match name.as_str() {
            "orphan_class" => classes = true,
            "orphan_stylesheet" => sheets = true,
            other => {
                return Err(QueryError::new(
                    QueryErrorCode::InvalidInput,
                    format!(
                        "check_css: unknown category `{other}` — use orphan_class|orphan_stylesheet"
                    ),
                ))
            }
        }
    }
    Ok((classes, sheets))
}

/// List dead CSS in the current index, bounded + category-filtered.
pub async fn check_css(
    ctx: &QueryCtx<'_>,
    args: &CheckCssArgs,
) -> Result<CheckCssResponse, QueryError> {
    let limit = args.limit.map_or(DEFAULT_LIMIT, |l| l.clamp(1, MAX_LIMIT));
    let (want_classes, want_sheets) = want(args.category.as_ref())?;
    let (rows, counts) = ctx
        .read
        .scan_css_health(want_classes, want_sheets, limit)
        .await
        .map_err(internal)?;
    let total = counts.orphan_classes + counts.orphan_stylesheets;
    let returned = rows.len();
    let note = (want_classes && !counts.usage_mining_on).then(|| {
        "orphan_class skipped: no class-usage mining ran — set [language.css] \
                 usage_sources to map where classes are used"
            .to_string()
    });
    let findings = rows
        .into_iter()
        .map(|r| CssDiagnostic {
            category: r.category,
            pub_id: r.pub_id,
            location: r.location,
        })
        .collect();
    Ok(CheckCssResponse {
        total,
        returned,
        truncated: total > returned as u64,
        note,
        findings,
    })
}
