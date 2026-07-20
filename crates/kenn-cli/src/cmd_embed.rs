//! `kenn embed` — the incremental embedding pass.
//!
//! `kenn index` builds the structural store and reconciles cached
//! vectors from the committed sidecar; `kenn embed` then embeds only the
//! symbols left null, appends a new sidecar segment, and republishes the
//! store with a rebuilt vector index. The MCP server runs the same job
//! automatically on cold start, so this command is the headless / CI
//! trigger.

use std::time::Instant;

use anyhow::Result;
use kenn_config::Config;
use kenn_store::Layout;

use crate::exit::ExitCodes;

#[expect(
    clippy::needless_pass_by_value,
    reason = "uniform by-value subcommand signature for CLI dispatch"
)]
pub fn run(layout: Layout, config: Config, model_id: &str) -> Result<ExitCodes> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    println!(
        "embedding pending symbols for {}",
        layout.source_root().display()
    );
    let wall = Instant::now();
    let embedder = kenn_store::shared_embedder();
    let report = rt.block_on(kenn_store::embed_pending(
        &layout,
        config.staleness.git_aware_skip,
        config.indexing_signature(),
        model_id,
        embedder,
    ))?;
    println!(
        "embedded {} new vectors in {:.1}s wall ({:.1}s in the model)",
        report.vectors,
        wall.elapsed().as_secs_f64(),
        report.embed_seconds,
    );
    Ok(ExitCodes::Ok)
}
