//! `kenn update` — the embedding pass.
//!
//! `kenn index` builds the structural store only (fast, even on a large
//! repo). `kenn update` then embeds the committed code rows so hybrid /
//! vector search and `find_similar` light up. It is a separate flow so a
//! routine re-index never pays the embedding cost.

use std::time::Instant;

use anyhow::Result;
use kenn_config::Config;
use kenn_store::Layout;

use crate::exit::ExitCodes;

#[expect(
    clippy::needless_pass_by_value,
    reason = "uniform by-value subcommand signature for CLI dispatch"
)]
pub fn run(layout: Layout, config: Config) -> Result<ExitCodes> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    println!("updating embeddings for {}", layout.source_root().display());
    let wall = Instant::now();
    let embedder = kenn_store::shared_embedder();
    let report = rt.block_on(kenn_store::reembed(
        &layout,
        config.staleness.git_aware_skip,
        config.indexing_signature(),
        embedder,
    ))?;
    println!(
        "embedded {} vectors (one per symbol) in {:.1}s wall ({:.1}s in the model)",
        report.vectors,
        wall.elapsed().as_secs_f64(),
        report.embed_seconds,
    );
    Ok(ExitCodes::Ok)
}
