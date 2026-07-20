//! Multi-run merge and collision detection (section 8 of the proposal).
//!
//! After all language drivers have produced their record streams (one
//! `RunPartition` per unit), we materialize a single deduplicated view
//! that storage will consume. Two records pinning the same logical entity
//! collapse; symbols whose `pub_id` is observed under conflicting file
//! paths surface as `SymbolCollision` warnings on the consolidated report.

use std::collections::{HashMap, HashSet};

use kenn_model::{
    DefRecord, EdgeRecord, FileRecord, Language, ShortId, SymbolDocsRecord, SymbolRecord,
};
use serde::{Deserialize, Serialize};

/// One language driver's output for one unit.
#[derive(Debug, Default, Clone)]
pub struct RunPartition {
    pub files: Vec<FileRecord>,
    pub symbols: Vec<SymbolRecord>,
    pub symbol_docs: Vec<SymbolDocsRecord>,
    pub defs: Vec<DefRecord>,
    pub edges: Vec<EdgeRecord>,
}

#[derive(Debug, Default, Clone)]
pub struct MaterializedView {
    pub files: Vec<FileRecord>,
    pub symbols: Vec<SymbolRecord>,
    pub symbol_docs: Vec<SymbolDocsRecord>,
    pub defs: Vec<DefRecord>,
    pub edges: Vec<EdgeRecord>,
    pub symbol_collisions: Vec<SymbolCollision>,
}

/// Same `pub_id` appearing under multiple file paths — surfaced as a
/// warning on the consolidated report (task 8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolCollision {
    pub language: Language,
    pub pub_id: String,
    pub file_paths: Vec<String>,
}

#[must_use]
pub fn materialize(runs: &[RunPartition]) -> MaterializedView {
    let mut files: HashMap<String, FileRecord> = HashMap::new();
    let mut symbols: HashMap<(Language, String), SymbolRecord> = HashMap::new();
    let mut symbol_docs: HashMap<ShortId, SymbolDocsRecord> = HashMap::new();
    let mut defs: Vec<DefRecord> = Vec::new();
    let mut edges: HashSet<EdgeRecord> = HashSet::new();
    // pub_id → set of file paths the symbol was defined in (collected from defs).
    let mut symbol_to_paths: HashMap<(Language, String), HashSet<String>> = HashMap::new();
    let mut file_id_to_path: HashMap<ShortId, String> = HashMap::new();
    // Symbol short_id → (language, pub_id) so we can map defs back to a key.
    let mut sym_id_to_key: HashMap<ShortId, (Language, String)> = HashMap::new();

    for run in runs {
        for f in &run.files {
            file_id_to_path.insert(f.id, f.path.clone());
            files.entry(f.path.clone()).or_insert_with(|| f.clone());
        }
        for s in &run.symbols {
            let key = (s.language, s.pub_id.clone());
            symbols.entry(key.clone()).or_insert_with(|| s.clone());
            sym_id_to_key.insert(s.id, key);
        }
        for d in &run.defs {
            defs.push(d.clone());
            if let (Some(path), Some(key)) = (
                file_id_to_path.get(&d.file_id),
                sym_id_to_key.get(&d.sym_id),
            ) {
                symbol_to_paths
                    .entry(key.clone())
                    .or_default()
                    .insert(path.clone());
            }
        }
        for d in &run.symbol_docs {
            symbol_docs.entry(d.sym_id).or_insert_with(|| d.clone());
        }
        for e in &run.edges {
            edges.insert(e.clone());
        }
    }

    let mut symbol_collisions: Vec<SymbolCollision> = symbol_to_paths
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|((language, pub_id), paths)| {
            let mut file_paths: Vec<String> = paths.into_iter().collect();
            file_paths.sort();
            SymbolCollision {
                language,
                pub_id,
                file_paths,
            }
        })
        .collect();
    symbol_collisions.sort_by(|a, b| a.pub_id.cmp(&b.pub_id));

    MaterializedView {
        files: files.into_values().collect(),
        symbols: symbols.into_values().collect(),
        symbol_docs: symbol_docs.into_values().collect(),
        defs,
        edges: edges.into_iter().collect(),
        symbol_collisions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_model::{EdgeProperties, Kind};

    fn file(id: ShortId, path: &str) -> FileRecord {
        FileRecord {
            id,
            path: path.into(),
            language: Language::Csharp,
            test: false,
            external: false,
            content_hash: 0,
        }
    }

    fn def(sym_id: ShortId, file_id: ShortId) -> DefRecord {
        DefRecord {
            sym_id,
            file_id,
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
            body_start_line: 0,
            body_end_line: 0,
        }
    }

    fn sym(id: ShortId, pub_id: &str, _file: ShortId) -> SymbolRecord {
        SymbolRecord {
            id,
            pub_id: pub_id.into(),
            language: Language::Csharp,
            pkg_id: 0,
            kind: Kind::Class,
            name: "x".into(),
            enclosing_sym_id: 0,
            partial: false,
            nargs: 0,
            targs: 0,
            external: false,
            test: false,
        }
    }

    #[test]
    fn dedup_overlapping_runs() {
        // 8.4 — two runs that both index the same `Host.cs` collapse.
        let run1 = RunPartition {
            files: vec![file(1, "src/Host.cs")],
            symbols: vec![sym(1, "cs:Host", 1)],
            edges: vec![EdgeRecord {
                src_id: 1,
                target_id: 1,
                properties: EdgeProperties::DefinedIn,
            }],
            ..Default::default()
        };
        let run2 = run1.clone();
        let view = materialize(&[run1, run2]);
        assert_eq!(view.files.len(), 1);
        assert_eq!(view.symbols.len(), 1);
        assert_eq!(view.edges.len(), 1);
        assert!(view.symbol_collisions.is_empty());
    }

    #[test]
    fn collision_emitted_when_same_pub_id_under_two_paths() {
        // 8.5 — two projects each define `Common.Helpers` at the same
        // public id; both retained, warning emitted.
        let run_a = RunPartition {
            files: vec![file(1, "ProjA/Helpers.cs")],
            symbols: vec![sym(1, "cs:Common.Helpers", 1)],
            defs: vec![def(1, 1)],
            ..Default::default()
        };
        let run_b = RunPartition {
            files: vec![file(2, "ProjB/Helpers.cs")],
            symbols: vec![sym(2, "cs:Common.Helpers", 2)],
            defs: vec![def(2, 2)],
            ..Default::default()
        };
        let view = materialize(&[run_a, run_b]);
        assert_eq!(view.files.len(), 2);
        assert_eq!(view.symbol_collisions.len(), 1);
        let coll = &view.symbol_collisions[0];
        assert_eq!(coll.pub_id, "cs:Common.Helpers");
        assert_eq!(coll.file_paths.len(), 2);
        assert!(coll.file_paths.contains(&"ProjA/Helpers.cs".to_string()));
        assert!(coll.file_paths.contains(&"ProjB/Helpers.cs".to_string()));
    }
}
