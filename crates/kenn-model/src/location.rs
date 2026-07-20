//! Wire location format: `./<workspace_relative_path>#<start_line>` or
//! `./<workspace_relative_path>#<start_line>-<end_line>`. Single-line ranges
//! suppress the trailing `-end`. Synthetic / external symbols use the literal
//! string `"null"` (the API surface materializes JSON `null` from this).

use thiserror::Error;

/// Inclusive line range `[start_line, end_line]` (0-based).
pub type Range = (u32, u32);

/// SCIP-shaped 4-tuple `[start_line, start_col, end_line, end_col]`.
pub type DefRange = [u32; 4];

#[derive(Debug, Error)]
pub enum LocationError {
    #[error("location is missing the `./` prefix: {0}")]
    NoDotSlashPrefix(String),
    #[error("location is missing the `#line` suffix: {0}")]
    NoLineFragment(String),
    #[error("line range is malformed: {0}")]
    BadRange(String),
}

pub const NULL_LOCATION: &str = "null";

#[must_use]
pub fn format_location(file_path: &str, def_range: DefRange) -> String {
    let start = def_range[0];
    let end = def_range[2];
    if start == end {
        format!("./{file_path}#{start}")
    } else {
        format!("./{file_path}#{start}-{end}")
    }
}

/// Format a location with sentinel-aware nullability — `file_path` empty or
/// `def_range` all-zero produces the literal `"null"` string. Used by API
/// emitters that need to render JSON `null` for absent locations.
#[must_use]
pub fn format_location_or_null(file_path: &str, def_range: DefRange) -> String {
    if file_path.is_empty() || def_range == [0, 0, 0, 0] {
        NULL_LOCATION.to_string()
    } else {
        format_location(file_path, def_range)
    }
}

pub fn parse_location(s: &str) -> Result<(String, Range), LocationError> {
    let body = s
        .strip_prefix("./")
        .ok_or_else(|| LocationError::NoDotSlashPrefix(s.into()))?;
    let (path, frag) = body
        .rsplit_once('#')
        .ok_or_else(|| LocationError::NoLineFragment(s.into()))?;
    if path.is_empty() {
        return Err(LocationError::NoDotSlashPrefix(s.into()));
    }
    // The wildcards intentionally drop the inner `ParseIntError`; the
    // user-facing `BadRange` already carries the full input string for
    // diagnosis and the parser detail adds nothing.
    #[expect(
        clippy::map_err_ignore,
        reason = "BadRange wraps the full input; the inner ParseIntError adds no actionable detail"
    )]
    let range = if let Some((a, b)) = frag.split_once('-') {
        let start: u32 = a.parse().map_err(|_| LocationError::BadRange(s.into()))?;
        let end: u32 = b.parse().map_err(|_| LocationError::BadRange(s.into()))?;
        (start, end)
    } else {
        let line: u32 = frag
            .parse()
            .map_err(|_| LocationError::BadRange(s.into()))?;
        (line, line)
    };
    Ok((path.to_string(), range))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_round_trip() {
        let s = format_location("src/api.ts", [42, 8, 42, 32]);
        assert_eq!(s, "./src/api.ts#42");
        let (path, range) = parse_location(&s).unwrap();
        assert_eq!(path, "src/api.ts");
        assert_eq!(range, (42, 42));
    }

    #[test]
    fn multi_line_round_trip() {
        let s = format_location("src/api.ts", [3, 0, 14, 1]);
        assert_eq!(s, "./src/api.ts#3-14");
        let (path, range) = parse_location(&s).unwrap();
        assert_eq!(path, "src/api.ts");
        assert_eq!(range, (3, 14));
    }

    #[test]
    fn columns_are_dropped_in_format() {
        // Round-trip preserves only line range, not columns.
        let s = format_location("a/b.rs", [1, 5, 9, 12]);
        let (_, range) = parse_location(&s).unwrap();
        assert_eq!(range, (1, 9));
    }

    #[test]
    fn null_for_external_or_synthetic() {
        assert_eq!(format_location_or_null("", [1, 0, 1, 0]), "null");
        assert_eq!(format_location_or_null("a.rs", [0, 0, 0, 0]), "null");
        assert_eq!(format_location_or_null("a.rs", [1, 0, 1, 0]), "./a.rs#1");
    }

    #[test]
    fn parse_rejects_bad_inputs() {
        assert!(matches!(
            parse_location("a.rs#1"),
            Err(LocationError::NoDotSlashPrefix(_))
        ));
        assert!(matches!(
            parse_location("./a.rs"),
            Err(LocationError::NoLineFragment(_))
        ));
        assert!(matches!(
            parse_location("./a.rs#x"),
            Err(LocationError::BadRange(_))
        ));
        assert!(matches!(
            parse_location("./a.rs#1-x"),
            Err(LocationError::BadRange(_))
        ));
    }
}
