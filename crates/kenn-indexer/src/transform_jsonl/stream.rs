//! The stateful frame handler: `handle_frame` dispatch, the per-stream
//! `StreamState` interning machine, and the end-of-job stub flush.

use std::collections::{HashMap, HashSet};

use kenn_model::{
    DefRecord, EdgeRecord, FileRecord, Kind, Language, PackageRecord, ShortId, SymbolRecord,
};
use kenn_store::api::DbError;

use crate::canonicalize::Workspace;
use crate::parse_jsonl::{
    EdgeFrame, ErrorFrame, FileFrame, Frame, MetaFrame, PackageFrame, Ref, StubFrame, SymbolFrame,
};
use crate::sink::BatchSink;
use crate::transform::{language_from_scip, IdRegistry};

use super::{
    build_docs_record, edge_properties, emit_ts_bench, file_doc_record, kind_from_str,
    JsonlIngestStats, JsonlTransformError,
};

pub(crate) fn handle_frame(
    frame: Frame,
    state: &mut StreamState<'_>,
    registry: &mut IdRegistry,
    sink: &mut BatchSink,
    counts: &mut JsonlIngestStats,
) -> Result<(), JsonlTransformError> {
    match frame {
        Frame::Meta(m) => {
            emit_ts_bench("meta", &m.ts);
            counts.tool_version = Some(m.tool_version.clone());
            state.on_meta(&m);
        }
        Frame::File(f) => {
            if let Some(rec) = state.on_file(&f, registry) {
                let file_id = rec.id;
                sink.push_file(rec)?;
                counts.files += 1;
                if let Some(doc) = file_doc_record(file_id, &f.path, &f.doc) {
                    sink.push_file_docs(doc)?;
                }
            }
        }
        Frame::Package(p) => {
            if let Some(rec) = state.on_package(&p, registry) {
                sink.push_package(rec)?;
                counts.packages += 1;
            }
        }
        Frame::Stub(s) => state.on_stub(s, registry),
        Frame::Symbol(s) => {
            state.on_symbol(s, registry, sink, counts)?;
        }
        Frame::Edge(e) => {
            if let Some(rec) = state.on_edge(&e) {
                sink.push_edge(rec)?;
                counts.edges += 1;
            }
        }
        Frame::Error(e) => note_error_frame(&e, counts),
        Frame::End(e) => {
            emit_ts_bench("end", &e.ts);
        }
    }
    Ok(())
}

/// Fold one `ErrorFrame` into the stats. Every frame counts toward
/// `errors`. Error-severity frames (and unknown severities — fail loud)
/// get a bounded attribution destined for `RunReport.failed_projects`;
/// warning-severity frames get one destined for `RunReport.warnings` —
/// producers promise these surface in `kenn status` (e.g. the Swift
/// stale-unit notices), so they must not die in a counter.
pub(crate) fn note_error_frame(e: &ErrorFrame, counts: &mut JsonlIngestStats) {
    counts.errors += 1;
    let attribution = || match &e.path {
        Some(p) => format!("{}: {p}: {}", e.source, e.message),
        None => format!("{}: {}", e.source, e.message),
    };
    if e.severity == crate::parse_jsonl::Severity::Warning {
        counts.warning_total += 1;
        if counts.warned.len() < super::JSONL_FAILED_ATTRIBUTION_CAP {
            counts.warned.push(attribution());
        }
        return;
    }
    counts.failed_errors += 1;
    if counts.failed.len() < super::JSONL_FAILED_ATTRIBUTION_CAP {
        counts.failed.push(attribution());
    }
}

/// Flush any stubs the registry buffered during ingest that never
/// received a full `SymbolFrame` upgrade. Typical cases: external
/// (standard library, vendored / third-party package) symbols whose
/// declaration is outside the workspace, and cross-document references
/// on the SCIP path that this run never saw a defining document for.
/// Call once per job, after the last per-unit ingest.
///
/// Every drained stub is by construction a symbol whose full
/// `SymbolFrame` never arrived, which is exactly the condition for "no
/// workspace definition exists" — so each pushed record is stamped
/// `external = true`. The JSONL/C# path's `pkg_external` plumbing
/// already tags *full* symbols correctly; this closes the stub-only
/// gap on both ingest paths. `mark_full_emitted` removes upgraded stubs
/// from `pending_stub_records` before drain, so a cross-document
/// workspace symbol that gets defined later in the same run is never
/// drained and therefore never mis-tagged.
pub fn flush_registry_stubs(
    registry: &mut IdRegistry,
    sink: &mut BatchSink,
) -> Result<u64, DbError> {
    let mut n: u64 = 0;
    let stubs: Vec<SymbolRecord> = registry.drain_pending_stubs().collect();
    for rec in stubs {
        sink.push_symbol(tag_drained_stub_external(rec))?;
        n += 1;
    }
    Ok(n)
}

/// Stamp `external = true` on a drained stub before persistence.
/// Extracted from [`flush_registry_stubs`] so the tagging invariant is
/// unit-testable without a real `BatchSink` (which requires a tokio
/// runtime and a Lance writer).
pub(crate) fn tag_drained_stub_external(mut rec: SymbolRecord) -> SymbolRecord {
    rec.external = true;
    rec
}

pub(crate) struct StreamState<'ws> {
    workspace: &'ws Workspace,
    language: Language,
    files: HashMap<Ref, ShortId>,
    /// Producer wire id → consumer package `ShortId`. Per-stream because
    /// wire ids are run-local; the underlying `(name, version)` intern
    /// lives on the shared `IdRegistry` so packages collapse across units.
    pkgs: HashMap<Ref, ShortId>,
    /// External flag per consumer package short id (denormalization source).
    pkg_external: HashMap<ShortId, bool>,
    /// Producer wire id → consumer symbol `ShortId`.
    syms: HashMap<Ref, ShortId>,
    /// Wire ids whose `(key, pkg_short)` was a cross-wire-id duplicate of a
    /// non-partial symbol — outgoing edges from these ids are skipped.
    dup_sym_wires: HashSet<Ref>,
}

impl<'ws> StreamState<'ws> {
    pub(crate) fn new(workspace: &'ws Workspace) -> Self {
        Self {
            workspace,
            language: Language::Csharp,
            files: HashMap::new(),
            pkgs: HashMap::new(),
            pkg_external: HashMap::new(),
            syms: HashMap::new(),
            dup_sym_wires: HashSet::new(),
        }
    }

    fn on_meta(&mut self, m: &MetaFrame) {
        if let Some(lang) = language_from_scip(&m.language) {
            self.language = lang;
        }
    }

    fn on_file(&mut self, f: &FileFrame, registry: &mut IdRegistry) -> Option<FileRecord> {
        let project_root_uri = format!("file://{}", self.workspace.root().display());
        let Ok(canon) = self.workspace.canonicalize(&project_root_uri, &f.path) else {
            return None;
        };
        let (short_id, is_new) = registry.intern_file_with_seen(canon.as_str());
        self.files.insert(f.id, short_id);
        if !is_new {
            return None;
        }
        let content_hash = u64::from_str_radix(&f.content_hash, 16).unwrap_or(0);
        Some(FileRecord {
            id: short_id,
            path: canon.into_string(),
            language: self.language,
            test: f.test,
            external: f.external,
            content_hash,
        })
    }

    fn on_package(&mut self, p: &PackageFrame, registry: &mut IdRegistry) -> Option<PackageRecord> {
        let version = p.version.clone().unwrap_or_default();
        let manager = p.manager.clone().unwrap_or_default();
        let (short_id, is_new) = registry.intern_package(&p.name, &version);
        self.pkgs.insert(p.id, short_id);
        // External flag is recorded on first sighting only; later sightings
        // of the same `(name, version)` retain the original.
        self.pkg_external.entry(short_id).or_insert(p.external);
        if !is_new {
            return None;
        }
        Some(PackageRecord {
            id: short_id,
            name: p.name.clone(),
            version,
            manager,
            external: p.external,
        })
    }

    fn pkg_short_for(&self, wire_pkg: Ref) -> ShortId {
        if wire_pkg == 0 {
            return 0;
        }
        self.pkgs.get(&wire_pkg).copied().unwrap_or(0)
    }

    fn pub_id_for(&self, key: &str) -> String {
        format!("{}:{}", self.language.prefix(), key)
    }

    /// Intern a symbol by `(language, pub_id, pkg_short)`. The intern
    /// table is salted with `pkg_short` so genuine multi-version
    /// duplicates get distinct rows.
    fn intern_symbol(
        &mut self,
        registry: &mut IdRegistry,
        wire_id: Ref,
        key: &str,
        pkg_short: ShortId,
    ) -> (ShortId, bool) {
        let pub_id = self.pub_id_for(key);
        let intern_key = if pkg_short == 0 {
            pub_id.clone()
        } else {
            format!("{pub_id}@{pkg_short}")
        };
        let (short_id, is_new) = registry.intern_with_pub_id(self.language, &intern_key, &pub_id);
        self.syms.insert(wire_id, short_id);
        (short_id, is_new)
    }

    fn on_stub(&mut self, s: StubFrame, registry: &mut IdRegistry) {
        if self.syms.contains_key(&s.id) {
            // Repeat stub for an already-known wire id — no-op.
            return;
        }
        let pkg_short = self.pkg_short_for(s.pkg);
        let (short_id, is_new) = self.intern_symbol(registry, s.id, &s.key, pkg_short);
        if !is_new {
            // Cross-wire-id stub — another wire id already buffered or
            // emitted a row for this (key, pkg). Just remap.
            return;
        }
        let kind = kind_from_str(&s.kind).unwrap_or(Kind::Variable);
        let external =
            pkg_short != 0 && self.pkg_external.get(&pkg_short).copied().unwrap_or(false);
        let rec = SymbolRecord {
            id: short_id,
            pub_id: crate::pubid::render(self.language, &self.pub_id_for(&s.key)),
            language: self.language,
            pkg_id: pkg_short,
            kind,
            name: s.name,
            enclosing_sym_id: 0,
            partial: false,
            nargs: 0,
            targs: 0,
            external,
            test: false,
        };
        // Buffer the stub on the SHARED registry so a SymbolFrame
        // arriving in a later stream can still upgrade it before the
        // bare stub row reaches the sink.
        registry.buffer_stub(short_id, rec);
    }

    fn on_symbol(
        &mut self,
        s: SymbolFrame,
        registry: &mut IdRegistry,
        sink: &mut BatchSink,
        stats: &mut JsonlIngestStats,
    ) -> Result<(), JsonlTransformError> {
        // Same-wire-id repeat (within this stream).
        if let Some(&short_id) = self.syms.get(&s.id) {
            if registry.take_pending_stub(short_id).is_some() {
                // Stub buffered earlier in this stream — upgrade now.
                self.emit_full(s, short_id, sink, registry, stats)?;
            }
            // Else: already-emitted full record under this wire id; ignore.
            return Ok(());
        }

        // New wire id. Intern by (key, pkg_short) on the shared registry.
        let pkg_short = self.pkg_short_for(s.pkg);
        let (short_id, is_new) = self.intern_symbol(registry, s.id, &s.key, pkg_short);
        if is_new {
            self.emit_full(s, short_id, sink, registry, stats)?;
            return Ok(());
        }

        // Cross-wire-id dedup. Three sub-cases:
        //   1. partial: true → legitimate additional declaration site.
        //      Append a defs row; the symbol row stays as-is.
        //   2. existing row was buffered as a stub (cross-stream upgrade
        //      candidate) → emit the full record + def now; the buffered
        //      stub never reached the sink, so no UPDATE is needed.
        //   3. existing row was already a full SymbolFrame → real
        //      duplicate; mark wire id so its outgoing edges are skipped.
        if s.partial {
            sink.push_def(self.def_for(&s, short_id))?;
            stats.defs += 1;
        } else if registry.take_pending_stub(short_id).is_some() {
            self.emit_full(s, short_id, sink, registry, stats)?;
        } else if registry.was_full_emitted(short_id) {
            self.dup_sym_wires.insert(s.id);
        } else {
            // Defensive: short_id is known to the registry but neither
            // buffered as a stub nor marked full-emitted. Treat as dup.
            self.dup_sym_wires.insert(s.id);
        }
        Ok(())
    }

    fn emit_full(
        &mut self,
        s: SymbolFrame,
        short_id: ShortId,
        sink: &mut BatchSink,
        registry: &mut IdRegistry,
        stats: &mut JsonlIngestStats,
    ) -> Result<(), JsonlTransformError> {
        let pkg_short = self.pkg_short_for(s.pkg);
        let parent_short = if s.parent == 0 {
            0
        } else {
            self.syms.get(&s.parent).copied().unwrap_or(0)
        };
        let kind = kind_from_str(&s.kind).unwrap_or(Kind::Variable);
        let external =
            pkg_short != 0 && self.pkg_external.get(&pkg_short).copied().unwrap_or(false);
        let docs = build_docs_record(short_id, s.sig.clone(), s.doc.clone());
        let def = self.def_for(&s, short_id);
        sink.push_symbol(SymbolRecord {
            id: short_id,
            pub_id: crate::pubid::render(self.language, &self.pub_id_for(&s.key)),
            language: self.language,
            pkg_id: pkg_short,
            kind,
            name: s.name,
            enclosing_sym_id: parent_short,
            partial: s.partial,
            nargs: s.nargs,
            targs: s.targs,
            external,
            test: s.test,
        })?;
        stats.symbols += 1;
        if let Some(d) = docs {
            sink.push_symbol_docs(d)?;
        }
        sink.push_def(def)?;
        stats.defs += 1;
        registry.mark_full_emitted(short_id);
        Ok(())
    }

    fn def_for(&self, s: &SymbolFrame, short_id: ShortId) -> DefRecord {
        let to_u32 = |v: i64| -> u32 { u32::try_from(v.max(0)).unwrap_or(0) };
        let file_id = if s.file == 0 {
            0
        } else {
            self.files.get(&s.file).copied().unwrap_or(0)
        };
        // Wire `def_range` is 0-based per dotnet-stream-indexer; the store
        // is 1-based per source-data-model. Convert lines on ingest; columns
        // pass through. Synthetic symbols (all-zero range) bypass the +1
        // so they stay `[0,0,0,0]` — null-location carve-out.
        let is_synthetic = s.range == [0, 0, 0, 0];
        let (start_line, end_line) = if is_synthetic {
            (0, 0)
        } else {
            (to_u32(s.range[0]) + 1, to_u32(s.range[2]) + 1)
        };
        // Optional body span (whole declaration incl. doc comment / attributes),
        // 0-based lines `+1` like the name span. Absent → 0, and `get_source`
        // falls back to the name span. Never derived for synthetic symbols.
        let (body_start_line, body_end_line) = match s.body {
            Some(b) if !is_synthetic => (to_u32(b[0]) + 1, to_u32(b[2]) + 1),
            _ => (0, 0),
        };
        DefRecord {
            sym_id: short_id,
            file_id,
            start_line,
            start_col: to_u32(s.range[1]),
            end_line,
            end_col: to_u32(s.range[3]),
            body_start_line,
            body_end_line,
        }
    }

    fn on_edge(&self, e: &EdgeFrame) -> Option<EdgeRecord> {
        if self.dup_sym_wires.contains(&e.source) {
            return None;
        }
        let source = self.syms.get(&e.source).copied()?;
        let target = self
            .syms
            .get(&e.target)
            .copied()
            .or_else(|| self.files.get(&e.target).copied())?;
        let props = edge_properties(&e.edge_kind, e.field_op.as_deref())?;
        Some(EdgeRecord {
            src_id: source,
            target_id: target,
            properties: props,
        })
    }
}

#[cfg(test)]
mod error_frame_tests {
    use super::{note_error_frame, ErrorFrame};
    use crate::parse_jsonl::Severity;
    use crate::transform_jsonl::{JsonlIngestStats, JSONL_FAILED_ATTRIBUTION_CAP};

    fn frame(severity: Severity, path: Option<&str>) -> ErrorFrame {
        ErrorFrame {
            severity,
            source: "msbuild".into(),
            message: "load failed".into(),
            path: path.map(Into::into),
            range: None,
            code: None,
        }
    }

    #[test]
    fn error_severity_records_attribution_with_path() {
        let mut counts = JsonlIngestStats::default();
        note_error_frame(&frame(Severity::Error, Some("App.sln")), &mut counts);
        assert_eq!(counts.errors, 1);
        assert_eq!(counts.failed_errors, 1);
        assert_eq!(counts.failed, vec!["msbuild: App.sln: load failed"]);
    }

    #[test]
    fn warning_severity_is_captured_not_failed() {
        let mut counts = JsonlIngestStats::default();
        note_error_frame(&frame(Severity::Warning, Some("App.sln")), &mut counts);
        assert_eq!(counts.errors, 1);
        assert_eq!(counts.failed_errors, 0);
        assert!(counts.failed.is_empty());
        // Warnings are attributed, not dropped — producers promise they
        // surface in `kenn status` (e.g. Swift stale-unit notices).
        assert_eq!(counts.warning_total, 1);
        assert_eq!(counts.warned, vec!["msbuild: App.sln: load failed"]);
    }

    #[test]
    fn unknown_severity_fails_loud() {
        let mut counts = JsonlIngestStats::default();
        note_error_frame(&frame(Severity::Other, Some("App.sln")), &mut counts);
        assert_eq!(
            counts.failed_errors, 1,
            "unknown severity attributes like an error"
        );
    }

    #[test]
    fn severity_parses_case_insensitively() {
        let err: ErrorFrame =
            serde_json::from_str(r#"{"severity":"Error","source":"x","message":"m"}"#).unwrap();
        assert_eq!(err.severity, Severity::Error);
        let warn: ErrorFrame =
            serde_json::from_str(r#"{"severity":"WARNING","source":"x","message":"m"}"#).unwrap();
        assert_eq!(warn.severity, Severity::Warning);
        let other: ErrorFrame =
            serde_json::from_str(r#"{"severity":"fatal","source":"x","message":"m"}"#).unwrap();
        assert_eq!(other.severity, Severity::Other);
    }

    #[test]
    fn attributions_are_capped() {
        let mut counts = JsonlIngestStats::default();
        for _ in 0..(JSONL_FAILED_ATTRIBUTION_CAP + 8) {
            note_error_frame(&frame(Severity::Error, None), &mut counts);
        }
        assert_eq!(counts.failed.len(), JSONL_FAILED_ATTRIBUTION_CAP);
        assert_eq!(
            counts.failed_errors,
            (JSONL_FAILED_ATTRIBUTION_CAP + 8) as u64
        );
        assert_eq!(counts.failed[0], "msbuild: load failed");
    }
}

#[cfg(test)]
mod drain_tag_tests {
    use super::tag_drained_stub_external;
    use crate::transform::{intern_symbol_with_stub, IdRegistry};
    use kenn_model::Language;

    /// Task §2.2 — every drained stub gets `external = true` before
    /// reaching the sink. Mirrors the registry-only test pattern in
    /// `transform::tests::mark_full_emitted_drops_pending_stub_for_same_short_id`:
    /// drains directly from the registry and applies the same per-record
    /// transform `flush_registry_stubs` does, avoiding a real `BatchSink`.
    #[test]
    fn drained_stub_is_tagged_external() {
        let mut r = IdRegistry::new(Language::Rust);
        // Intern a SCIP-shaped external symbol; no full SymbolFrame
        // will arrive for it during this fake run.
        let _id = intern_symbol_with_stub(
            &mut r,
            Language::Rust,
            "rust-analyzer cargo core 0.0 result/Result#unwrap().",
        );

        let drained: Vec<_> = r
            .drain_pending_stubs()
            .map(tag_drained_stub_external)
            .collect();

        assert_eq!(drained.len(), 1, "expected exactly one drained stub");
        assert!(
            drained[0].external,
            "flush_registry_stubs must tag drained stubs external=true"
        );
    }

    /// Task §2.3 — a stub upgraded mid-run by `mark_full_emitted` is NOT
    /// drained, so the drain-time external tag never applies to it. The
    /// full `SymbolFrame` path is responsible for persisting that symbol
    /// with `external = false` (from its workspace definition).
    #[test]
    fn upgraded_symbol_is_not_tagged_at_drain() {
        let mut r = IdRegistry::new(Language::Rust);
        let scip = "rust-analyzer cargo k 0.1 m/CrossDocSym#";
        let id = intern_symbol_with_stub(&mut r, Language::Rust, scip);

        // Later document in the same run provides the defining SymbolFrame
        // and the production callsite invokes `mark_full_emitted`.
        r.mark_full_emitted(id);

        let drained: Vec<_> = r
            .drain_pending_stubs()
            .map(tag_drained_stub_external)
            .collect();

        assert!(
            drained.iter().all(|s| s.id != id),
            "mark_full_emitted must remove the stub before drain — \
             otherwise it would be mis-tagged external=true",
        );
    }
}
