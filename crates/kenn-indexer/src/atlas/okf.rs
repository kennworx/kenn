//! OKF v0.1 serialization for the atlas bundle (`atlas` capability): concept
//! documents (YAML frontmatter + markdown body), the reserved `index.md`
//! (frontmatter-free shape header + concept list) and `log.md` (append-preserved,
//! newest-first), plus a conformance probe. Renders [`super::model`] values;
//! no I/O, no store access. Deterministic — no wall-clock in a concept doc, stable
//! key order, so re-indexing an unchanged repo is a no-op diff.

use std::collections::BTreeMap;

use serde::Serialize;

use super::model::{AtlasShape, Concept, DomainConcept};

/// The reserved OKF filenames — never treated as concept documents.
pub const INDEX_MD: &str = "index.md";
pub const LOG_MD: &str = "log.md";
const LOG_TITLE: &str = "# Atlas update log";

/// Concept id for a `language` package named `anchor`: the language, then the
/// anchor with path separators flattened to `_`, under `packages/`. Segments
/// join with a single `_` (runs collapse — never a doubled `__`), so
/// `@acme/web` reads `packages/typescript_@acme_web`. The language prefix keeps
/// a Rust `geo` and a C# `Geo` from colliding as *filenames* on a
/// case-insensitive filesystem (mac/Windows), where `packages/geo.md` and
/// `packages/Geo.md` would be one file. `language` empty (a language-less
/// concept) omits the prefix. The `.md` suffix is added by the file writer.
#[must_use]
pub fn concept_id(language: &str, anchor: &str) -> String {
    let flat = anchor.trim_matches('/').replace(['/', '\\'], "_");
    let id = if language.is_empty() {
        format!("packages/{flat}")
    } else {
        format!("packages/{language}_{flat}")
    };
    collapse_underscores(&id)
}

/// Collapse any run of underscores into a single `_`. Path separators flatten to
/// `_`, so adjacent separators (or a segment ending in `_`) never produce a
/// doubled `__`. A name's own single `_` (e.g. `code_with_me`) is preserved;
/// only runs collapse.
pub(crate) fn collapse_underscores(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev = false;
    for ch in s.chars() {
        if ch == '_' {
            if !prev {
                out.push('_');
            }
            prev = true;
        } else {
            out.push(ch);
            prev = false;
        }
    }
    out
}

/// A readable domain concept id from its hub symbol name: a slug under
/// `domains/`. Non-identifier chars collapse to a single `-` (a hub like
/// `Cow<'_, B>` → `domains/Cow`); the producer disambiguates any residual slug
/// collision. The `.md` suffix is added by the file writer.
#[must_use]
pub fn domain_id(hub: &str) -> String {
    let mut slug = String::with_capacity(hub.len());
    let mut prev_dash = false;
    for ch in hub.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            slug.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    format!("domains/{}", if slug.is_empty() { "domain" } else { slug })
}

/// The concept's YAML frontmatter — OKF-standard fields only (`type`, `title`,
/// `description`, `resource`, `tags`). No `kenn.*` extension: dependencies and
/// central symbols are expressed the standard way in the body (markdown links /
/// a list), and `language` rides the standard `tags` field. No `timestamp`
/// (determinism, R3-C); `description` omitted when absent.
#[derive(Serialize)]
struct Frontmatter<'a> {
    #[serde(rename = "type")]
    type_: &'a str,
    title: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    /// Omitted for a `domain` concept — a cross-package cluster is not backed by
    /// a single directory or manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<&'a str>,
}

/// The leaf display name of a dependency concept id (`packages/foo` → `foo`) —
/// the link label. Split on `/` only: names may contain `_` (`code_with_me`).
fn dep_leaf(id: &str) -> &str {
    id.rsplit('/').find(|s| !s.is_empty()).unwrap_or(id)
}

/// Render one concept document: `---\n<yaml>---\n<body>`. Follows the findings
/// record framing (bare mapping, exactly one `---` pair). Returns the markdown.
#[must_use]
#[expect(
    clippy::format_push_string,
    reason = "building a markdown document line by line; format! per line reads clearer than write!"
)]
pub fn render_concept(c: &Concept) -> String {
    let mut tags: Vec<&str> = if c.language.is_empty() {
        Vec::new()
    } else {
        vec![c.language.as_str()]
    };
    if c.test {
        tags.push("tests");
    }
    let front = Frontmatter {
        type_: &c.concept_type,
        title: &c.title,
        description: c.description.as_deref(),
        resource: Some(c.resource.as_str()),
        tags,
    };
    // serde_yaml_ng emits a bare mapping; normalize the framing exactly as the
    // findings record does so it is always one `---` pair.
    let yaml = serde_yaml_ng::to_string(&front).unwrap_or_default();
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let sep = if yaml.ends_with('\n') { "" } else { "\n" };

    let mut body = String::new();
    if let Some(parent) = &c.parent {
        body.push_str(&format!(
            "\n_Part of [{}](/{parent}.md)_\n",
            dep_leaf(parent)
        ));
    }
    if !c.central.is_empty() {
        // A table so the id (usable with `kenn get <id>`) and location fit
        // alongside the name without wrapping. Locations are shortened relative to
        // the package root named in the heading (same prefix as `## Files under`).
        let prefix = format!("{}/", c.resource);
        body.push_str(&format!(
            "\n## Central symbols under {}\n\n| ID | Location |\n|---|---|\n",
            c.resource
        ));
        for s in &c.central {
            let path = s.path.strip_prefix(&prefix).unwrap_or(&s.path);
            let loc = if s.line_start == 0 {
                path.to_string()
            } else if s.line_end > s.line_start {
                format!("{}:{}-{}", path, s.line_start, s.line_end)
            } else {
                format!("{}:{}", path, s.line_start)
            };
            body.push_str(&format!("| `{}` | {loc} |\n", s.pub_id));
        }
    }
    if !c.components.is_empty() {
        // A subdivided package links its source-directory components; the label is
        // the sub-area (the trailing `_`-segment of the flattened concept id).
        body.push_str("\n## Components\n\n");
        for comp in &c.components {
            let label = comp.rsplit('_').next().unwrap_or(comp);
            body.push_str(&format!("- [{label}](/{comp}.md)\n"));
        }
    }
    if !c.deps.is_empty() {
        body.push_str("\n## Depends on\n\n");
        for d in &c.deps {
            body.push_str(&format!("- [{}](/{d}.md)\n", dep_leaf(d)));
        }
    }
    if !c.dir_counts.is_empty() {
        // A package summarizes its files as a total + per-directory histogram
        // (D7): the heading carries the true file count, each line a directory
        // (relative to the package root) and its file count.
        body.push_str(&format!(
            "\n## Files under {} - {}\n\n",
            c.resource, c.file_count
        ));
        for (dir, count) in &c.dir_counts {
            body.push_str(&format!("- {dir} - {count}\n"));
        }
    } else if !c.members.is_empty() {
        // A component/document lists its files individually. Show them relative
        // to the resource dir — the shared prefix is stated in the heading, so
        // the short paths stay unambiguous.
        let prefix = format!("{}/", c.resource);
        body.push_str(&format!("\n## Members (under `{}/`)\n\n", c.resource));
        for m in &c.members {
            body.push_str(&format!("- {}\n", m.strip_prefix(&prefix).unwrap_or(m)));
        }
    }
    format!("---\n{yaml}{sep}---\n{body}")
}

/// Render one domain concept document — same OKF framing as [`render_concept`],
/// but a `## Spanned packages` section (links to the packages it crosses) in
/// place of `## Depends on`, and full-path central locations (a domain has no
/// single dir to shorten against).
#[must_use]
#[expect(
    clippy::format_push_string,
    reason = "building a markdown document line by line; format! per line reads clearer than write!"
)]
pub fn render_domain(d: &DomainConcept) -> String {
    let front = Frontmatter {
        type_: "domain",
        title: &d.title,
        description: None,
        resource: None,
        tags: Vec::new(),
    };
    let yaml = serde_yaml_ng::to_string(&front).unwrap_or_default();
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let sep = if yaml.ends_with('\n') { "" } else { "\n" };

    let mut body = String::new();
    if !d.central.is_empty() {
        body.push_str("\n## Central symbols\n\n| ID | Location |\n|---|---|\n");
        for s in &d.central {
            let loc = if s.line_start == 0 {
                s.path.clone()
            } else if s.line_end > s.line_start {
                format!("{}:{}-{}", s.path, s.line_start, s.line_end)
            } else {
                format!("{}:{}", s.path, s.line_start)
            };
            body.push_str(&format!("| `{}` | {loc} |\n", s.pub_id));
        }
    }
    if !d.packages.is_empty() {
        body.push_str("\n## Spanned packages\n\n");
        for p in &d.packages {
            body.push_str(&format!("- [{}](/{p}.md)\n", dep_leaf(p)));
        }
    }
    format!("---\n{yaml}{sep}---\n{body}")
}

/// A prettier heading for a `db_name` language token.
fn lang_display(lang: &str) -> &str {
    match lang {
        "rust" => "Rust",
        "typescript" => "TypeScript",
        "csharp" => "C#",
        "go" => "Go",
        "python" => "Python",
        "swift" => "Swift",
        "markdown" => "Markdown",
        "" => "Other",
        other => other,
    }
}

/// Render the reserved `index.md`: a frontmatter-free shape/status header, then
/// one `## <Language>` section per language (sorted), each listing that
/// language's packages (in the producer's order) with the module-doc first line
/// as a gloss.
#[must_use]
#[expect(
    clippy::format_push_string,
    reason = "building a markdown document line by line; format! per line reads clearer than write!"
)]
pub fn render_index(shape: &AtlasShape, concepts: &[Concept], domains: &[DomainConcept]) -> String {
    let langs = if shape.languages.is_empty() {
        "—".to_string()
    } else {
        shape.languages.join(", ")
    };
    let mut out = format!(
        "# {name} atlas\n\n_{pkgs} packages · {doms} domains · {syms} symbols · {test}% test · {langs}_\n\
         _{fresh} · {ts} · {n} concepts (skeletons)_\n",
        name = shape.name,
        pkgs = shape.packages,
        doms = domains.len(),
        syms = shape.symbols,
        test = shape.test_ratio_pct,
        fresh = shape.freshness,
        ts = shape.timestamp,
        n = concepts.len() + domains.len(),
    );

    // Section code packages by language; group non-code `document` concepts under
    // a single "Documents" section. BTreeMap → deterministic; documents sort last.
    let mut by_section: BTreeMap<String, Vec<&Concept>> = BTreeMap::new();
    for c in concepts {
        let key = if c.concept_type == "document" {
            "Documents".to_string()
        } else {
            lang_display(&c.language).to_string()
        };
        by_section.entry(key).or_default().push(c);
    }
    let mut sections: Vec<(String, Vec<&Concept>)> = by_section.into_iter().collect();
    sections.sort_by(|a, b| {
        (a.0 == "Documents")
            .cmp(&(b.0 == "Documents"))
            .then(a.0.cmp(&b.0))
    });
    for (section, list) in sections {
        out.push_str(&format!("\n## {section}\n\n"));
        for c in list {
            let gloss = c
                .description
                .as_deref()
                .and_then(|d| d.lines().next())
                .map(|l| format!(" — {}", l.trim()))
                .or_else(|| {
                    (c.concept_type == "document").then(|| format!(" — {} files", c.symbols))
                })
                .unwrap_or_default();
            out.push_str(&format!("- [{}](/{}.md){gloss}\n", c.title, c.id));
        }
    }

    // The cross-package domains axis, pinned last — a distinct axis, not a
    // language-sectioned package.
    if !domains.is_empty() {
        out.push_str("\n## Domains\n\n");
        for d in domains {
            out.push_str(&format!(
                "- [{}](/{}.md) — {} packages · {} symbols\n",
                d.title,
                d.id,
                d.packages.len(),
                d.size,
            ));
        }
    }
    out
}

/// Append-preserve a `log.md` entry: prepend a new dated section (newest-first)
/// ahead of any prior sections, keeping history across re-index (design R2-4).
/// Frontmatter-free.
#[must_use]
pub fn render_log(existing: Option<&str>, date: &str, summary: &str) -> String {
    let section = format!("## {date}\n\n* {summary}\n");
    match existing {
        None => format!("{LOG_TITLE}\n\n{section}"),
        Some(prev) => {
            let prior = prev
                .strip_prefix(LOG_TITLE)
                .map_or(prev, |s| s.trim_start_matches('\n'));
            format!("{LOG_TITLE}\n\n{section}\n{prior}")
        }
    }
}

/// Conformance probe: the non-empty `type` of a concept document's frontmatter,
/// or `None` when the framing/YAML is malformed or `type` is missing/empty.
/// Backs the §9 mutation-checked conformance test.
#[must_use]
pub fn concept_type(md: &str) -> Option<String> {
    let rest = md.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let yaml = rest.get(..end)?;
    let map: serde_yaml_ng::Value = serde_yaml_ng::from_str(yaml).ok()?;
    let t = map.get("type")?.as_str()?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlas::model::CentralSymbol;

    fn concept() -> Concept {
        Concept {
            concept_type: "package".into(),
            id: "packages/crates_kenn-store".into(),
            title: "kenn-store".into(),
            description: Some("SQLite code graph + vectors.\nMore detail.".into()),
            resource: "crates/kenn-store/Cargo.toml".into(),
            language: "rust".into(),
            test: false,
            symbols: 1240,
            deps: vec!["packages/crates_kenn-model".into()],
            central: vec![CentralSymbol {
                name: "Store".into(),
                pub_id: "rs:kenn-store::Store".into(),
                path: "crates/kenn-store/src/lib.rs".into(),
                line_start: 42,
                line_end: 78,
            }],
            members: vec!["crates/kenn-store/src/lib.rs".into()],
            file_count: 0,
            dir_counts: Vec::new(),
            parent: None,
            components: Vec::new(),
        }
    }

    #[test]
    fn lang_display_maps_every_arm() {
        for (tok, want) in [
            ("rust", "Rust"),
            ("typescript", "TypeScript"),
            ("csharp", "C#"),
            ("go", "Go"),
            ("python", "Python"),
            ("swift", "Swift"),
            ("markdown", "Markdown"),
            ("", "Other"),
            ("elixir", "elixir"), // unknown → passthrough
        ] {
            assert_eq!(lang_display(tok), want);
        }
    }

    #[test]
    fn concept_id_flattens_path_collision_safe() {
        assert_eq!(
            concept_id("rust", "crates/kenn-store"),
            "packages/rust_crates_kenn-store"
        );
        assert_eq!(concept_id("go", "/a/b/"), "packages/go_a_b");
        // Runs collapse: adjacent separators never double up.
        assert_eq!(concept_id("rust", "a//b"), "packages/rust_a_b");
        // Two units sharing a leaf name get distinct ids.
        assert_ne!(concept_id("rust", "x/foo"), concept_id("rust", "y/foo"));
        // Language-less concept omits the prefix.
        assert_eq!(concept_id("", "foo"), "packages/foo");
    }

    /// A Rust `geo` and a C# `Geo` must not collide as *filenames* on a
    /// case-insensitive filesystem — the language prefix makes the ids differ
    /// even after case-folding.
    #[test]
    fn concept_id_case_folds_distinct_across_languages() {
        let rust = concept_id("rust", "geo");
        let csharp = concept_id("csharp", "Geo");
        assert_ne!(rust, csharp);
        assert_ne!(
            rust.to_lowercase(),
            csharp.to_lowercase(),
            "ids must differ even on a case-insensitive filesystem"
        );
    }

    #[test]
    fn render_concept_is_conformant_and_deterministic() {
        let c = concept();
        let md = render_concept(&c);
        // Conformant: parseable frontmatter with a non-empty `type`.
        assert_eq!(concept_type(&md).as_deref(), Some("package"));
        // Structural body present, no synthesized prose beyond the verbatim doc.
        assert!(md.contains("## Central symbols"));
        // The dependency renders as a bundle-relative link (label is the id leaf).
        assert!(md.contains("(/packages/crates_kenn-model.md)"));
        assert!(md.contains("resource: crates/kenn-store/Cargo.toml"));
        // Deterministic: rendering twice is byte-identical (no wall-clock).
        assert_eq!(md, render_concept(&c));
        // No per-concept timestamp key (R3-C).
        assert!(!md.contains("timestamp"));
    }

    #[test]
    fn test_package_renders_tests_tag() {
        let mut c = concept();
        c.test = true;
        let md = render_concept(&c);
        assert!(
            md.contains("- tests"),
            "test package carries the `tests` tag"
        );
        // A production concept does not.
        let prod = concept();
        assert!(!render_concept(&prod).contains("- tests"));
    }

    #[test]
    fn central_location_uses_same_shortening_as_members() {
        let mut c = concept();
        c.resource = "crates/kenn-store".into();
        c.central[0].path = "crates/kenn-store/src/lib.rs".into();
        c.members = vec!["crates/kenn-store/src/lib.rs".into()];
        let md = render_concept(&c);
        // Central location is shortened relative to the resource, and the shared
        // prefix is stated in the heading — matching `## Members`.
        assert!(md.contains("## Central symbols under crates/kenn-store\n"));
        assert!(md.contains("| `rs:kenn-store::Store` | src/lib.rs:42-78 |"));
        // Members render the same short path.
        assert!(md.contains("## Members (under `crates/kenn-store/`)"));
        assert!(md.contains("- src/lib.rs\n"));
    }

    #[test]
    fn package_members_render_as_total_and_per_dir_histogram() {
        // D7: a package renders `## Members (under `X`) - <total>` + `- dir - n`
        // lines (no trailing slash, no truncated file list).
        let mut c = concept();
        c.resource = "Account.Data".into();
        c.members = Vec::new();
        c.file_count = 47;
        c.dir_counts = vec![
            ("src/Core".into(), 18),
            ("src/Features/Auth".into(), 5),
            ("src".into(), 2),
        ];
        let md = render_concept(&c);
        assert!(
            md.contains("## Files under Account.Data - 47\n"),
            "heading names the package dir + true total, no `Members`/backticks"
        );
        assert!(md.contains("\n- src/Core - 18\n"));
        assert!(md.contains("\n- src/Features/Auth - 5\n"));
        assert!(md.contains("\n- src - 2\n"));
        // A package never renders the flat per-file `## Members` list.
        assert!(!md.contains("## Members"));
    }

    #[test]
    fn concept_type_rejects_empty_or_malformed() {
        // Mutation-check backing (§9): an empty `type` is NOT conformant.
        let bad = render_concept(&concept()).replacen("type: package", "type: ''", 1);
        assert_eq!(concept_type(&bad), None, "empty type must fail conformance");
        assert_eq!(concept_type("no frontmatter here"), None);
    }

    #[test]
    fn description_omitted_when_absent() {
        let mut c = concept();
        c.description = None;
        let md = render_concept(&c);
        assert!(
            !md.contains("description:"),
            "no description key when absent"
        );
        assert_eq!(concept_type(&md).as_deref(), Some("package"));
    }

    #[test]
    fn render_log_prepends_and_preserves_history() {
        let first = render_log(None, "2026-07-13", "indexed at abc123");
        assert!(first.starts_with(LOG_TITLE));
        assert!(first.contains("## 2026-07-13"));
        let second = render_log(Some(&first), "2026-07-14", "indexed at def456");
        // Newest first, and the first run's entry survives (append-preserve).
        let i14 = second.find("## 2026-07-14").unwrap();
        let i13 = second.find("## 2026-07-13").unwrap();
        assert!(i14 < i13, "newest section first");
        assert!(second.contains("abc123"), "prior history retained");
    }

    #[test]
    fn index_has_no_frontmatter_and_lists_concepts() {
        let shape = AtlasShape {
            name: "code_with_me".into(),
            languages: vec!["rust".into(), "ts".into()],
            packages: 1,
            symbols: 1240,
            test_ratio_pct: 18,
            freshness: "HEAD abc123".into(),
            timestamp: "2026-07-13T00:00:00Z".into(),
        };
        let idx = render_index(&shape, std::slice::from_ref(&concept()), &[]);
        assert!(!idx.starts_with("---"), "index.md carries no frontmatter");
        assert!(idx.contains("# code_with_me atlas"));
        assert!(idx.contains("1 packages · 0 domains · 1240 symbols · 18% test · rust, ts"));
        assert!(idx.contains("[kenn-store](/packages/crates_kenn-store.md) — SQLite code graph"));
    }

    fn domain() -> DomainConcept {
        DomainConcept {
            id: "domains/Hub".into(),
            title: "Hub".into(),
            size: 12,
            packages: vec!["packages/alpha".into(), "packages/beta".into()],
            central: vec![CentralSymbol {
                name: "Hub".into(),
                pub_id: "rs:alpha::Hub".into(),
                path: "alpha/src/hub.rs".into(),
                line_start: 5,
                line_end: 20,
            }],
        }
    }

    #[test]
    fn domain_id_slugifies_hub_name() {
        assert_eq!(domain_id("SharedEmbedder"), "domains/SharedEmbedder");
        assert_eq!(domain_id("build_concepts"), "domains/build_concepts");
        assert_eq!(domain_id("a<b>"), "domains/a-b"); // angle brackets → one dash, trailing trimmed
        assert_eq!(domain_id("<>"), "domains/domain"); // no identifier chars → fallback
    }

    #[test]
    fn render_domain_is_okf_conformant_and_deterministic() {
        let md = render_domain(&domain());
        assert_eq!(concept_type(&md).as_deref(), Some("domain"));
        assert!(md.contains("## Central symbols"));
        assert!(md.contains("## Spanned packages"));
        assert!(md.contains("[alpha](/packages/alpha.md)"));
        assert!(md.contains("alpha/src/hub.rs:5-20"));
        // A domain is not directory-backed → no resource field.
        assert!(!md.contains("resource:"));
        assert_eq!(md, render_domain(&domain()));
    }

    #[test]
    fn index_lists_a_domains_section() {
        let shape = AtlasShape {
            name: "code_with_me".into(),
            languages: vec!["rust".into()],
            packages: 1,
            symbols: 1240,
            test_ratio_pct: 18,
            freshness: "HEAD abc123".into(),
            timestamp: "2026-07-13T00:00:00Z".into(),
        };
        let idx = render_index(
            &shape,
            std::slice::from_ref(&concept()),
            std::slice::from_ref(&domain()),
        );
        assert!(idx.contains("1 packages · 1 domains · 1240 symbols"));
        assert!(idx.contains("## Domains"));
        assert!(idx.contains("[Hub](/domains/Hub.md) — 2 packages · 12 symbols"));
    }
}
