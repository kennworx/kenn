//! `[xml_sql]` — the XML↔SQL bridge's configuration.
//!
//! Top-level rather than nested under `[language.xml]`, because the bridge is
//! not a language: it runs after both producers have joined, reads what each
//! wrote, and belongs to neither. Nesting it would also grow `XmlConfig` a set
//! of SQL concerns that the XML producer must never act on.
//!
//! **The vocabulary is the workspace's, not kenn's.** Which attribute names a
//! table is a fact about whichever migration tool a workspace chose, and
//! shipping a list of them would make kenn's correctness a function of which
//! tools its authors had heard of. So the rules are empty by default — the
//! bridge still works from element text alone — and a workspace that wants the
//! attribute-declared half of its schema declares one line of vocabulary.

use serde::{Deserialize, Serialize};

/// What a matched rule says the element does to the table it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableRole {
    /// Brings the table into being.
    Declares,
    /// Changes an existing table's definition.
    Modifies,
    /// Reads or writes the table's data.
    Accesses,
}

/// One workspace-declared rule: "an attribute by this name holds a table name".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableRule {
    /// The attribute whose value is a table name. The only required field —
    /// an element name alone identifies no table.
    pub attribute: String,
    /// Restrict the rule to elements with this tag. Absent: any element
    /// carrying the attribute.
    #[serde(default)]
    pub element: Option<String>,
    /// The role a match gives the reference. Absent: a plain access, which is
    /// the safe reading — claiming a declaration that is not one would mark a
    /// table internal that the workspace does not own.
    #[serde(default)]
    pub role: Option<TableRole>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XmlSqlConfig {
    /// Sources to bridge. Globs over files/dirs, workspace-relative.
    ///
    /// This is a coverage control with a real cost behind it: the dialect sweep
    /// retries only a *failed* parse, so real SQL costs one permissive parse and
    /// the entire 14-dialect sweep falls on text that is not SQL — which is most
    /// element text. Narrowing the roots removes that population outright rather
    /// than amortizing it.
    #[serde(default = "default_roots")]
    pub roots: Vec<String>,
    /// Primary dialect by name. `None` ⇒ the permissive cross-dialect parse,
    /// which is the normal case and usually the better one: measured over a
    /// fixed statement set the permissive parse scored 13/16 against oracle
    /// 10/16, postgres 10/16 and mysql 11/16. A named dialect is *stricter*,
    /// not faster and not better informed.
    #[serde(default)]
    pub dialect: Option<String>,
    /// Attribute→table rules. Empty by default: with no configuration the
    /// bridge still works from element text alone.
    #[serde(default)]
    pub rules: Vec<TableRule>,
}

fn default_roots() -> Vec<String> {
    vec![".".to_string()]
}

impl Default for XmlSqlConfig {
    fn default() -> Self {
        Self {
            roots: default_roots(),
            dialect: None,
            rules: Vec::new(),
        }
    }
}

impl XmlSqlConfig {
    /// True when nothing would be bridged from attributes.
    #[must_use]
    pub fn has_rules(&self) -> bool {
        !self.rules.is_empty()
    }

    /// Every attribute name any rule names, for the candidate prefilter.
    #[must_use]
    pub fn rule_attributes(&self) -> Vec<&str> {
        self.rules.iter().map(|r| r.attribute.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_bridge_the_whole_workspace_with_no_dialect_and_no_rules() {
        let c = XmlSqlConfig::default();
        assert_eq!(c.roots, ["."]);
        assert!(
            c.dialect.is_none(),
            "unset dialect is the permissive parse, not a named one"
        );
        assert!(
            c.rules.is_empty(),
            "the bridge works from element text with no configuration"
        );
    }

    #[test]
    fn a_rule_needs_only_an_attribute() {
        let c: XmlSqlConfig =
            toml::from_str("rules = [{ attribute = \"tableName\" }]").expect("parses");
        assert_eq!(c.rules[0].attribute, "tableName");
        assert!(c.rules[0].element.is_none());
        assert!(c.rules[0].role.is_none());
    }

    #[test]
    fn a_rule_round_trips_its_element_and_role() {
        let c: XmlSqlConfig = toml::from_str(
            "rules = [{ attribute = \"tableName\", element = \"createTable\", role = \"declares\" }]",
        )
        .expect("parses");
        assert_eq!(c.rules[0].element.as_deref(), Some("createTable"));
        assert_eq!(c.rules[0].role, Some(TableRole::Declares));
    }

    #[test]
    fn a_named_dialect_round_trips() {
        let c: XmlSqlConfig = toml::from_str("dialect = \"mssql\"").expect("parses");
        assert_eq!(c.dialect.as_deref(), Some("mssql"));
    }
}

#[cfg(test)]
mod validation_tests {
    use crate::Config;

    #[test]
    fn an_absent_section_yields_defaults() {
        let c = Config::from_toml("").expect("empty config is valid");
        assert_eq!(c.xml_sql.roots, ["."]);
        assert!(c.xml_sql.rules.is_empty());
        assert!(!c.xml_sql.has_rules());
    }

    #[test]
    fn the_section_round_trips_through_a_full_config_load() {
        let c = Config::from_toml(
            "[xml_sql]\n\
             roots = [\"db\"]\n\
             dialect = \"postgresql\"\n\
             rules = [{ attribute = \"tableName\", element = \"createTable\", role = \"declares\" }]\n",
        )
        .expect("valid");
        assert_eq!(c.xml_sql.roots, ["db"]);
        assert_eq!(c.xml_sql.dialect.as_deref(), Some("postgresql"));
        assert_eq!(c.xml_sql.rule_attributes(), ["tableName"]);
    }

    #[test]
    fn a_rule_without_an_attribute_is_a_config_error_naming_the_rule() {
        // Not a silent no-op: an inert rule looks configured, so someone would
        // conclude the bridge cannot see their schema rather than that they
        // wrote the rule wrong.
        let err = Config::from_toml(
            "[xml_sql]\nrules = [{ attribute = \"\", element = \"createTable\" }]\n",
        )
        .expect_err("must reject");
        let msg = err.to_string();
        assert!(msg.contains("rules[0]"), "names the offending rule: {msg}");
        assert!(msg.contains("createTable"), "names the element: {msg}");
    }

    #[test]
    fn the_bridge_config_does_not_move_the_reindex_staleness_key() {
        // `indexing_signature` hashes `[language.*]` only. The bridge changes
        // what is DERIVED from an index, not what is indexed — but it does
        // change the graph, so this pins the current behaviour rather than
        // asserting it is the final answer.
        let bare = Config::from_toml("").expect("valid");
        let ruled = Config::from_toml("[xml_sql]\nrules = [{ attribute = \"tableName\" }]\n")
            .expect("valid");
        assert_eq!(bare.indexing_signature(), ruled.indexing_signature());
    }
}
