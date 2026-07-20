//! Size-bounded recursive text splitter (design D1).
//!
//! No tree-sitter, no AST: the "dumb but sufficient" tool for config/prose
//! files. It cuts at the strongest boundary that keeps a chunk under
//! `target` bytes — a blank-line run, then a single newline, then a hard
//! character cut — with a best-effort byte `overlap` re-included at the start
//! of the next chunk for context continuity. A file at or below `target` is a
//! single chunk; whitespace-only fragments are dropped.

/// One emitted chunk: its text plus the 1-based inclusive line range it spans
/// in the source file (used for the chunk node's def).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub text: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// Split `content` into size-bounded chunks. `target` is the soft byte ceiling
/// per chunk; `overlap` is best-effort trailing context. Returns an empty vec
/// for empty / whitespace-only input.
#[must_use]
#[expect(
    clippy::string_slice,
    reason = "b0/b1 are line_starts offsets and split_ranges bounds — all char boundaries by construction"
)]
pub fn split(content: &str, target: usize, overlap: usize) -> Vec<Chunk> {
    if content.trim().is_empty() {
        return Vec::new();
    }
    let target = target.max(1);
    let starts = line_starts(content);
    split_ranges(content, target, overlap)
        .into_iter()
        .filter(|&(b0, b1)| !content[b0..b1].trim().is_empty())
        .map(|(b0, b1)| Chunk {
            text: content[b0..b1].to_string(),
            start_line: line_of(&starts, b0),
            end_line: line_of(&starts, b1 - 1),
        })
        .collect()
}

/// Byte ranges `[start, end)` for each chunk. Every range advances `start`
/// strictly, so the loop always terminates.
#[expect(
    clippy::string_slice,
    reason = "start is always a char boundary and hard_boundary returns one, so the window slice is boundary-safe"
)]
fn split_ranges(s: &str, target: usize, overlap: usize) -> Vec<(usize, usize)> {
    let n = s.len();
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < n {
        if n - start <= target {
            ranges.push((start, n));
            break;
        }
        let hard = hard_boundary(s, start, target);
        let window = &s[start..hard];
        // Prefer the rightmost blank-line boundary, then the rightmost single
        // newline, then a hard cut at the window's char-boundary end.
        let split_rel = window
            .rfind("\n\n")
            .map(|i| i + 2)
            .or_else(|| window.rfind('\n').map(|i| i + 1))
            .unwrap_or(window.len());
        let end = start + split_rel;
        ranges.push((start, end));
        // Re-include `overlap` bytes of context only if it still advances past
        // the chunk we just emitted (else skip it — progress must be strict).
        // `overlap` is a raw byte count, so snap the backtrack to a char
        // boundary — otherwise `end - overlap` can land mid-UTF8-char and the
        // next `&s[start..]` panics on multibyte input.
        let next = floor_char_boundary(s, end.saturating_sub(overlap));
        start = if next > start { next } else { end };
    }
    ranges
}

/// The char-boundary end of the `target`-sized window at `start`, guaranteed
/// strictly greater than `start` (advances to the next boundary if flooring
/// `start + target` lands back on `start`).
fn hard_boundary(s: &str, start: usize, target: usize) -> usize {
    let mut hard = floor_char_boundary(s, start + target);
    if hard <= start {
        hard = start + 1;
        while hard < s.len() && !s.is_char_boundary(hard) {
            hard += 1;
        }
    }
    hard
}

/// Largest char boundary `<= idx` (clamped to the string length).
fn floor_char_boundary(s: &str, idx: usize) -> usize {
    let mut idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Byte offset of the start of each line (0, and each index just past a `\n`).
fn line_starts(s: &str) -> Vec<usize> {
    let mut v = vec![0];
    v.extend(
        s.bytes()
            .enumerate()
            .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
    );
    v
}

/// 1-based line number containing byte offset `byte`.
fn line_of(starts: &[usize], byte: usize) -> u32 {
    let n = starts.partition_point(|&o| o <= byte);
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_file_is_a_single_chunk() {
        let out = split("a: 1\nb: 2\n", 1000, 150);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "a: 1\nb: 2\n");
        assert_eq!((out[0].start_line, out[0].end_line), (1, 2));
    }

    #[test]
    fn empty_or_whitespace_is_no_chunks() {
        assert!(split("", 1000, 150).is_empty());
        assert!(split("   \n\n\t\n", 1000, 150).is_empty());
    }

    #[test]
    fn splits_on_blank_line_boundary_under_target() {
        // Two paragraphs; target forces a split, and the blank line is the
        // strongest boundary within the first window.
        let content = "para one line\npara one more\n\npara two line\npara two more\n";
        let out = split(content, 30, 0);
        assert!(
            out.len() >= 2,
            "expected multiple chunks, got {}",
            out.len()
        );
        // First chunk ends at the blank line (its text carries no "para two").
        assert!(out[0].text.contains("para one"));
        assert!(!out[0].text.contains("para two"));
    }

    #[test]
    fn hard_cut_when_no_newline_boundary() {
        // One long line, no newline: the splitter must still make progress via
        // a hard cut and cover the whole input across chunks.
        let content = "x".repeat(2500);
        let out = split(&content, 1000, 0);
        assert!(out.len() >= 3);
        let joined: String = out.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(joined, content);
    }

    #[test]
    fn overlap_re_includes_trailing_context() {
        let content = "line1\nline2\nline3\nline4\nline5\nline6\n";
        let no_overlap = split(content, 12, 0);
        let with_overlap = split(content, 12, 6);
        // Overlap yields at least as many chunks and duplicates some content.
        assert!(with_overlap.len() >= no_overlap.len());
        let total_overlap: usize = with_overlap.iter().map(|c| c.text.len()).sum();
        let total_plain: usize = no_overlap.iter().map(|c| c.text.len()).sum();
        assert!(total_overlap >= total_plain);
    }

    #[test]
    fn multibyte_content_never_panics_on_hard_cut() {
        // Emoji are 4 bytes each; a hard cut must land on a char boundary.
        let content = "😀".repeat(1000);
        let out = split(&content, 1000, 100);
        assert!(!out.is_empty());
        let joined: String = out.iter().map(|c| c.text.as_str()).collect();
        // With overlap the joined form may repeat, but every chunk is valid
        // UTF-8 (constructed by slicing on boundaries) — reaching here is proof.
        assert!(joined.starts_with('😀'));
    }

    #[test]
    fn misaligned_overlap_on_multibyte_never_panics() {
        // Regression: the default overlap (150) is not a multiple of the 4-byte
        // emoji width, so `end - overlap` lands mid-char. Before snapping the
        // backtrack to a char boundary this panicked in the next slice.
        let content = "😀".repeat(300); // 1200 bytes
        let out = split(&content, 1000, 150);
        assert!(out.len() >= 2);
        // Every chunk boundary is a valid char boundary (no panic, valid UTF-8).
        assert!(out.iter().all(|c| c.text.starts_with('😀')));
    }
}
