//! SQL public-ID construction.
//!
//! SQL nodes are not produced from SCIP. A file and a statement are identified
//! by the file's workspace-relative path — `sql:<relpath>` for the file-as-node
//! and `sql:<relpath>#<index>` for one top-level statement, mirroring the
//! text-fallback chunk scheme.
//!
//! A **table** is different, and deliberately so: its id carries no path at all.
//! `sql:<table>`, or `sql:<schema>.<table>` when and only when the declaring
//! source states a schema. A table's definition is a fold over many statements
//! in many files — a `CREATE` here, an `ALTER` there — so scoping its identity
//! to whichever file happened to name it first would split one table into as
//! many nodes as the files that mention it. The identity is the name the source
//! states; nothing infers or defaults a schema for an unqualified declaration.

use crate::id::PublicId;
use crate::language::Language;

/// Public ID of a `.sql` file-as-node (`document` kind): `sql:<relpath>`.
#[must_use]
pub fn file_id(relpath: &str) -> PublicId {
    PublicId::new(Language::Sql, relpath)
}

/// Public ID of one top-level statement (`sql_statement` kind), disambiguated
/// by its 0-based index within the file: `sql:<relpath>#<index>`.
#[must_use]
pub fn statement_id(relpath: &str, index: usize) -> PublicId {
    PublicId::new(Language::Sql, &format!("{relpath}#{index}"))
}

/// Public ID of a table (`sql_table` kind): `sql:<table>`, or
/// `sql:<schema>.<table>` when the source stated a schema.
///
/// Carries no file path: one table has one node however many files name it.
#[must_use]
pub fn table_id(schema: Option<&str>, name: &str) -> PublicId {
    let native = match schema {
        Some(s) => format!("{s}.{name}"),
        None => name.to_string(),
    };
    PublicId::new(Language::Sql, &native)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_and_statement_ids_carry_the_path() {
        assert_eq!(file_id("db/0001.sql").as_str(), "sql:db/0001.sql");
        assert_eq!(statement_id("db/0001.sql", 2).as_str(), "sql:db/0001.sql#2");
    }

    #[test]
    fn a_table_id_carries_no_path() {
        let id = table_id(None, "users");
        assert_eq!(id.as_str(), "sql:users");
        assert!(!id.as_str().contains('/'));
    }

    #[test]
    fn the_same_table_from_different_files_is_one_id() {
        // The whole point of the scheme: a CREATE in one migration and an ALTER
        // in another must land on one node.
        assert_eq!(table_id(None, "users"), table_id(None, "users"));
    }

    #[test]
    fn an_explicit_schema_is_a_distinct_identity() {
        assert_eq!(
            table_id(Some("analytics"), "users").as_str(),
            "sql:analytics.users"
        );
        assert_ne!(
            table_id(Some("analytics"), "users"),
            table_id(None, "users")
        );
    }
}
