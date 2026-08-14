//! [`FindingsStore`] — the durable findings store (committed-findings).
//!
//! Findings are committed as per-finding `.kenn/findings/<id>.md` record
//! files — the source of truth ([`record`](super::record)). Search is
//! served from a **persistent** `SQLite` FTS5 index ([`index`]) in the
//! run's derived store (`<derived_root>/findings.db`): built from the
//! committed records at [`open`](FindingsStore::open) and maintained on
//! [`flush`](FindingsStore::flush). `search_findings` only *queries* it —
//! it never builds an index on the read path.
//!
//! Supersede / tombstone tag conventions (resolved by `search_findings`,
//! ignored by `get_finding`):
//! - Correction: new finding tags `supersedes:<old_id>` + `<old_id>` parent.
//! - Deletion: tombstone finding tags `tombstone:<target_id>` + parent.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::api::types::{DbError, Finding, FindingHit};
use crate::embed::sidecar;
use crate::layout::Layout;

use super::anchor::{self, Anchor, AnchorEvent};
use super::directives::{
    self, AnchorHealth, BrokenAnchors, DirectiveInput, DriftedAnchors, UnverifiedClaim,
};
use super::index;
use super::lifecycle::{
    carries_lifecycle_tag, finding_is_stale, is_directive_or_guide, lifecycle_sets,
    CodeNodeResolver, FINDING_ID_PREFIX,
};
use super::record;

/// `EmbeddingGemma` full dimension — matches the findings `vec0 float[768]`.
const EMBED_DIM: u32 = 768;
/// Cosine similarity at/above which `store_finding` flags a prior finding
/// as a near-duplicate worth surfacing to the author.
const NEAR_DUP_THRESHOLD: f32 = 0.82;
/// Most near-duplicates `store_finding` surfaces.
const NEAR_DUP_LIMIT: usize = 5;
/// Minimum cosine for a directive's body to enter `find_directives`' semantic
/// arm — below this a query match is noise (tuning knob; see design).
const DIRECTIVE_SEMANTIC_MIN: f32 = 0.5;

/// True iff `anchor` resolves to a file whose current content hash differs from
/// the sha recorded at attach. A missing path, a directory, an unreadable file,
/// and an anchor with no recorded sha are all *not* drift (the first is broken;
/// the rest are live / drift-unknown).
fn anchor_drifted(source_root: &Path, anchor: &Anchor) -> bool {
    let Some(recorded) = &anchor.sha else {
        return false;
    };
    let abs = source_root.join(&anchor.path);
    matches!(crate::staleness::file_content_sha(&abs), Some(current) if &current != recorded)
}

/// Cosine similarity of two equal-length vectors; `0.0` on a length
/// mismatch or a zero vector.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|y| y * y).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// The durable findings store — committed `<id>.md` records plus an
/// in-memory pending buffer. Search queries the persistent
/// `<derived_root>/findings.db` index ([`index`]); a bounded result set is
/// then resolved back to full records.
pub struct FindingsStore {
    records_dir: PathBuf,
    /// The findings vector sidecar's current **generation** dir (keyed by
    /// the configured model) — derived `fingerprint → vector`, filled by
    /// the findings embed pass.
    vectors_dir: PathBuf,
    /// The pre-generation flat sidecar (`<vectors_root>/findings/`) —
    /// still read as a reuse fallback for the same generation.
    legacy_vectors_dir: PathBuf,
    /// The configured embedding model id — gates every sidecar read so a
    /// vector from another model is never compared against this model's
    /// query vectors.
    model_id: String,
    /// The workspace root — anchor paths are resolved against it by
    /// `check_anchors`.
    source_root: PathBuf,
    /// The persistent search index (`<derived_root>/findings.db`) — built
    /// at open from the committed records, maintained on writes, queried by
    /// `search_findings`. Derived/local, never committed. See [`index`].
    index_db: PathBuf,
    pending: Vec<Finding>,
}

impl FindingsStore {
    /// Open the findings store for `layout`.
    #[expect(
        clippy::unused_async,
        reason = "kept async for API symmetry with the async store surface"
    )]
    pub async fn open(layout: &Layout) -> Result<Self, DbError> {
        let records_dir = layout.findings_dir();
        std::fs::create_dir_all(&records_dir).map_err(DbError::Io)?;
        record::ensure_tmp_dir(&records_dir)?;
        record::sweep_legacy_tmp(&records_dir);
        crate::layout::write_gitignore(layout)
            .map_err(|e| DbError::Backend(format!("write .gitignore: {e}")))?;
        // Build the persistent search index from the committed records at
        // open (a startup build, never on a read). Derived/local store.
        let index_db = layout.derived_root().join(crate::db::names::FINDINGS_DB);
        let (committed, _, _) = record::read_records(&records_dir)?;
        index::rebuild(&index_db, &committed)?;
        let model_id = sidecar::current_model_id();
        Ok(Self {
            records_dir,
            vectors_dir: sidecar::findings_generation_dir(layout, &model_id),
            legacy_vectors_dir: layout.findings_vectors_dir(),
            model_id,
            source_root: layout.source_root().to_path_buf(),
            index_db,
            pending: Vec::new(),
        })
    }

    /// Open the findings store with the in-repo default layout.
    pub async fn open_default(workspace: &Path) -> Result<Self, DbError> {
        Self::open(&Layout::default_for(workspace)).await
    }

    /// All committed records plus the pending buffer.
    fn all_findings(&self) -> Result<Vec<Finding>, DbError> {
        let (mut all, _, _) = record::read_records(&self.records_dir)?;
        all.extend(self.pending.iter().cloned());
        Ok(all)
    }

    /// The committed `fingerprint → vector` reuse map for the current
    /// generation — generation dir unioned with the legacy flat dir.
    fn reuse_map(&self) -> Result<HashMap<u64, crate::embed::sidecar::QuantVector>, DbError> {
        sidecar::load_reuse_map_with_legacy(
            &self.vectors_dir,
            Some(&self.legacy_vectors_dir),
            &self.model_id,
            EMBED_DIM,
            sidecar::FINDING_TEXT_RECIPE,
        )
    }

    // ── store / drop / flush ────────────────────────────────────────

    /// Store a finding. When `text_vec` is supplied, `similar` carries the
    /// committed findings whose embedding is a near-duplicate of the new
    /// text (cosine ≥ [`NEAR_DUP_THRESHOLD`]) — the author decides whether
    /// to merge. Empty when no vector is supplied or none are close.
    #[expect(
        clippy::unused_async,
        reason = "kept async for a stable findings API; delegates to synchronous record-file I/O"
    )]
    pub async fn store_finding(
        &mut self,
        text: String,
        parent_ids: Vec<String>,
        tags: Vec<String>,
        text_vec: Option<&[f32]>,
    ) -> Result<(String, Vec<Finding>), DbError> {
        let similar = match text_vec {
            Some(v) => self.near_duplicates(v)?,
            None => Vec::new(),
        };
        let id = self.push_finding(text, parent_ids, tags);
        Ok((id, similar))
    }

    /// Committed findings whose sidecar vector is a near-duplicate of
    /// `query_vec`, ranked by descending cosine (superseded / tombstoned
    /// findings excluded). Empty when the findings sidecar is absent.
    fn near_duplicates(&self, query_vec: &[f32]) -> Result<Vec<Finding>, DbError> {
        let cached = self.reuse_map()?;
        if cached.is_empty() {
            return Ok(Vec::new());
        }
        let (committed, _, _) = record::read_records(&self.records_dir)?;
        let (superseded, tombstoned) = lifecycle_sets(&committed);
        let mut scored: Vec<(f32, Finding)> = Vec::new();
        for f in committed {
            if superseded.contains(&f.id)
                || tombstoned.contains(&f.id)
                || carries_lifecycle_tag(&f, "tombstone:")
            {
                continue;
            }
            let Some(qv) = cached.get(&sidecar::fingerprint(&f.text)) else {
                continue;
            };
            let sim = cosine(query_vec, &qv.dequantize());
            if sim >= NEAR_DUP_THRESHOLD {
                scored.push((sim, f));
            }
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        Ok(scored
            .into_iter()
            .take(NEAR_DUP_LIMIT)
            .map(|(_, f)| f)
            .collect())
    }

    fn push_finding(&mut self, text: String, parent_ids: Vec<String>, tags: Vec<String>) -> String {
        let id = format!("{FINDING_ID_PREFIX}{}", Uuid::new_v4());
        self.pending.push(Finding {
            id: id.clone(),
            text,
            embedding: None,
            tags,
            parent_ids,
            created_at: crate::clock::Timestamp::now(),
        });
        id
    }

    /// Remove a not-yet-flushed finding from the pending buffer.
    pub fn drop_pending(&mut self, id: &str) {
        self.pending.retain(|f| f.id != id);
    }

    /// Commit every pending finding to its `<id>.md` record.
    #[expect(
        clippy::unused_async,
        reason = "kept async for a stable findings API; delegates to synchronous record-file I/O"
    )]
    pub async fn flush(&mut self) -> Result<(), DbError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        for finding in &self.pending {
            record::write_record(&self.records_dir, finding)?;
            // Maintain the persistent search index alongside the committed
            // record (supersede/tombstone tags update their targets' flags),
            // so `search_findings` never rebuilds an index on the read path.
            index::insert(&self.index_db, finding)?;
            // A correction inherits the superseded finding's anchors so it stays
            // reachable by `find_directives` (anchors-store, supersede seeding).
            if let Some(predecessor) = finding
                .tags
                .iter()
                .find_map(|t| t.strip_prefix("supersedes:"))
            {
                let ts = crate::clock::Timestamp::now();
                anchor::seed_from_predecessor(&self.records_dir, predecessor, &finding.id, ts)?;
            }
        }
        record::fsync_dir(&self.records_dir)?;
        self.pending.clear();
        Ok(())
    }

    // ── anchors (forward applicability + liveness) ───────────────────

    /// Append an anchor event to `finding_id`'s `<id>.anchor.jsonl` log
    /// (`attach` / `rename` / `detach`). A repeat `attach` to a path already in
    /// the set is the liveness signal.
    #[expect(
        clippy::unused_async,
        reason = "kept async for a stable findings API; delegates to synchronous append I/O"
    )]
    pub async fn record_anchor_event(
        &self,
        finding_id: &str,
        event: &AnchorEvent,
    ) -> Result<(), DbError> {
        anchor::append_event(&self.records_dir, finding_id, event)
    }

    /// The current anchor set for `finding_id`, folded from its log, with
    /// per-anchor recency and attach-count. Empty when the finding has no log.
    #[expect(
        clippy::unused_async,
        reason = "kept async for a stable findings API; delegates to synchronous record-file I/O"
    )]
    pub async fn anchors_for(&self, finding_id: &str) -> Result<Vec<Anchor>, DbError> {
        Ok(anchor::fold(&anchor::read_log(
            &self.records_dir,
            finding_id,
        )?))
    }

    /// Retrieve directives/guides relevant to `paths` (changed files/dirs),
    /// ranked by RRF of a structural (anchor) leg and, when `query_vec` is
    /// supplied and the embedder is warm, a semantic (body-vector) leg —
    /// boosted by recency-weighted anchor liveness at `now_ts`. Superseded and
    /// tombstoned findings are excluded; each hit carries the read-time `stale`
    /// flag. With no `query_vec` (or a cold embedder) ranking degrades to the
    /// structural leg alone — anchors are committed and resolvable without an
    /// index.
    #[expect(
        clippy::unused_async,
        reason = "kept async for a stable findings API; delegates to synchronous record-file I/O"
    )]
    pub async fn find_directives(
        &self,
        paths: &[String],
        query_vec: Option<&[f32]>,
        now: crate::clock::Timestamp,
        limit: usize,
        resolver: &impl CodeNodeResolver,
    ) -> Result<Vec<FindingHit>, DbError> {
        let all = self.all_findings()?;
        if all.is_empty() {
            return Ok(Vec::new());
        }
        let (superseded, tombstoned) = lifecycle_sets(&all);
        let cached = if query_vec.is_some() {
            self.reuse_map()?
        } else {
            HashMap::new()
        };
        let mut inputs = Vec::new();
        for finding in all {
            if !is_directive_or_guide(&finding)
                || superseded.contains(&finding.id)
                || tombstoned.contains(&finding.id)
                || carries_lifecycle_tag(&finding, "tombstone:")
            {
                continue;
            }
            let anchors = anchor::fold(&anchor::read_log(&self.records_dir, &finding.id)?);
            // Only a genuinely-similar body enters the semantic arm — without a
            // floor, a query would pull in every vectored directive at near-zero
            // cosine and flood the results.
            let semantic = match (query_vec, cached.get(&sidecar::fingerprint(&finding.text))) {
                (Some(qv), Some(bv)) => {
                    Some(cosine(qv, &bv.dequantize())).filter(|c| *c >= DIRECTIVE_SEMANTIC_MIN)
                }
                _ => None,
            };
            inputs.push(DirectiveInput {
                finding,
                anchors,
                semantic,
            });
        }
        let mut hits = directives::rank_directives(&inputs, paths, now, limit, resolver);
        // Drift needs file I/O (the workspace root + hashing), kept out of the
        // pure ranking. Flag a hit drifted if any of the finding's file anchors
        // changed content since attach.
        let anchors_by_id: HashMap<&str, &[Anchor]> = inputs
            .iter()
            .map(|i| (i.finding.id.as_str(), i.anchors.as_slice()))
            .collect();
        for hit in &mut hits {
            if let Some(anchors) = anchors_by_id.get(hit.finding.id.as_str()) {
                hit.drifted = anchors.iter().any(|a| anchor_drifted(&self.source_root, a));
            }
        }
        Ok(hits)
    }

    /// Report committed findings whose anchors no longer resolve on disk (file
    /// or directory paths tested against the workspace root), so an agent can
    /// repair moves/deletions before a commit. v1 anchors are paths, so this
    /// needs only the filesystem, not the index.
    #[expect(
        clippy::unused_async,
        reason = "kept async for a stable findings API; delegates to synchronous record-file I/O"
    )]
    pub async fn check_anchors(&self) -> Result<AnchorHealth, DbError> {
        let (all, _, _) = record::read_records(&self.records_dir)?;
        // Superseded and tombstoned findings are already excluded from
        // retrieval, so they can never be served as guidance and repairing their
        // anchors changes nothing. Reporting them is pure noise — measured on
        // this repository, 26 of 127 drifted entries were superseded ancestors.
        let (superseded, tombstoned) = super::lifecycle::lifecycle_sets(&all);
        let mut health = AnchorHealth::default();
        for finding in all {
            if superseded.contains(&finding.id) || tombstoned.contains(&finding.id) {
                continue;
            }
            let mut broken = Vec::new();
            let mut changed = Vec::new();
            for anchor in anchor::fold(&anchor::read_log(&self.records_dir, &finding.id)?) {
                if self.source_root.join(&anchor.path).exists() {
                    if anchor_drifted(&self.source_root, &anchor) {
                        changed.push(anchor.path);
                    }
                } else {
                    broken.push(anchor.path);
                }
            }
            if !broken.is_empty() {
                health.broken.push(BrokenAnchors {
                    finding_id: finding.id.clone(),
                    anchors: broken,
                });
            }
            if !changed.is_empty() {
                // Same observation, different question. For a rule, changed
                // content means "re-read before relying on it". For a claim it
                // means the assertion may have stopped being true while still
                // being served as fact, which is the failure worth a bucket.
                if super::lifecycle::is_claim(&finding) {
                    health.unverified.push(UnverifiedClaim {
                        finding_id: finding.id,
                        anchors: changed,
                    });
                } else {
                    health.drifted.push(DriftedAnchors {
                        finding_id: finding.id,
                        anchors: changed,
                    });
                }
            }
        }
        Ok(health)
    }

    // ── retrieval ───────────────────────────────────────────────────

    /// Return the finding with `id` (pending first, then records), raw
    /// regardless of supersede / tombstone state.
    #[expect(
        clippy::unused_async,
        reason = "kept async for a stable findings API; delegates to synchronous record-file I/O"
    )]
    pub async fn get_finding(&self, id: &str) -> Result<Option<Finding>, DbError> {
        if let Some(f) = self.pending.iter().find(|f| f.id == id) {
            return Ok(Some(f.clone()));
        }
        let (all, _, _) = record::read_records(&self.records_dir)?;
        Ok(all.into_iter().find(|f| f.id == id))
    }

    /// Blended search over committed findings' `text` (records + pending):
    /// a BM25 arm over the text plus, when `query_vec` is supplied, a vector
    /// arm blending cosine similarity over the committed findings sidecar —
    /// so a paraphrase with no shared terms still surfaces the right finding.
    /// Superseded / tombstoned findings and tombstone findings themselves are
    /// excluded; each hit is flagged stale by [`finding_is_stale`].
    #[expect(
        clippy::unused_async,
        reason = "kept async for a stable findings API; delegates to synchronous record-file I/O"
    )]
    pub async fn search_findings(
        &self,
        query: &str,
        query_vec: Option<&[f32]>,
        limit: usize,
        resolver: &impl CodeNodeResolver,
    ) -> Result<Vec<FindingHit>, DbError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let pool = limit.saturating_mul(8).max(64);

        // Lexical arm: bounded BM25 candidates from the PERSISTENT index
        // (`<derived_root>/findings.db`, built at open / maintained on
        // flush). Lifecycle filtering is applied in SQL. No read-path index
        // build.
        let mut scores: HashMap<String, f32> = HashMap::new();
        for (id, score) in index::search_lexical(&self.index_db, query, pool)? {
            scores.insert(id, score);
        }

        // Vector arm: blend cosine similarity over the committed findings
        // sidecar, surfacing paraphrases the BM25 arm misses (no shared
        // term). Candidates come from the persisted index's live rows — a
        // read, not a build. (An ANN index that bounds this scan is a future
        // refinement.)
        if let Some(query_vec) = query_vec {
            let cached = self.reuse_map()?;
            if !cached.is_empty() {
                for (id, text) in index::live_records(&self.index_db)? {
                    if let Some(qv) = cached.get(&sidecar::fingerprint(&text)) {
                        let sim = cosine(query_vec, &qv.dequantize());
                        *scores.entry(id).or_default() += sim;
                    }
                }
            }
        }

        if scores.is_empty() {
            return Ok(Vec::new());
        }
        // Rank in-memory, then resolve ONLY the top-`limit` records — a
        // bounded read, not a full-corpus load. (Ordering matches
        // `sort_hits`: score desc, ties broken by id.)
        let mut ranked: Vec<(String, f32)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        ranked.truncate(limit);
        let mut hits: Vec<FindingHit> = Vec::new();
        for (id, score) in ranked {
            // Pending (unflushed) findings live only in memory; committed
            // ones are resolved by id from their record file. The index
            // already excluded superseded/tombstoned, so a resolution miss
            // just means the record is gone — skip it.
            let Some(finding) = self
                .pending
                .iter()
                .find(|f| f.id == id)
                .cloned()
                .or_else(|| record::read_record(&self.records_dir, &id))
            else {
                continue;
            };
            let stale = finding_is_stale(&finding, resolver);
            hits.push(FindingHit {
                finding,
                score,
                stale,
                // Lexical finding search has no anchor/workspace context.
                drifted: false,
            });
        }
        Ok(hits)
    }

    // ── derivation DAG ──────────────────────────────────────────────

    /// Transitively collect ids reachable from `id` through `parent_ids`.
    #[expect(
        clippy::unused_async,
        reason = "kept async for a stable findings API; delegates to synchronous record-file I/O"
    )]
    pub async fn find_predecessors(&self, id: &str) -> Result<Vec<String>, DbError> {
        let all = self.all_findings()?;
        let by_id: HashMap<&str, &Finding> = all.iter().map(|f| (f.id.as_str(), f)).collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = Vec::new();
        if let Some(start) = by_id.get(id) {
            stack.extend(start.parent_ids.iter().cloned());
        }
        while let Some(node) = stack.pop() {
            if !seen.insert(node.clone()) {
                continue;
            }
            if node.starts_with(FINDING_ID_PREFIX) {
                if let Some(parent) = by_id.get(node.as_str()) {
                    stack.extend(parent.parent_ids.iter().cloned());
                }
            }
        }
        let mut out: Vec<String> = seen.into_iter().collect();
        out.sort();
        Ok(out)
    }

    /// Transitively collect ids of findings that derive from `id`.
    #[expect(
        clippy::unused_async,
        reason = "kept async for a stable findings API; delegates to synchronous record-file I/O"
    )]
    pub async fn find_successors(&self, id: &str) -> Result<Vec<String>, DbError> {
        let all = self.all_findings()?;
        let mut reached: HashSet<String> = HashSet::new();
        reached.insert(id.to_owned());
        loop {
            let mut grew = false;
            for f in &all {
                if reached.contains(&f.id) {
                    continue;
                }
                if f.parent_ids.iter().any(|p| reached.contains(p)) {
                    reached.insert(f.id.clone());
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        reached.remove(id);
        let mut out: Vec<String> = reached.into_iter().collect();
        out.sort();
        Ok(out)
    }

    /// Synthesize a new finding from several inputs.
    pub fn merge_findings(
        &mut self,
        input_ids: Vec<String>,
        text: String,
        tags: Vec<String>,
    ) -> String {
        self.push_finding(text, input_ids, tags)
    }
}

#[cfg(test)]
mod tests {
    use super::super::lifecycle::{finding_is_stale, CodeNodeResolver};
    use super::FindingsStore;
    use crate::api::types::Finding;
    use std::collections::HashSet;

    struct SetResolver(HashSet<String>);
    impl CodeNodeResolver for SetResolver {
        fn contains(&self, code_node_id: &str) -> bool {
            self.0.contains(code_node_id)
        }
    }
    fn resolver(ids: &[&str]) -> SetResolver {
        SetResolver(ids.iter().map(|s| (*s).to_owned()).collect())
    }
    fn finding_with_parents(parents: &[&str]) -> Finding {
        Finding {
            id: "fnd_x".to_owned(),
            text: "t".to_owned(),
            embedding: None,
            tags: vec![],
            parent_ids: parents.iter().map(|s| (*s).to_owned()).collect(),
            created_at: time::OffsetDateTime::from_unix_timestamp(1_700_000_000)
                .unwrap()
                .into(),
        }
    }
    async fn store(workspace: &std::path::Path) -> FindingsStore {
        FindingsStore::open_default(workspace).await.unwrap()
    }

    #[test]
    fn stale_when_a_code_parent_is_missing() {
        let f = finding_with_parents(&["rust:foo::bar", "fnd_other"]);
        assert!(finding_is_stale(&f, &resolver(&[])));
        assert!(!finding_is_stale(&f, &resolver(&["rust:foo::bar"])));
    }

    #[test]
    fn not_stale_without_code_parents() {
        let f = finding_with_parents(&["fnd_a", "fnd_b"]);
        assert!(!finding_is_stale(&f, &resolver(&[])));
    }

    #[tokio::test]
    async fn store_drop_flush_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(dir.path()).await;
        let (keep, _) = s
            .store_finding(
                "the lexer skips bom".to_owned(),
                vec!["rust:lex::scan".to_owned()],
                vec!["gotcha".to_owned()],
                None,
            )
            .await
            .unwrap();
        let (drop, _) = s
            .store_finding("noise".to_owned(), vec![], vec![], None)
            .await
            .unwrap();
        s.drop_pending(&drop);
        s.flush().await.unwrap();

        let got = s.get_finding(&keep).await.unwrap().unwrap();
        assert_eq!(got.text, "the lexer skips bom");
        assert_eq!(got.tags, vec!["gotcha".to_owned()]);
        assert!(s.get_finding(&drop).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn keyword_searchable_after_flush() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(dir.path()).await;
        let (id, _) = s
            .store_finding(
                "zphixspike borrow checker invariant".to_owned(),
                vec![],
                vec![],
                None,
            )
            .await
            .unwrap();
        s.flush().await.unwrap();
        let hits = s
            .search_findings("zphixspike", None, 10, &resolver(&[]))
            .await
            .unwrap();
        assert!(hits.iter().any(|h| h.finding.id == id));
    }

    #[tokio::test]
    async fn predecessors_and_successors() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(dir.path()).await;
        let (base, _) = s
            .store_finding(
                "base".to_owned(),
                vec!["rust:mod::leaf".to_owned()],
                vec![],
                None,
            )
            .await
            .unwrap();
        let (derived, _) = s
            .store_finding("derived".to_owned(), vec![base.clone()], vec![], None)
            .await
            .unwrap();
        s.flush().await.unwrap();
        let preds = s.find_predecessors(&derived).await.unwrap();
        assert!(preds.contains(&base));
        assert!(preds.contains(&"rust:mod::leaf".to_owned()));
        assert_eq!(s.find_successors(&base).await.unwrap(), vec![derived]);
    }

    #[tokio::test]
    async fn search_excludes_superseded_and_tombstoned() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(dir.path()).await;
        let (original, _) = s
            .store_finding(
                "the cache evicts on every write".to_owned(),
                vec![],
                vec![],
                None,
            )
            .await
            .unwrap();
        let (target, _) = s
            .store_finding(
                "the cache evicts on read too".to_owned(),
                vec![],
                vec![],
                None,
            )
            .await
            .unwrap();
        s.flush().await.unwrap();
        s.store_finding(
            "the cache evicts on write and read".to_owned(),
            vec![original.clone()],
            vec![format!("supersedes:{original}")],
            None,
        )
        .await
        .unwrap();
        s.store_finding(
            "removed".to_owned(),
            vec![target.clone()],
            vec![format!("tombstone:{target}")],
            None,
        )
        .await
        .unwrap();
        s.flush().await.unwrap();

        let hits = s
            .search_findings("cache evicts", None, 10, &resolver(&[]))
            .await
            .unwrap();
        let ids: HashSet<&str> = hits.iter().map(|h| h.finding.id.as_str()).collect();
        assert!(!ids.contains(original.as_str()), "superseded hidden");
        assert!(!ids.contains(target.as_str()), "tombstoned hidden");
        assert!(s.get_finding(&original).await.unwrap().is_some());
    }
}
