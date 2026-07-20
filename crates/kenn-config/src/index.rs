//! `[index]` and `[index.analysis]` sections — what `kenn index` does
//! after writing the per-symbol + aggregate tables.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexConfig {
    /// Compute the derived analysis (anchor map, hierarchical Louvain,
    /// flat Louvain, god-nodes) and persist it in the snapshot. When
    /// false, no analysis tables are written and `kenn visualize` will
    /// error out against the resulting snapshot.
    #[serde(default = "default_true")]
    pub persist_analysis: bool,
    /// Output path for `kenn visualize`'s graph, relative to the workspace
    /// root (absolute paths are honored). Default: `kenn_graph.html`.
    #[serde(default = "default_graph_path")]
    pub graph_path: String,
    /// Knobs for the analysis itself (top-N, hierarchy depth, minimum
    /// community size). Used at index time only.
    #[serde(default)]
    pub analysis: IndexAnalysisOptions,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            persist_analysis: true,
            graph_path: default_graph_path(),
            analysis: IndexAnalysisOptions::default(),
        }
    }
}

/// `[index.analysis]` knobs — match the previous `kenn analyze` CLI
/// defaults so behaviour is unchanged for the now-implicit index-time
/// run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexAnalysisOptions {
    #[serde(default = "default_top_n")]
    pub top_n: usize,
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    #[serde(default = "default_min_cluster")]
    pub min_cluster: usize,
}

impl Default for IndexAnalysisOptions {
    fn default() -> Self {
        Self {
            top_n: default_top_n(),
            max_depth: default_max_depth(),
            min_cluster: default_min_cluster(),
        }
    }
}

const fn default_top_n() -> usize {
    20
}
const fn default_max_depth() -> usize {
    4
}
const fn default_min_cluster() -> usize {
    20
}
const fn default_true() -> bool {
    true
}
fn default_graph_path() -> String {
    "kenn_graph.html".to_string()
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    #[test]
    fn defaults_to_persist() {
        let c = Config::from_toml("").unwrap();
        assert!(c.index.persist_analysis);
        assert_eq!(c.index.graph_path, "kenn_graph.html");
        assert_eq!(c.index.analysis.top_n, 20);
        assert_eq!(c.index.analysis.max_depth, 4);
        assert_eq!(c.index.analysis.min_cluster, 20);
    }

    #[test]
    fn round_trips_overrides() {
        let toml = r#"
[index]
persist_analysis = false
graph_path = "out/graph.html"

[index.analysis]
top_n = 50
max_depth = 6
min_cluster = 10
"#;
        let c = Config::from_toml(toml).unwrap();
        assert!(!c.index.persist_analysis);
        assert_eq!(c.index.graph_path, "out/graph.html");
        assert_eq!(c.index.analysis.top_n, 50);
        assert_eq!(c.index.analysis.max_depth, 6);
        assert_eq!(c.index.analysis.min_cluster, 10);
    }
}
