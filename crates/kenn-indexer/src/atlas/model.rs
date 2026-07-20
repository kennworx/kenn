//! Atlas domain model (`atlas` capability) — the structural facts a package
//! concept carries, decoupled from both the store row types (the producer maps
//! rows into these) and OKF serialization (`super::okf` renders these). v1 emits
//! one [`Concept`] per internal (non-external) package; every field is a
//! checkable structural fact, never kenn-authored prose.

/// One concept — a code `package` (a structural skeleton) or a non-code
/// `document` (a first-party directory that isn't a code package: `openspec`,
/// `docs`, `claude-plugins`, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Concept {
    /// OKF `type`: `package` for a code crate/package, `document` for a
    /// first-party non-code directory.
    pub concept_type: String,
    /// Path-qualified concept id (bundle path minus `.md`), e.g.
    /// `packages/crates_kenn-store`. Built by [`super::okf::concept_id`].
    pub id: String,
    /// Display title — the package name.
    pub title: String,
    /// The package's root-module doc, copied verbatim; `None` when absent
    /// (never synthesized).
    pub description: Option<String>,
    /// Workspace-relative manifest path, or the package directory when
    /// manifest-less. Never absolute.
    pub resource: String,
    /// The package's plurality language (most symbols), e.g. `rust`.
    pub language: String,
    /// A test-dominant package (a `*.Test` project — more test classes than
    /// production). Rendered with a `tests` tag; its central symbols include the
    /// test classes (testing is its purpose).
    pub test: bool,
    /// Total symbol count in the package (a `kenn.*` fact).
    pub symbols: u64,
    /// Directed dependencies: the concept ids of packages this one depends on.
    pub deps: Vec<String>,
    /// Most central non-test symbols, ranked (highest weighted degree first).
    pub central: Vec<CentralSymbol>,
    /// Individual member files, workspace-relative. Populated for a `component`
    /// (which maps one directory, so it lists every file) and a `document`;
    /// empty for a `package`, which summarizes via [`Self::file_count`] +
    /// [`Self::dir_counts`] instead.
    pub members: Vec<String>,
    /// For a `package`, the total distinct member-file count (the `## Members`
    /// heading). `0` for a component/document (they list files individually).
    pub file_count: u64,
    /// For a `package`, per-directory file counts — the file's parent directory
    /// relative to the package root → number of member files in it, sorted
    /// count-desc then path. Empty for a component/document.
    pub dir_counts: Vec<(String, u64)>,
    /// For a `component`, the parent `package` concept id; `None` for a package
    /// or document.
    pub parent: Option<String>,
    /// For a subdivided `package`, its child `component` concept ids (source
    /// sub-areas, sorted); empty otherwise.
    pub components: Vec<String>,
}

/// A central symbol: its name and the workspace-relative location it is defined
/// at — `line_start`..`line_end` (a range for a multiline def). `line_start` 0
/// when unknown; `line_end == line_start` for a single line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CentralSymbol {
    pub name: String,
    /// The stable `pub_id` — usable directly with kenn (`kenn get <pub_id>`),
    /// so the atlas is actionable, not just descriptive.
    pub pub_id: String,
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
}

/// One **domain** — a flat-Louvain community that spans more than one package.
/// The `domains` axis complements the `package` axis: a semantic cluster the
/// package structure doesn't show (e.g. an "embedding" domain crossing store +
/// indexer + server). Built from the persisted analysis (never recomputed);
/// every field is a structural fact, like [`Concept`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainConcept {
    /// Path-qualified concept id, e.g. `domains/shared-embedder`. Built by
    /// [`super::okf::domain_id`].
    pub id: String,
    /// Display title — the domain's hub symbol (its highest-weighted-degree
    /// member): the most recognizable handle for an otherwise anonymous cluster.
    pub title: String,
    /// Member count (aggregate nodes in the community).
    pub size: u64,
    /// The packages this domain spans, as `package` concept ids, heaviest
    /// (most members) first.
    pub packages: Vec<String>,
    /// The domain's most central members, ranked by weighted degree.
    pub central: Vec<CentralSymbol>,
}

/// The `index.md` shape/status header — all structural facts. The `timestamp`
/// is the only wall-clock value in the whole bundle and lives here (never on a
/// concept), so concept docs stay deterministic across re-index (design R3-C).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasShape {
    /// Workspace (repo) name, for the header title.
    pub name: String,
    /// Languages present, sorted.
    pub languages: Vec<String>,
    /// Internal package count = concept count.
    pub packages: usize,
    /// Total internal symbol count.
    pub symbols: u64,
    /// Test-symbol ratio as a whole percent (0..=100).
    pub test_ratio_pct: u8,
    /// Concrete freshness: HEAD sha, or the staleness key when git is absent.
    pub freshness: String,
    /// ISO-8601 build timestamp (header-only, ephemeral).
    pub timestamp: String,
}
