//! Generation-namespaced sidecar layout (`shared-vector-cache` Phase 2).
//!
//! A *generation* is one `(model, dim, quant, recipe)` combination; its
//! vectors live in their own directory subtree so multiple generations
//! coexist — a recipe or model change writes a **new** subtree instead of
//! wiping the old one (the destructive `reset_vectors` is gone). The
//! pre-generation flat dirs (`<root>/code`, `<root>/findings`) remain
//! readable as *legacy* generations, identified — like every generation —
//! by the `manifest.toml` they carry.

use std::path::{Path, PathBuf};

use crate::api::types::DbError;
use crate::layout::Layout;

use super::manifest::{Manifest, CODE_TEXT_RECIPE, FINDING_TEXT_RECIPE, MANIFEST_FILE};
use super::quant::{FINGERPRINT_HASH, FORMAT_VERSION, QUANT_INT8_SYM_PERVEC};

/// The embedding dimension of the one supported model family. Matches
/// the `EMBED_DIM` constants at the embed-pass call sites.
pub(crate) const EMBED_DIM: u32 = 768;

/// Per-generation LRU stamp file — its mtime is the generation's
/// last-access time, touched on every reuse read and append. Gitignored.
pub(crate) const LAST_ACCESS_FILE: &str = ".last-access";

/// The directory for one generation:
/// `<vectors_root>/<model>/<dim>/<quant>/<recipe>/`. The recipe tag may
/// contain `/` (e.g. `doc/v1`), which simply nests one more level; the
/// model id is sanitized so provider spellings like `embeddinggemma:300m`
/// stay portable path components on every target platform.
pub(crate) fn generation_dir(
    vectors_root: &Path,
    model_id: &str,
    dim: u32,
    recipe: &str,
) -> PathBuf {
    vectors_root
        .join(sanitize_component(model_id))
        .join(dim.to_string())
        .join(QUANT_INT8_SYM_PERVEC)
        .join(recipe)
}

/// The current code-vector generation dir for `layout` + `model_id`.
#[must_use]
pub fn code_generation_dir(layout: &Layout, model_id: &str) -> PathBuf {
    generation_dir(layout.vectors_root(), model_id, EMBED_DIM, CODE_TEXT_RECIPE)
}

/// The current findings-vector generation dir for `layout` + `model_id`.
#[must_use]
pub fn findings_generation_dir(layout: &Layout, model_id: &str) -> PathBuf {
    generation_dir(
        layout.vectors_root(),
        model_id,
        EMBED_DIM,
        FINDING_TEXT_RECIPE,
    )
}

/// The embedding model id the current process configuration selects —
/// the value that keys the active generation. Reads the global config
/// (with its env overrides); falls back to the built-in default when no
/// config is loadable.
#[must_use]
pub fn current_model_id() -> String {
    kenn_config::GlobalConfig::load()
        .unwrap_or_default()
        .embeddings
        .model
}

/// Map a model id to a portable path component: keep `[A-Za-z0-9._-]`,
/// replace everything else (`:`, `/`, `\`, …) with `-`.
fn sanitize_component(model_id: &str) -> String {
    let mapped: String = model_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if mapped.is_empty() {
        "unknown-model".to_owned()
    } else {
        mapped
    }
}

/// Touch a generation's `.last-access` stamp. Best-effort — LRU accuracy
/// is not worth failing an embed or a search over.
pub(crate) fn touch_last_access(dir: &Path) {
    if dir.is_dir() {
        drop(std::fs::write(dir.join(LAST_ACCESS_FILE), b""));
    }
}

/// Every sidecar directory under `vectors_root` that carries a
/// `manifest.toml` — the generation dirs plus the legacy flat
/// `code/`/`findings/` dirs. Bounded-depth recursive walk.
#[must_use]
pub fn sidecar_dirs(vectors_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_for_manifests(vectors_root, 0, &mut out);
    out.sort();
    out
}

/// Nesting bound for the walk: model ids and recipes may add levels, but
/// the layout is a handful deep — anything past this is not ours.
const MAX_WALK_DEPTH: usize = 8;

fn walk_for_manifests(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    if dir.join(MANIFEST_FILE).is_file() {
        out.push(dir.to_path_buf());
        // Generations never nest inside one another; stop here.
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_for_manifests(&path, depth + 1, out);
        }
    }
}

/// Outcome of a vector-cache GC pass.
#[derive(Debug, Default)]
pub struct VectorsGcReport {
    /// Generation directories removed, oldest-first.
    pub evicted: Vec<PathBuf>,
    /// Bytes those directories held.
    pub freed_bytes: u64,
    /// Cache size after the pass (every manifest-bearing dir).
    pub remaining_bytes: u64,
}

/// One enumerated generation with what GC needs to rank and filter it.
struct GenInfo {
    dir: PathBuf,
    size: u64,
    last_access: std::time::SystemTime,
    has_pack: bool,
    manifest: Option<Manifest>,
}

/// LRU-evict generation directories under `layout.vectors_root()` until
/// the cache fits `cap_mb` MiB. Never evicts the **active** generations
/// (the ones `model_id` currently reads/writes, matched by manifest, so
/// the legacy flat dir is protected exactly while it still serves the
/// active generation) and never evicts a dir holding committed
/// `pack-*.bin` files (design D6: GC only touches non-committed
/// generations). `cap_mb = 0` disables GC. The pass holds a `gc.lock`
/// flock scoped to itself — appends stay lock-free; a contended lock
/// means another collector is running, so this pass is skipped.
pub fn gc_vector_cache(
    layout: &Layout,
    model_id: &str,
    cap_mb: u64,
) -> Result<VectorsGcReport, DbError> {
    let root = layout.vectors_root();
    if cap_mb == 0 || !root.is_dir() {
        return Ok(VectorsGcReport::default());
    }
    let lock_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("gc.lock"))
        .map_err(DbError::Io)?;
    if fs2::FileExt::try_lock_exclusive(&lock_file).is_err() {
        return Ok(VectorsGcReport::default());
    }

    let cap_bytes = cap_mb.saturating_mul(1024 * 1024);
    let mut gens: Vec<GenInfo> = sidecar_dirs(root)
        .into_iter()
        .map(|dir| {
            let (size, has_pack) = dir_size_and_packs(&dir);
            let last_access = std::fs::metadata(dir.join(LAST_ACCESS_FILE))
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let manifest = Manifest::read(&dir).unwrap_or(None);
            GenInfo {
                dir,
                size,
                last_access,
                has_pack,
                manifest,
            }
        })
        .collect();
    let mut total: u64 = gens.iter().map(|g| g.size).sum();
    let mut report = VectorsGcReport {
        remaining_bytes: total,
        ..VectorsGcReport::default()
    };
    if total <= cap_bytes {
        return Ok(report);
    }

    gens.sort_by_key(|g| g.last_access);
    for gen in &gens {
        if total <= cap_bytes {
            break;
        }
        if gen.has_pack
            || gen
                .manifest
                .as_ref()
                .is_some_and(|m| is_active(m, model_id))
        {
            continue;
        }
        std::fs::remove_dir_all(&gen.dir).map_err(DbError::Io)?;
        remove_empty_parents(&gen.dir, root);
        total = total.saturating_sub(gen.size);
        report.freed_bytes += gen.size;
        report.evicted.push(gen.dir.clone());
    }
    report.remaining_bytes = total;
    Ok(report)
}

/// Whether a manifest identifies one of the generations the current
/// process configuration actively reads and writes.
fn is_active(m: &Manifest, model_id: &str) -> bool {
    m.format_version == FORMAT_VERSION
        && m.embedding_model.id == model_id
        && m.vector.dim == EMBED_DIM
        && m.vector.quant == QUANT_INT8_SYM_PERVEC
        && m.fingerprint.hash == FINGERPRINT_HASH
        && (m.fingerprint.text == CODE_TEXT_RECIPE || m.fingerprint.text == FINDING_TEXT_RECIPE)
}

/// Total byte size of the files directly in `dir`, and whether any is a
/// committed `pack-*.bin`.
fn dir_size_and_packs(dir: &Path) -> (u64, bool) {
    let mut size = 0;
    let mut has_pack = false;
    if let Ok(read) = std::fs::read_dir(dir) {
        for entry in read.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_file() {
                size += meta.len();
                let name = entry.file_name();
                if name.to_string_lossy().starts_with("pack-") {
                    has_pack = true;
                }
            }
        }
    }
    (size, has_pack)
}

/// After evicting a generation, drop the now-empty namespace dirs above
/// it (model/dim/quant levels) up to — but never including — `root`.
fn remove_empty_parents(dir: &Path, root: &Path) {
    let mut cur = dir.parent();
    while let Some(p) = cur {
        if p == root || !p.starts_with(root) {
            break;
        }
        if std::fs::remove_dir(p).is_err() {
            break; // non-empty or gone — either way, stop.
        }
        cur = p.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::super::io::{append_vectors, WriterPrefix};
    use super::*;

    fn write_generation(root: &Path, model: &str, recipe: &str, fp: u64) -> PathBuf {
        let dir = generation_dir(root, model, EMBED_DIM, recipe);
        let tmp = root.join(".tmp");
        append_vectors(
            &dir,
            &tmp,
            WriterPrefix::Seg,
            EMBED_DIM,
            &[(fp, vec![0.5_f32; EMBED_DIM as usize])],
        )
        .expect("append");
        Manifest::current(model.to_owned(), EMBED_DIM, recipe)
            .write(&dir)
            .expect("manifest");
        dir
    }

    #[test]
    fn generation_dir_nests_model_dim_quant_recipe() {
        let d = generation_dir(Path::new("/v"), "embeddinggemma-300M", 768, "doc/v1");
        assert_eq!(
            d,
            Path::new("/v/embeddinggemma-300M/768/int8-sym-pervec/doc/v1")
        );
    }

    #[test]
    fn model_component_is_sanitized_for_portability() {
        assert_eq!(
            sanitize_component("embeddinggemma:300m"),
            "embeddinggemma-300m"
        );
        assert_eq!(
            sanitize_component("ggml-org/embeddinggemma-300M"),
            "ggml-org-embeddinggemma-300M"
        );
        assert_eq!(sanitize_component(""), "unknown-model");
    }

    #[test]
    fn sidecar_dirs_finds_generation_and_legacy_dirs() {
        let root = tempfile::tempdir().expect("tempdir");
        let gen = write_generation(root.path(), "embeddinggemma-300M", CODE_TEXT_RECIPE, 1);
        // Legacy flat dir with a manifest.
        let legacy = root.path().join("code");
        Manifest::current("old-model".to_owned(), EMBED_DIM, CODE_TEXT_RECIPE)
            .write(&legacy)
            .expect("legacy manifest");
        let dirs = sidecar_dirs(root.path());
        assert!(dirs.contains(&gen), "{dirs:?}");
        assert!(dirs.contains(&legacy), "{dirs:?}");
        assert_eq!(dirs.len(), 2);
    }

    #[test]
    fn generation_switch_leaves_prior_vectors_intact_and_reusable() {
        // A model (or recipe) change targets a NEW dir; the old
        // generation's files stay put, and switching back reuses them
        // with zero re-embeds (shared-vector-cache task 2.5).
        let root = tempfile::tempdir().expect("tempdir");
        let gen_a = write_generation(root.path(), "model-a", CODE_TEXT_RECIPE, 1);
        let gen_b = write_generation(root.path(), "model-b", CODE_TEXT_RECIPE, 2);
        assert_ne!(gen_a, gen_b);
        assert!(gen_a.join("manifest.toml").is_file(), "a intact after b");

        let reused =
            super::super::io::load_reuse_map(&gen_a, "model-a", EMBED_DIM, CODE_TEXT_RECIPE)
                .expect("reuse");
        assert_eq!(reused.len(), 1, "switching back reuses generation a");
    }

    #[test]
    fn gc_evicts_lru_inactive_generation_and_keeps_active() {
        let root = tempfile::tempdir().expect("tempdir");
        let layout_root = root.path();
        let old = write_generation(layout_root, "old-model", CODE_TEXT_RECIPE, 1);
        let active = write_generation(layout_root, "embeddinggemma-300M", CODE_TEXT_RECIPE, 2);
        touch_last_access(&old);
        // Make the active generation strictly more recent.
        std::thread::sleep(std::time::Duration::from_millis(20));
        touch_last_access(&active);

        // The cap is in MiB — pad the old generation past 1 MiB so a
        // cap_mb = 1 pass must evict it.
        std::fs::write(
            old.join("seg-ffffffffffffffff.bin"),
            vec![0u8; 2 * 1024 * 1024],
        )
        .expect("pad");
        let report = gc_at(layout_root, "embeddinggemma-300M", 1).expect("gc");
        assert_eq!(report.evicted, vec![old.clone()], "old generation evicted");
        assert!(!old.exists());
        assert!(active.exists(), "active generation kept");
        assert!(
            !layout_root.join("old-model").exists(),
            "empty namespace parents removed"
        );
    }

    #[test]
    fn gc_never_evicts_pack_holding_generations() {
        let root = tempfile::tempdir().expect("tempdir");
        let old = write_generation(root.path(), "old-model", CODE_TEXT_RECIPE, 1);
        std::fs::write(
            old.join("pack-0000000000000000.bin"),
            vec![0u8; 2 * 1024 * 1024],
        )
        .expect("pack");
        let report = gc_at(root.path(), "embeddinggemma-300M", 1).expect("gc");
        assert!(report.evicted.is_empty(), "{report:?}");
        assert!(old.exists());
    }

    #[test]
    fn gc_under_cap_is_a_noop() {
        let root = tempfile::tempdir().expect("tempdir");
        let gen = write_generation(root.path(), "old-model", CODE_TEXT_RECIPE, 1);
        let report = gc_at(root.path(), "embeddinggemma-300M", 1024).expect("gc");
        assert!(report.evicted.is_empty());
        assert!(gen.exists());
    }

    /// Test shim: run the GC body against an arbitrary root (the public
    /// entry takes a `Layout`, whose vectors root is derived).
    fn gc_at(root: &Path, model_id: &str, cap_mb: u64) -> Result<VectorsGcReport, DbError> {
        let mut layout = Layout::default_for(root);
        layout.set_vectors_root_for_tests(root.to_path_buf());
        gc_vector_cache(&layout, model_id, cap_mb)
    }
}
