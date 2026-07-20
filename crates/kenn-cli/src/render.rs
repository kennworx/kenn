//! Output rendering for the query CLI surface: TOON (default) or JSON.
//!
//! TOON collapses a uniform `{items: [...]}` array into a header-once table;
//! the non-list shapes still render, as nested key:value. `--json` yields the
//! same JSON value the MCP server returns (pretty-printed).

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Toon,
    Json,
}

impl Format {
    /// The `--json` flag picks JSON; TOON is the default.
    #[must_use]
    pub fn from_json_flag(json: bool) -> Self {
        if json {
            Self::Json
        } else {
            Self::Toon
        }
    }
}

/// Print a JSON value to stdout in the chosen format.
pub fn emit(value: &Value, fmt: Format) {
    match fmt {
        // `to_string_pretty` over a plain `Value` is infallible in practice
        // (no custom `Serialize` in the tree); fall back to compact on the
        // theoretical error rather than panic.
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        ),
        Format::Toon => println!("{}", toon::encode(value, None)),
    }
}
