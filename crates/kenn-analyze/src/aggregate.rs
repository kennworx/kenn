//! Walk each symbol's `enclosing_symbol` chain upward to find its
//! aggregate node: the nearest enclosing class-like (class, struct, trait,
//! interface, enum) symbol, falling back to the nearest module-like
//! (module, namespace, package) symbol, falling back to the symbol itself.
//!
//! Phase 1: the aggregate ID is the symbol's own `short_id`. Methods,
//! fields, parameters, etc. roll up to their enclosing class. Free fns
//! roll up to their enclosing module.

use std::collections::HashMap;

use kenn_model::ShortId;
use kenn_store::api::types::SymbolRow;

/// Returns `true` for kinds that anchor an aggregate (class-like).
fn is_class_like(kind: &str) -> bool {
    matches!(
        kind,
        "class" | "struct" | "trait" | "interface" | "enum" | "type_alias"
    )
}

/// Returns `true` for kinds that anchor an aggregate (module-like fallback).
fn is_module_like(kind: &str) -> bool {
    matches!(kind, "module" | "namespace" | "package")
}

/// Compute `aggregate_id` for each symbol by walking `enclosing_symbol`.
/// Cycle-safe: terminates on a re-visited node.
#[must_use]
pub fn compute<S: std::hash::BuildHasher>(
    symbols: &HashMap<ShortId, SymbolRow, S>,
) -> HashMap<ShortId, ShortId> {
    let mut out = HashMap::with_capacity(symbols.len());
    for &sid in symbols.keys() {
        out.insert(sid, walk(sid, symbols));
    }
    out
}

fn walk<S: std::hash::BuildHasher>(
    start: ShortId,
    symbols: &HashMap<ShortId, SymbolRow, S>,
) -> ShortId {
    let mut seen = std::collections::HashSet::new();
    let mut cur = start;
    let mut module_fallback: Option<ShortId> = None;
    loop {
        if !seen.insert(cur) {
            // cycle — anchor on whatever module we passed through, else self
            return module_fallback.unwrap_or(start);
        }
        let Some(row) = symbols.get(&cur) else {
            return module_fallback.unwrap_or(start);
        };
        if is_class_like(&row.kind) {
            return cur;
        }
        if is_module_like(&row.kind) && module_fallback.is_none() {
            module_fallback = Some(cur);
        }
        if row.enclosing_sym_id == 0 || row.enclosing_sym_id == cur {
            // top of chain — return the highest class-like we saw (none),
            // else the lowest module-like, else self
            return module_fallback.unwrap_or(start);
        }
        cur = row.enclosing_sym_id;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(id: ShortId, kind: &str, enclosing: ShortId) -> SymbolRow {
        SymbolRow {
            id,
            pub_id: format!("test:s{id}"),
            language: "rs".into(),
            pkg_id: 0,
            kind: kind.into(),
            name: format!("s{id}"),
            partial: false,
            nargs: 0,
            targs: 0,
            external: false,
            test: false,
            enclosing_sym_id: enclosing,
        }
    }

    fn map_of(rows: Vec<SymbolRow>) -> HashMap<ShortId, SymbolRow> {
        rows.into_iter().map(|s| (s.id, s)).collect()
    }

    #[test]
    fn method_rolls_up_to_class() {
        // module(1) → class(2) → method(3)
        let m = map_of(vec![
            sym(1, "module", 0),
            sym(2, "class", 1),
            sym(3, "method", 2),
        ]);
        let agg = compute(&m);
        assert_eq!(agg[&3], 2, "method aggregates to enclosing class");
        assert_eq!(agg[&2], 2, "class is its own aggregate");
        assert_eq!(agg[&1], 1, "module is its own aggregate");
    }

    #[test]
    fn free_fn_rolls_up_to_module() {
        // module(1) → function(2)
        let m = map_of(vec![sym(1, "module", 0), sym(2, "function", 1)]);
        let agg = compute(&m);
        assert_eq!(agg[&2], 1, "free function aggregates to module");
        assert_eq!(agg[&1], 1);
    }

    #[test]
    fn method_in_nested_module_aggregates_to_class_not_module() {
        // package(1) → module(2) → class(3) → method(4)
        let m = map_of(vec![
            sym(1, "package", 0),
            sym(2, "module", 1),
            sym(3, "class", 2),
            sym(4, "method", 3),
        ]);
        let agg = compute(&m);
        assert_eq!(agg[&4], 3, "method anchors on nearest class, not module");
    }

    #[test]
    fn field_inside_class_rolls_up_to_class() {
        let m = map_of(vec![
            sym(1, "namespace", 0),
            sym(2, "class", 1),
            sym(3, "field", 2),
        ]);
        let agg = compute(&m);
        assert_eq!(agg[&3], 2);
    }

    #[test]
    fn orphan_symbol_is_its_own_aggregate() {
        let m = map_of(vec![sym(1, "function", 0)]);
        let agg = compute(&m);
        assert_eq!(agg[&1], 1);
    }

    #[test]
    fn enclosing_points_to_missing_symbol_falls_back_to_self() {
        // enclosing = 99 but 99 is not in the map
        let m = map_of(vec![sym(1, "method", 99)]);
        let agg = compute(&m);
        assert_eq!(agg[&1], 1);
    }

    #[test]
    fn cycle_terminates() {
        // 1 → 2 → 1
        let mut m = map_of(vec![sym(1, "function", 2), sym(2, "function", 1)]);
        // overwrite to create cycle
        m.get_mut(&1).unwrap().enclosing_sym_id = 2;
        m.get_mut(&2).unwrap().enclosing_sym_id = 1;
        let agg = compute(&m);
        // No class or module on the chain — fall back to start
        assert_eq!(agg[&1], 1);
        assert_eq!(agg[&2], 2);
    }
}
