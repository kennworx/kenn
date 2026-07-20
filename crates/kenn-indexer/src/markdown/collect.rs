//! Phase-1 markdown collect: a cheap scan that extracts YAML frontmatter and
//! the heading slugs from a file *without* a full body parse.
//!
//! The frontmatter (`title`, `aliases`) and the per-file heading slugs feed
//! the global resolution index, which must be complete before any link
//! resolves (design D4). The heavy body parse (sections, prose, links) is a
//! later phase.

use kenn_model::id::md::{slugify, SlugDeduper};
use serde::Deserialize;

/// A typed-frontmatter `related:` entry (`{ slug, type }`). Captured here;
/// its consumption as a typed edge is deferred (design Open Question).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedLink {
    pub slug: String,
    pub relation: String,
}

/// Parsed frontmatter fields relevant to indexing and resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    pub title: Option<String>,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub related: Vec<RelatedLink>,
}

/// One heading found in the body, with its GitHub slug (deduped within file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingSlug {
    /// ATX level (1–6).
    pub level: u8,
    /// Heading text (closing `#`s stripped).
    pub text: String,
    /// Slug, unique within the file.
    pub slug: String,
    /// 1-based line number of the heading in the file (phase-2 uses it for
    /// section spans).
    pub line: u32,
}

/// Result of the phase-1 scan of one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectedFile {
    pub frontmatter: Frontmatter,
    pub headings: Vec<HeadingSlug>,
}

#[derive(Debug, Default, Deserialize)]
struct RawFrontmatter {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    related: Vec<RawRelated>,
}

#[derive(Debug, Deserialize)]
struct RawRelated {
    slug: String,
    #[serde(default, rename = "type")]
    relation: String,
}

/// Run the phase-1 collect over a file's full text.
#[must_use]
pub fn collect(content: &str) -> CollectedFile {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let (yaml, body_line, body) = split_frontmatter(content);
    let frontmatter = yaml.map(parse_frontmatter).unwrap_or_default();
    let headings = scan_headings(body, body_line);
    CollectedFile {
        frontmatter,
        headings,
    }
}

/// Split a leading `---` … `---`/`...` YAML frontmatter block off the front.
/// Returns `(Some(yaml), body_start_line, body)` (1-based line where `body`
/// begins in the file) or `(None, 1, whole)` when there is no well-formed
/// frontmatter.
#[expect(
    clippy::string_slice,
    reason = "offsets accumulate from split_inclusive line lengths — always UTF-8 char boundaries"
)]
fn split_frontmatter(content: &str) -> (Option<&str>, u32, &str) {
    let Some(after_open) = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
    else {
        return (None, 1, content);
    };
    let mut offset = 0;
    for (idx, raw) in after_open.split_inclusive('\n').enumerate() {
        let trimmed = raw.trim_end_matches(['\n', '\r']);
        if trimmed == "---" || trimmed == "..." {
            let yaml = &after_open[..offset];
            let body = &after_open[offset + raw.len()..];
            // idx 0 is file line 2 (line 1 was the opening `---`); body starts
            // on the line after this close marker → 3 + idx.
            return (Some(yaml), 3 + u32::try_from(idx).unwrap_or(0), body);
        }
        offset += raw.len();
    }
    (None, 1, content)
}

fn parse_frontmatter(yaml: &str) -> Frontmatter {
    // A malformed frontmatter block degrades to "no metadata" rather than
    // failing the file — phase-1 collect must be resilient.
    let raw: RawFrontmatter = serde_yaml_ng::from_str(yaml).unwrap_or_default();
    Frontmatter {
        title: raw.title,
        aliases: raw.aliases,
        tags: raw.tags,
        related: raw
            .related
            .into_iter()
            .map(|r| RelatedLink {
                slug: r.slug,
                relation: r.relation,
            })
            .collect(),
    }
}

fn scan_headings(body: &str, base_line: u32) -> Vec<HeadingSlug> {
    let mut dedup = SlugDeduper::new();
    let mut out = Vec::new();
    let mut fence: Option<char> = None;
    for (idx, line) in body.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(marker) = fence_marker(trimmed) {
            // Toggle on matching marker; ignore headings while inside a fence.
            match fence {
                Some(open) if open == marker => fence = None,
                Some(_) => {}
                None => fence = Some(marker),
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        if let Some((level, text)) = parse_atx_heading(trimmed) {
            let slug = dedup.dedup(&slugify(&text));
            let line_no = base_line + u32::try_from(idx).unwrap_or(0);
            out.push(HeadingSlug {
                level,
                text,
                slug,
                line: line_no,
            });
        }
    }
    out
}

/// The fence character (` ``` ` → `` ` ``, `~~~` → `~`) if `line` opens/closes a
/// fenced code block, else `None`.
fn fence_marker(line: &str) -> Option<char> {
    if line.starts_with("```") {
        Some('`')
    } else if line.starts_with("~~~") {
        Some('~')
    } else {
        None
    }
}

/// Parse an ATX heading line into `(level, text)`. Requires 1–6 leading `#`
/// followed by whitespace (or end of line); strips closing `#`s.
#[expect(
    clippy::string_slice,
    reason = "`hashes` counts leading ASCII '#' — the byte index is a char boundary"
)]
fn parse_atx_heading(line: &str) -> Option<(u8, String)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let after = &line[hashes..]; // `#` is ASCII → byte index is a char boundary
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    let text = after.trim().trim_end_matches('#').trim_end().to_string();
    Some((u8::try_from(hashes).unwrap_or(6), text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_fields() {
        let md = "---\ntitle: Auth Flow\naliases: [auth, login-flow]\ntags: [security]\n\
                  related:\n  - slug: order-handler\n    type: extends\n---\n# Body\n";
        let c = collect(md);
        assert_eq!(c.frontmatter.title.as_deref(), Some("Auth Flow"));
        assert_eq!(c.frontmatter.aliases, ["auth", "login-flow"]);
        assert_eq!(c.frontmatter.tags, ["security"]);
        assert_eq!(c.frontmatter.related.len(), 1);
        assert_eq!(c.frontmatter.related[0].slug, "order-handler");
        assert_eq!(c.frontmatter.related[0].relation, "extends");
        assert_eq!(c.headings.len(), 1);
        assert_eq!(c.headings[0].slug, "body");
    }

    #[test]
    fn scans_headings_with_dedup_and_levels() {
        let md = "# Title\n## Notes\ntext\n### Sub\n## Notes\n";
        let c = collect(md);
        let got: Vec<(u8, &str)> = c
            .headings
            .iter()
            .map(|h| (h.level, h.slug.as_str()))
            .collect();
        assert_eq!(
            got,
            [(1, "title"), (2, "notes"), (3, "sub"), (2, "notes-1")]
        );
    }

    #[test]
    fn ignores_headings_inside_code_fences() {
        let md = "# Real\n```\n# not a heading\n```\n## Also Real\n";
        let c = collect(md);
        let slugs: Vec<&str> = c.headings.iter().map(|h| h.slug.as_str()).collect();
        assert_eq!(slugs, ["real", "also-real"]);
    }

    #[test]
    fn no_frontmatter_still_scans_headings() {
        let c = collect("# Just A Heading\n");
        assert!(c.frontmatter.title.is_none());
        assert_eq!(c.headings.len(), 1);
        assert_eq!(c.headings[0].slug, "just-a-heading");
    }

    #[test]
    fn malformed_frontmatter_degrades_gracefully() {
        let md = "---\n: : not valid yaml : :\n---\n# H\n";
        let c = collect(md);
        assert_eq!(c.frontmatter, Frontmatter::default());
        assert_eq!(c.headings.len(), 1);
    }
}
