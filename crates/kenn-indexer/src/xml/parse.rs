//! XML text → a flat element list.
//!
//! Pure: no file IO, no store access. Backed by `roxmltree` — a read-only
//! positioned DOM, one mandatory dependency, giving parent/child for the
//! containment edges, byte ranges for locations, namespace resolution, and a
//! positioned error rather than a panic on malformed input.
//!
//! **No vocabulary is privileged.** The only attribute names this module knows
//! are `id` and `name`, which are conventions of XML itself rather than of any
//! framework. Everything a specific vocabulary means is left to configuration.

use kenn_model::id::xml::Segment;

/// The attribute names conventionally carrying an element's identity. A
/// property of XML usage, not of any vocabulary.
const ID_ATTRS: [&str; 2] = ["id", "name"];

/// One element, flattened out of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// Root-to-element chain, every segment discriminated. This is the id.
    pub chain: Vec<Segment>,
    /// Local tag name, without any namespace prefix.
    pub tag: String,
    /// Resolved namespace URI, when the element has one. Resolved rather than
    /// the source prefix, so the same element is identified consistently
    /// however the document happens to bind its prefixes.
    pub namespace: Option<String>,
    /// Attributes in document order, as `(name, value)`.
    pub attributes: Vec<(String, String)>,
    /// Text directly inside this element — not text belonging to a child.
    pub text: String,
    /// Byte range the element occupies in the source.
    pub span: std::ops::Range<usize>,
    /// Index of this element's parent in the returned list, when it has one.
    pub parent: Option<usize>,
}

/// A document that could not be parsed, with the position the parser reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlParseError(pub String);

impl std::fmt::Display for XmlParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Flatten a document into its elements, in document order.
///
/// # Errors
/// Returns [`XmlParseError`] carrying the parser's position when the document
/// is not well-formed. Malformed input is never a panic.
pub fn parse(source: &str) -> Result<Vec<Element>, XmlParseError> {
    let doc = roxmltree::Document::parse(source).map_err(|e| XmlParseError(e.to_string()))?;

    let mut out: Vec<Element> = Vec::new();
    // roxmltree node id → index in `out`, so a child can find its parent.
    let mut index_of: std::collections::HashMap<roxmltree::NodeId, usize> =
        std::collections::HashMap::new();

    for node in doc.descendants().filter(roxmltree::Node::is_element) {
        let tag = node.tag_name().name().to_string();
        let identity = ID_ATTRS
            .iter()
            .find_map(|a| node.attribute(*a).map(ToString::to_string));

        // Ordinal among same-named siblings — the fallback when the element
        // carries no identifying attribute.
        let ordinal = node.parent().map_or(0, |p| {
            p.children()
                .filter(|c| c.is_element() && c.tag_name() == node.tag_name())
                .position(|c| c.id() == node.id())
                .unwrap_or(0)
        });

        // A named segment assumes the identity attribute tells siblings apart.
        // Usually it does; found on a real repository, sometimes it does not —
        // two `<configuration name="app">` siblings differing only
        // in a `type` attribute. Reaching for `type` would mean knowing one
        // vocabulary's business, so position disambiguates instead: it is
        // always available and never wrong. Only when the value is actually
        // shared, so the common case keeps its stable, position-free id.
        let segment = identity.as_ref().map_or_else(
            || Segment::ordinal(&tag, ordinal),
            |v| {
                if identity_is_shared(&node, v) {
                    Segment::named_at(&tag, v, ordinal)
                } else {
                    Segment::named(&tag, v)
                }
            },
        );

        // The parent's chain plus this element's own segment. Every segment is
        // discriminated, so two same-named leaves under different parents can
        // never collide.
        let parent_idx = node.parent().and_then(|p| index_of.get(&p.id()).copied());
        let mut chain = parent_idx
            .and_then(|i| out.get(i))
            .map_or_else(Vec::new, |p: &Element| p.chain.clone());
        chain.push(segment);

        // Only this element's own text: a child's text belongs to the child.
        let text = node
            .children()
            .filter(roxmltree::Node::is_text)
            .filter_map(|c| c.text())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        index_of.insert(node.id(), out.len());
        out.push(Element {
            chain,
            tag,
            namespace: node.tag_name().namespace().map(ToString::to_string),
            attributes: node
                .attributes()
                .map(|a| (a.name().to_string(), a.value().to_string()))
                .collect(),
            text,
            span: node.range(),
            parent: parent_idx,
        });
    }
    Ok(out)
}

/// Whether another same-tag sibling carries the same identity value.
///
/// Checked against siblings only, matching how the ordinal is scoped: a chain
/// is built from its parent's, so two elements can only collide if they share
/// a parent. A document-wide check would disambiguate elements that were never
/// in danger and cost every one of them a stable id.
fn identity_is_shared(node: &roxmltree::Node<'_, '_>, value: &str) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    parent
        .children()
        .filter(|c| c.is_element() && c.tag_name() == node.tag_name() && c.id() != node.id())
        .any(|c| {
            ID_ATTRS
                .iter()
                .find_map(|a| c.attribute(*a))
                .is_some_and(|v| v == value)
        })
}

/// An element's signature: its start tag, rendered as well-formed markup.
///
/// Rendered, not sliced out of the source. That matches how code signatures are
/// produced — `format_signature_documentation` takes what the indexer rendered
/// rather than cutting bytes from the file — and it is what lets a later
/// consumer read an attribute back out **exactly** instead of guessing. A
/// flattened `createTable tableName users` cannot be parsed back: nothing says
/// which words were names, which were values, or where a value containing a
/// space began and ended.
///
/// Only the start tag. The element's own text goes to the content surface
/// verbatim, because a consumer that wants to parse it — the SQL bridge reading
/// a `<sql>` body — needs it with nothing prepended: `sqlparser` rejects
/// `sql ALTER TABLE users` at the first token.
#[must_use]
pub fn signature(el: &Element) -> String {
    let mut out = String::with_capacity(el.tag.len() + el.attributes.len() * 24 + 3);
    out.push('<');
    out.push_str(&el.tag);
    for (k, v) in &el.attributes {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&escape_attribute(v));
        out.push('"');
    }
    out.push('>');
    out
}

/// Escape a value for an attribute rendered inside double quotes.
///
/// The quote and the ampersand are what break round-tripping: an unescaped `"`
/// ends the value early, and an unescaped `&` makes the rendering ill-formed.
/// `<` is escaped because a raw one is not well-formed in an attribute value
/// either. `>` needs no escape there and is left as written, so the common case
/// stays readable.
fn escape_attribute(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(src: &str) -> Vec<String> {
        parse(src)
            .expect("well-formed")
            .iter()
            .map(|e| {
                kenn_model::id::xml::element_id("f.xml", &e.chain)
                    .as_str()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn same_named_leaves_under_different_parents_get_distinct_ids() {
        // The measured collision: leaf-only ordinals put both `groupId`
        // elements at zero. Real repositories collide on 69.4% of elements.
        let src = "<project><dependencies>\
                   <dependency><groupId>a</groupId></dependency>\
                   <dependency><groupId>b</groupId></dependency>\
                   </dependencies></project>";
        let all = ids(src);
        let group_ids: Vec<&String> = all.iter().filter(|i| i.contains("groupId")).collect();
        assert_eq!(group_ids.len(), 2);
        assert_ne!(
            group_ids[0], group_ids[1],
            "distinct ids, not one merged node"
        );
    }

    #[test]
    fn an_identifying_attribute_is_preferred_to_a_position() {
        let src = "<log><changeSet id=\"0001\"/><changeSet id=\"0002\"/></log>";
        let all = ids(src);
        assert!(all.iter().any(|i| i.contains("changeSet=0001")));
        assert!(all.iter().any(|i| i.contains("changeSet=0002")));
    }

    #[test]
    fn an_id_survives_a_sibling_inserted_above_it() {
        let before = ids("<log><changeSet id=\"b\"/></log>");
        let after = ids("<log><changeSet id=\"a\"/><changeSet id=\"b\"/></log>");
        let b_before = before.iter().find(|i| i.contains("changeSet=b")).unwrap();
        let b_after = after.iter().find(|i| i.contains("changeSet=b")).unwrap();
        assert_eq!(b_before, b_after, "a named segment does not move");
    }

    #[test]
    fn text_belongs_to_the_element_that_directly_contains_it() {
        let els = parse("<outer><inner>the text</inner></outer>").expect("parse");
        let outer = els.iter().find(|e| e.tag == "outer").unwrap();
        let inner = els.iter().find(|e| e.tag == "inner").unwrap();
        assert_eq!(inner.text, "the text");
        assert!(outer.text.is_empty(), "the parent does not claim it");
    }

    #[test]
    fn nesting_is_walkable_in_both_directions() {
        let els = parse("<a><b><c/></b></a>").expect("parse");
        assert_eq!(els.len(), 3);
        let c = els.iter().position(|e| e.tag == "c").unwrap();
        let b = els[c].parent.expect("c has a parent");
        assert_eq!(els[b].tag, "b");
        let a = els[b].parent.expect("b has a parent");
        assert_eq!(els[a].tag, "a");
        assert!(els[a].parent.is_none(), "the root has none");
    }

    #[test]
    fn a_namespace_is_recorded_resolved_not_as_its_prefix() {
        let default_ns = parse("<r xmlns=\"urn:x\"><c/></r>").expect("parse");
        let prefixed = parse("<p:r xmlns:p=\"urn:x\"><p:c/></p:r>").expect("parse");
        assert_eq!(
            default_ns[1].namespace.as_deref(),
            prefixed[1].namespace.as_deref(),
            "the same namespace however the source bound it"
        );
        assert_eq!(default_ns[1].namespace.as_deref(), Some("urn:x"));
    }

    #[test]
    fn a_byte_range_selects_the_element() {
        let src = "<r><child>x</child></r>";
        let els = parse(src).expect("parse");
        let child = els.iter().find(|e| e.tag == "child").unwrap();
        assert_eq!(
            src.get(child.span.clone())
                .expect("span is a char boundary"),
            "<child>x</child>"
        );
    }

    #[test]
    fn malformed_input_is_a_positioned_error_not_a_panic() {
        let err = parse("<a><b></a>").expect_err("mismatched tags");
        assert!(err.0.contains("at "), "carries a position: {}", err.0);
    }

    #[test]
    fn siblings_sharing_an_identity_value_still_get_distinct_ids() {
        // Found by indexing a real repository, not by reading the code: two
        // `<configuration name="app">` siblings differing only in a
        // `type` attribute. A named segment assumed the identity attribute told
        // siblings apart, so both rendered one id — and since a child's chain is
        // built from its parent's, every descendant collided too. Two elements
        // produced 17 colliding ids.
        let src = r#"<project>
            <configuration name="app" type="DotNetProject"><method/></configuration>
            <configuration name="app" type="LaunchSettings"><method/></configuration>
        </project>"#;
        let all = ids(src);
        let unique: std::collections::HashSet<&String> = all.iter().collect();
        assert_eq!(unique.len(), all.len(), "no id used twice: {all:?}");
        // The descendants are what made this expensive, so they are asserted.
        let methods: Vec<&String> = all.iter().filter(|i| i.contains("method")).collect();
        assert_eq!(methods.len(), 2);
        assert_ne!(
            methods[0], methods[1],
            "children inherit the fix: {methods:?}"
        );
    }

    #[test]
    fn a_unique_identity_keeps_its_position_free_id() {
        // The other half: disambiguating unconditionally would make every id
        // positional, so inserting a sibling above would renumber the lot. The
        // ordinal appears only where the value is actually shared.
        let src = r#"<project>
            <configuration name="A"/>
            <configuration name="B"/>
        </project>"#;
        let all = ids(src);
        assert!(
            all.iter().any(|i| i.ends_with("configuration=A")),
            "no ordinal on a unique name: {all:?}"
        );
        assert!(
            all.iter().any(|i| i.ends_with("configuration=B")),
            "{all:?}"
        );
    }

    #[test]
    fn a_signature_is_the_start_tag_and_not_the_body() {
        let els = parse("<e one=\"1\" two=\"2\">body</e>").expect("parse");
        assert_eq!(els.len(), 1, "one node, whatever its attribute count");
        assert_eq!(signature(&els[0]), r#"<e one="1" two="2">"#);
        // The text is carried separately, for the content surface.
        assert_eq!(els[0].text, "body");
    }

    #[test]
    fn structured_values_survive_verbatim() {
        // The reason XML is not identifier-split: the punctuation IS the value.
        let els = parse("<dep groupId=\"org.springframework\" version=\"1.2.3\"/>").expect("parse");
        let sig = signature(&els[0]);
        assert!(
            sig.contains("org.springframework"),
            "namespace intact: {sig}"
        );
        assert!(sig.contains("1.2.3"), "version pin intact: {sig}");
    }

    #[test]
    fn an_attribute_value_round_trips_out_of_the_signature() {
        // The point of rendering rather than flattening. A value containing a
        // space is indistinguishable from two words once flattened, so a
        // consumer reading an attribute back could only guess. Re-parsing the
        // rendered signature recovers name and value exactly.
        let els = parse(r#"<task name="run all tests" when="on push"/>"#).expect("parse");
        let sig = signature(&els[0]);
        assert_eq!(sig, r#"<task name="run all tests" when="on push">"#);

        let round = parse(&format!("{sig}</task>")).expect("signature is well-formed markup");
        assert_eq!(
            round[0].attributes, els[0].attributes,
            "name and value survive"
        );
    }

    #[test]
    fn a_value_containing_markup_characters_still_round_trips() {
        // Unescaped, a `"` ends the value early and an `&` makes the rendering
        // ill-formed — either way the signature stops being re-parseable, which
        // is the one property it exists to have.
        let els =
            parse(r#"<e q="say &quot;hi&quot;" amp="a &amp; b" lt="x &lt; y"/>"#).expect("parse");
        let sig = signature(&els[0]);
        let round = parse(&format!("{sig}</e>")).expect("escaped rendering re-parses");
        assert_eq!(round[0].attributes, els[0].attributes);
        assert_eq!(
            round[0].attributes[0].1, r#"say "hi""#,
            "the quote survives as data, not as a delimiter"
        );
    }
}
