//! OKF v0.1 serialization for the atlas bundle (`atlas` capability): concept
//! documents (YAML frontmatter + markdown body), the reserved `index.md`
//! (frontmatter-free shape header + concept list) and `log.md` (append-preserved,
//! newest-first), plus a conformance probe. Renders [`super::model`] values;
//! no I/O, no store access. Deterministic — no wall-clock in a concept doc, stable
//! key order, so re-indexing an unchanged repo is a no-op diff.

use std::collections::BTreeMap;

use serde::Serialize;

use super::model::{
    AtlasShape, Concept, ContractConcept, Coupling, DomainConcept, Role, SymbolRef, TableConcept,
};

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
    let flat: String = anchor
        .trim_matches('/')
        .chars()
        .map(|c| if is_id_safe(c) { c } else { '_' })
        .collect();
    let flat = collapse_underscores(&flat);
    // Never leave a separator or a dot on either end: `foo.` is an illegal
    // filename on Windows, and `_foo` reads as a missing segment.
    let flat = flat.trim_matches(|c| c == '_' || c == '.');
    let id = if language.is_empty() {
        format!("packages/{flat}")
    } else {
        format!("packages/{language}_{flat}")
    };
    collapse_underscores(&id)
}

/// Whether `ch` may appear literally in an atlas concept id.
///
/// An ALLOWLIST, not a denylist: alphanumerics plus the four marks real package
/// names carry — `_` (the separator), `-` (`kenn-store`, `go-plugin`), `.` (a C#
/// namespace like `Acme.Billing`, or `platform-socket.io`) and `@` (a scoped npm
/// package, `@nestjs/core`). Everything else, path separators included, becomes
/// `_`.
///
/// Two things this prevents, both observed. A Swift package anchored on
/// `ArgumentParser/Parsable Properties` produced the filename
/// `swift_ArgumentParser_Parsable Properties.md`, and the index linked it as
/// `](/packages/swift_ArgumentParser_Parsable Properties.md)` — an unescaped
/// space TERMINATES a link destination in `CommonMark`, so the file existed on
/// disk while every markdown reader saw a broken link. And `:` `<` `>` `"` `|`
/// `?` `*` are illegal in Windows filenames, which this project targets; a
/// denylist would have had to enumerate them and would still miss the next one.
fn is_id_safe(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '@')
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

/// A filesystem-safe slug from a symbol name: alphanumerics kept, every other
/// char mapped to `_`, runs collapsed to a single `_`, ends trimmed. `fallback`
/// when nothing survives.
///
/// `_` is the separator, matching [`concept_id`] — one flattening rule for every
/// atlas id rather than `_` for packages and `-` for symbols. Swift is what
/// forced it: an argument-labelled name is mostly punctuation, and mapping each
/// run to a dash while KEEPING the name's own underscores produced
/// `replacing(_:with:)` → `replacing-_-with`, a stutter of separators standing
/// in for characters a reader never typed. Mapping everything to `_` and
/// coalescing gives `replacing_with`.
///
/// This also earns an invariant: a slug can never contain `-`, so the `-{n}`
/// suffix the producer appends to break id collisions cannot be confused with
/// slug content.
fn concept_slug<'a>(name: &str, fallback: &'a str) -> std::borrow::Cow<'a, str> {
    // `_` maps to itself, so alphanumeric is the whole keep-set. Unicode-aware
    // on purpose: an ASCII-only test would map a non-Latin type name to solid
    // underscores and drop the whole concept onto `fallback`, losing its name.
    let mapped: String = name
        .chars()
        .map(|ch| if ch.is_alphanumeric() { ch } else { '_' })
        .collect();
    let collapsed = collapse_underscores(&mapped);
    let trimmed = collapsed.trim_matches('_');
    if trimmed.is_empty() {
        std::borrow::Cow::Borrowed(fallback)
    } else {
        std::borrow::Cow::Owned(trimmed.to_string())
    }
}

/// A readable contract concept id from the interface/base name: a slug under
/// `contracts/`. The producer disambiguates any residual slug collision.
#[must_use]
pub fn contract_id(name: &str) -> String {
    format!("contracts/{}", concept_slug(name, "contract"))
}

/// Bundle-relative concept id for a table, under `tables/`. The producer
/// disambiguates any residual slug collision.
#[must_use]
pub fn table_id(name: &str) -> String {
    format!("tables/{}", concept_slug(name, "table"))
}

/// A readable domain concept id from its hub symbol name: a slug under
/// `domains/`. Non-alphanumerics collapse to a single `_` (a hub like
/// `Cow<'_, B>` → `domains/Cow_B`); the producer disambiguates any residual slug
/// collision. The `.md` suffix is added by the file writer.
#[must_use]
pub fn domain_id(hub: &str) -> String {
    format!("domains/{}", concept_slug(hub, "domain"))
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

/// The ` — {total}, heaviest {shown}` suffix an axis heading carries when its
/// render cap binds, and nothing when it doesn't — so the suffix itself is the
/// truncation signal. The same rule [`render_couplings`] applies to a package's
/// coupling lists, which is where it was already honoured; the domains and
/// contracts axes reported their CAPPED length as the total until this existed.
fn cap_suffix(total: usize, shown: usize, verb: &str) -> String {
    if total > shown {
        // Naming the cap tells a reader the page is partial; naming the VERB tells
        // them how to see the rest. Without it the heading is honest but a dead
        // end — a reader learns 54 domains exist and has nowhere to go, when
        // `kenn domains` returns all 78 uncapped by design.
        format!(" — {total}, heaviest {shown} shown · all via `kenn {verb}`")
    } else {
        String::new()
    }
}

/// Render one coupling direction as a `Package | Weight | Relations` table, or
/// the empty string when there is nothing to show (a leaf package has no
/// dependents; a vocabulary package has no dependencies).
///
/// The relation split is the point of the table. A bare package name says two
/// packages are coupled; `type_use 102 · calls 15 · implements 6` says HOW —
/// and `implements` in particular marks a contract/implementer pair rather than
/// incidental use, which no weight sum can express.
#[must_use]
#[expect(
    clippy::format_push_string,
    reason = "building a markdown table row by row; format! per row reads clearer than write!"
)]
fn render_couplings(heading: &str, items: &[Coupling], total: u64) -> String {
    if items.is_empty() {
        return String::new();
    }
    // Name the cap when it binds. A truncated list that says nothing reads as
    // the WHOLE list — on a real 125-package solution one package showed 8 of
    // its 100 dependents, indistinguishable from a package with 8.
    let shown = items.len() as u64;
    let suffix = if total > shown {
        format!(" — {total} packages, heaviest {shown}")
    } else {
        String::new()
    };
    let mut out =
        format!("\n## {heading}{suffix}\n\n| Package | Weight | Relations |\n|---|---|---|\n");
    for c in items {
        let rels: Vec<String> = c
            .relations
            .iter()
            .map(|(kind, w)| format!("{kind} {w}"))
            .collect();
        out.push_str(&format!(
            "| [{}](/{}.md) | {} | {} |\n",
            c.title,
            c.concept_id,
            c.weight,
            rels.join(" · "),
        ));
    }
    out
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
    // `Used by` leads: "who breaks if I change this" is the question a reader
    // brings to a package they are about to touch, and it is the one the
    // outgoing list cannot answer.
    body.push_str(&render_couplings("Used by", &c.used_by, c.used_by_total));
    body.push_str(&render_couplings("Depends on", &c.deps, c.deps_total));
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
        // Members = how much of the domain lives in the package; Links = its
        // first-party edges to the domain's OTHER packages — the coupling that
        // earned it the span, distinguishing a real participant from a straggler.
        body.push_str("\n## Spanned packages\n\n| Package | Members | Links |\n|---|---|---|\n");
        for p in &d.packages {
            body.push_str(&format!(
                "| [{}](/{}.md) | {} | {} |\n",
                p.title, p.concept_id, p.members, p.links,
            ));
        }
    }
    format!("---\n{yaml}{sep}---\n{body}")
}

/// Render one contract concept document: the interface/base, the package it is
/// defined in, and its implementers grouped by package — read directly from the
/// is-a edges, so it is complete and deterministic (unlike a domain's clustered
/// span). A heading like `## Implementers — 426 across 55 packages` names the
/// full breadth even when the table below is capped.
#[must_use]
#[expect(
    clippy::format_push_string,
    reason = "building a markdown document line by line; format! per line reads clearer than write!"
)]
pub fn render_contract(c: &ContractConcept) -> String {
    let front = Frontmatter {
        type_: "contract",
        title: &c.title,
        description: None,
        resource: None,
        tags: if c.kind.is_empty() {
            Vec::new()
        } else {
            vec![c.kind.as_str()]
        },
    };
    let yaml = serde_yaml_ng::to_string(&front).unwrap_or_default();
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let sep = if yaml.ends_with('\n') { "" } else { "\n" };

    let mut body = format!(
        "\n_Defined in [{}](/{}.md)_\n\n| ID | Location |\n|---|---|\n| `{}` | {} |\n",
        c.defined_in_title,
        c.defined_in_id,
        c.symbol.pub_id,
        location(&c.symbol),
    );
    // Name the full breadth in the heading; the table below may be capped.
    let shown = c.implementers.len() as u64;
    let cap_note = if c.package_span > shown {
        format!(", heaviest {shown} shown")
    } else {
        String::new()
    };
    body.push_str(&format!(
        "\n## Implementers — {} across {} packages{cap_note}\n",
        c.total_implementers, c.package_span,
    ));
    // One section per package: an `ID | Location` table (same shape a package
    // concept uses for central symbols), so each implementer is `kenn get`-able
    // and jump-to-source. The package heading carries its full implementer count.
    for p in &c.implementers {
        body.push_str(&format!(
            "\n### [{}](/{}.md) — {}\n\n| ID | Location |\n|---|---|\n",
            p.title, p.concept_id, p.count,
        ));
        for s in &p.symbols {
            body.push_str(&format!("| `{}` | {} |\n", s.pub_id, location(s)));
        }
        // Name the extras the per-package cap dropped rather than truncate silently.
        if p.count > p.symbols.len() as u64 {
            body.push_str(&format!(
                "\n_… (+{} more)_\n",
                p.count - p.symbols.len() as u64
            ));
        }
    }
    format!("---\n{yaml}{sep}---\n{body}")
}

/// The workspace-relative source location of a central symbol: `path`,
/// `path:line`, or `path:start-end`. `line_start` 0 means unknown → path only.
fn location(s: &SymbolRef) -> String {
    if s.line_start == 0 {
        s.path.clone()
    } else if s.line_end > s.line_start {
        format!("{}:{}-{}", s.path, s.line_start, s.line_end)
    } else {
        format!("{}:{}", s.path, s.line_start)
    }
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

/// The module-doc first line as a one-line gloss, or a file count for a
/// document concept. Empty when the package carries no doc.
fn gloss(c: &Concept) -> String {
    c.description
        .as_deref()
        .and_then(|d| d.lines().next())
        .map(|l| format!(" — {}", l.trim()))
        .or_else(|| (c.concept_type == "document").then(|| format!(" — {} files", c.symbols)))
        .unwrap_or_default()
}

/// The dependent/dependency counts a package's [`Role`] was derived from, so a
/// reader can check the grouping instead of trusting it. The language rides
/// here too — role sections replaced the per-language ones, and in a
/// multi-language repo that fact would otherwise be lost from the index.
fn counts_note(c: &Concept, multilingual: bool) -> String {
    let lang = if multilingual && !c.language.is_empty() {
        format!("{} · ", lang_display(&c.language))
    } else {
        String::new()
    };
    format!(
        " ({lang}{} used by · {} deps)",
        c.used_by_total, c.deps_total
    )
}

/// Render one table concept: what it is, and every site that names it grouped
/// by the file the reference was made in.
///
/// Grouped by FILE rather than by package, unlike a contract's implementers. A
/// table's references have no package to roll into — a statement in a migration,
/// an element in a changelog and a function in application code are three files
/// in three languages, and which file made the reference is the reader's actual
/// question.
#[must_use]
#[expect(
    clippy::format_push_string,
    reason = "building a markdown document line by line; format! per line reads clearer than write! — same as every sibling renderer"
)]
pub fn render_table(t: &TableConcept) -> String {
    let front = Frontmatter {
        type_: "table",
        title: &t.title,
        description: None,
        resource: None,
        tags: vec![if t.internal { "declared" } else { "external" }],
    };
    let yaml = serde_yaml_ng::to_string(&front).unwrap_or_default();
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let sep = if yaml.ends_with('\n') { "" } else { "\n" };

    // "external" is ordinary rather than a defect: measured on a real
    // repository, 85 of 133 tables were named only by an attribute and declared
    // by no statement in the workspace at all.
    let origin = if t.internal {
        "Declared in this workspace"
    } else {
        "Declared elsewhere — this workspace only references it"
    };
    let mut body = format!("\n_{origin}_\n\n| ID |\n|---|\n| `{}` |\n", t.pub_id);
    let shown = t.by_file.len() as u64;
    let cap_note = if t.file_span > shown {
        format!(", heaviest {shown} shown")
    } else {
        String::new()
    };
    body.push_str(&format!(
        "\n## References — {} across {} files in {} languages{cap_note}\n",
        t.total_refs, t.file_span, t.language_span,
    ));
    for f in &t.by_file {
        body.push_str(&format!(
            "\n### `{}` — {} ({})\n\n| Does | ID | Location |\n|---|---|---|\n",
            f.file, f.count, f.language,
        ));
        for (kind, s) in &f.sites {
            body.push_str(&format!("| {kind} | `{}` | {} |\n", s.pub_id, location(s)));
        }
        if f.count > f.sites.len() as u64 {
            body.push_str(&format!(
                "\n_… (+{} more)_\n",
                f.count - f.sites.len() as u64
            ));
        }
    }
    format!("---\n{yaml}{sep}---\n{body}")
}

/// The tables axis section of `index.md`. Extracted because each axis is a
/// self-contained block and `render_index` grows by one every time an axis
/// lands — this one took it past the line limit.
#[expect(
    clippy::format_push_string,
    reason = "building a markdown document line by line; same as every sibling renderer"
)]
fn push_tables_axis(out: &mut String, shape: &AtlasShape, tables: &[TableConcept]) {
    // The tables axis — the schema, and how broadly each table is referenced.
    // Ranked by breadth (files, then languages) because a table named by a
    // migration, a changelog and application code is the interesting one.
    if !tables.is_empty() {
        out.push_str(&format!(
            "\n## Tables{}\n\n",
            cap_suffix(shape.tables_total, tables.len(), "tables")
        ));
        for t in tables {
            let origin = if t.internal { "" } else { " · external" };
            out.push_str(&format!(
                "- [{}](/{}.md) — {} references across {} files{origin}\n",
                t.title, t.id, t.total_refs, t.file_span,
            ));
        }
    }
}

/// Render the reserved `index.md`: a frontmatter-free shape/status header, then
/// one section per [`Role`] (foundation-first), a `## Documents` section, and
/// the cross-package `## Domains`, `## Contracts` and `## Tables` axes last.
#[must_use]
#[expect(
    clippy::format_push_string,
    reason = "building a markdown document line by line; format! per line reads clearer than write!"
)]
pub fn render_index(
    shape: &AtlasShape,
    concepts: &[Concept],
    domains: &[DomainConcept],
    contracts: &[ContractConcept],
    tables: &[TableConcept],
) -> String {
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
        // The repo's shape, NOT the bundle's: the pre-cap count. `n concepts`
        // below is the written-file count, and the two legitimately differ once
        // a cap binds — which is why the axis heading names it.
        doms = shape.domains_total,
        syms = shape.symbols,
        test = shape.test_ratio_pct,
        fresh = shape.freshness,
        ts = shape.timestamp,
        n = concepts.len() + domains.len() + contracts.len(),
    );

    // Group code packages by ROLE, not language. Language is a filesystem fact
    // and it collapses at scale: a real 125-package solution rendered as 123
    // alphabetical bullets under one `## C#`, which says nothing about which
    // packages matter. Role sections are ordered foundation-first (the reading
    // order of a stack), with tests and isolated packages last — they are the
    // bulk of a large list and the least of its architecture.
    let mut by_role: BTreeMap<Role, Vec<&Concept>> = BTreeMap::new();
    let mut components: Vec<&Concept> = Vec::new();
    let mut documents: Vec<&Concept> = Vec::new();
    // Route by concept TYPE first. A component carries `role: None` (it is a
    // package's source sub-area, not a package), so a role-first match files
    // every one of them under Documents — which is what it used to do.
    for c in concepts {
        match c.concept_type.as_str() {
            "document" => documents.push(c),
            "component" => components.push(c),
            _ => match c.role {
                Some(r) => by_role.entry(r).or_default().push(c),
                None => documents.push(c),
            },
        }
    }
    let multilingual = shape.languages.len() > 1;
    for (role, mut list) in by_role {
        // Heaviest first: on a long list the packages everything rests on must
        // be the ones a reader sees, not whichever sorts alphabetically first.
        list.sort_by(|a, b| {
            b.used_by_total
                .cmp(&a.used_by_total)
                .then_with(|| a.title.cmp(&b.title))
        });
        out.push_str(&format!("\n## {}\n\n", role.heading()));
        for c in list {
            out.push_str(&format!(
                "- [{}](/{}.md){}{}\n",
                c.title,
                c.id,
                // The counts the role came from, so the label is checkable
                // rather than asserted.
                counts_note(c, multilingual),
                gloss(c),
            ));
        }
    }
    if !components.is_empty() {
        // A single-dominant repo is one big package, so its sub-areas ARE the
        // structure a reader navigates — they belong in the index, under their
        // own heading rather than mislabelled as documents.
        components.sort_by(|a, b| a.title.cmp(&b.title));
        out.push_str("\n## Components — source sub-areas\n\n");
        for c in components {
            out.push_str(&format!("- [{}](/{}.md){}\n", c.title, c.id, gloss(c)));
        }
    }
    if !documents.is_empty() {
        out.push_str("\n## Documents\n\n");
        for c in documents {
            out.push_str(&format!("- [{}](/{}.md){}\n", c.title, c.id, gloss(c)));
        }
    }

    // The cross-package domains axis, pinned last — a distinct axis, not a
    // language-sectioned package.
    if !domains.is_empty() {
        out.push_str(&format!(
            "\n## Domains{}\n\n",
            cap_suffix(shape.domains_total, domains.len(), "domains")
        ));
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

    // The cross-package contracts axis — first-party abstractions and where they
    // are implemented across the tree, widest span first.
    if !contracts.is_empty() {
        out.push_str(&format!(
            "\n## Contracts{}\n\n",
            cap_suffix(shape.contracts_total, contracts.len(), "contracts")
        ));
        for c in contracts {
            out.push_str(&format!(
                "- [{}](/{}.md) — {} implementers across {} packages\n",
                c.title, c.id, c.total_implementers, c.package_span,
            ));
        }
    }

    push_tables_axis(&mut out, shape, tables);
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
    use crate::atlas::model::{ContractImplementers, SpannedPackage, SymbolRef};

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
            deps: vec![Coupling {
                concept_id: "packages/crates_kenn-model".into(),
                title: "kenn-model".into(),
                weight: 2007,
                relations: vec![("type_use".into(), 1900), ("calls".into(), 107)],
            }],
            used_by: vec![Coupling {
                concept_id: "packages/crates_kenn-indexer".into(),
                title: "kenn-indexer".into(),
                weight: 1199,
                relations: vec![("type_use".into(), 1199)],
            }],
            deps_total: 1,
            used_by_total: 1,
            role: Some(Role::Layer),
            central: vec![SymbolRef {
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

    /// A concept id becomes a FILENAME and a markdown link destination, so it may
    /// not contain a space or any character Windows forbids.
    ///
    /// The space is the one that bit: a Swift package anchored on
    /// `ArgumentParser/Parsable Properties` was written to
    /// `swift_ArgumentParser_Parsable Properties.md` and linked as
    /// `](/packages/… Parsable Properties.md)`. An unescaped space terminates a
    /// link destination in `CommonMark`, so the file existed while every markdown
    /// reader saw a broken link — and a file-existence check (which is what the
    /// bundle verifier did) could not see it.
    ///
    /// Mutation-checked: reverting `concept_id` to `replace(['/', '\\'], "_")`
    /// fails the first assertion.
    #[test]
    fn concept_id_is_a_safe_filename_and_link_target() {
        assert_eq!(
            concept_id("swift", "ArgumentParser/Parsable Properties"),
            "packages/swift_ArgumentParser_Parsable_Properties"
        );
        // Every character Windows forbids in a filename maps to the separator.
        for hostile in [':', '<', '>', '"', '|', '?', '*'] {
            let id = concept_id("rust", &format!("a{hostile}b"));
            assert_eq!(id, "packages/rust_a_b", "{hostile:?} must not survive");
        }
        // A trailing dot is an illegal Windows filename; a leading one hides it.
        assert_eq!(concept_id("go", "pkg."), "packages/go_pkg");
        assert_eq!(concept_id("go", ".pkg"), "packages/go_pkg");
        // The marks real package names carry are KEPT — this is an allowlist, so
        // regressing it would silently mangle ordinary names.
        assert_eq!(
            concept_id("typescript", "@nestjs/core"),
            "packages/typescript_@nestjs_core"
        );
        assert_eq!(
            concept_id("csharp", "Acme.Billing"),
            "packages/csharp_Acme.Billing"
        );
        assert_eq!(
            concept_id("typescript", "platform-socket.io"),
            "packages/typescript_platform-socket.io"
        );
        // Non-ASCII names survive rather than collapsing to underscores.
        assert_eq!(concept_id("go", "пакет"), "packages/go_пакет");
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

    /// Both coupling directions render, with the relation split. `Used by` is
    /// the direction the outgoing list cannot answer — a package that four
    /// others depend on used to render a concept that said only what IT
    /// depended on. Mutation-checked: dropping the `used_by` render call, or
    /// the relation column, fails the corresponding assertion.
    #[test]
    fn couplings_render_both_directions_with_relations() {
        let md = render_concept(&concept());
        assert!(md.contains("## Used by"), "the inverse direction: {md}");
        assert!(md.contains("## Depends on"));
        assert!(
            md.contains("| [kenn-model](/packages/crates_kenn-model.md) | 2007 | type_use 1900 · calls 107 |"),
            "weight AND relation split, heaviest relation first: {md}"
        );
        assert!(
            md.contains(
                "| [kenn-indexer](/packages/crates_kenn-indexer.md) | 1199 | type_use 1199 |"
            ),
            "dependents carry the same shape: {md}"
        );
        // `Used by` leads — a reader about to change this package wants its
        // blast radius before its own dependencies.
        assert!(
            md.find("## Used by") < md.find("## Depends on"),
            "Used by precedes Depends on: {md}"
        );
    }

    /// A capped list that says nothing reads as the whole list. On a real
    /// 125-package solution one package showed 8 of its 100 dependents,
    /// indistinguishable from a package that genuinely has 8. Mutation-checked:
    /// dropping the suffix fails this; so does emitting it when nothing is cut.
    #[test]
    fn a_truncated_coupling_list_names_what_it_dropped() {
        let mut c = concept();
        c.used_by_total = 100;
        let md = render_concept(&c);
        assert!(
            md.contains("## Used by — 100 packages, heaviest 1"),
            "the true total AND how many are shown: {md}"
        );
        // The uncapped direction stays clean — no noise when nothing is cut.
        assert!(
            md.contains("## Depends on\n"),
            "an untruncated heading carries no suffix: {md}"
        );
    }

    /// A leaf package has no dependents and a vocabulary package no
    /// dependencies; neither should render an empty table.
    #[test]
    fn empty_coupling_renders_no_heading() {
        let mut c = concept();
        c.used_by = Vec::new();
        let md = render_concept(&c);
        assert!(!md.contains("## Used by"), "no empty table: {md}");
        assert!(md.contains("## Depends on"), "the other side still renders");
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
            domains_total: 0,
            contracts_total: 0,
            tables_total: 0,
            freshness: "HEAD abc123".into(),
            timestamp: "2026-07-13T00:00:00Z".into(),
        };
        let idx = render_index(&shape, std::slice::from_ref(&concept()), &[], &[], &[]);
        assert!(!idx.starts_with("---"), "index.md carries no frontmatter");
        assert!(idx.contains("# code_with_me atlas"));
        assert!(idx.contains("1 packages · 0 domains · 1240 symbols · 18% test · rust, ts"));
        // Entry: link, then the counts the role came from, then the module-doc
        // gloss. Two languages in the shape, so the language rides the note —
        // role sections replaced the per-language ones.
        assert!(
            idx.contains(
                "[kenn-store](/packages/crates_kenn-store.md) (Rust · 1 used by · 1 deps) — SQLite code graph"
            ),
            "{idx}"
        );
    }

    /// Packages group by ROLE, not language, and each section states its rule.
    /// A flat per-language list is what made a real 125-package solution render
    /// as 123 alphabetical bullets. Mutation-checked: grouping on
    /// `lang_display(&c.language)` again produces a `## Rust` heading and no
    /// role heading.
    #[test]
    fn index_groups_packages_by_role() {
        let shape = AtlasShape {
            name: "ws".into(),
            languages: vec!["rust".into()],
            packages: 3,
            symbols: 10,
            test_ratio_pct: 0,
            domains_total: 0,
            contracts_total: 0,
            tables_total: 0,
            freshness: "HEAD abc".into(),
            timestamp: "2026-07-13T00:00:00Z".into(),
        };
        let mk = |title: &str, role: Role, used_by: u64| Concept {
            title: title.into(),
            id: format!("packages/rust_{title}"),
            description: None,
            role: Some(role),
            used_by_total: used_by,
            ..concept()
        };
        let idx = render_index(
            &shape,
            &[
                mk("app", Role::Consumer, 0),
                mk("util", Role::Provider, 40),
                mk("core", Role::Provider, 90),
            ],
            &[],
            &[],
            &[],
        );
        assert!(idx.contains("## Providers — depended on, depending on little"));
        assert!(idx.contains("## Consumers — depending on much, little depends on them"));
        assert!(!idx.contains("## Rust"), "language is not the axis: {idx}");
        // Foundation first, and within a section the most-depended-on leads —
        // alphabetical would have put `util` above `core`.
        let (providers, core, util, consumers) = (
            idx.find("## Providers").unwrap(),
            idx.find("[core]").unwrap(),
            idx.find("[util]").unwrap(),
            idx.find("## Consumers").unwrap(),
        );
        assert!(providers < core && core < util, "heaviest first: {idx}");
        assert!(util < consumers, "providers precede consumers: {idx}");
        // Single-language repo: no language noise in the counts note.
        assert!(
            idx.contains("[core](/packages/rust_core.md) (90 used by · 1 deps)"),
            "{idx}"
        );
    }

    fn domain() -> DomainConcept {
        DomainConcept {
            id: "domains/Hub".into(),
            title: "Hub".into(),
            size: 12,
            packages: vec![
                SpannedPackage {
                    concept_id: "packages/rust_alpha".into(),
                    title: "alpha".into(),
                    members: 7,
                    links: 4,
                },
                SpannedPackage {
                    concept_id: "packages/rust_beta".into(),
                    title: "beta".into(),
                    members: 5,
                    links: 4,
                },
            ],
            central: vec![SymbolRef {
                name: "Hub".into(),
                pub_id: "rs:alpha::Hub".into(),
                path: "alpha/src/hub.rs".into(),
                line_start: 5,
                line_end: 20,
            }],
        }
    }

    /// One separator for every atlas id: `_`, runs collapsed, ends trimmed.
    #[test]
    fn domain_id_slugifies_hub_name() {
        assert_eq!(domain_id("SharedEmbedder"), "domains/SharedEmbedder");
        // A name's own underscores survive as single separators.
        assert_eq!(domain_id("build_concepts"), "domains/build_concepts");
        assert_eq!(domain_id("a<b>"), "domains/a_b"); // angle brackets → one `_`, trailing trimmed
        assert_eq!(domain_id("<>"), "domains/domain"); // nothing survives → fallback
    }

    /// Swift argument labels are mostly punctuation, and they are what forced
    /// the separator change: a rule that mapped runs of "other" chars to `-`
    /// while KEEPING the name's own `_` emitted `replacing-_-with`, three
    /// separators for one word boundary. Mapping everything to `_` and
    /// coalescing reads as the name does.
    ///
    /// Mutation-checked: putting `_` back in the keep-set and mapping the rest
    /// to `-` fails here with `replacing-_-with--`, the stutter this replaced.
    #[test]
    fn punctuation_heavy_names_collapse_to_single_underscores() {
        assert_eq!(domain_id("replacing(_:with:)"), "domains/replacing_with");
        assert_eq!(
            domain_id("AssertEqualStrings(actual:expected:file:line:sourceLocation:)"),
            "domains/AssertEqualStrings_actual_expected_file_line_sourceLocation"
        );
        assert_eq!(contract_id("Cow<'_, B>"), "contracts/Cow_B");
        // A generic C# interface keeps its arity readable, not stuttered.
        assert_eq!(contract_id("IReadOnlyList<T>"), "contracts/IReadOnlyList_T");
    }

    /// A slug can never contain `-`, so the producer's `-{n}` collision suffix
    /// is unambiguous by construction — a name cannot forge one.
    #[test]
    fn a_slug_never_contains_a_dash() {
        for name in [
            "replacing(_:with:)",
            "kebab-cased-name",
            "Cow<'_, B>",
            "a-2",
        ] {
            let id = domain_id(name);
            let slug = id.strip_prefix("domains/").expect("domain_id prefix");
            assert!(!slug.contains('-'), "{name} → {id}");
        }
    }

    #[test]
    fn render_domain_is_okf_conformant_and_deterministic() {
        let md = render_domain(&domain());
        assert_eq!(concept_type(&md).as_deref(), Some("domain"));
        assert!(md.contains("## Central symbols"));
        assert!(md.contains("## Spanned packages"));
        // The link label is the package's display title, not the flattened,
        // language-prefixed concept-id leaf (`rust_alpha`).
        assert!(md.contains("| [alpha](/packages/rust_alpha.md) | 7 | 4 |"));
        assert!(md.contains("| [beta](/packages/rust_beta.md) | 5 | 4 |"));
        assert!(md.contains("alpha/src/hub.rs:5-20"));
        // A domain is not directory-backed → no resource field.
        assert!(!md.contains("resource:"));
        assert_eq!(md, render_domain(&domain()));
    }

    fn impl_sym(name: &str, line: u32) -> SymbolRef {
        SymbolRef {
            name: name.into(),
            pub_id: format!("rs:core::{name}"),
            path: format!("src/{name}.rs"),
            line_start: line,
            line_end: line + 5,
        }
    }

    fn contract() -> ContractConcept {
        ContractConcept {
            id: "contracts/Store".into(),
            title: "Store".into(),
            kind: "interface".into(),
            symbol: impl_sym("Store", 5),
            defined_in_id: "packages/rust_core".into(),
            defined_in_title: "core".into(),
            implementers: vec![
                ContractImplementers {
                    concept_id: "packages/rust_mem".into(),
                    title: "mem".into(),
                    symbols: vec![impl_sym("FastStore", 10), impl_sym("MemStore", 20)],
                    count: 8, // 6 more than the 2 shown
                },
                ContractImplementers {
                    concept_id: "packages/rust_disk".into(),
                    title: "disk".into(),
                    symbols: vec![impl_sym("DiskStore", 3)],
                    count: 1,
                },
            ],
            total_implementers: 9,
            package_span: 3, // 3 spanned but only 2 rendered → the cap note fires
        }
    }

    #[test]
    fn render_contract_is_conformant_and_deterministic() {
        let md = render_contract(&contract());
        assert_eq!(concept_type(&md).as_deref(), Some("contract"));
        // The `kind` rides the standard `tags` field.
        assert!(md.contains("- interface"));
        assert!(md.contains("_Defined in [core](/packages/rust_core.md)_"));
        // The contract type itself is a resolvable ID | Location row.
        assert!(md.contains("| `rs:core::Store` | src/Store.rs:5-10 |"));
        // Heading names the full breadth (9 / 3) and that the package list is capped.
        assert!(md.contains("## Implementers — 9 across 3 packages, heaviest 2 shown"));
        // Each package is a section with an `ID | Location` table of implementers.
        assert!(md.contains("### [mem](/packages/rust_mem.md) — 8"));
        assert!(md.contains("| `rs:core::MemStore` | src/MemStore.rs:20-25 |"));
        assert!(md.contains("### [disk](/packages/rust_disk.md) — 1"));
        assert!(md.contains("| `rs:core::DiskStore` | src/DiskStore.rs:3-8 |"));
        // The per-package cap names the dropped extras rather than truncating.
        assert!(md.contains("_… (+6 more)_"));
        assert_eq!(md, render_contract(&contract()));
    }

    #[test]
    fn index_lists_a_domains_section() {
        let shape = AtlasShape {
            name: "code_with_me".into(),
            languages: vec!["rust".into()],
            packages: 1,
            symbols: 1240,
            test_ratio_pct: 18,
            domains_total: 1,
            contracts_total: 0,
            tables_total: 0,
            freshness: "HEAD abc123".into(),
            timestamp: "2026-07-13T00:00:00Z".into(),
        };
        let idx = render_index(
            &shape,
            std::slice::from_ref(&concept()),
            std::slice::from_ref(&domain()),
            &[],
            &[],
        );
        assert!(idx.contains("1 packages · 1 domains · 1240 symbols"));
        // Uncapped: the heading carries no suffix, so the suffix itself signals
        // truncation wherever it appears.
        assert!(idx.contains("## Domains\n"));
        assert!(!idx.contains("## Domains —"));
        assert!(idx.contains("[Hub](/domains/Hub.md) — 2 packages · 12 symbols"));
    }

    /// A capped axis must not report its capped length as the total. On a real
    /// 125-package solution the header read `24 domains` for a repo with 78 —
    /// indistinguishable from a repo that genuinely has 24, and the `## Domains`
    /// heading said nothing either. `MAX_DOMAINS`/`MAX_CONTRACTS` are 24, so
    /// only a repo past that ever exposed it; every small repo passed silently.
    ///
    /// Mutation-checked twice: reverting the header to `domains.len()` fails the
    /// first assertion, and making `cap_suffix` always return `String::new()`
    /// fails the second.
    #[test]
    fn a_capped_axis_names_what_it_dropped() {
        let shape = AtlasShape {
            name: "big".into(),
            languages: vec!["csharp".into()],
            packages: 125,
            symbols: 86_619,
            test_ratio_pct: 19,
            domains_total: 78,
            contracts_total: 40,
            tables_total: 0,
            freshness: "HEAD abc123".into(),
            timestamp: "2026-07-13T00:00:00Z".into(),
        };
        let idx = render_index(
            &shape,
            std::slice::from_ref(&concept()),
            std::slice::from_ref(&domain()),
            std::slice::from_ref(&contract()),
            &[],
        );
        // The header states the repo's shape, not the bundle's contents.
        assert!(
            idx.contains("125 packages · 78 domains ·"),
            "header must report the pre-cap total: {idx}"
        );
        // Names the cap AND the verb that reaches the rest: a heading that only
        // admits truncation leaves a reader with 77 domains and nowhere to go.
        assert!(
            idx.contains("## Domains — 78, heaviest 1 shown · all via `kenn domains`"),
            "the domains heading must name the cap and the query: {idx}"
        );
        assert!(
            idx.contains("## Contracts — 40, heaviest 1 shown · all via `kenn contracts`"),
            "the contracts heading must name the cap and the query: {idx}"
        );
    }

    /// A component is a package's source sub-area, not a document. It carries
    /// `role: None`, so a role-first match files every one under `## Documents`
    /// — which is what shipped: a single-dominant Swift repo listed all seven
    /// `ArgumentParser / <area>` sub-areas as documents.
    ///
    /// Mutation-checked: restoring the role-first match puts the component back
    /// under Documents and fails the second assertion.
    #[test]
    fn components_are_not_documents() {
        let shape = AtlasShape {
            name: "w".into(),
            languages: vec!["rust".into()],
            packages: 1,
            symbols: 10,
            test_ratio_pct: 0,
            domains_total: 0,
            contracts_total: 0,
            tables_total: 0,
            freshness: "HEAD abc".into(),
            timestamp: "2026-07-13T00:00:00Z".into(),
        };
        let pkg = concept();
        let mut comp = concept();
        comp.concept_type = "component".into();
        comp.id = "packages/rust_pkg_parsing".into();
        comp.title = "pkg / parsing".into();
        comp.role = None;
        let mut doc = concept();
        doc.concept_type = "document".into();
        doc.id = "documents/docs".into();
        doc.title = "docs".into();
        doc.role = None;

        let md = render_index(&shape, &[pkg, comp, doc], &[], &[], &[]);
        let comp_head = md.find("## Components").expect("a Components heading");
        let doc_head = md.find("## Documents").expect("a Documents heading");
        let comp_at = md.find("pkg / parsing").expect("the component is listed");
        assert!(
            comp_head < comp_at && comp_at < doc_head,
            "the component belongs under Components, above Documents:\n{md}"
        );
        let after_docs = md.get(doc_head..).unwrap_or_default();
        assert!(
            !after_docs.contains("pkg / parsing"),
            "Documents must not hold a component:\n{md}"
        );
    }
}
