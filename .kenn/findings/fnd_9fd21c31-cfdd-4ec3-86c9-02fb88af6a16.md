---
id: fnd_9fd21c31-cfdd-4ec3-86c9-02fb88af6a16
tags:
- guide
parent_ids: []
created_at: 2026-06-15T09:41:52.642999Z
---
`kenn analyze` outputs are configurable and default to the WORKSPACE ROOT, not the old `kenn-out/` dir. `[index] report_path` (default `kenn_report.md`) is the markdown analysis report `kenn index` renders; `[index] graph_path` (default `kenn_graph.html`) is what `kenn visualize` writes. Both are relative to the workspace root (absolute honored) and their parent dirs are created on write. Plumbing: `build_analysis_hook(opts, report_path: Option<PathBuf>)` — `None` skips the report (folds the old `write_report` bool); `analysis_hook_from_config` computes `write_report.then(|| workspace_root.join(report_path))`; `cmd_visualize` joins `config.index.graph_path`. Both generated files are gitignored. CRAP gotcha hit here: adding the parent-dir `if let` branch directly in `cmd_visualize::run` pushed that already-large fn over threshold (crap 33.6, cyc 15) — extracting a `create_with_parents(path)` helper restored it (per CLAUDE.md §6, reduce cyclomatic by splitting). Separately: `just crap-ci` SIGABRTs locally when the EmbeddingGemma GGUF is cached, because the `kenn-store/tests/hybrid_search.rs` real-model test runs the Metal embedder under llvm-cov; `kenn server stop` (resetting the per-user daemon) before crap-ci avoids the abort.