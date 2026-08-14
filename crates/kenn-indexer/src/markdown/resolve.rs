//! The link resolution ladder (design D5): turn a [`RawLink`] into one or more
//! resolved targets, downgrading through grades rather than failing.
//!
//! ```text
//! exact      path/name + location current
//! drifted    name current, path/qualifier stale (inline path → basename)
//! ambiguous  several name matches — keep all
//! dangling   no match — edge to an external stub, never dropped
//! ```
//!
//! Fuzzy (approximate-name) resolution is deferred until the index grows a
//! fuzzy matcher; md→code resolution (Group 5) adds the qualifier-drift cases.

use kenn_model::id::md::slugify;
use kenn_model::LinkGrade;

use super::index::ResolutionIndex;
use super::links::RawLink;
use crate::relpath::join_relative;

/// One resolved endpoint of a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTarget {
    /// Public id of the target node (`md:…` doc or section), or — for a
    /// dangling link — the written target, which the caller mints as an
    /// external stub.
    pub node_id: String,
    pub grade: LinkGrade,
    /// True when `node_id` is an unresolved target the caller must
    /// materialize as an external stub node.
    pub external_stub: bool,
}

/// Resolve one link against the global index, in the context of the file it
/// appears in (`current_doc_id` = that file's `document` node id). Returns:
/// - `[]` for an external URL (not graphed),
/// - one target for exact/drifted,
/// - several (grade `Ambiguous`) when a name matches multiple docs,
/// - one dangling stub when nothing matches (never empty for an internal link).
#[must_use]
pub fn resolve_link(
    raw: &RawLink,
    current_doc_id: &str,
    linking_relpath: &str,
    index: &ResolutionIndex,
) -> Vec<LinkTarget> {
    if raw.external {
        return Vec::new();
    }

    // 1. Candidate document nodes + the base grade for a single match.
    let (docs, base_grade) = if raw.target.is_empty() {
        // same-file `[[#sec]]` / `(#sec)`
        (vec![current_doc_id.to_string()], LinkGrade::Exact)
    } else if raw.wikilink {
        resolve_wikilink(&raw.target, index)
    } else {
        resolve_inline(&raw.target, linking_relpath, index)
    };

    if docs.is_empty() {
        return vec![LinkTarget {
            node_id: dangling_id(raw),
            grade: LinkGrade::Dangling,
            external_stub: true,
        }];
    }

    let grade = if docs.len() > 1 {
        LinkGrade::Ambiguous
    } else {
        base_grade
    };

    // 2. Apply the anchor (section) against each candidate document.
    docs.into_iter()
        .map(|doc_id| apply_anchor(doc_id, raw.anchor.as_deref(), grade, index))
        .collect()
}

/// Wikilink order: exact path → stem → alias → title (all `Exact`; a wikilink
/// carries no path that could be stale).
fn resolve_wikilink(target: &str, index: &ResolutionIndex) -> (Vec<String>, LinkGrade) {
    for hits in [
        index.by_path(target),
        index.by_stem(target),
        index.by_alias(target),
        index.by_title(target),
    ] {
        if !hits.is_empty() {
            return (ids(hits), LinkGrade::Exact);
        }
    }
    (Vec::new(), LinkGrade::Exact)
}

/// Inline order: exact path as written → path resolved relative to the linking
/// file's directory (both `Exact`) → basename fallback (`Drifted`, path stale
/// but filename current). An inline `[t](../foo/bar.md)` is written relative to
/// the *linking* file, so it must be joined onto that file's directory and
/// normalized before lookup — otherwise it misses by-path and collapses to the
/// basename (massively ambiguous in a repo with many same-named files).
fn resolve_inline(
    target: &str,
    linking_relpath: &str,
    index: &ResolutionIndex,
) -> (Vec<String>, LinkGrade) {
    // exact path as written (already workspace-relative)
    let exact = index.by_path(target);
    if !exact.is_empty() {
        return (ids(exact), LinkGrade::Exact);
    }
    // relative to the linking file's directory (the common case)
    if let Some(joined) = join_relative(linking_relpath, target) {
        let hits = index.by_path(&joined);
        if !hits.is_empty() {
            return (ids(hits), LinkGrade::Exact);
        }
    }
    // Path stale (e.g. a doc copied into a mirror dir whose siblings differ).
    // Don't collapse to a *global* basename match — that keep-alls every
    // same-named file. Instead take the same-basename candidates, narrow to
    // those whose relpath ends with the link's fuller relative suffix
    // (`../react-testing/SKILL.md` → `react-testing/SKILL.md`), then pick the
    // one nearest the linking file by directory locality — walking up the
    // hierarchy. Only a true locality tie stays Ambiguous.
    let candidates = index.by_stem(stem(target));
    if candidates.is_empty() {
        return (Vec::new(), LinkGrade::Exact);
    }
    let suffix = relative_suffix(target);
    let narrowed: Vec<&super::index::NodeRef> = candidates
        .iter()
        .filter(|n| path_has_suffix(&n.relpath, suffix))
        .collect();
    let pool: Vec<&super::index::NodeRef> = if narrowed.is_empty() {
        candidates.iter().collect()
    } else {
        narrowed
    };
    let nearest = nearest_by_locality(&pool, linking_relpath);
    (
        nearest.iter().map(|n| n.id.clone()).collect(),
        LinkGrade::Drifted,
    )
}

/// The candidates sharing the longest `/`-segment prefix with `linking_relpath`
/// (nearest by directory locality). Returns all at the best depth (ties).
fn nearest_by_locality<'a>(
    candidates: &[&'a super::index::NodeRef],
    linking_relpath: &str,
) -> Vec<&'a super::index::NodeRef> {
    let depth = |n: &super::index::NodeRef| common_prefix_segments(&n.relpath, linking_relpath);
    let best = candidates.iter().map(|n| depth(n)).max().unwrap_or(0);
    candidates
        .iter()
        .filter(|n| depth(n) == best)
        .copied()
        .collect()
}

fn common_prefix_segments(a: &str, b: &str) -> usize {
    a.split('/')
        .zip(b.split('/'))
        .take_while(|(x, y)| x == y)
        .count()
}

/// The link target with leading `./` / `../` segments stripped — the
/// directory-bearing suffix used to disambiguate same-basename candidates.
fn relative_suffix(target: &str) -> &str {
    let mut s = target;
    while let Some(rest) = s.strip_prefix("../").or_else(|| s.strip_prefix("./")) {
        s = rest;
    }
    s
}

/// Whether `relpath` ends with the path `suffix` on a `/` boundary
/// (case-insensitive), so `react-testing/SKILL.md` matches
/// `skills/react-testing/SKILL.md` but not `x/other-testing/SKILL.md`.
fn path_has_suffix(relpath: &str, suffix: &str) -> bool {
    let (r, s) = (relpath.to_lowercase(), suffix.to_lowercase());
    r == s || r.ends_with(&format!("/{s}"))
}

fn apply_anchor(
    doc_id: String,
    anchor: Option<&str>,
    grade: LinkGrade,
    index: &ResolutionIndex,
) -> LinkTarget {
    // A link anchor (`#Flow`) is matched against heading slugs (`flow`).
    let anchor = anchor.map(slugify);
    match anchor.as_deref() {
        // anchor present and the section exists → target the section node.
        Some(a) if index.has_section(&doc_id, a) => LinkTarget {
            node_id: format!("{doc_id}#{a}"),
            grade,
            external_stub: false,
        },
        // anchor present but missing → target the doc, at least Drifted.
        Some(_) => LinkTarget {
            node_id: doc_id,
            grade: worsen(grade),
            external_stub: false,
        },
        None => LinkTarget {
            node_id: doc_id,
            grade,
            external_stub: false,
        },
    }
}

fn worsen(grade: LinkGrade) -> LinkGrade {
    match grade {
        LinkGrade::Exact => LinkGrade::Drifted,
        other => other,
    }
}

fn ids(nodes: &[super::index::NodeRef]) -> Vec<String> {
    nodes.iter().map(|n| n.id.clone()).collect()
}

/// Filename of `target` without directory or `.md`/`.markdown` extension.
fn stem(target: &str) -> &str {
    let file = target.rsplit('/').next().unwrap_or(target);
    file.strip_suffix(".md")
        .or_else(|| file.strip_suffix(".markdown"))
        .unwrap_or(file)
}

/// Stable, shell-safe id for an unresolved target (the external stub's `pub_id`).
///
/// The `@unresolved` sentinel is a reserved namespace no real note path uses
/// (`@` is shell-safe; `!` was not), and the target/anchor — which carry raw link
/// text (spaces, `%`, …) — are floored through [`crate::pubid::floor`]. So every
/// dangling `pub_id` satisfies the DB's shell-safe invariant. The human-readable
/// form lives in the stub's `name` (see [`dangling_name`]).
pub(crate) fn dangling_id(raw: &RawLink) -> String {
    use crate::pubid::floor;
    match &raw.anchor {
        Some(a) if raw.target.is_empty() => format!("md:@unresolved/#{}", floor(a)),
        Some(a) => format!("md:@unresolved/{}#{}", floor(&raw.target), floor(a)),
        None => format!("md:@unresolved/{}", floor(&raw.target)),
    }
}

/// Human-readable display name for a dangling stub — the raw target (+ anchor),
/// unescaped. The shell-safe transform lives only in the `pub_id`; the `name`
/// stays the note title / asset filename the author wrote.
pub(crate) fn dangling_name(raw: &RawLink) -> String {
    match &raw.anchor {
        Some(a) if raw.target.is_empty() => format!("#{a}"),
        Some(a) => format!("{}#{a}", raw.target),
        None => raw.target.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::collect::{collect, CollectedFile};
    use crate::markdown::discover::DiscoveredMarkdown;
    use crate::markdown::links::{extract_links, LinkKind};
    use std::path::PathBuf;

    fn disc(label: &str, relpath: &str) -> DiscoveredMarkdown {
        DiscoveredMarkdown {
            abs_path: PathBuf::from(relpath),
            label: label.into(),
            relpath: relpath.into(),
            in_repo: true,
        }
    }

    /// Build an index from `(relpath, content)` pairs.
    fn index_of(files: &[(&str, &str)]) -> (Vec<DiscoveredMarkdown>, Vec<CollectedFile>) {
        let discs: Vec<_> = files.iter().map(|(p, _)| disc("workspace", p)).collect();
        let collected: Vec<_> = files.iter().map(|(_, c)| collect(c)).collect();
        (discs, collected)
    }

    /// Workspace-relative path of a `md:<label>/<relpath>` doc id (test helper).
    fn relpath_of(doc_id: &str) -> &str {
        doc_id
            .strip_prefix("md:")
            .and_then(|s| s.split_once('/'))
            .map_or("", |(_, rel)| rel)
    }

    fn one(raw: &RawLink, current: &str, idx: &ResolutionIndex) -> LinkTarget {
        let mut v = resolve_link(raw, current, relpath_of(current), idx);
        assert_eq!(v.len(), 1, "expected single target for {raw:?}");
        v.remove(0)
    }

    #[test]
    fn inline_exact_and_drifted() {
        let (d, c) = index_of(&[("docs/order.md", "# Order\n")]);
        let idx = ResolutionIndex::build(d.iter().zip(c.iter()));

        // exact path
        let r = extract_links("[x](docs/order.md)");
        let t = one(&r[0], "md:workspace/self.md", &idx);
        assert_eq!(t.node_id, "md:workspace/docs/order.md");
        assert_eq!(t.grade, LinkGrade::Exact);

        // stale path, filename current → drifted by basename
        let r = extract_links("[x](../old/order.md)");
        let t = one(&r[0], "md:workspace/self.md", &idx);
        assert_eq!(t.node_id, "md:workspace/docs/order.md");
        assert_eq!(t.grade, LinkGrade::Drifted);
    }

    #[test]
    fn inline_relative_resolves_against_linking_dir() {
        // Two same-named files in sibling dirs (the SKILL.md-everywhere case).
        let (d, c) = index_of(&[
            ("skills/a/SKILL.md", "# A\n"),
            ("skills/b/SKILL.md", "# B\n"),
        ]);
        let idx = ResolutionIndex::build(d.iter().zip(c.iter()));
        // From skills/a/SKILL.md, `[x](../b/SKILL.md)` resolves against the
        // linking dir to skills/b/SKILL.md — Exact, NOT ambiguous across the
        // two same-named files.
        let r = extract_links("[x](../b/SKILL.md)");
        let t = one(&r[0], "md:workspace/skills/a/SKILL.md", &idx);
        assert_eq!(t.node_id, "md:workspace/skills/b/SKILL.md");
        assert_eq!(t.grade, LinkGrade::Exact);
    }

    #[test]
    fn inline_stale_path_disambiguates_by_suffix_and_locality() {
        // A doc copied into `.mirror/` keeps a `../react-testing/SKILL.md` link,
        // but `.mirror/` has no react-testing sibling — the real one is under
        // top-level skills/. The exact join misses; rather than keep-all every
        // SKILL.md, the fuller suffix `react-testing/SKILL.md` + locality pick
        // the single canonical target.
        let (d, c) = index_of(&[
            (".mirror/react-patterns/SKILL.md", "# RP\n"),
            ("skills/react-testing/SKILL.md", "# RT\n"),
            ("skills/accessibility/SKILL.md", "# A11y\n"), // decoy, same basename
        ]);
        let idx = ResolutionIndex::build(d.iter().zip(c.iter()));
        let r = extract_links("[x](../react-testing/SKILL.md)");
        let t = one(&r[0], "md:workspace/.mirror/react-patterns/SKILL.md", &idx);
        assert_eq!(t.node_id, "md:workspace/skills/react-testing/SKILL.md");
        assert_eq!(t.grade, LinkGrade::Drifted); // single, located by suffix
    }

    #[test]
    fn wikilink_by_stem_and_anchor() {
        let (d, c) = index_of(&[("docs/auth.md", "# Auth\n## Flow\nbody\n")]);
        let idx = ResolutionIndex::build(d.iter().zip(c.iter()));

        let r = extract_links("see [[auth#flow]]");
        let t = one(&r[0], "md:workspace/self.md", &idx);
        assert_eq!(t.node_id, "md:workspace/docs/auth.md#flow");
        assert_eq!(t.grade, LinkGrade::Exact);

        // missing anchor → target the doc, downgraded to Drifted
        let r = extract_links("see [[auth#nope]]");
        let t = one(&r[0], "md:workspace/self.md", &idx);
        assert_eq!(t.node_id, "md:workspace/docs/auth.md");
        assert_eq!(t.grade, LinkGrade::Drifted);
    }

    #[test]
    fn ambiguous_keeps_all() {
        let (d, c) = index_of(&[("api/auth.md", "# A\n"), ("ui/auth.md", "# B\n")]);
        let idx = ResolutionIndex::build(d.iter().zip(c.iter()));
        let r = extract_links("[[auth]]");
        let mut got = resolve_link(&r[0], "md:workspace/self.md", "self.md", &idx);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|t| t.grade == LinkGrade::Ambiguous));
        got.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        assert_eq!(got[0].node_id, "md:workspace/api/auth.md");
        assert_eq!(got[1].node_id, "md:workspace/ui/auth.md");
    }

    #[test]
    fn dangling_links_become_external_stub() {
        let idx = ResolutionIndex::default();
        let r = extract_links("[[ghost]]");
        let t = one(&r[0], "md:workspace/self.md", &idx);
        assert_eq!(t.grade, LinkGrade::Dangling);
        assert!(t.external_stub);
        assert!(t.node_id.contains("ghost"));
    }

    #[test]
    fn external_urls_not_graphed() {
        let idx = ResolutionIndex::default();
        let r = extract_links("[site](https://example.com)");
        assert_eq!(r[0].kind, LinkKind::Link);
        assert!(resolve_link(&r[0], "md:workspace/self.md", "self.md", &idx).is_empty());
    }

    #[test]
    fn same_file_anchor_targets_current_doc() {
        let (d, c) = index_of(&[("docs/self.md", "# Self\n## Overview\ntext\n")]);
        let idx = ResolutionIndex::build(d.iter().zip(c.iter()));
        let r = extract_links("[[#overview]]");
        let t = one(&r[0], "md:workspace/docs/self.md", &idx);
        assert_eq!(t.node_id, "md:workspace/docs/self.md#overview");
        assert_eq!(t.grade, LinkGrade::Exact);
    }
}
