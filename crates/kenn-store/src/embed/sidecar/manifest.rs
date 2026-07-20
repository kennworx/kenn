//! `manifest.toml` — the per-sidecar generation stamp. Records the
//! producing model id, the vector representation, and the fingerprint
//! recipe. Any field change here invalidates every committed vector.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::api::types::DbError;

use super::quant::{FINGERPRINT_HASH, FORMAT_VERSION, QUANT_INT8_SYM_PERVEC};

/// Manifest filename within the sidecar directory.
pub(crate) const MANIFEST_FILE: &str = "manifest.toml";

/// The code-symbol `embeddable_text` recipe tag recorded in the manifest —
/// the name-row vector embeds the **doc prose only** (the signature is owned by
/// the lexical arms). Undocumented symbols are not embedded. Renaming the tag
/// invalidates the prior `sig-lf-doc` vectors so they are re-embedded, not reused.
pub(crate) const CODE_TEXT_RECIPE: &str = "doc/v1";

/// The finding `embeddable_text` recipe tag — a finding's raw `text`.
pub(crate) const FINDING_TEXT_RECIPE: &str = "finding-text/v1";

/// The vector "generation" stamp — `manifest.toml`. Recorded
/// fields are exactly those that, if changed, invalidate every
/// committed vector; nothing that churns per-append. TOML schema is
/// unchanged from KVS1; only `format_version` bumps to `2`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    /// Segment + manifest file-format version.
    pub format_version: u32,
    /// Producing embedding model identity.
    pub embedding_model: EmbeddingModelStamp,
    /// Stored-vector representation.
    pub vector: VectorStamp,
    /// How fingerprints (keys) are derived.
    pub fingerprint: FingerprintStamp,
}

/// `[embedding_model]` — what produced the vectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmbeddingModelStamp {
    /// Model id string (e.g. `"embeddinggemma-300M"`).
    pub id: String,
}

/// `[vector]` — how a stored vector decodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VectorStamp {
    /// Full model dimension (no MRL truncation).
    pub dim: u32,
    /// Quantization tag — see [`QUANT_INT8_SYM_PERVEC`].
    pub quant: String,
    /// Normalization applied to vectors (`"l2"`).
    pub norm: String,
}

/// `[fingerprint]` — how keys are derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FingerprintStamp {
    /// Hash algorithm for the key (`"xxh3-64"`).
    pub hash: String,
    /// Versioned `embeddable_text` recipe tag.
    pub text: String,
}

impl Manifest {
    /// The manifest for the current build generation.
    pub fn current(model_id: String, dim: u32, recipe: &str) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            embedding_model: EmbeddingModelStamp { id: model_id },
            vector: VectorStamp {
                dim,
                quant: QUANT_INT8_SYM_PERVEC.to_owned(),
                norm: "l2".to_owned(),
            },
            fingerprint: FingerprintStamp {
                hash: FINGERPRINT_HASH.to_owned(),
                text: recipe.to_owned(),
            },
        }
    }

    /// Read `manifest.toml` from a sidecar directory. `Ok(None)` when
    /// missing or unparseable.
    pub fn read(dir: &Path) -> Result<Option<Self>, DbError> {
        let path = dir.join(MANIFEST_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(DbError::Io(e)),
        };
        match toml::from_str::<Self>(&text) {
            Ok(m) => Ok(Some(m)),
            Err(e) => {
                tracing::warn!(
                    target: "kenn_store::embed::sidecar",
                    path = %path.display(),
                    error = %e,
                    "ignoring incompatible {MANIFEST_FILE}; reconciliation will re-embed"
                );
                Ok(None)
            }
        }
    }

    /// Write `manifest.toml` into a sidecar directory.
    pub fn write(&self, dir: &Path) -> Result<(), DbError> {
        let text = toml::to_string(self)
            .map_err(|e| DbError::Backend(format!("serialize {MANIFEST_FILE}: {e}")))?;
        std::fs::create_dir_all(dir).map_err(DbError::Io)?;
        std::fs::write(dir.join(MANIFEST_FILE), text).map_err(DbError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::super::quant::{QuantVector, QUANT_INT8_SYM_PERVEC};
    use super::{
        EmbeddingModelStamp, FingerprintStamp, Manifest, VectorStamp, CODE_TEXT_RECIPE,
        FORMAT_VERSION, MANIFEST_FILE,
    };
    use tempfile::TempDir;

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
                text: "sig-lf-doc/v1".into(),
            },
        }
    }

    #[test]
    fn manifest_round_trips_through_toml() {
        let root = TempDir::new().unwrap();
        let manifest = sample_manifest(768);
        manifest.write(root.path()).expect("write");
        let read = Manifest::read(root.path()).expect("read").expect("present");
        assert_eq!(read, manifest);
    }

    #[test]
    fn manifest_read_absent_is_none() {
        let root = TempDir::new().unwrap();
        assert!(Manifest::read(root.path()).expect("read").is_none());
    }

    #[test]
    fn manifest_stamps_only_the_model_id() {
        let root = TempDir::new().unwrap();
        let m = Manifest::current("embeddinggemma-300M".to_owned(), 768, CODE_TEXT_RECIPE);
        m.write(root.path()).expect("write");
        let raw = std::fs::read_to_string(root.path().join(MANIFEST_FILE)).expect("read raw");
        assert!(raw.contains("[embedding_model]"), "{raw}");
        assert!(raw.contains("id = \"embeddinggemma-300M\""), "{raw}");
        assert!(!raw.contains("gguf_xxh3"), "{raw}");
        assert!(!raw.contains("prompt"), "{raw}");
        let read = Manifest::read(root.path()).expect("read").expect("present");
        assert_eq!(read.embedding_model.id, "embeddinggemma-300M");
    }

    #[test]
    fn old_model_table_is_treated_as_incompatible() {
        // A pre-`extract-kenn-server` manifest using `[model]` with
        // `gguf_xxh3` is unparseable under the new schema and must be
        // treated as absent.
        let root = TempDir::new().unwrap();
        let legacy = r#"
format_version = 1

[model]
name = "embeddinggemma-300M"
gguf_xxh3 = "9f3a1c2e7b4d8a06"
prompt = "none"

[vector]
dim = 768
quant = "int8-sym-pervec"
norm = "l2"

[fingerprint]
hash = "xxh3-64"
text = "sig-lf-doc/v1"
"#;
        std::fs::write(root.path().join(MANIFEST_FILE), legacy).expect("write legacy");
        assert!(Manifest::read(root.path()).expect("read").is_none());
        // QuantVector unused here but kept to keep this module's test
        // imports useful for the larger reuse-map test, which exercises
        // segments + manifest together (see io.rs).
        let _ = QuantVector::quantize(&[0.0_f32, 0.0]);
    }
}
