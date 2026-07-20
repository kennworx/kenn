//! Pure record builders and decoders: docs-record assembly, the file-doc
//! license filter, and the JSONL `kind` / `edge_kind` string decoders.

use kenn_model::{
    EdgeProperties, FieldOp, FileDocsRecord, ImportKind, IsomorphismSource, Kind, ShortId,
    SymbolDocsRecord,
};

pub(crate) fn build_docs_record(
    short_id: ShortId,
    sig: Option<String>,
    doc: Option<String>,
) -> Option<SymbolDocsRecord> {
    match (sig, doc) {
        (None, None) => None,
        (sig, doc) => Some(SymbolDocsRecord {
            sym_id: short_id,
            sig: sig.unwrap_or_default(),
            doc: doc.unwrap_or_default(),
        }),
    }
}

/// Build a `file_docs` row from a file's raw comment blocks, dropping
/// license/copyright boilerplate per block. A dropped block that carries
/// an `SPDX-License-Identifier:` tag is classified (copyleft / OSI / …)
/// and logged, so we have a record of what was stripped. Survivors are
/// joined with a blank line. Returns `None` when nothing useful remains.
pub(crate) fn file_doc_record(
    file_id: ShortId,
    path: &str,
    raw: &[String],
) -> Option<FileDocsRecord> {
    let mut kept: Vec<String> = Vec::with_capacity(raw.len());
    for entry in raw {
        if is_license_boilerplate(entry) {
            if let Some(id) = classify_license_tag(entry) {
                tracing::debug!(
                    path,
                    license = id.name,
                    copyleft = id.is_copyleft(),
                    osi = id.is_osi_approved(),
                    "stripped a classified license header from file docs"
                );
            }
            continue;
        }
        // Store clean prose, not the comment syntax: the doc feeds the atlas
        // description, doc search (`doc_fts`), and embeddings, none of which want
        // `//!`/`///` on every line.
        let prose = strip_doc_markers(entry);
        if !prose.trim().is_empty() {
            kept.push(prose);
        }
    }
    if kept.is_empty() {
        return None;
    }
    Some(FileDocsRecord {
        file_id,
        doc: kept.join("\n\n"),
    })
}

/// Strip Rust comment markers from a doc block, leaving the prose: each line's
/// leading `//!`/`///`/`//` or `/**`/`/*`/`*`/`*/` marker (plus one following
/// space) is removed, and a trailing `*/` dropped — so `//! foo` → `foo`.
fn strip_doc_markers(block: &str) -> String {
    const MARKERS: [&str; 7] = ["//!", "///", "//", "/**", "/*", "*/", "*"];
    block
        .lines()
        .map(|line| {
            let t = line.trim_start();
            let body = MARKERS.iter().find_map(|m| t.strip_prefix(m)).unwrap_or(t);
            let body = body.strip_suffix("*/").unwrap_or(body);
            body.strip_prefix(' ').unwrap_or(body).trim_end()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Resolve the SPDX license carried by a dropped boilerplate block via its
/// `SPDX-License-Identifier:` tag, if present. Core-crate id lookup only
/// (no detection corpus). Splitting on whitespace/comment punctuation also
/// strips a trailing `*/`; for a compound expression (`MIT OR Apache-2.0`)
/// this resolves only the FIRST id — adequate for a diagnostic log.
fn classify_license_tag(block: &str) -> Option<spdx::LicenseId> {
    const TAG: &str = "spdx-license-identifier:";
    let lower = block.to_ascii_lowercase();
    // `to_ascii_lowercase` preserves byte length and the tag is ASCII, so
    // the offset is a valid char boundary in the original `block`.
    let after = block.get(lower.find(TAG)? + TAG.len()..)?;
    let value = after
        .split([' ', '\t', '\r', '\n', '*', '/'])
        .find(|s| !s.is_empty())?;
    spdx::license_id(value).or_else(|| spdx::imprecise_license_id(value).map(|(id, _)| id))
}

/// Conservative license/boilerplate matcher: drops a comment *block*
/// (one coalesced entry) that looks like a copyright/license notice,
/// without discarding adjacent prose blocks. Matched case-insensitively.
///
/// The phrase set covers each *paragraph* of a multi-paragraph license
/// body, not just its opening line: a blank-line-separated MIT/BSD body
/// arrives as several blocks (copyright / grant / warranty), and the
/// markerless-looking "AS IS" warranty clause must be recognised too or
/// it leaks into the search index. Every phrase is distinctive enough to
/// license text that it is very unlikely in an ordinary code comment.
fn is_license_boilerplate(entry: &str) -> bool {
    const MARKERS: [&str; 18] = [
        // Identification / attribution.
        "spdx-license-identifier",
        "copyright (c)",
        "copyright ©",
        "all rights reserved",
        "licensed under",
        // Named licenses.
        "apache license",
        "bsd license",
        "isc license",
        "mozilla public license",
        "eclipse public license",
        "gnu general public license",
        "gnu lesser general public license",
        // Grant / conditions clauses.
        "permission is hereby granted",        // MIT grant
        "subject to the following conditions", // MIT conditions
        "redistribution and use",              // BSD
        "redistributions of source code",      // BSD
        // Warranty-disclaimer clauses (the markerless-looking tail).
        "the software is provided", // MIT / BSD "AS IS"
        "without warranty",         // covers "WITHOUT WARRANTY" / "WITHOUT ANY WARRANTY"
    ];
    let lower = entry.to_ascii_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}

pub(crate) fn kind_from_str(s: &str) -> Option<Kind> {
    Some(match s {
        "namespace" => Kind::Namespace,
        "module" => Kind::Module,
        "class" => Kind::Class,
        "struct" => Kind::Struct,
        "interface" => Kind::Interface,
        "enum" => Kind::Enum,
        "enum_member" => Kind::EnumMember,
        "delegate" | "type" => Kind::TypeAlias,
        "constructor" => Kind::Constructor,
        "destructor" => Kind::Destructor,
        "method" | "accessor" => Kind::Method,
        "function" => Kind::Function,
        "property" | "event" => Kind::Property,
        "field" => Kind::Field,
        "const" => Kind::Constant,
        "symbol" => Kind::Variable,
        _ => return None,
    })
}

pub(crate) fn edge_properties(kind: &str, field_op: Option<&str>) -> Option<EdgeProperties> {
    Some(match kind {
        "defined_in" => EdgeProperties::DefinedIn,
        "contains" => EdgeProperties::Contains,
        "calls" => EdgeProperties::Calls,
        "type_use" => EdgeProperties::TypeUse,
        "field_access" => {
            let op = match field_op? {
                "read" => FieldOp::Read,
                "write" => FieldOp::Write,
                _ => return None,
            };
            EdgeProperties::FieldAccess { op }
        }
        "implements" => EdgeProperties::Implements,
        "overrides" => EdgeProperties::Overrides,
        "instantiates" => EdgeProperties::Instantiates,
        "generic_constraint" => EdgeProperties::GenericConstraint,
        "imports" => EdgeProperties::Imports {
            kind: ImportKind::Explicit,
        },
        "corresponds_to" => EdgeProperties::CorrespondsTo {
            source: IsomorphismSource::AutoInferred,
            generator: String::new(),
            canonical: 0,
        },
        "extends_type" => EdgeProperties::ExtendsType,
        _ => return None,
    })
}

#[cfg(test)]
mod kind_edge_tests {
    use super::{edge_properties, kind_from_str};
    use kenn_model::{EdgeProperties, FieldOp, ImportKind, IsomorphismSource, Kind};

    /// `kind_from_str` is the JSONL frame `Kind` field decoder. Cover
    /// every match arm including the multi-string aliases and the
    /// catch-all `None`.
    #[test]
    fn kind_from_str_decodes_every_string() {
        for (s, expected) in [
            ("namespace", Kind::Namespace),
            ("module", Kind::Module),
            ("class", Kind::Class),
            ("struct", Kind::Struct),
            ("interface", Kind::Interface),
            ("enum", Kind::Enum),
            ("delegate", Kind::TypeAlias),
            ("type", Kind::TypeAlias),
            ("constructor", Kind::Constructor),
            ("destructor", Kind::Destructor),
            ("method", Kind::Method),
            ("accessor", Kind::Method),
            ("property", Kind::Property),
            ("event", Kind::Property),
            ("field", Kind::Field),
            ("const", Kind::Constant),
            ("symbol", Kind::Variable),
        ] {
            assert_eq!(kind_from_str(s), Some(expected), "decode {s:?}");
        }
        for unknown in ["", "unknown", "Class", "namespace "] {
            assert!(
                kind_from_str(unknown).is_none(),
                "{unknown:?} must not decode"
            );
        }
    }

    /// `edge_properties` (the `transform_jsonl` variant — takes a
    /// string kind + optional field-op string) covers every edge kind
    /// and the field-op subdispatch.
    #[test]
    fn edge_properties_decodes_every_kind() {
        // Simple variants.
        assert!(matches!(
            edge_properties("defined_in", None),
            Some(EdgeProperties::DefinedIn)
        ));
        assert!(matches!(
            edge_properties("contains", None),
            Some(EdgeProperties::Contains)
        ));
        assert!(matches!(
            edge_properties("calls", None),
            Some(EdgeProperties::Calls)
        ));
        assert!(matches!(
            edge_properties("type_use", None),
            Some(EdgeProperties::TypeUse)
        ));
        assert!(matches!(
            edge_properties("implements", None),
            Some(EdgeProperties::Implements)
        ));
        assert!(matches!(
            edge_properties("overrides", None),
            Some(EdgeProperties::Overrides)
        ));
        assert!(matches!(
            edge_properties("instantiates", None),
            Some(EdgeProperties::Instantiates)
        ));
        assert!(matches!(
            edge_properties("generic_constraint", None),
            Some(EdgeProperties::GenericConstraint)
        ));

        // field_access requires a valid field_op.
        assert!(matches!(
            edge_properties("field_access", Some("read")),
            Some(EdgeProperties::FieldAccess { op: FieldOp::Read })
        ));
        assert!(matches!(
            edge_properties("field_access", Some("write")),
            Some(EdgeProperties::FieldAccess { op: FieldOp::Write })
        ));
        assert!(edge_properties("field_access", None).is_none());
        assert!(edge_properties("field_access", Some("bogus")).is_none());

        // Tagged variants.
        let imp = edge_properties("imports", None).expect("imports");
        assert!(matches!(
            imp,
            EdgeProperties::Imports {
                kind: ImportKind::Explicit
            }
        ));
        let corr = edge_properties("corresponds_to", None).expect("corresponds_to");
        assert!(matches!(
            corr,
            EdgeProperties::CorrespondsTo {
                source: IsomorphismSource::AutoInferred,
                ..
            }
        ));

        // Producer-emitted augmentation edge (C# extension methods).
        assert!(matches!(
            edge_properties("extends_type", None),
            Some(EdgeProperties::ExtendsType)
        ));

        // Catch-all.
        assert!(edge_properties("unknown", None).is_none());
    }
}

#[cfg(test)]
mod license_filter_tests {
    use super::{file_doc_record, is_license_boilerplate};

    #[test]
    fn markers_match_common_license_blocks() {
        // Each is one coalesced block; every line of a multi-paragraph
        // body must be recognised, including the markerless-looking
        // warranty clause.
        for block in [
            "// SPDX-License-Identifier: MIT",
            "// Copyright (c) 2024 MyCorp Inc.\n// All rights reserved.",
            "// Licensed under the Apache License, Version 2.0",
            "// Permission is hereby granted, free of charge, to any person",
            "// THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND",
            "// Redistribution and use in source and binary forms",
            "// This program is free software: under the GNU General Public License",
        ] {
            assert!(
                is_license_boilerplate(block),
                "should be flagged as license boilerplate: {block:?}"
            );
        }
    }

    #[test]
    fn genuine_comments_are_not_flagged() {
        // The bare word "copyright" (not "copyright (c)") must not trip.
        for block in [
            "// Handles authentication for the public API.",
            "// Domain models for the billing subsystem.",
            "// Returns the copyright year shown in the page footer.",
        ] {
            assert!(
                !is_license_boilerplate(block),
                "should NOT be flagged: {block:?}"
            );
        }
    }

    #[test]
    fn whole_license_block_dropped_trailing_comment_kept() {
        // The producer coalesces contiguous comments and breaks on a blank
        // line, so a license header and a following purpose comment arrive
        // as two entries. The license entry is dropped whole; the purpose
        // entry survives.
        let raw = vec![
            "// Copyright (c) 2024 Foo\n// Licensed under the MIT License.".to_owned(),
            "// Implements the order pipeline.".to_owned(),
        ];
        let rec = file_doc_record(7, "src/Order.cs", &raw).expect("a surviving doc");
        assert_eq!(rec.file_id, 7);
        // Survivor kept, comment marker stripped to prose.
        assert_eq!(rec.doc, "Implements the order pipeline.");
    }

    #[test]
    fn comment_markers_stripped_to_prose() {
        // `//!`/`///`/`//` (and one following space) removed per line; blocks
        // joined by a blank line. Mutation-check (§9): dropping the strip leaves
        // `//! …` verbatim, failing this exact equality.
        let raw = vec![
            "//! The store layer.\n//! Handles snapshots.".to_owned(),
            "/// A helper.".to_owned(),
            "/* a block\n * of prose */".to_owned(),
        ];
        let rec = file_doc_record(3, "src/lib.rs", &raw).expect("doc");
        assert_eq!(
            rec.doc,
            "The store layer.\nHandles snapshots.\n\nA helper.\n\na block\nof prose"
        );
    }

    #[test]
    fn license_only_header_yields_no_record() {
        let raw = vec!["// SPDX-License-Identifier: MIT".to_owned()];
        assert!(file_doc_record(1, "src/Widget.cs", &raw).is_none());
    }

    #[test]
    fn classify_license_tag_resolves_spdx_id() {
        use super::classify_license_tag;
        // Single-id tag → resolved + classified.
        let gpl = classify_license_tag("// SPDX-License-Identifier: GPL-3.0-only")
            .expect("GPL tag resolves");
        assert_eq!(gpl.name, "GPL-3.0-only");
        assert!(gpl.is_copyleft());

        let mit =
            classify_license_tag("/* SPDX-License-Identifier: MIT */").expect("MIT tag resolves");
        assert_eq!(mit.name, "MIT");
        assert!(!mit.is_copyleft());

        // No tag, or an unknown id → None.
        assert!(classify_license_tag("// Copyright (c) 2024 Acme. All rights reserved.").is_none());
        assert!(classify_license_tag("// SPDX-License-Identifier: Proprietary-Nonsense").is_none());
    }
}
