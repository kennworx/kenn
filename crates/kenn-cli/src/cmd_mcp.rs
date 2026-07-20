use anyhow::Result;
use kenn_config::Config;
use kenn_mcp::server::serve_stdio;
use kenn_mcp::WorkspaceSource;
use kenn_store::Layout;

use crate::exit::ExitCodes;

pub fn run(
    layout: Layout,
    config: Config,
    source: WorkspaceSource,
    model_id: String,
) -> Result<ExitCodes> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(serve_stdio(config, layout, source, model_id))?;
    Ok(ExitCodes::Ok)
}
