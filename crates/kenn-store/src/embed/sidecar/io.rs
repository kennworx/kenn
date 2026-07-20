//! Sidecar file IO — writers (`append_vectors`, `promote_segs_to_packs`),
//! reader (`load_vectors`, `load_reuse_map`), and the atomic-write
//! helper that backs both. `WriterPrefix` distinguishes the producer
//! role; pack/seg files share one byte layout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::api::types::DbError;

use super::manifest::Manifest;
use super::quant::{QuantVector, FINGERPRINT_HASH, QUANT_INT8_SYM_PERVEC};
use super::segment::{Segment, MAX_ENTRIES};

/// Which file prefix a writer produces. The prefix distinguishes the
/// producer's role only; the on-disk byte layout is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriterPrefix {
    /// `pack-{hash}.bin` — CI-produced, committed via git. Used by
    /// the `--repack` indexer flow (D13) — wired in §4.3.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "writer protocol API; the `--repack` seg→pack promote (§4.3) is the only producer and renames by string, so Pack is staged but not yet constructed in non-test builds"
        )
    )]
    Pack,
    /// `seg-{hash}.bin` — dev-local, gitignored.
    Seg,
}

impl WriterPrefix {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pack => "pack",
            Self::Seg => "seg",
        }
    }
}

/// Atomic write: write to a per-writer unique tmp path under
/// `tmp_dir`, fsync, then rename onto the content-addressed
/// destination. Both paths must be on the same filesystem (per D8);
/// `Layout::writer_tmp_dir()` resolves to one that satisfies that.
/// A crash leaves at most a stray `*.tmp` file in `tmp_dir`.
fn write_atomic(tmp_dir: &Path, dest: &Path, bytes: &[u8]) -> Result<(), DbError> {
    use std::io::Write as _;
    std::fs::create_dir_all(tmp_dir).map_err(DbError::Io)?;
    let tmp = tmp_dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(DbError::Io)?;
    }
    let mut file = std::fs::File::create(&tmp).map_err(DbError::Io)?;
    file.write_all(bytes).map_err(DbError::Io)?;
    file.sync_all().map_err(DbError::Io)?;
    drop(file);
    if let Err(e) = std::fs::rename(&tmp, dest) {
        drop(std::fs::remove_file(&tmp));
        return Err(DbError::Io(e));
    }
    Ok(())
}

/// Quantize `entries`, chunk into batches of at most [`MAX_ENTRIES`],
/// and write each batch as a content-addressed file
/// (`{prefix}-{xxh3_64(bytes):016x}.bin`) under `dest_dir`, using
/// `tmp_dir` for the tmp+rename atomic step.
///
/// The batching rule is sort-and-chunk: entries are sorted ascending
/// by fingerprint, then split into runs of `MAX_ENTRIES`. The same
/// input set produces the same chunk boundaries and therefore the
/// same content hashes / filenames on every machine.
///
/// Returns the destination paths of the chunks written (some entries
/// may dedup into existing identical files, in which case those paths
/// are still returned).
pub(crate) fn append_vectors(
    dest_dir: &Path,
    tmp_dir: &Path,
    prefix: WriterPrefix,
    dim: u32,
    entries: &[(u64, Vec<f32>)],
) -> Result<Vec<PathBuf>, DbError> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let mut quantized: Vec<(u64, QuantVector)> = entries
        .iter()
        .map(|(fp, v)| (*fp, QuantVector::quantize(v)))
        .collect();
    quantized.sort_by_key(|(fp, _)| *fp);
    quantized.dedup_by_key(|(fp, _)| *fp);

    let mut out = Vec::new();
    for chunk in quantized.chunks(MAX_ENTRIES) {
        let segment = Segment {
            dim,
            entries: chunk.to_vec(),
        };
        let bytes = segment.encode()?;
        let hash = xxhash_rust::xxh3::xxh3_64(&bytes);
        let path = dest_dir.join(format!("{}-{hash:016x}.bin", prefix.as_str()));
        if path.exists() {
            // Idempotent — same hash means same content. Skip the write.
            out.push(path);
            continue;
        }
        write_atomic(tmp_dir, &path, &bytes)?;
        out.push(path);
    }
    super::generation::touch_last_access(dest_dir);
    Ok(out)
}

/// Write `segment` to `dest_dir` as a single content-addressed file
/// under `prefix`, via `tmp_dir`. Test-only building block.
#[cfg(test)]
fn write_segment(
    dest_dir: &Path,
    tmp_dir: &Path,
    prefix: WriterPrefix,
    segment: &Segment,
) -> Result<PathBuf, DbError> {
    let bytes = segment.encode()?;
    let hash = xxhash_rust::xxh3::xxh3_64(&bytes);
    let path = dest_dir.join(format!("{}-{hash:016x}.bin", prefix.as_str()));
    if !path.exists() {
        write_atomic(tmp_dir, &path, &bytes)?;
    }
    Ok(path)
}

/// Rename every `seg-{hash}.bin` in `dir` to `pack-{hash}.bin`
/// (per D13's `--repack` promote step). The content hash is preserved.
/// Idempotent.
pub fn promote_segs_to_packs(dir: &Path) -> Result<Vec<PathBuf>, DbError> {
    let mut out = Vec::new();
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(DbError::Io(e)),
    };
    for entry in read {
        let path = entry.map_err(DbError::Io)?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let Some(hash_suffix) = name
            .strip_prefix("seg-")
            .and_then(|s| s.strip_suffix(".bin"))
        else {
            continue;
        };
        let pack = dir.join(format!("pack-{hash_suffix}.bin"));
        if pack.exists() {
            std::fs::remove_file(&path).map_err(DbError::Io)?;
        } else {
            std::fs::rename(&path, &pack).map_err(DbError::Io)?;
        }
        out.push(pack);
    }
    Ok(out)
}

/// Every committed pack/seg file in `dir`, ordered so the reader's
/// last-wins `HashMap` insertion implements pack-over-seg precedence
/// (D11): segs sort first, then packs.
fn pack_seg_paths(dir: &Path) -> Result<Vec<PathBuf>, DbError> {
    let mut paths = Vec::new();
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(paths),
        Err(e) => return Err(DbError::Io(e)),
    };
    for entry in read {
        let path = entry.map_err(DbError::Io)?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if is_pack_or_seg_name(name) {
            paths.push(path);
        }
    }
    paths.sort_by(|a, b| {
        let a_is_pack = name_starts_with_pack(a);
        let b_is_pack = name_starts_with_pack(b);
        a_is_pack.cmp(&b_is_pack).then_with(|| a.cmp(b))
    });
    Ok(paths)
}

fn name_starts_with_pack(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("pack-"))
}

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "kenn writes lowercase `.bin` extensions deterministically"
)]
fn is_pack_or_seg_name(name: &str) -> bool {
    (name.starts_with("seg-") || name.starts_with("pack-")) && name.ends_with(".bin")
}

/// The union of every committed vector in `dir`, keyed by fingerprint.
/// Reader-side dedup applies pack-over-seg precedence (D11) by load
/// order: segs first (alphabetical), then packs.
pub(crate) fn load_vectors(dir: &Path) -> Result<HashMap<u64, QuantVector>, DbError> {
    let mut map = HashMap::new();
    for path in pack_seg_paths(dir)? {
        let bytes = std::fs::read(&path).map_err(DbError::Io)?;
        let segment = match Segment::decode(&bytes) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    target: "kenn_store::embed::sidecar",
                    path = %path.display(),
                    error = %e,
                    "ignoring undecodable sidecar file"
                );
                continue;
            }
        };
        for (fp, qv) in segment.entries {
            map.insert(fp, qv);
        }
    }
    Ok(map)
}

/// The reconciliation reuse map for `dir` — committed `fingerprint ->
/// vector` entries, but only when the sidecar's manifest is compatible
/// with this build's model, fingerprint scheme, and vector
/// representation. The model gate is defense-in-depth on top of the
/// generation path already separating models (`shared-vector-cache`
/// D4): the fingerprint identifies the *text*, not the model, so a
/// vector from another model must never be reused. Touches the
/// generation's `.last-access` stamp on a compatible read (GC LRU).
pub(crate) fn load_reuse_map(
    dir: &Path,
    expected_model: &str,
    expected_dim: u32,
    recipe: &str,
) -> Result<HashMap<u64, QuantVector>, DbError> {
    let compatible = match Manifest::read(dir)? {
        Some(m) => {
            m.format_version == super::quant::FORMAT_VERSION
                && m.embedding_model.id == expected_model
                && m.vector.dim == expected_dim
                && m.vector.quant == QUANT_INT8_SYM_PERVEC
                && m.fingerprint.hash == FINGERPRINT_HASH
                && m.fingerprint.text == recipe
        }
        None => false,
    };
    if compatible {
        super::generation::touch_last_access(dir);
        load_vectors(dir)
    } else {
        Ok(HashMap::new())
    }
}

/// [`load_reuse_map`] over the current generation dir, unioned with the
/// **legacy** flat sidecar (`<vectors_root>/{code,findings}` from before
/// generation namespacing) when the legacy manifest matches the same
/// generation — so committed `pack-*.bin` files keep serving fresh
/// clones without a migration. Generation entries win on a duplicate
/// fingerprint.
pub(crate) fn load_reuse_map_with_legacy(
    generation_dir: &Path,
    legacy_dir: Option<&Path>,
    expected_model: &str,
    expected_dim: u32,
    recipe: &str,
) -> Result<HashMap<u64, QuantVector>, DbError> {
    let mut map = match legacy_dir {
        Some(legacy) => load_reuse_map(legacy, expected_model, expected_dim, recipe)?,
        None => HashMap::new(),
    };
    map.extend(load_reuse_map(
        generation_dir,
        expected_model,
        expected_dim,
        recipe,
    )?);
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::super::manifest::{
        EmbeddingModelStamp, FingerprintStamp, Manifest, VectorStamp, CODE_TEXT_RECIPE,
        FINDING_TEXT_RECIPE, MANIFEST_FILE,
    };
    use super::super::quant::{QuantVector, FORMAT_VERSION, QUANT_INT8_SYM_PERVEC};
    use super::super::segment::{Segment, MAX_ENTRIES};
    use super::{
        append_vectors, load_reuse_map, load_reuse_map_with_legacy, load_vectors,
        promote_segs_to_packs, write_segment, WriterPrefix,
    };

    fn sample_manifest(dim: u32) -> Manifest {
        Manifest {
            format_version: FORMAT_VERSION,
            embedding_model: EmbeddingModelStamp {
                id: "embeddinggemma-300M".into(),
            },
            vector: VectorStamp {
                dim,
                quant: QUANT_INT8_SYM_PERVEC.into(),
                norm: "l2".into(),
            },
            fingerprint: FingerprintStamp {
                hash: "xxh3-64".into(),
                text: CODE_TEXT_RECIPE.into(),
            },
        }
    }

    fn dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let root = tempfile::tempdir().expect("tempdir");
        let dest = root.path().join("dest");
        let tmp = root.path().join("tmp");
        std::fs::create_dir_all(&dest).expect("dest");
        std::fs::create_dir_all(&tmp).expect("tmp");
        (root, dest, tmp)
    }

    #[test]
    fn append_vectors_writes_content_addressed_files() {
        let (_root, dest, tmp) = dirs();
        let entries = vec![(1_u64, vec![0.1_f32, 0.2]), (2_u64, vec![0.3_f32, 0.4])];
        let paths = append_vectors(&dest, &tmp, WriterPrefix::Seg, 2, &entries).expect("append");
        assert_eq!(paths.len(), 1, "two entries fit in one chunk");
        let path = paths.first().expect("at least one path");
        let name = path.file_name().expect("name").to_str().expect("utf-8");
        assert!(name.starts_with("seg-"), "{name}");
        assert_eq!(
            path.extension().and_then(|e| e.to_str()),
            Some("bin"),
            "got {name}"
        );
        let bytes = std::fs::read(path).expect("read");
        let hash = xxhash_rust::xxh3::xxh3_64(&bytes);
        let expected_name = format!("seg-{hash:016x}.bin");
        assert_eq!(name, expected_name);
    }

    #[test]
    fn append_vectors_chunks_over_cap_into_multiple_files() {
        let (_root, dest, tmp) = dirs();
        let total = MAX_ENTRIES + 1;
        let entries: Vec<(u64, Vec<f32>)> = (0..u64::try_from(total).unwrap())
            .map(|i| (i, vec![0.1_f32, 0.2]))
            .collect();
        let paths = append_vectors(&dest, &tmp, WriterPrefix::Seg, 2, &entries).expect("append");
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn append_vectors_idempotent_for_same_input() {
        let (_root, dest, tmp) = dirs();
        let entries = vec![(1_u64, vec![0.1_f32, 0.2]), (2_u64, vec![0.3_f32, 0.4])];
        let a = append_vectors(&dest, &tmp, WriterPrefix::Seg, 2, &entries).expect("first");
        let b = append_vectors(&dest, &tmp, WriterPrefix::Seg, 2, &entries).expect("second");
        assert_eq!(a, b);
        // Count vector files only — the dir also carries the GC
        // `.last-access` stamp `append_vectors` touches.
        let count = std::fs::read_dir(&dest)
            .expect("read_dir")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bin"))
            .count();
        assert_eq!(count, 1, "no duplicate file written");
    }

    #[test]
    fn pack_overrides_seg_on_duplicate_fp() {
        let (_root, dest, tmp) = dirs();
        write_segment(
            &dest,
            &tmp,
            WriterPrefix::Seg,
            &Segment::new(2, vec![(1, QuantVector::quantize(&[0.1_f32, 0.2]))]),
        )
        .expect("seg");
        write_segment(
            &dest,
            &tmp,
            WriterPrefix::Pack,
            &Segment::new(2, vec![(1, QuantVector::quantize(&[0.9_f32, 0.8]))]),
        )
        .expect("pack");
        let map = load_vectors(&dest).expect("load");
        let qv = map.get(&1).expect("present");
        let reconstructed = qv.dequantize();
        let first = reconstructed.first().copied().expect("dim >= 1");
        assert!((first - 0.9_f32).abs() < 0.05, "got {reconstructed:?}");
    }

    #[test]
    fn load_vectors_unions_pack_and_seg() {
        let (_root, dest, tmp) = dirs();
        write_segment(
            &dest,
            &tmp,
            WriterPrefix::Pack,
            &Segment::new(2, vec![(1_u64, QuantVector::quantize(&[0.1_f32, 0.2]))]),
        )
        .expect("pack");
        write_segment(
            &dest,
            &tmp,
            WriterPrefix::Seg,
            &Segment::new(2, vec![(2_u64, QuantVector::quantize(&[0.3_f32, 0.4]))]),
        )
        .expect("seg");
        let map = load_vectors(&dest).expect("load");
        assert_eq!(map.len(), 2, "pack + seg unioned");
        assert!(map.contains_key(&1) && map.contains_key(&2));
    }

    #[test]
    fn load_vectors_on_missing_dir_is_empty() {
        let (root, _dest, _tmp) = dirs();
        let map = load_vectors(&root.path().join("absent")).expect("load");
        assert!(map.is_empty());
    }

    #[test]
    fn load_reuse_map_gates_on_manifest_compatibility() {
        let (_root, dest, tmp) = dirs();
        let seg = Segment::new(2, vec![(7_u64, QuantVector::quantize(&[0.1_f32, 0.2]))]);
        write_segment(&dest, &tmp, WriterPrefix::Seg, &seg).expect("segment");
        assert!(
            load_reuse_map(&dest, "embeddinggemma-300M", 2, CODE_TEXT_RECIPE)
                .expect("load")
                .is_empty()
        );
        sample_manifest(2).write(&dest).expect("manifest");
        assert_eq!(
            load_reuse_map(&dest, "embeddinggemma-300M", 2, CODE_TEXT_RECIPE)
                .expect("load")
                .len(),
            1
        );
        assert!(
            load_reuse_map(&dest, "embeddinggemma-300M", 768, CODE_TEXT_RECIPE)
                .expect("load")
                .is_empty()
        );
        assert!(
            load_reuse_map(&dest, "embeddinggemma-300M", 2, FINDING_TEXT_RECIPE)
                .expect("load")
                .is_empty()
        );
    }

    #[test]
    fn load_reuse_map_rejects_model_mismatch() {
        // Defense-in-depth (shared-vector-cache 2.3): the fingerprint
        // identifies the text, not the model — a sidecar stamped by a
        // different model must never be reused.
        let (_root, dest, tmp) = dirs();
        let seg = Segment::new(2, vec![(7_u64, QuantVector::quantize(&[0.1_f32, 0.2]))]);
        write_segment(&dest, &tmp, WriterPrefix::Seg, &seg).expect("segment");
        sample_manifest(2).write(&dest).expect("manifest");
        assert!(
            load_reuse_map(&dest, "some-other-model", 2, CODE_TEXT_RECIPE)
                .expect("load")
                .is_empty()
        );
    }

    #[test]
    fn legacy_dir_serves_reuse_alongside_the_generation_dir() {
        // A pre-generation flat sidecar with a matching manifest keeps
        // serving (committed packs survive the layout change); entries
        // from the generation dir union in on top.
        let (_root, legacy, tmp) = dirs();
        write_segment(
            &legacy,
            &tmp,
            WriterPrefix::Seg,
            &Segment::new(2, vec![(1_u64, QuantVector::quantize(&[0.1_f32, 0.2]))]),
        )
        .expect("legacy seg");
        sample_manifest(2).write(&legacy).expect("legacy manifest");

        let generation = legacy.parent().expect("parent").join("generation");
        write_segment(
            &generation,
            &tmp,
            WriterPrefix::Seg,
            &Segment::new(2, vec![(2_u64, QuantVector::quantize(&[0.3_f32, 0.4]))]),
        )
        .expect("generation seg");
        sample_manifest(2)
            .write(&generation)
            .expect("generation manifest");

        let map = load_reuse_map_with_legacy(
            &generation,
            Some(&legacy),
            "embeddinggemma-300M",
            2,
            CODE_TEXT_RECIPE,
        )
        .expect("union");
        assert_eq!(map.len(), 2, "legacy + generation unioned");
        assert!(map.contains_key(&1) && map.contains_key(&2));

        // A legacy dir whose manifest names another model contributes nothing.
        let map = load_reuse_map_with_legacy(
            &generation,
            Some(&legacy),
            "some-other-model",
            2,
            CODE_TEXT_RECIPE,
        )
        .expect("union");
        assert!(map.is_empty());
    }

    #[test]
    fn load_reuse_map_rejects_old_format_version() {
        let (_root, dest, _tmp) = dirs();
        let mut manifest = sample_manifest(2);
        manifest.format_version = 1;
        manifest.write(&dest).expect("write");
        assert!(
            load_reuse_map(&dest, "embeddinggemma-300M", 2, CODE_TEXT_RECIPE)
                .expect("load")
                .is_empty()
        );
    }

    #[test]
    fn load_reuse_map_skips_when_manifest_uses_old_model_table() {
        // Pair the manifest-side test with a vectors check — a sidecar
        // with an unparseable manifest plus a real segment yields an
        // empty reuse-map.
        let (_root, dest, _tmp) = dirs();
        let legacy = r#"
format_version = 1

[model]
name = "embeddinggemma-300M"

[vector]
dim = 2
quant = "int8-sym-pervec"
norm = "l2"

[fingerprint]
hash = "xxh3-64"
text = "sig-lf-doc/v1"
"#;
        std::fs::write(dest.join(MANIFEST_FILE), legacy).expect("write legacy");
        write_segment(
            &dest,
            &dest,
            WriterPrefix::Seg,
            &Segment::new(2, vec![(7_u64, QuantVector::quantize(&[0.1_f32, 0.2]))]),
        )
        .expect("segment");
        assert!(
            load_reuse_map(&dest, "embeddinggemma-300M", 2, CODE_TEXT_RECIPE)
                .expect("load")
                .is_empty()
        );
    }

    #[test]
    fn promote_segs_to_packs_renames_segs() {
        let (_root, dest, tmp) = dirs();
        write_segment(
            &dest,
            &tmp,
            WriterPrefix::Seg,
            &Segment::new(2, vec![(1, QuantVector::quantize(&[0.1_f32, 0.2]))]),
        )
        .expect("seg 1");
        write_segment(
            &dest,
            &tmp,
            WriterPrefix::Seg,
            &Segment::new(2, vec![(2, QuantVector::quantize(&[0.3_f32, 0.4]))]),
        )
        .expect("seg 2");
        let packs = promote_segs_to_packs(&dest).expect("promote");
        assert_eq!(packs.len(), 2);
        for p in &packs {
            assert!(p.exists());
            let name = p.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with("pack-"), "{name}");
        }
        let segs_left = std::fs::read_dir(&dest)
            .expect("read_dir")
            .filter_map(|e| {
                let name = e.ok()?.file_name();
                name.to_str()?.starts_with("seg-").then_some(())
            })
            .count();
        assert_eq!(segs_left, 0);
        assert!(promote_segs_to_packs(&dest).expect("idempotent").is_empty());
    }

    #[test]
    fn promote_handles_existing_pack_collision() {
        let (_root, dest, tmp) = dirs();
        let seg_content = Segment::new(2, vec![(5, QuantVector::quantize(&[0.5_f32, 0.5]))]);
        write_segment(&dest, &tmp, WriterPrefix::Seg, &seg_content).expect("seg");
        write_segment(&dest, &tmp, WriterPrefix::Pack, &seg_content).expect("pack");
        let packs = promote_segs_to_packs(&dest).expect("promote");
        assert_eq!(packs.len(), 1);
        let names: Vec<_> = std::fs::read_dir(&dest)
            .expect("read_dir")
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .collect();
        let bin_names: Vec<_> = names
            .iter()
            .filter_map(|n| n.to_str())
            .filter(|n| {
                std::path::Path::new(n)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("bin"))
            })
            .collect();
        assert_eq!(bin_names.len(), 1);
        let only = bin_names.first().expect("one bin");
        assert!(only.starts_with("pack-"), "{only}");
    }

    #[test]
    fn a_crash_leftover_tmp_file_is_ignored_by_readers() {
        let (_root, dest, tmp) = dirs();
        write_segment(
            &dest,
            &tmp,
            WriterPrefix::Seg,
            &Segment::new(2, vec![(5_u64, QuantVector::quantize(&[0.1_f32, 0.2]))]),
        )
        .expect("seg");
        std::fs::write(tmp.join("9000.tmp"), b"torn").expect("tmp");
        let map = load_vectors(&dest).expect("load");
        assert_eq!(map.len(), 1, "tmp not decoded");
        assert!(map.contains_key(&5));
    }
}
