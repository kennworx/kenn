//! Link extraction from a markdown body.
//!
//! Hybrid (design D1, Group 4): `CommonMark` inline/reference links come from
//! `pulldown-cmark` (which correctly excludes code spans, code blocks, and
//! escapes and parses the destination grammar); wikilinks (`[[…]]`) and
//! transclusions (`![[…]]`) — which are *not* `CommonMark` — are scanned on top,
//! skipping any match that falls inside a code span/block the parser marked.
//!
//! Extraction yields raw, unresolved [`RawLink`]s carrying the source line (to
//! map a link to its enclosing section) and the target split into
//! path/name + `#anchor`. Resolution against the global index is a later step.

use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

/// Whether a link references (`[[x]]`, `[x](y)`) or transcludes (`![[x]]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Link,
    Embed,
}

/// A raw, unresolved link found in a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLink {
    pub kind: LinkKind,
    /// True for `[[…]]`/`![[…]]` (resolve by stem/alias/title); false for
    /// `CommonMark` `[t](dest)` (resolve by path).
    pub wikilink: bool,
    /// Target without the `#anchor`. Empty for a same-file `[[#sec]]`/`(#sec)`.
    pub target: String,
    /// The `#anchor` (section slug), if any.
    pub anchor: Option<String>,
    /// 1-based source line of the link (maps it to its enclosing section).
    pub line: u32,
    /// True for external URLs (`http(s)://`, `mailto:`) — not graphed.
    pub external: bool,
}

/// Extract every link from `content` (file-coordinate line numbers).
#[must_use]
pub fn extract_links(content: &str) -> Vec<RawLink> {
    let newlines = newline_offsets(content);
    let mut out = Vec::new();
    let mut code: Vec<Range<usize>> = Vec::new();
    let mut code_block_depth = 0u32;

    for (event, range) in Parser::new(content).into_offset_iter() {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                let (target, anchor) = split_anchor(&dest_url);
                out.push(RawLink {
                    kind: LinkKind::Link,
                    wikilink: false,
                    target,
                    anchor,
                    line: line_at(&newlines, range.start),
                    external: is_external(&dest_url),
                });
            }
            Event::Code(_) => code.push(range),
            Event::Start(Tag::CodeBlock(_)) => code_block_depth += 1,
            Event::End(TagEnd::CodeBlock) => code_block_depth = code_block_depth.saturating_sub(1),
            Event::Text(_) if code_block_depth > 0 => code.push(range),
            _ => {}
        }
    }

    scan_wikilinks(content, &code, &newlines, &mut out);
    out
}

/// Scan `[[target#anchor|alias]]` and `![[…]]`, skipping code regions.
fn scan_wikilinks(
    content: &str,
    code: &[Range<usize>],
    newlines: &[usize],
    out: &mut Vec<RawLink>,
) {
    for (start, _) in content.match_indices("[[") {
        if in_code(code, start) {
            continue;
        }
        let Some(rest) = content.get(start + 2..) else {
            continue;
        };
        let Some(close) = rest.find("]]") else {
            continue;
        };
        let Some(inner) = rest.get(..close) else {
            continue;
        };
        // A real wikilink target is a single line and never contains an
        // inline-link `](`. Bail on a malformed `[[` (usually a typo for a
        // `[text](url)` link, e.g. `[[BUG]: title](url)`) so its `]]` search
        // doesn't run across the line/file and swallow a garbage target.
        if inner.contains('\n') || inner.contains("](") {
            continue;
        }
        let embed = start > 0 && content.get(start - 1..start) == Some("!");
        let (target, anchor) = parse_wikitarget(inner);
        out.push(RawLink {
            kind: if embed {
                LinkKind::Embed
            } else {
                LinkKind::Link
            },
            wikilink: true,
            target,
            anchor,
            line: line_at(newlines, start),
            external: false,
        });
    }
}

/// `[[name#anchor|alias]]` → (`name`, `Some(anchor)`); alias is display-only
/// and dropped. A leading `#` means a same-file section (empty target).
fn parse_wikitarget(inner: &str) -> (String, Option<String>) {
    let without_alias = inner.split('|').next().unwrap_or(inner).trim();
    split_anchor(without_alias)
}

/// Split `path#anchor` into (`path`, `Some(anchor)`). A leading `#` yields an
/// empty target (same-file reference).
fn split_anchor(target: &str) -> (String, Option<String>) {
    match target.split_once('#') {
        Some((path, anchor)) => (path.to_string(), Some(anchor.to_string())),
        None => (target.to_string(), None),
    }
}

/// URL schemes that mark a link as external (not a note/attachment). Matched
/// case-insensitively against the destination's prefix.
const EXTERNAL_SCHEMES: &[&str] = &[
    "http:", "https:", "ftp:", "ftps:", "mailto:", "tel:", "sms:", "file:", "data:",
];

/// True for a link destination that is not an in-vault note: a URL (any known
/// scheme, including the `https:/typo` single-slash case, or a generic
/// `scheme://`) or a bare email address. pulldown-cmark surfaces email
/// autolinks without the `mailto:` prefix (that's added only by the HTML
/// renderer, which we don't run), so emails must be detected by shape.
fn is_external(dest: &str) -> bool {
    let lower = dest.trim_start().to_ascii_lowercase();
    EXTERNAL_SCHEMES.iter().any(|s| lower.starts_with(s))
        || dest.contains("://")
        || looks_like_email(dest)
}

/// `local@domain.tld` shape, with no path/space chars — distinguishes an email
/// (`mail@lila.rest`) from a social-handle wikilink (`@kepano`, no domain dot).
fn looks_like_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !local.contains(['/', ' '])
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains(['/', ' '])
}

fn newline_offsets(content: &str) -> Vec<usize> {
    content.match_indices('\n').map(|(i, _)| i).collect()
}

/// 1-based line number containing byte `offset`.
fn line_at(newlines: &[usize], offset: usize) -> u32 {
    let before = newlines.partition_point(|&p| p < offset);
    u32::try_from(before + 1).unwrap_or(u32::MAX)
}

fn in_code(code: &[Range<usize>], pos: usize) -> bool {
    code.iter().any(|r| r.contains(&pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(content: &str) -> Vec<RawLink> {
        extract_links(content)
    }

    #[test]
    fn inline_link_with_anchor_and_line() {
        let r = links("intro\nsee [docs](./docs/auth.md#flow) here\n");
        assert_eq!(r.len(), 1);
        assert!(!r[0].wikilink);
        assert_eq!(r[0].kind, LinkKind::Link);
        assert_eq!(r[0].target, "./docs/auth.md");
        assert_eq!(r[0].anchor.as_deref(), Some("flow"));
        assert_eq!(r[0].line, 2);
        assert!(!r[0].external);
    }

    #[test]
    fn external_urls_flagged() {
        let r = links("[site](https://example.com) and [m](mailto:a@b.c)");
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|l| l.external));
    }

    #[test]
    fn bare_emails_and_malformed_schemes_flagged_external() {
        // pulldown surfaces email autolinks without `mailto:`; a single-slash
        // `https:/…` typo still must read as external, not a note link.
        assert!(is_external("mail@lila.rest"));
        assert!(is_external("ivaylo.dabravin@gmail.com"));
        assert!(is_external("https:/warpcast.com/mani"));
        assert!(is_external("tel:+1-555-0100"));
        // …but a relative note path and a social-handle wikilink are NOT.
        assert!(!is_external("./docs/auth.md"));
        assert!(!is_external("People/LilaRest"));
        assert!(!is_external("@kepano"));
        assert!(!is_external("note.md"));
    }

    #[test]
    fn wikilink_and_transclusion() {
        let r = links("ref [[auth#flow|the flow]] and embed ![[daily/today]]");
        assert_eq!(r.len(), 2);
        let wl = &r[0];
        assert!(wl.wikilink);
        assert_eq!(wl.kind, LinkKind::Link);
        assert_eq!(wl.target, "auth");
        assert_eq!(wl.anchor.as_deref(), Some("flow"));
        let em = &r[1];
        assert_eq!(em.kind, LinkKind::Embed);
        assert_eq!(em.target, "daily/today");
        assert!(em.anchor.is_none());
    }

    #[test]
    fn malformed_double_bracket_link_is_not_a_wikilink() {
        // `[[BUG]: title](url)` is a typo for a `[text](url)` link; the naive
        // `[[…]]` scan would otherwise swallow a garbage cross-line target.
        // The valid wikilink on the same line still resolves.
        let r = links("- [[plugin|Name]] - [[BUG]: title](https://x.com/i/1)\n- [[other|O]]\n");
        let targets: Vec<&str> = r.iter().map(|l| l.target.as_str()).collect();
        assert!(
            targets.contains(&"plugin"),
            "valid wikilink kept: {targets:?}"
        );
        assert!(
            targets.contains(&"other"),
            "next-line wikilink kept: {targets:?}"
        );
        assert!(
            !targets
                .iter()
                .any(|t| t.contains("](") || t.contains("BUG")),
            "malformed target leaked: {targets:?}"
        );
    }

    #[test]
    fn same_file_anchor_wikilink() {
        let r = links("see [[#overview]]");
        assert_eq!(r[0].target, "");
        assert_eq!(r[0].anchor.as_deref(), Some("overview"));
    }

    #[test]
    fn excludes_links_in_code_spans_and_blocks() {
        // inline `[x](y)` and `[[w]]` inside code must be ignored.
        assert!(links("inline `[x](y)` and `[[w]]` code").is_empty());
        assert!(links("```\n[x](y)\n[[w]]\n```\n").is_empty());
    }

    #[test]
    fn respects_escapes_and_balanced_parens() {
        assert!(links(r"\[no\](x)").is_empty());
        assert_eq!(
            links("[v](https://e.com/p_(v2))")[0].target,
            "https://e.com/p_(v2)"
        );
    }

    #[test]
    fn reference_links_resolved() {
        let r = links("see [spec][s]\n\n[s]: ./spec.md\n");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].target, "./spec.md");
    }
}
