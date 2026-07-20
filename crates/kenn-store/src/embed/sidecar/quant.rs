//! Fingerprint hash + int8-quantized vector representation, plus the
//! format-level constants that bind them to the on-disk layout.

/// On-disk format version. Bumped from 1 → 2 with the KVS2 layout
/// (D6); the manifest schema otherwise unchanged.
pub(crate) const FORMAT_VERSION: u32 = 2;
/// `quant` tag for per-vector symmetric scalar int8 quantization.
pub(crate) const QUANT_INT8_SYM_PERVEC: &str = "int8-sym-pervec";
/// Fingerprint hash-algorithm tag recorded in the manifest.
pub(crate) const FINGERPRINT_HASH: &str = "xxh3-64";
/// Numeric `quant` code packed into the `ver_quant` header field.
pub(super) const QUANT_CODE_INT8: u32 = 1;

/// The content-addressed key for an embeddable text — xxh3-64. A vector
/// is a pure function of `(embeddable_text, model)`; this hashes the
/// text half, so an unchanged fingerprint means an unchanged vector.
pub(crate) fn fingerprint(embeddable_text: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(embeddable_text.as_bytes())
}

/// An int8-quantized vector: one f32 scale plus `dim` signed codes
/// (per-vector symmetric scalar quantization).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuantVector {
    /// `value ≈ code * scale`.
    pub scale: f32,
    /// One signed code per dimension.
    pub codes: Vec<i8>,
}

impl QuantVector {
    /// Quantize a float vector: `scale = maxabs / 127`,
    /// `code = round(x / scale)` clamped to the int8 range.
    pub fn quantize(vector: &[f32]) -> Self {
        let maxabs = vector.iter().fold(0.0_f32, |m, &x| m.max(x.abs()));
        let scale = if maxabs > 0.0 { maxabs / 127.0 } else { 1.0 };
        let codes = vector.iter().map(|&x| quantize_one(x, scale)).collect();
        Self { scale, codes }
    }

    /// Reconstruct the float vector — `x = code * scale`.
    pub fn dequantize(&self) -> Vec<f32> {
        self.codes
            .iter()
            .map(|&c| f32::from(c) * self.scale)
            .collect()
    }
}

/// Quantize one component to a signed int8 code.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the value is rounded and clamped to [-127, 127] before the cast, so it is an exact whole number in i8 range"
)]
fn quantize_one(x: f32, scale: f32) -> i8 {
    ((x / scale).round().clamp(-127.0, 127.0)) as i8
}

#[cfg(test)]
mod tests {
    use super::{fingerprint, QuantVector};

    #[test]
    fn fingerprint_is_stable_and_text_sensitive() {
        assert_eq!(
            fingerprint("fn parse(s: &str)"),
            fingerprint("fn parse(s: &str)")
        );
        assert_ne!(
            fingerprint("fn parse(s: &str)"),
            fingerprint("fn parse(s: &Path)")
        );
    }

    #[test]
    fn quantize_round_trips_within_tolerance() {
        let v: Vec<f32> = (0..768_u16).map(|i| (f32::from(i) / 768.0) - 0.5).collect();
        let qv = QuantVector::quantize(&v);
        assert_eq!(qv.codes.len(), 768);
        let back = qv.dequantize();
        let step = 0.5_f32 / 127.0;
        for (a, b) in v.iter().zip(&back) {
            assert!((a - b).abs() <= step * 1.01, "{a} vs {b}");
        }
    }

    #[test]
    fn quantize_handles_an_all_zero_vector() {
        let qv = QuantVector::quantize(&[0.0_f32; 8]);
        assert!(qv.dequantize().iter().all(|&x| x == 0.0));
    }
}
