//! `[docker]` section — caches for the docker indexer runtime.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockerConfig {
    /// Named Docker volume holding downloaded dependency *sources* (cargo
    /// registry, Go module cache, `NuGet` packages, pip/npm caches). A named
    /// volume — not a host bind mount — because on macOS/Windows a bind mount
    /// crosses the host↔VM filesystem boundary, and that I/O penalty is severe
    /// for a cache this hot.
    ///
    /// `None` (default): a **per-repository** volume (`kenn-deps-<hash>`) bound
    /// to the repo's main worktree and shared by all its worktrees, reclaimed by
    /// `kenn docker-cache --orphans` when the repo is deleted. `Some(name)`: one
    /// **shared cross-repo** volume with that name, fetched once across every
    /// repo and never reclaimed automatically.
    #[serde(default)]
    pub cache_volume: Option<String>,
    /// Keep build artifacts (cargo `target/`, Go build cache) in a
    /// per-workspace named volume that survives re-indexes of the same repo.
    /// Off by default: build artifacts are ephemeral, dropped with the
    /// container. They are NEVER shared across repos — cargo locks `target/`,
    /// so a shared build cache would stall parallel indexer runs.
    #[serde(default)]
    pub persist_build_cache: bool,
}
