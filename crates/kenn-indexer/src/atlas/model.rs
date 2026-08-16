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
    /// Outgoing coupling: packages this one depends on, heaviest first.
    pub deps: Vec<Coupling>,
    /// How many packages this one depends on BEFORE the render cap — so the
    /// renderer can name what it dropped rather than truncate silently.
    pub deps_total: u64,
    /// Incoming coupling: packages that depend on this one, heaviest first.
    /// The inverse of [`Self::deps`] — and the direction a reader usually wants
    /// first, since "who breaks if I change this" is not answerable from the
    /// outgoing list alone.
    pub used_by: Vec<Coupling>,
    /// How many packages depend on this one before the render cap. See
    /// [`Self::deps_total`].
    pub used_by_total: u64,
    /// Where this package sits in the dependency graph — the `index.md`
    /// grouping axis. `None` for a component or document, which are not
    /// packages and take no part in it.
    pub role: Option<Role>,
    /// Most central non-test symbols, ranked (highest weighted degree first).
    pub central: Vec<SymbolRef>,
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

/// A package's place in the dependency graph — the axis `index.md` groups by.
///
/// Grouping by LANGUAGE is a filesystem fact, and it collapses at scale: a real
/// 125-package solution rendered as 123 alphabetical bullets under one `## C#`
/// heading, which tells a reader nothing about which packages matter or how they
/// relate. Role is derived from one number — how much a package is depended on
/// versus how much it depends — because that is the number with a defensible
/// threshold. Finer labels (adapter vs engine, vocabulary vs contract) need axes
/// that had almost no dynamic range on the repos measured, and a confident wrong
/// label in a bird's-eye view is worse than a number.
///
/// The rendered entry always carries the dependent/dependency counts the label
/// came from, so a reader can check the classification rather than trust it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Depended on far more than it depends. The foundation everything rests on.
    Provider,
    /// Both depended on and depending — the middle of the stack.
    Layer,
    /// Depends on much, little depends on it. Entry points, apps, leaves.
    Consumer,
    /// Test-dominant. Split out first: on a large solution these are ~30% of the
    /// package list and none of its architecture.
    Tests,
    /// No cross-package coupling in either direction — vendored, dead, or not
    /// wired up. Worth seeing precisely because it is invisible in a flat list.
    Isolated,
}

impl Role {
    /// The `index.md` section heading, and its rule stated so the grouping is
    /// checkable.
    #[must_use]
    pub const fn heading(self) -> &'static str {
        match self {
            Self::Provider => "Providers — depended on, depending on little",
            Self::Layer => "Layers — both depended on and depending",
            Self::Consumer => "Consumers — depending on much, little depends on them",
            Self::Tests => "Tests",
            Self::Isolated => "Isolated — no cross-package coupling",
        }
    }
}

/// One directed coupling between two packages: the other package's concept id,
/// the summed aggregate-edge weight, and the per-relation split.
///
/// The weight was always computed (it ranks the dependency list) and always
/// thrown away at render. Keeping it — and the relation split with it — is what
/// separates "kenn-store depends on kenn-model" from "kenn-store depends on
/// kenn-model 2007 times, almost all type references": the first is a fact, the
/// second is the architecture. `implements` in the split is the sharpest signal
/// of the set, marking a contract/implementer pair rather than incidental use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coupling {
    /// The other package's concept id (a bundle-relative link target).
    pub concept_id: String,
    /// The other package's display name — its anchor, as the reader knows it.
    /// Carried rather than derived from [`Self::concept_id`], whose leaf is the
    /// flattened, language-prefixed filename (`rust_kenn-config`) and not a name
    /// anyone types.
    pub title: String,
    /// Summed weight across every relation in [`Self::relations`].
    pub weight: u64,
    /// `(relation, weight)` — the edge kind's `db_name`, heaviest first.
    pub relations: Vec<(String, u64)>,
}

/// A resolvable reference to one symbol: its name, stable `pub_id`, and the
/// workspace-relative location it is defined at — `line_start`..`line_end` (a
/// range for a multiline def). `line_start` 0 when unknown; `line_end ==
/// line_start` for a single line. Used wherever the atlas lists symbols a reader
/// can act on — a package/domain's central symbols, a contract's implementers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRef {
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
    /// Member count — the domain's members that live in a [`Self::packages`]
    /// package. Members glued in only through shared external types (no
    /// first-party edge to the span) are excluded, so this is the honest size of
    /// the cluster, not the raw community.
    pub size: u64,
    /// The packages this domain genuinely spans, heaviest (most members) first.
    /// Only packages that clear the member floor AND connect by a first-party
    /// edge to another such package survive — see the producer's `supported_span`.
    pub packages: Vec<SpannedPackage>,
    /// The domain's most central members, ranked by weighted degree.
    pub central: Vec<SymbolRef>,
}

/// One package a domain genuinely spans: which package, how many of the domain's
/// members live in it, and how many intra-domain edges connect it to the domain's
/// OTHER spanned packages. The link count is what earned it the span — a package
/// with no first-party edge into the rest of the community is a reference into
/// the domain, not part of it, and the producer drops it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpannedPackage {
    /// The package's concept id (a bundle-relative link target).
    pub concept_id: String,
    /// The package's display name — its anchor, as the reader knows it. Carried
    /// rather than derived from [`Self::concept_id`], whose leaf is the flattened,
    /// language-prefixed filename (`rust_kenn-store`) and not a name anyone types.
    pub title: String,
    /// The domain's members living in this package.
    pub members: u64,
    /// Intra-domain aggregate edges connecting this package to the domain's other
    /// spanned packages — the coupling that earned it the span.
    pub links: u64,
}

/// One **contract** — a first-party interface / base class / protocol whose
/// implementers span more than one package. Unlike a [`DomainConcept`] (an
/// emergent Louvain cluster), a contract is read STRAIGHT from the `implements` /
/// `extends_type` edges: it is explicit, complete (every implementer, not the
/// subset a clustering happens to merge), and deterministic. It answers the one
/// question the package axis can't — "where is this abstraction implemented
/// across the tree" — which is the reader's question when touching a shared type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractConcept {
    /// Path-qualified concept id, e.g. `contracts/IValidator`.
    pub id: String,
    /// The contract's name (the interface / base type).
    pub title: String,
    /// The contract's kind `db_name` — `interface`, `class` (base class), etc.
    pub kind: String,
    /// The contract type itself — its resolvable `pub_id` and definition
    /// location, so a reader can `kenn get` the interface or jump to it.
    pub symbol: SymbolRef,
    /// The package the contract is DEFINED in: `(concept_id, display title)`.
    pub defined_in_id: String,
    pub defined_in_title: String,
    /// Implementers grouped by package, widest (most implementers) first, capped.
    pub implementers: Vec<ContractImplementers>,
    /// Distinct implementers across all packages, BEFORE the render cap.
    pub total_implementers: u64,
    /// Distinct implementer packages, BEFORE the render cap — the breadth that
    /// makes this a cross-package contract.
    pub package_span: u64,
}

/// One database table and every site that names it.
///
/// A table is the one entity a repository's schema, its migrations, its mapper
/// files and its application code all name in common — and no other axis covers
/// it, because a table is not in a package. Read STRAIGHT from the table edges,
/// like a contract and for the same reasons: explicit, complete, deterministic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableConcept {
    /// Path-qualified concept id, e.g. `tables/public.orders`.
    pub id: String,
    /// The table's name, schema-qualified when the source qualified it.
    pub title: String,
    /// The table node's own `pub_id`, so a reader can `kenn list usages` it.
    pub pub_id: String,
    /// True when some statement in this workspace declares the table. False
    /// means the schema is owned elsewhere — ordinary, not a defect.
    pub internal: bool,
    /// References grouped by the file that made them, heaviest first, capped.
    pub by_file: Vec<TableFileRefs>,
    /// Distinct referencing files, BEFORE the render cap — the breadth that
    /// ranks the axis.
    pub file_span: u64,
    /// Distinct referencing languages, BEFORE the render cap.
    pub language_span: u64,
    /// Total reference sites, BEFORE the render cap.
    pub total_refs: u64,
}

/// The references to a [`TableConcept`] made from one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableFileRefs {
    /// The file the references were made in.
    pub file: String,
    /// That file's language — so a table named by a migration, a changelog and
    /// application code reads as such at a glance.
    pub language: String,
    /// The referencing symbols, each with its `pub_id` and location, capped for
    /// render, paired with what the reference does (`declares` / `modifies` /
    /// `accesses`).
    pub sites: Vec<(String, SymbolRef)>,
    /// Total references from this file, BEFORE the render cap.
    pub count: u64,
}

/// The implementers of a [`ContractConcept`] that live in one package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractImplementers {
    /// The implementer package's concept id (a bundle-relative link target).
    pub concept_id: String,
    /// The implementer package's display name.
    pub title: String,
    /// Implementer types in this package — each with its resolvable `pub_id` and
    /// source location (`kenn get <pub_id>` / jump-to-def), sorted, capped for
    /// render. Same shape the package concept uses for its central symbols.
    pub symbols: Vec<SymbolRef>,
    /// Total distinct implementers in this package, BEFORE the render cap.
    pub count: u64,
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
    /// Domains and contracts the repo HAS, counted before the render caps
    /// (`MAX_DOMAINS` / `MAX_CONTRACTS`) bound what the bundle writes. The
    /// header states the repo's shape; the axis heading names the cap when it
    /// binds. Without these the index reported the capped count as the total —
    /// a 125-package solution read as "24 domains" when it has 78.
    pub domains_total: usize,
    pub contracts_total: usize,
    /// Tables selected before the render cap, so the index can name what it
    /// dropped rather than showing a capped count as the whole.
    pub tables_total: usize,
    /// Concrete freshness: HEAD sha, or the staleness key when git is absent.
    pub freshness: String,
    /// ISO-8601 build timestamp (header-only, ephemeral).
    pub timestamp: String,
}
