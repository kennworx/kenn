//! `.css` parse → `kenn_model` records via lightningcss (the sibling-producer
//! analogue of the SCIP path's `TransformedDocument` / markdown's `walk`).
//!
//! Each stylesheet file becomes a `Kind::Module` node; each atomic class/id
//! selector and each custom-property definition becomes a `css_class`/`css_id`/
//! `css_var` node `defined_in` that module (which `contains` the file). Compound
//! selectors are split into atoms (`.a.b` → `a`, `b`); CSS nesting (`&`) is
//! handled by recursing into nested rules and collecting their atoms.

use std::collections::HashMap;

use kenn_model::id::css::{module_id, selector_id, SelectorKind};
use kenn_model::{
    compose_short_id, DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, ShortId,
    SymbolDocsRecord, SymbolRecord,
};

use super::discover::DiscoveredStylesheet;

/// Allocates stylesheet `short_id`s, with separate file/symbol counters per
/// language partition (`Css` and `Sass`) so a single ingest run can mint nodes
/// for both without collision.
#[derive(Debug, Default)]
pub struct CssIds {
    css_file: u32,
    css_symbol: u32,
    sass_file: u32,
    sass_symbol: u32,
}

impl CssIds {
    #[must_use]
    pub fn new() -> Self {
        Self {
            css_file: 1,
            css_symbol: 1,
            sass_file: 1,
            sass_symbol: 1,
        }
    }

    pub(crate) fn file_id(&mut self, language: Language) -> ShortId {
        // Only ever Css/Sass; map anything else to the css partition behind a
        // debug assert so counter and partition can never silently desync.
        let counter = match language {
            Language::Sass => &mut self.sass_file,
            Language::Css => &mut self.css_file,
            other => {
                debug_assert!(false, "CssIds minted for non-stylesheet language {other:?}");
                &mut self.css_file
            }
        };
        let id = compose_short_id(language, *counter);
        *counter += 1;
        id
    }

    pub(crate) fn symbol_id(&mut self, language: Language) -> ShortId {
        let counter = match language {
            Language::Sass => &mut self.sass_symbol,
            Language::Css => &mut self.css_symbol,
            other => {
                debug_assert!(false, "CssIds minted for non-stylesheet language {other:?}");
                &mut self.css_symbol
            }
        };
        let id = compose_short_id(language, *counter);
        *counter += 1;
        id
    }
}

/// Records produced for one stylesheet file.
#[derive(Debug)]
pub struct CssRecords {
    pub file: FileRecord,
    pub symbols: Vec<SymbolRecord>,
    pub defs: Vec<DefRecord>,
    pub edges: Vec<EdgeRecord>,
    pub docs: Vec<SymbolDocsRecord>,
}

/// One extracted selector atom: its node kind, bare name, and the **0-based**
/// source line of the rule it was found on (lightningcss's `loc.line`).
pub(crate) struct Atom {
    pub(crate) kind: SelectorKind,
    pub(crate) name: String,
    pub(crate) line: u32,
    /// 0-based source column of the rule (for source-map lookup on the Sass path).
    pub(crate) col: u32,
}

/// Parse a CSS string with lightningcss (error-recovery on) and collect every
/// atomic class/id selector and custom-property def. Shared by the `.css` path
/// and the Sass path (which runs it over dart-sass's compiled output). Returns
/// `None` when the parser cannot produce a stylesheet at all.
#[must_use]
pub(crate) fn collect_atoms(css: &str) -> Option<Vec<Atom>> {
    use lightningcss::stylesheet::{ParserOptions, StyleSheet};

    let opts = ParserOptions {
        error_recovery: true,
        ..Default::default()
    };
    let sheet = StyleSheet::parse(css, opts).ok()?;
    let mut atoms = Vec::new();
    for rule in &sheet.rules.0 {
        collect_rule(rule, &mut atoms);
    }
    Some(atoms)
}

/// Parse a `.css` source into records. Returns `None` when lightningcss cannot
/// parse it even in error-recovery mode (the caller skips + logs).
#[must_use]
pub fn parse_css(
    file: &DiscoveredStylesheet,
    content: &str,
    ids: &mut CssIds,
) -> Option<CssRecords> {
    let atoms = collect_atoms(content)?;

    let language = file.language;
    let lines: Vec<&str> = content.lines().collect();
    let total = u32::try_from(lines.len()).unwrap_or(u32::MAX).max(1);
    let file_id = ids.file_id(language);
    let module_sym = ids.symbol_id(language);

    let mut out = CssRecords {
        file: FileRecord {
            id: file_id,
            path: file.relpath.clone(),
            language,
            test: false,
            external: false,
            content_hash: xxhash_rust::xxh3::xxh3_64(content.as_bytes()),
        },
        symbols: Vec::new(),
        defs: Vec::new(),
        edges: Vec::new(),
        docs: Vec::new(),
    };

    // The stylesheet-as-module node owns the file row (`contains`) and every
    // selector (`defined_in`).
    out.symbols.push(symbol(
        module_sym,
        crate::pubid::floor(&module_id(language, &file.relpath).into_string()),
        language,
        Kind::Module,
        basename(&file.relpath),
        0,
    ));
    out.defs.push(def(module_sym, file_id, 1, total));
    out.edges.push(EdgeRecord {
        src_id: module_sym,
        target_id: file_id,
        properties: EdgeProperties::Contains,
    });

    // Atoms are deduped by node pub_id — a class used in many rules is one node
    // per file, keeping the first line we saw it on.
    let mut seen: HashMap<String, ShortId> = HashMap::new();
    for atom in atoms {
        let pub_id = crate::pubid::floor(
            &selector_id(language, &file.relpath, atom.kind, &atom.name).into_string(),
        );
        if seen.contains_key(&pub_id) {
            continue;
        }
        let sym = ids.symbol_id(language);
        seen.insert(pub_id.clone(), sym);
        // Selector text + the immediately-preceding comment feed FTS +
        // embeddings (the embedding value is in the prose comment, not the name).
        let sig = selector_text(atom.kind, &atom.name);
        let doc = preceding_comment(&lines, atom.line as usize);
        if !doc.is_empty() {
            out.docs.push(SymbolDocsRecord {
                sym_id: sym,
                sig,
                doc,
            });
        }
        let line = atom.line.saturating_add(1); // 0-based → 1-based def line
        out.symbols.push(symbol(
            sym,
            pub_id,
            language,
            kind_of(atom.kind),
            atom.name,
            module_sym,
        ));
        out.defs.push(def(sym, file_id, line, line));
        out.edges.push(EdgeRecord {
            src_id: sym,
            target_id: module_sym,
            properties: EdgeProperties::DefinedIn,
        });
    }

    Some(out)
}

/// Recursively collect class/id atoms and custom-property defs from a rule and
/// its nested rules (CSS nesting). `&`-nesting needs no special handling: the
/// nested rule's own `Component::Class`/`ID` atoms are the registry entries.
fn collect_rule(rule: &lightningcss::rules::CssRule, out: &mut Vec<Atom>) {
    use lightningcss::properties::Property;
    use lightningcss::rules::CssRule;
    use lightningcss::selector::Component;

    let CssRule::Style(style) = rule else {
        return;
    };
    let line = style.loc.line; // 0-based; callers convert as needed
    let col = style.loc.column.saturating_sub(1); // lightningcss column is 1-based
    for sel in &style.selectors.0 {
        for comp in sel.iter_raw_match_order() {
            match comp {
                Component::Class(c) => out.push(Atom {
                    kind: SelectorKind::Class,
                    name: c.0.as_ref().to_string(),
                    line,
                    col,
                }),
                Component::ID(i) => out.push(Atom {
                    kind: SelectorKind::Id,
                    name: i.0.as_ref().to_string(),
                    line,
                    col,
                }),
                _ => {}
            }
        }
    }
    for (decl, _important) in style.declarations.iter() {
        if let Property::Custom(custom) = decl {
            out.push(Atom {
                kind: SelectorKind::Var,
                name: custom_property_name(&custom.name),
                line,
                col,
            });
        }
    }
    for nested in &style.rules.0 {
        collect_rule(nested, out);
    }
}

/// The `--name` of a custom-property declaration (the dashed form, kept with
/// its `--`).
fn custom_property_name(name: &lightningcss::properties::custom::CustomPropertyName) -> String {
    use lightningcss::properties::custom::CustomPropertyName;
    let raw = match name {
        CustomPropertyName::Custom(d) => d.as_ref(),
        CustomPropertyName::Unknown(i) => i.0.as_ref(),
    };
    if raw.starts_with("--") {
        raw.to_string()
    } else {
        format!("--{raw}")
    }
}

/// The written selector form for FTS (`.btn` / `#app` / `--brand`).
pub(crate) fn selector_text(kind: SelectorKind, name: &str) -> String {
    match kind {
        SelectorKind::Class => format!(".{name}"),
        SelectorKind::Id => format!("#{name}"),
        SelectorKind::Var => name.to_string(),
    }
}

/// The `/* … */` comment block immediately preceding the rule at 0-based
/// `rule_line` (blank lines between the comment and the rule are allowed).
/// Returns the inner text (leading `*` stripped, trimmed), or empty when there
/// is no preceding comment.
pub(crate) fn preceding_comment(lines: &[&str], rule_line: usize) -> String {
    // Skip blank lines just above the rule.
    let mut i = rule_line;
    while i > 0 && lines.get(i - 1).is_some_and(|l| l.trim().is_empty()) {
        i -= 1;
    }
    if i == 0 {
        return String::new();
    }
    let end = i - 1;
    if !lines.get(end).is_some_and(|l| l.trim_end().ends_with("*/")) {
        return String::new();
    }
    // Walk up to the line that opens the block.
    let mut start = end;
    while start > 0 && !lines.get(start).is_some_and(|l| l.contains("/*")) {
        start -= 1;
    }
    if !lines.get(start).is_some_and(|l| l.contains("/*")) {
        return String::new();
    }
    let raw = lines.get(start..=end).unwrap_or(&[]).join("\n");
    let inner = raw
        .trim()
        .trim_start_matches("/*")
        .trim_end_matches("*/")
        .trim();
    inner
        .lines()
        .map(|l| l.trim().trim_start_matches('*').trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn kind_of(s: SelectorKind) -> Kind {
    match s {
        SelectorKind::Class => Kind::CssClass,
        SelectorKind::Id => Kind::CssId,
        SelectorKind::Var => Kind::CssVar,
    }
}

pub(crate) fn symbol(
    id: ShortId,
    pub_id: String,
    language: Language,
    kind: Kind,
    name: String,
    enclosing_sym_id: ShortId,
) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id,
        language,
        pkg_id: 0,
        kind,
        name,
        enclosing_sym_id,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

pub(crate) fn def(sym_id: ShortId, file_id: ShortId, start_line: u32, end_line: u32) -> DefRecord {
    DefRecord {
        sym_id,
        file_id,
        start_line,
        start_col: 0,
        end_line,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    }
}

fn basename(relpath: &str) -> String {
    relpath.rsplit('/').next().unwrap_or(relpath).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn disc(relpath: &str, language: Language) -> DiscoveredStylesheet {
        DiscoveredStylesheet {
            abs_path: PathBuf::from(relpath),
            relpath: relpath.into(),
            language,
        }
    }

    fn parse(relpath: &str, css: &str) -> CssRecords {
        let mut ids = CssIds::new();
        parse_css(&disc(relpath, Language::Css), css, &mut ids).expect("parse")
    }

    #[test]
    fn module_owns_atoms_and_compounds_split() {
        let r = parse(
            "src/x.css",
            ".btn.btn-primary { color: red }\n#app .card {}\n",
        );
        // module + btn + btn-primary + app + card
        let names: Vec<&str> = r.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"btn") && names.contains(&"btn-primary"));
        assert!(names.contains(&"app") && names.contains(&"card"));
        // first symbol is the module; it contains the file.
        assert_eq!(r.symbols[0].kind, Kind::Module);
        assert_eq!(r.edges[0].target_id, r.file.id);
        assert!(matches!(r.edges[0].properties, EdgeProperties::Contains));
        // every selector is defined_in the module.
        let defined_in = r
            .edges
            .iter()
            .filter(|e| matches!(e.properties, EdgeProperties::DefinedIn))
            .count();
        assert_eq!(defined_in, 4);
        let btn = r.symbols.iter().find(|s| s.name == "btn").unwrap();
        assert_eq!(btn.kind, Kind::CssClass);
        assert_eq!(btn.pub_id, "css:src/x.css#class:btn");
    }

    #[test]
    fn custom_properties_become_var_nodes() {
        let r = parse("t.css", ":root { --brand: #36f; --space-1: 4px }\n");
        let vars: Vec<&str> = r
            .symbols
            .iter()
            .filter(|s| s.kind == Kind::CssVar)
            .map(|s| s.name.as_str())
            .collect();
        assert!(vars.contains(&"--brand") && vars.contains(&"--space-1"));
    }

    #[test]
    fn css_nesting_collects_nested_atoms() {
        // `&.active` and the descendant `.title` are atoms under `.card`.
        let r = parse(
            "n.css",
            ".card { &.active { color: green } .title { font-weight: bold } }\n",
        );
        let names: Vec<&str> = r.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"card"));
        assert!(names.contains(&"active"));
        assert!(names.contains(&"title"));
    }

    #[test]
    fn duplicate_class_is_one_node() {
        let r = parse("d.css", ".btn{}\n.btn{color:red}\n.btn:hover{}\n");
        let btn_nodes = r.symbols.iter().filter(|s| s.name == "btn").count();
        assert_eq!(btn_nodes, 1);
    }

    #[test]
    fn def_site_is_one_based_source_line() {
        // `.foo` sits on editor line 3 (two leading blank lines).
        let r = parse("L.css", "\n\n.foo { color: red }\n");
        let foo = r.symbols.iter().find(|s| s.name == "foo").unwrap();
        let d = r.defs.iter().find(|d| d.sym_id == foo.id).unwrap();
        assert_eq!(d.start_line, 3);
    }

    #[test]
    fn preceding_comment_feeds_docs() {
        let css = "/* Primary call-to-action button */\n.btn-primary { color: blue }\n.plain {}\n";
        let r = parse("c.css", css);
        let btn = r.symbols.iter().find(|s| s.name == "btn-primary").unwrap();
        let doc = r.docs.iter().find(|d| d.sym_id == btn.id).unwrap();
        assert_eq!(doc.doc, "Primary call-to-action button");
        assert_eq!(doc.sig, ".btn-primary");
        // `.plain` has no comment → no docs record.
        let plain = r.symbols.iter().find(|s| s.name == "plain").unwrap();
        assert!(r.docs.iter().all(|d| d.sym_id != plain.id));
    }

    #[test]
    fn malformed_tail_still_yields_valid_nodes() {
        // error_recovery: the valid `.ok` rule survives the broken tail.
        let r = parse("m.css", ".ok { color: red }\n.broken { color: ;\n");
        assert!(r.symbols.iter().any(|s| s.name == "ok"));
    }
}
