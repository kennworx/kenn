//! `kenn status` — print the current state of the local `.kenn/`.
//!
//! Reads the live snapshot's `meta.json` (written by `cmd_index` at publish
//! time) and prints a human-readable summary or one JSON blob.

use anyhow::Result;
use kenn_indexer::{SnapshotMeta, SNAPSHOT_META_FILE};
use kenn_store::{open_for_read, Layout, ReadContext, ReadSource, Store};
use serde::Serialize;

use crate::exit::ExitCodes;

#[expect(
    clippy::needless_pass_by_value,
    reason = "uniform by-value subcommand signature for CLI dispatch"
)]
pub fn run(layout: Layout, json: bool) -> Result<ExitCodes> {
    let ctx = open_for_read(&layout);
    let (report, schema_mismatch) = match &ctx {
        ReadContext::Available { snapshot, source } => {
            let meta_path = snapshot.join(SNAPSHOT_META_FILE);
            let meta = if meta_path.exists() {
                Some(serde_json::from_slice::<SnapshotMeta>(&std::fs::read(
                    &meta_path,
                )?)?)
            } else {
                None
            };
            // Compare persisted schema_version against the binary's
            // STORE_SCHEMA_VERSION so the agent / user sees the mismatch
            // explicitly (and the CLI exits non-zero) rather than waiting
            // for a downstream tool to fail on open. Pre-versioning
            // snapshots resolve to v1 per the store-layout requirement.
            let persisted = meta.as_ref().and_then(|m| m.schema_version).unwrap_or(1);
            let mismatch = if persisted == kenn_store::STORE_SCHEMA_VERSION {
                None
            } else {
                Some((persisted, kenn_store::STORE_SCHEMA_VERSION))
            };
            (
                StatusReport::Available {
                    snapshot: snapshot.clone(),
                    source: source.clone(),
                    meta: Box::new(meta),
                    schema_mismatch: mismatch,
                    // Live embed health (from the last embed pass), independent
                    // of the persisted meta.json: `Some` only when the embedder
                    // is degraded (a model is configured but embedding failed).
                    embed_error: kenn_store::read_embed_error(&layout),
                },
                mismatch,
            )
        }
        ReadContext::Tier2Unavailable => {
            // Distinguish "no live anywhere" from "uninitialized" only by
            // hint text; structurally the answer is the same.
            let initialized = Store::open(layout.clone())
                .is_ok_and(|s| s.live_path().exists() || s.runs_dir().exists());
            (StatusReport::Tier2Unavailable { initialized }, None)
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
    }
    Ok(if schema_mismatch.is_some() {
        ExitCodes::Generic
    } else {
        ExitCodes::Ok
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum StatusReport {
    Available {
        snapshot: std::path::PathBuf,
        source: ReadSource,
        meta: Box<Option<SnapshotMeta>>,
        /// `(persisted, expected)` when the snapshot's schema version
        /// disagrees with the binary's `STORE_SCHEMA_VERSION`. `None`
        /// otherwise.
        #[serde(skip_serializing_if = "Option::is_none")]
        schema_mismatch: Option<(u32, u32)>,
        /// The last embed pass's backend error, when the embedder is degraded
        /// (a model is configured but embedding failed). `None` when healthy,
        /// disabled, or never run.
        #[serde(skip_serializing_if = "Option::is_none")]
        embed_error: Option<String>,
    },
    Tier2Unavailable {
        initialized: bool,
    },
}

/// The provisioned-toolchain line of the run summary, shown only when a
/// toolchain was recorded (the docker/on-demand path) — a plain `kenn index`
/// with everything on `PATH` records none, so the line stays absent there.
fn print_toolchains(m: &SnapshotMeta) {
    if !m.toolchains.is_empty() {
        println!(
            "toolchains: {}",
            kenn_indexer::render_toolchains(&m.toolchains)
        );
    }
}

fn print_human(r: &StatusReport) {
    match r {
        StatusReport::Available {
            snapshot,
            source,
            meta,
            schema_mismatch,
            embed_error,
        } => {
            let label = match source {
                ReadSource::Local => "local".to_string(),
                ReadSource::FallbackFromParent { parent } => {
                    format!("fallback: parent ({})", parent.display())
                }
            };
            println!("snapshot: {}", snapshot.display());
            println!("source:   {label}");
            if let Some(m) = meta.as_ref() {
                println!("status:   {}", m.status);
                println!(
                    "counts:   documents={} symbols={} definitions={} edges={}",
                    m.documents, m.symbols, m.definitions, m.edges
                );
                print_toolchains(m);
                // The true counts include attributions dropped past the
                // per-unit retention cap; the overflow renders as `+N more`.
                let failed_total = m.failed_total();
                if failed_total > 0 {
                    println!(
                        "failed projects ({failed_total}): {}",
                        kenn_indexer::report::render_with_overflow(
                            &m.failed_projects,
                            m.failed_overflow
                        )
                        .join(", ")
                    );
                }
                let warning_total = m.warning_total();
                if warning_total > 0 {
                    println!(
                        "warnings ({warning_total}): {}",
                        kenn_indexer::report::render_with_overflow(
                            &m.warnings,
                            m.warnings_overflow
                        )
                        .join(", ")
                    );
                }
                for w in &m.regression_warnings {
                    println!(
                        "regression: {}: {} → {} ({}% drop)",
                        w.metric, w.previous, w.current, w.drop_pct
                    );
                }
            } else {
                println!("status:   meta.json missing — older snapshot or partial publish");
            }
            if let Some((persisted, expected)) = schema_mismatch {
                eprintln!("schema:   v{persisted} (binary expects v{expected}) — reindex required");
            }
            if let Some(cause) = embed_error {
                println!("embedder: degraded — {cause}");
            }
        }
        StatusReport::Tier2Unavailable { initialized } => {
            if *initialized {
                println!("status:   no live snapshot. Run `kenn index`.");
            } else {
                println!("status:   uninitialized. Run `kenn init`.");
            }
        }
    }
}
