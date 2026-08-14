//! md→code link resolution (design D5/D6, Groups 5–6).
//!
//! When a markdown link does not resolve within the markdown corpus, an
//! **in-repo** file may instead reference code: a path to a source file, or a
//! symbol by name. Resolution is recall-first and name-anchored, the same
//! ladder as md↔md:
//!
//! - file path: exact relpath → `Exact`; stale path, basename current →
//!   `Drifted`; multiple basenames → locality, else keep-all (`Ambiguous`).
//! - symbol: by short (last-segment) name; written qualifier matches →
//!   `Exact`, stale qualifier → `Drifted`; multiple → locality, else keep-all.
//!
//! The code graph lives in the store and only exists after code ingest
//! completes, so lookups go through [`CodeLookup`] — mocked here, implemented
//! against the post-barrier store in the orchestrator (Group 6). Resolution is
//! gated to in-repo roots by the caller (design D6).

use kenn_model::{Language, LinkGrade, ShortId};

use crate::relpath::join_relative;

/// A code-graph node the md→code resolver may target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeCandidate {
    pub id: ShortId,
    /// Source path (for the exact-path check and locality).
    pub relpath: String,
    /// Fully-qualified name/id (for symbol qualifier-drift detection).
    pub qualified: String,
}

/// A resolved md→code target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeTarget {
    pub id: ShortId,
    pub grade: LinkGrade,
    /// True when `id` is a code FILE node (→ a `links_to_file` edge, hydrated
    /// from the files table); false for a code SYMBOL (→ a `links_to` edge).
    pub is_file: bool,
}

/// Lookup surface over the code graph. Implemented against the post-code-ingest
/// store (Group 6); mocked in tests.
pub trait CodeLookup {
    /// Code FILE nodes whose filename (basename) equals `basename`.
    fn files_by_basename(&self, basename: &str) -> Vec<CodeCandidate>;
    /// Code SYMBOL nodes whose short (last-segment) name equals `name`.
    fn symbols_by_short_name(&self, name: &str) -> Vec<CodeCandidate>;
}

/// Resolve a link `target` (that failed md resolution) against the code graph,
/// in the context of the linking markdown file's `linking_relpath` (for
/// locality). Returns `[]` when nothing matches (caller keeps it dangling).
#[must_use]
pub fn resolve_code_link(
    target: &str,
    linking_relpath: &str,
    code: &dyn CodeLookup,
) -> Vec<CodeTarget> {
    if is_code_path(target) {
        resolve_file_ref(target, linking_relpath, code)
    } else {
        let (qualifier, short) = split_qualifier(target);
        let candidates = code.symbols_by_short_name(short);
        pick(candidates, linking_relpath, false, |c| {
            qualifier.is_empty() || c.qualified.contains(qualifier)
        })
    }
}

/// Resolve `target` strictly as a file *path* reference (never a bare symbol),
/// reusing the file-resolution grade ladder ([`pick`] with `is_file = true`):
/// exact relpath → `Exact`; stale path, basename current → `Drifted`; multiple
/// basenames → locality, else keep-all (`Ambiguous`); no match → `[]`. HTML
/// `<a href>` / `<link>` / `<script>` references are always paths, so they call
/// this directly rather than [`resolve_code_link`], whose code-extension
/// heuristic would misroute a bare `b.html` to the symbol branch.
#[must_use]
pub fn resolve_file_ref(
    target: &str,
    linking_relpath: &str,
    code: &dyn CodeLookup,
) -> Vec<CodeTarget> {
    let candidates = code.files_by_basename(basename(target));
    // Same two-step as md↔md `resolve_inline`: the path **as written** (already
    // workspace-relative), then the path **joined** onto the linking file's
    // directory. Both are Exact — a doc may spell a target either way, and
    // accepting only one is what made a correct `../frames.ts` degrade to
    // Drifted. Joining is what `..` requires: popping a segment, not deleting
    // the token, so `../../x/mod.rs` can no longer match a root-level
    // `x/mod.rs` it does not name. `None` from the join = the target walks
    // above the workspace root, so only the as-written form can match. The
    // anchor is dropped first, matching [`basename`], so a `path#L10` target
    // still compares as a path.
    let written = target.split('#').next().unwrap_or(target);
    let written = written.trim_start_matches("./");
    let joined = join_relative(linking_relpath, written);
    pick(candidates, linking_relpath, true, |c| {
        c.relpath == written || joined.as_deref() == Some(c.relpath.as_str())
    })
}

/// Pick targets from `candidates`: prefer those satisfying `is_exact`
/// (`Exact`), else fall back to the locality-nearest (`Drifted`); either tier
/// keeps all when it cannot disambiguate (`Ambiguous`).
fn pick(
    candidates: Vec<CodeCandidate>,
    linking_relpath: &str,
    is_file: bool,
    is_exact: impl Fn(&CodeCandidate) -> bool,
) -> Vec<CodeTarget> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let (exact, rest): (Vec<_>, Vec<_>) = candidates.into_iter().partition(&is_exact);
    if !exact.is_empty() {
        return grade(&exact, single_grade(exact.len(), LinkGrade::Exact), is_file);
    }
    let nearest = nearest_by_locality(&rest, linking_relpath);
    let g = single_grade(nearest.len(), LinkGrade::Drifted);
    nearest
        .into_iter()
        .map(|c| CodeTarget {
            id: c.id,
            grade: g,
            is_file,
        })
        .collect()
}

fn single_grade(count: usize, when_one: LinkGrade) -> LinkGrade {
    if count == 1 {
        when_one
    } else {
        LinkGrade::Ambiguous
    }
}

fn grade(candidates: &[CodeCandidate], g: LinkGrade, is_file: bool) -> Vec<CodeTarget> {
    candidates
        .iter()
        .map(|c| CodeTarget {
            id: c.id,
            grade: g,
            is_file,
        })
        .collect()
}

/// The candidates sharing the longest common path-prefix with `linking_relpath`
/// (closest by directory locality). Returns owned clones for the winners.
fn nearest_by_locality(candidates: &[CodeCandidate], linking_relpath: &str) -> Vec<CodeCandidate> {
    let best = candidates
        .iter()
        .map(|c| common_prefix_segments(&c.relpath, linking_relpath))
        .max()
        .unwrap_or(0);
    candidates
        .iter()
        .filter(|c| common_prefix_segments(&c.relpath, linking_relpath) == best)
        .cloned()
        .collect()
}

fn common_prefix_segments(a: &str, b: &str) -> usize {
    a.split('/')
        .zip(b.split('/'))
        .take_while(|(x, y)| x == y)
        .count()
}

/// A target is a code *path* when it contains a `/` or its basename carries a
/// known code-language extension. A target that is neither goes down the
/// **symbol** branch of [`resolve_code_link`] — which is why callers need to
/// know: only a bare name can be shadowed by a same-named code symbol.
#[must_use]
pub fn is_code_path(target: &str) -> bool {
    target.contains('/') || code_extension(target)
}

fn code_extension(target: &str) -> bool {
    let file = basename(target);
    let Some((_, ext)) = file.rsplit_once('.') else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    [
        Language::Rust,
        Language::TypeScript,
        Language::Csharp,
        Language::Go,
        Language::Python,
    ]
    .iter()
    .flat_map(|l| l.extensions())
    .any(|e| *e == ext)
}

fn basename(target: &str) -> &str {
    let no_anchor = target.split('#').next().unwrap_or(target);
    no_anchor.rsplit('/').next().unwrap_or(no_anchor)
}

/// Split `Auth.OrderHandler` / `mod::Type` into (`qualifier`, `short`).
fn split_qualifier(target: &str) -> (&str, &str) {
    if let Some(idx) = target.rfind("::") {
        let (q, rest) = target.split_at(idx);
        return (q, rest.trim_start_matches("::"));
    }
    if let Some((q, short)) = target.rsplit_once('.') {
        return (q, short);
    }
    ("", target)
}

/// [`CodeLookup`] backed by the building code graph, read through a
/// `reader_from_writer` snapshot in the post-code barrier (Group 6). Each query
/// blocks the calling OS thread on the async reader via `handle` — the barrier
/// runs on a plain (runtime-free) thread, mirroring [`crate::sink::BatchSink`].
pub struct StoreCodeLookup<'a> {
    pub reader: &'a kenn_store::DbReader,
    pub handle: &'a tokio::runtime::Handle,
}

impl CodeLookup for StoreCodeLookup<'_> {
    fn files_by_basename(&self, basename: &str) -> Vec<CodeCandidate> {
        self.handle
            .block_on(self.reader.files_by_basename(basename))
            .unwrap_or_default()
            .into_iter()
            .map(|f| CodeCandidate {
                id: f.id,
                relpath: f.path.clone(),
                qualified: f.path,
            })
            .collect()
    }

    fn symbols_by_short_name(&self, name: &str) -> Vec<CodeCandidate> {
        self.handle
            .block_on(self.reader.symbols_by_short_name(name))
            .unwrap_or_default()
            .into_iter()
            .map(|s| CodeCandidate {
                id: s.id,
                relpath: s.relpath,
                qualified: s.qualified,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mock {
        files: Vec<CodeCandidate>,
        symbols: Vec<CodeCandidate>,
    }
    impl CodeLookup for Mock {
        fn files_by_basename(&self, basename: &str) -> Vec<CodeCandidate> {
            self.files
                .iter()
                .filter(|c| c.relpath.rsplit('/').next() == Some(basename))
                .cloned()
                .collect()
        }
        fn symbols_by_short_name(&self, name: &str) -> Vec<CodeCandidate> {
            self.symbols
                .iter()
                .filter(|c| c.qualified.rsplit(['.', ':']).next() == Some(name))
                .cloned()
                .collect()
        }
    }

    fn file(id: ShortId, relpath: &str) -> CodeCandidate {
        CodeCandidate {
            id,
            relpath: relpath.into(),
            qualified: relpath.into(),
        }
    }
    fn sym(id: ShortId, qualified: &str, relpath: &str) -> CodeCandidate {
        CodeCandidate {
            id,
            relpath: relpath.into(),
            qualified: qualified.into(),
        }
    }

    /// A correct *relative* file link is Exact, not Drifted. Before
    /// `honest-link-grades` the comparison ran through a `normalize` that
    /// deleted `../` instead of popping a segment, so `../frames.ts` was
    /// compared as `frames.ts` against `indexers/frames.ts`, missed, and fell
    /// to the locality rung.
    #[test]
    fn relative_file_link_resolves_against_the_linking_dir() {
        let m = Mock {
            files: vec![file(7, "indexers/frames.ts")],
            symbols: vec![],
        };
        let t = resolve_file_ref("../frames.ts", "indexers/kenn-dotnet/README.md", &m);
        assert_eq!(
            t,
            [CodeTarget {
                id: 7,
                grade: LinkGrade::Exact,
                is_file: true
            }]
        );
    }

    /// The same bug's dangerous half: deleting `../` also made a link match a
    /// same-basename file it does not name. `../../x/mod.rs` from
    /// `crates/a/src/m/README.md` means `crates/a/x/mod.rs`; graded through the
    /// old `normalize` it became `x/mod.rs` and matched the root file **Exact**.
    /// Basename pre-filtering makes this reachable wherever a name repeats, and
    /// this workspace has 38 `mod.rs`.
    #[test]
    fn a_parent_hop_does_not_match_a_same_named_file_elsewhere() {
        let m = Mock {
            files: vec![file(9, "x/mod.rs")],
            symbols: vec![],
        };
        let t = resolve_file_ref("../../x/mod.rs", "crates/a/src/m/README.md", &m);
        // The candidate is still reachable by basename + locality (the Drifted
        // rung), but it must not be called an exact match.
        assert_eq!(t.len(), 1);
        assert_ne!(
            t[0].grade,
            LinkGrade::Exact,
            "a joined path of crates/a/x/mod.rs must not grade exact against x/mod.rs"
        );
    }

    #[test]
    fn file_exact_path() {
        let m = Mock {
            files: vec![file(7, "src/order.rs")],
            symbols: vec![],
        };
        let t = resolve_code_link("src/order.rs", "docs/a.md", &m);
        assert_eq!(
            t,
            [CodeTarget {
                id: 7,
                grade: LinkGrade::Exact,
                is_file: true
            }]
        );
    }

    #[test]
    fn file_stale_path_drifts_by_basename() {
        let m = Mock {
            files: vec![file(7, "src/handlers/order.rs")],
            symbols: vec![],
        };
        let t = resolve_code_link("../old/order.rs", "docs/a.md", &m);
        assert_eq!(
            t,
            [CodeTarget {
                id: 7,
                grade: LinkGrade::Drifted,
                is_file: true
            }]
        );
    }

    #[test]
    fn file_basename_ambiguous_uses_locality_then_keep_all() {
        let m = Mock {
            files: vec![file(1, "api/order.rs"), file(2, "ui/order.rs")],
            symbols: vec![],
        };
        // A bare sibling name resolves by the join, not by locality: from
        // `api/docs.md`, `order.rs` *means* `api/order.rs`. Before the shared
        // join this reached the locality rung and graded Drifted.
        let sibling = resolve_code_link("order.rs", "api/docs.md", &m);
        assert_eq!(
            sibling,
            [CodeTarget {
                id: 1,
                grade: LinkGrade::Exact,
                is_file: true
            }]
        );
        // A stale path that no join can satisfy still falls to locality:
        // `api/old/order.rs` matches nothing, so basename + nearness pick
        // api/order.rs over ui/order.rs.
        let near = resolve_code_link("old/order.rs", "api/docs.md", &m);
        assert_eq!(
            near,
            [CodeTarget {
                id: 1,
                grade: LinkGrade::Drifted,
                is_file: true
            }]
        );
        // linking from an unrelated dir → tie → keep all, Ambiguous.
        let tie = resolve_code_link("order.rs", "zzz/docs.md", &m);
        assert_eq!(tie.len(), 2);
        assert!(tie.iter().all(|t| t.grade == LinkGrade::Ambiguous));
    }

    #[test]
    fn symbol_exact_and_qualifier_drift() {
        let m = Mock {
            files: vec![],
            symbols: vec![sym(9, "Billing.OrderHandler", "src/billing.rs")],
        };
        // bare short name → Exact
        assert_eq!(
            resolve_code_link("OrderHandler", "docs/a.md", &m),
            [CodeTarget {
                id: 9,
                grade: LinkGrade::Exact,
                is_file: false
            }]
        );
        // matching qualifier → Exact
        assert_eq!(
            resolve_code_link("Billing.OrderHandler", "docs/a.md", &m),
            [CodeTarget {
                id: 9,
                grade: LinkGrade::Exact,
                is_file: false
            }]
        );
        // stale qualifier, short name current → Drifted
        assert_eq!(
            resolve_code_link("Auth.OrderHandler", "docs/a.md", &m),
            [CodeTarget {
                id: 9,
                grade: LinkGrade::Drifted,
                is_file: false
            }]
        );
    }

    #[test]
    fn no_match_is_empty() {
        let m = Mock {
            files: vec![],
            symbols: vec![],
        };
        assert!(resolve_code_link("Nope", "docs/a.md", &m).is_empty());
        assert!(resolve_code_link("x/gone.rs", "docs/a.md", &m).is_empty());
    }
}
