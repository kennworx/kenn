//! `kenn visualize` — render the graph (`[index] graph_path`, default
//! `kenn_graph.html`) from the live snapshot. Reads the aggregated graph
//! (and once §2 ships, the persisted analysis tables), computes the anchor
//! layout, writes HTML. Does NOT recompute clustering or god-nodes.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use kenn_analyze::{graph as graph_render, layout, projection};
use kenn_config::Config;
use kenn_store::{open_for_read, Layout, ReadContext};

use crate::exit::ExitCodes;

/// Resolve the layout algorithm in the precedence order documented for
/// `kenn visualize`: CLI value wins, otherwise `[visualize] layout`,
/// otherwise `"spectral"`. Returns the parsed algo, or the raw string
/// that failed to parse (so the caller can report it).
///
/// Pure function — all the precedence + parse branches are testable
/// without any I/O.
pub fn resolve_layout_algo(
    cli_value: Option<&str>,
    config_value: Option<&str>,
) -> Result<layout::LayoutAlgo, String> {
    let chosen = cli_value
        .map(str::to_string)
        .or_else(|| config_value.map(str::to_string))
        .unwrap_or_else(|| "spectral".to_string());
    layout::LayoutAlgo::parse(&chosen).ok_or(chosen)
}

/// Pull the workspace name out of a source root: the directory's leaf
/// name, or empty string if the path has no file name (e.g. `/`).
#[must_use]
pub fn workspace_name_of(source_root: &Path) -> String {
    source_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

/// Pick the snapshot directory the visualizer should read from. Wraps
/// `ReadContext` so the caller doesn't need to match on it.
pub fn select_snapshot(ctx: ReadContext) -> Result<PathBuf, String> {
    match ctx {
        ReadContext::Available { snapshot, .. } => Ok(snapshot),
        ReadContext::Tier2Unavailable => {
            Err("no live snapshot. Run `kenn index` first.".to_string())
        }
    }
}

/// Create `path` for writing, first creating its parent directory chain when
/// the path has a non-empty parent (a root-level output file has none).
fn create_with_parents(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::File::create(path)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "uniform by-value subcommand signature for CLI dispatch"
)]
pub fn run(layout_cfg: Layout, config: Config, algo: Option<&str>) -> Result<ExitCodes> {
    let layout_algo = match resolve_layout_algo(algo, config.visualize.layout.as_deref()) {
        Ok(a) => a,
        Err(bad) => {
            eprintln!(
                "error: visualize layout must be `spectral`, `force`, `stress`, or `linlog` (got {bad:?})"
            );
            return Ok(ExitCodes::Generic);
        }
    };

    let snapshot = match select_snapshot(open_for_read(&layout_cfg)) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("error: {msg}");
            return Ok(ExitCodes::Generic);
        }
    };

    let graph_path = layout_cfg.source_root().join(&config.index.graph_path);
    let workspace_name = workspace_name_of(layout_cfg.source_root());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let html_path = rt.block_on(async {
        let reader = kenn_store::open_reader(&snapshot)
            .await
            .context("open reader")?;
        let graph = projection::load_from_reader(&reader).await?;
        if graph.is_empty() {
            anyhow::bail!(
                "snapshot has no aggregate-graph tables — run `kenn index --force` to rebuild"
            );
        }
        let anchors = projection::AnchorMap::from_graph(&graph);
        let positions = layout::compute(&graph, &anchors, layout_algo);
        let path = graph_path.clone();
        let file = create_with_parents(&path)?;
        let mut writer = std::io::BufWriter::new(file);
        graph_render::render(&graph, &positions, &workspace_name, &mut writer)?;
        writer.flush()?;
        Ok::<_, anyhow::Error>(path)
    })?;

    println!("wrote {}", html_path.display());
    Ok(ExitCodes::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// `resolve_layout_algo` precedence: CLI wins, then config,
    /// then spectral default.
    #[test]
    fn resolve_layout_algo_cli_wins() {
        let a = resolve_layout_algo(Some("force"), Some("stress")).expect("ok");
        assert_eq!(a, layout::LayoutAlgo::Force);
    }

    #[test]
    fn resolve_layout_algo_falls_back_to_config() {
        let a = resolve_layout_algo(None, Some("stress")).expect("ok");
        assert_eq!(a, layout::LayoutAlgo::Stress);
    }

    #[test]
    fn resolve_layout_algo_defaults_to_spectral() {
        let a = resolve_layout_algo(None, None).expect("ok");
        assert_eq!(a, layout::LayoutAlgo::Spectral);
    }

    #[test]
    fn resolve_layout_algo_invalid_returns_raw_string() {
        let err = resolve_layout_algo(Some("circular"), None).expect_err("err");
        assert_eq!(err, "circular");
        // Config-only invalid value also surfaces.
        let err2 = resolve_layout_algo(None, Some("blob")).expect_err("err");
        assert_eq!(err2, "blob");
    }

    /// `workspace_name_of` returns the leaf name; `/` and weird inputs
    /// fall back to empty.
    #[test]
    fn workspace_name_of_returns_leaf() {
        assert_eq!(workspace_name_of(&PathBuf::from("/foo/bar")), "bar");
        assert_eq!(workspace_name_of(&PathBuf::from("/")), "");
        assert_eq!(workspace_name_of(&PathBuf::from(".")), "");
    }

    /// `select_snapshot` unwraps the `Available` variant and maps
    /// `Tier2Unavailable` to a human-readable error.
    #[test]
    fn select_snapshot_unwraps_available() {
        use kenn_store::worktree::ReadSource;
        let p = PathBuf::from("/snap");
        let ctx = ReadContext::Available {
            snapshot: p.clone(),
            source: ReadSource::Local,
        };
        assert_eq!(select_snapshot(ctx).expect("ok"), p);
    }

    #[test]
    fn select_snapshot_tier2_unavailable_is_error() {
        let err = select_snapshot(ReadContext::Tier2Unavailable).expect_err("err");
        assert!(err.contains("Run `kenn index`"));
    }
}
