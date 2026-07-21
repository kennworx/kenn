//! Shared helpers for the per-language `resolve` tests.

use super::{Install, Resolved};

/// Unwrap a Tarball install, failing loudly on any other variant — a test
/// asserting on a URL must not silently pass because the shape changed.
pub(super) fn tarball(r: &Resolved) -> (String, String, bool, usize) {
    match &r.install {
        Install::Tarball {
            url,
            digest_hex,
            digest_is_sha512,
            strip_components,
        } => (
            url.clone(),
            digest_hex.clone(),
            *digest_is_sha512,
            *strip_components,
        ),
        other => panic!("expected a Tarball install, got {other:?}"),
    }
}

pub(super) fn fetcher(body: &'static str) -> impl Fn(&str) -> Result<String, String> {
    move |_: &str| Ok(body.to_string())
}
