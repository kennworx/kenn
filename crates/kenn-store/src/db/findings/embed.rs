//! The findings embed pass — derive vectors for committed finding records
//! into the findings sidecar (the model-keyed generation dir under
//! `<vectors_root>`).
//!
//! Records are the source of truth; embeddings are *derived*, so they live
//! in a content-addressed sidecar (not the records) keyed by the finding
//! text fingerprint. `embed_findings` runs alongside the code embed pass
//! (`db::jobs::embed_pending`) under the same per-snapshot lock.

use std::collections::HashSet;

use crate::api::types::DbError;
use crate::embed::sidecar;
use crate::layout::Layout;

use super::record;

/// `EmbeddingGemma` full dimension — matches the `vec0 float[768]` schema.
const EMBED_DIM: u32 = 768;

/// Embed every committed finding whose text is not yet in the findings
/// sidecar, appending the new vectors. Returns the number embedded. A
/// clean no-op when the embedder is disabled or every finding is already
/// covered.
pub(crate) async fn embed_findings(
    layout: &Layout,
    model_id: &str,
    embedder: &kenn_embed::SharedEmbedder,
) -> Result<usize, DbError> {
    let records_dir = layout.findings_dir();
    // Writes target the model-keyed generation dir; the legacy flat dir
    // still serves reads so pre-generation vectors are reused, not
    // re-embedded.
    let vectors_dir = sidecar::findings_generation_dir(layout, model_id);
    let legacy_dir = layout.findings_vectors_dir();
    let (findings, _, _) = record::read_records(&records_dir)?;
    if findings.is_empty() {
        return Ok(0);
    }

    let existing = sidecar::Manifest::read(&vectors_dir)?;
    let cached = sidecar::load_reuse_map_with_legacy(
        &vectors_dir,
        Some(&legacy_dir),
        model_id,
        EMBED_DIM,
        sidecar::FINDING_TEXT_RECIPE,
    )?;
    let mut seen: HashSet<u64> = HashSet::new();
    let mut pending: Vec<(u64, &str)> = Vec::new();
    for f in &findings {
        let fp = sidecar::fingerprint(&f.text);
        if cached.contains_key(&fp) || !seen.insert(fp) {
            continue;
        }
        pending.push((fp, f.text.as_str()));
    }
    if pending.is_empty() {
        return Ok(0);
    }

    let texts: Vec<&str> = pending.iter().map(|(_, t)| *t).collect();
    let Some(vectors) = embedder.embed_block_until_ready(&texts).await? else {
        return Ok(0);
    };
    if vectors.len() != pending.len() {
        return Err(DbError::Backend(format!(
            "embedder returned {} vectors for {} findings",
            vectors.len(),
            pending.len()
        )));
    }

    let entries: Vec<(u64, Vec<f32>)> = pending.iter().map(|(fp, _)| *fp).zip(vectors).collect();
    let tmp = layout.sidecar_tmp_dir();
    sidecar::append_vectors(
        &vectors_dir,
        &tmp,
        sidecar::WriterPrefix::Seg,
        EMBED_DIM,
        &entries,
    )?;
    if existing.is_none() {
        sidecar::Manifest::current(model_id.to_owned(), EMBED_DIM, sidecar::FINDING_TEXT_RECIPE)
            .write(&vectors_dir)?;
    }
    Ok(entries.len())
}
