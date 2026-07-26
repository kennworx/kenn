//! TOON encoder tests: upstream byte-compatibility for the items table, the
//! field-order guarantee, the nested-shape rejection, and the quoting grammar.

use super::grammar::{is_numeric_like, Error};
use super::write_table;
use serde::Serialize;

/// Test helper: render to a `String` (trailing newline trimmed) so
/// assertions read naturally.
fn try_table<T: Serialize + ?Sized>(value: &T) -> Result<String, Error> {
    let mut out = Vec::new();
    write_table(&mut out, value)?;
    let s = String::from_utf8(out).map_err(|e| Error::msg(&e.to_string()))?;
    Ok(match s.strip_suffix('\n') {
        Some(trimmed) => trimmed.to_owned(),
        None => s,
    })
}

/// The items table (rows + column formatting) must match the upstream
/// `toon` crate byte-for-byte, for a row struct whose fields are already
/// alphabetical (upstream sorts; we preserve declaration order).
#[test]
fn matches_upstream_toon() {
    #[derive(Serialize)]
    struct AlphaRow {
        a: u32,
        b: &'static str,
        c: bool,
    }
    #[derive(Serialize)]
    struct Wrap {
        items: Vec<AlphaRow>,
    }
    let w = Wrap {
        items: vec![
            AlphaRow {
                a: 1,
                b: "x",
                c: true,
            },
            AlphaRow {
                a: 2,
                b: "y:z",
                c: false,
            },
        ],
    };
    let upstream = toon::encode(&serde_json::to_value(&w).unwrap(), None);
    assert_eq!(try_table(&w).unwrap(), upstream);
}

/// The mechanism: serde visits fields in declaration order, so the id column
/// leads — the bug this module fixes.
#[test]
fn struct_field_order_drives_columns() {
    #[derive(Serialize)]
    struct Row {
        symbol: &'static str,
        name: &'static str,
        language: &'static str,
        role: &'static str,
        symbols: u64,
        used_by_count: u64,
        deps_count: u64,
    }
    #[derive(Serialize)]
    struct Wrap {
        items: Vec<Row>,
    }
    let w = Wrap {
        items: vec![Row {
            symbol: "rs:kenn-config::crate",
            name: "kenn-config",
            language: "rust",
            role: "provider",
            symbols: 115,
            used_by_count: 7,
            deps_count: 0,
        }],
    };
    assert_eq!(
        try_table(&w).unwrap(),
        "items[1]{symbol,name,language,role,symbols,used_by_count,deps_count}:\n  \"rs:kenn-config::crate\",kenn-config,rust,provider,115,7,0"
    );
}

/// A top-level scalar field is the wrapper's meta (render prints it), NOT a
/// table column — `write_table` drops it and emits only the array.
#[test]
fn top_level_scalars_are_dropped() {
    #[derive(Serialize)]
    struct S {
        name: &'static str,
        items: Vec<u8>,
    }
    assert_eq!(
        try_table(&S {
            name: "x",
            items: vec![]
        })
        .unwrap(),
        "items[0]:"
    );
}

#[test]
fn primitive_array_is_inline() {
    #[derive(Serialize)]
    struct S {
        tags: Vec<&'static str>,
    }
    assert_eq!(
        try_table(&S {
            tags: vec!["reading", "gaming"]
        })
        .unwrap(),
        "tags[2]: reading,gaming"
    );
}

/// A nested object (a field that is a struct, or a row with a nested field)
/// is NOT a flat table — `try_table` errors so the caller renders JSON.
#[test]
fn nested_is_rejected() {
    #[derive(Serialize)]
    struct Inner {
        deep: u32,
    }
    #[derive(Serialize)]
    struct NestedField {
        outer: Inner,
    }
    #[derive(Serialize)]
    struct RowWithNested {
        id: u32,
        sub: Inner,
    }
    #[derive(Serialize)]
    struct Wrap {
        items: Vec<RowWithNested>,
    }

    // A field that is itself a struct → not a flat table.
    try_table(&NestedField {
        outer: Inner { deep: 1 },
    })
    .unwrap_err();
    // A row (list element) with a nested field → not a flat table.
    try_table(&Wrap {
        items: vec![RowWithNested {
            id: 1,
            sub: Inner { deep: 2 },
        }],
    })
    .unwrap_err();
}

/// Numeric-looking strings must be quoted so they don't read back as
/// numbers — this pins the ASCII grammar the upstream regex encodes.
#[test]
fn numeric_like_matches_the_grammar() {
    for s in [
        "42", "-3", "0", "3.14", "-3.14", "1e6", "1e-6", "1e+6", "1.5e10", "05", "007",
    ] {
        assert!(is_numeric_like(s), "{s} should be numeric-like");
    }
    for s in [
        "", "-", "abc", "3.", ".5", "1e", "1e+", "1.2.3", "1E6", "0x1f", "12a",
    ] {
        assert!(!is_numeric_like(s), "{s} should NOT be numeric-like");
    }
}

/// A bare top-level scalar is written directly (integer, and the string
/// quoting/escaping path): a plain word is bare, a numeric-looking or
/// structural string is quoted, and specials are escaped inside the quotes.
#[test]
fn bare_scalar_encoding() {
    assert_eq!(try_table(&42u32).unwrap(), "42");
    assert_eq!(try_table(&"hello").unwrap(), "hello");
    assert_eq!(
        try_table(&"rs:widget::crate").unwrap(),
        "\"rs:widget::crate\""
    );
    assert_eq!(try_table(&"42").unwrap(), "\"42\"");
    assert_eq!(
        try_table(&"a\"b\\c\td\ne").unwrap(),
        "\"a\\\"b\\\\c\\td\\ne\""
    );
}

/// A later row whose field set differs from the FIRST row's — an omitted
/// `skip_serializing_if` field, say — must NOT be comma-joined under the
/// header: the values would shift a column and read as plausible. The shape is
/// rejected so `render::emit` falls back to JSON.
///
/// Regression: this printed `b,embeds,dangling` under `{src,location,kind,grade}`,
/// putting `embeds` in the `location` column. `kenn check links` and `kenn check
/// css` both have an `Option` field with `skip_serializing_if`, so both were
/// exposed.
#[test]
fn ragged_rows_are_rejected_not_shifted() {
    #[derive(Serialize)]
    struct Row {
        src: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<&'static str>,
        kind: &'static str,
    }
    #[derive(Serialize)]
    struct Wrap {
        items: Vec<Row>,
    }
    // Uniform rows still render as a table.
    let uniform = Wrap {
        items: vec![
            Row {
                src: "a",
                location: Some("f.md#L1"),
                kind: "links_to",
            },
            Row {
                src: "b",
                location: Some("g.md#L2"),
                kind: "embeds",
            },
        ],
    };
    assert_eq!(
        try_table(&uniform).unwrap(),
        "items[2]{src,location,kind}:\n  a,f.md#L1,links_to\n  b,g.md#L2,embeds"
    );
    // A ragged second row is not a table at all.
    let ragged = Wrap {
        items: vec![
            Row {
                src: "a",
                location: Some("f.md#L1"),
                kind: "links_to",
            },
            Row {
                src: "b",
                location: None,
                kind: "embeds",
            },
        ],
    };
    try_table(&ragged).unwrap_err();
}

/// A heterogeneous array (a scalar where rows are expected, or the reverse)
/// must error rather than silently drop the element — the header announces
/// `[N]`, so a dropped element makes a short listing read as complete.
#[test]
fn shape_changes_mid_array_are_rejected() {
    use serde_json::json;
    // serde_json::Value serializes untagged, so this is a scalar then an object.
    try_table(&json!({ "items": ["scalar", {"a": 1}] })).unwrap_err();
    try_table(&json!({ "items": [{"a": 1}, "scalar"] })).unwrap_err();
}
