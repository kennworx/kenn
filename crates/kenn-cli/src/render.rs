//! Output rendering for the query CLI surface: TOON (default) or JSON.
//!
//! Each command emits its own typed result — there is no type-erased carrier.
//! JSON pretty-prints it (serde keeps struct-field order — no `Value` round-trip,
//! so keys are not alphabetized). TOON is the header-once table for a flat list;
//! `write_table` emits only the table and drops the wrapper's scalar fields, so
//! this module prints `next`/meta beneath it. Anything nested (`kenn overview`,
//! `kenn get`) isn't a table, so it goes out as JSON.

use std::io::{self, Write};

use serde::Serialize;

use crate::toon;

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

/// Serialize a value straight to a locked stdout in the chosen format.
pub fn emit<T: Serialize + ?Sized>(value: &T, fmt: Format) -> anyhow::Result<()> {
    let mut w = io::stdout().lock();
    match fmt {
        Format::Json => write_json(&mut w, value)?,
        // Decide the shape BEFORE touching stdout: serialize once to a discarding
        // sink (nothing is printed) to learn whether it's a flat table. Only then
        // stream the real table to stdout — `toon` emits only the items table,
        // and we print the envelope (`next`, meta) beneath it ourselves. Anything
        // nested isn't a table at all and goes out as JSON.
        Format::Toon => {
            if toon::write_table(&mut io::sink(), value).is_ok() {
                toon::write_table(&mut w, value)?;
                write_meta(&mut w, value)?;
            } else {
                write_json(&mut w, value)?;
            }
        }
    }
    Ok(())
}

fn write_json<T: Serialize + ?Sized>(w: &mut dyn Write, value: &T) -> anyhow::Result<()> {
    serde_json::to_writer_pretty(&mut *w, value)?;
    w.write_all(b"\n")?;
    Ok(())
}

/// Print the wrapper's scalar meta (`next`, `targets`, `truncated`, …) beneath
/// the table — the envelope `toon` deliberately drops. Nulls are omitted (so a
/// non-paginated result stays a clean table). These are independent scalar
/// lines with no column order to preserve, so reading them off a JSON view is
/// fine — the table itself never goes through `Value`.
fn write_meta<T: Serialize + ?Sized>(w: &mut dyn Write, value: &T) -> anyhow::Result<()> {
    let serde_json::Value::Object(obj) = serde_json::to_value(value)? else {
        return Ok(());
    };
    for (k, val) in &obj {
        let text = match val {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Bool(_) | serde_json::Value::Number(_) => val.to_string(),
            // Null, the `items` array, and any nested object are not meta.
            _ => continue,
        };
        writeln!(w, "{k}: {text}")?;
    }
    Ok(())
}
