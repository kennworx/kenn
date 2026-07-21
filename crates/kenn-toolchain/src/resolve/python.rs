use super::common::{Arch, Install, ResolveError, Resolved, LATEST};

/// uv's curated index over python-build-standalone. PBS itself has no notion of
/// "the newest 3.12", so this is the only machine-readable place a partial pin
/// like `3.12` becomes a concrete asset.
const PYTHON_INDEX: &str =
    "https://raw.githubusercontent.com/astral-sh/uv/main/crates/uv-python/download-metadata.json";

/// Whether a python-build-standalone key names a prerelease.
///
/// Keys look like `cpython-3.12.13-linux-x86_64-gnu` or
/// `cpython-3.15.0b3-linux-x86_64-gnu`. `CPython` spells prereleases with a
/// trailing `aN`/`bN`/`rcN` on the version segment, so the test is whether that
/// segment ends in something other than a digit.
fn key_is_prerelease(key: &str) -> bool {
    key.split('-')
        .nth(1)
        .and_then(|v| v.split('+').next())
        .is_some_and(|v| {
            !v.ends_with(|c: char| c.is_ascii_digit())
                || v.contains('a')
                || v.contains('b')
                || v.contains("rc")
        })
}

pub(super) fn resolve_python(
    pin: &str,
    pin_source: &str,
    arch: Arch,
    fetch_text: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Resolved, ResolveError> {
    let body = fetch_text(PYTHON_INDEX).map_err(|message| ResolveError::Metadata {
        language: "python",
        url: PYTHON_INDEX.to_string(),
        message,
    })?;
    let index: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| ResolveError::Metadata {
            language: "python",
            url: PYTHON_INDEX.to_string(),
            message: e.to_string(),
        })?;

    // A pin may be `3.12` or `3.12.13`; both select by prefix, and the newest
    // matching patch wins.
    let wanted: Vec<u64> = pin.split('.').filter_map(|p| p.parse().ok()).collect();
    // A pin naming a prerelease opts into them; nothing else does. Tested on the
    // VERSION shape, not by substring: the LATEST sentinel is "*latest*", which
    // contains an 'a' and so opted every unpinned workspace into prereleases —
    // the exact bug this filter exists to stop.
    let want_prerelease = pin != LATEST && key_is_prerelease(&format!("x-{pin}"));
    let mut best: Option<(u64, String, String)> = None;
    for (key, entry) in index.as_object().into_iter().flatten() {
        // The KEY is the only place a prerelease is visible. uv's index carries
        // `cpython-3.15.0b3-…` alongside finals, and its numeric `major`/`minor`
        // /`patch` fields are 3/15/0 for BOTH — so comparing versions cannot
        // tell them apart, and "newest 3.15.0" silently selected an alpha.
        //
        // Measured: an unpinned real repo provisioned Python 3.15.0a1 and
        // indexed 1202 defs without complaint. A prerelease interpreter is not
        // wrong enough to fail, which is exactly why it has to be excluded here
        // rather than caught downstream.
        //
        // `+freethreaded` / `+debug` are build VARIANTS, not prereleases, and
        // are already excluded by the `.tar.gz` filter below.
        if !want_prerelease && key_is_prerelease(key) {
            continue;
        }
        let get = |k: &str| entry.get(k).and_then(serde_json::Value::as_u64);
        let s = |k: &str| entry.get(k).and_then(serde_json::Value::as_str);
        let (Some(major), Some(minor), Some(patch)) = (get("major"), get("minor"), get("patch"))
        else {
            continue;
        };
        let matches_pin = pin == LATEST
            || (wanted.first() == Some(&major)
                && wanted.get(1).is_none_or(|m| *m == minor)
                && wanted.get(2).is_none_or(|p| *p == patch));
        let right_platform = s("name") == Some("cpython")
            && s("os") == Some("linux")
            // glibc: the python image is noble-based, and a musl build would
            // resolve cleanly and then fail to exec.
            && s("libc") == Some("gnu")
            && entry
                .get("arch")
                .and_then(|a| a.get("family"))
                .and_then(serde_json::Value::as_str)
                == Some(arch.python_arch());
        if !matches_pin || !right_platform {
            continue;
        }
        let (Some(url), Some(sha)) = (s("url"), s("sha256")) else {
            continue;
        };
        // The index also carries `debug-full` and `pgo+lto-full` variants, which
        // are `.tar.zst`. We unpack gzip only, and selecting on version alone
        // picked one — the failure surfaced as "invalid gzip header" after a
        // successful resolve, naming neither the variant nor why it was chosen.
        // `install_only*` builds are the runnable ones and the only `.tar.gz`.
        if !url.ends_with(".tar.gz") {
            continue;
        }
        let key = major * 1_000_000 + minor * 1_000 + patch;
        if best.as_ref().is_none_or(|(k, _, _)| key > *k) {
            best = Some((key, url.to_string(), sha.to_string()));
        }
    }

    let Some((key, url, sha)) = best else {
        return Err(ResolveError::NoMatch {
            language: "python",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: format!(" — for linux/{} (gnu)", arch.python_arch()),
        });
    };

    Ok(Resolved {
        version: format!(
            "{}.{}.{}",
            key / 1_000_000,
            (key / 1_000) % 1_000,
            key % 1_000
        ),
        install: Install::Tarball {
            url,
            digest_hex: sha,
            digest_is_sha512: false,
            // `install_only` tarballs wrap everything in `python/`.
            strip_components: 1,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pin::Language;
    use crate::resolve::resolve;
    use crate::resolve::testutil::{fetcher, tarball};

    const PYTHON_INDEX_JSON: &str = r#"{
          "cpython-3.12.13-linux-x86_64-gnu": {"name":"cpython","os":"linux","libc":"gnu",
            "arch":{"family":"x86_64"},"major":3,"minor":12,"patch":13,
            "url":"https://x/cpython-3.12.13-x86_64-gnu.tar.gz","sha256":"p31213"},
          "cpython-3.12.9-linux-x86_64-gnu": {"name":"cpython","os":"linux","libc":"gnu",
            "arch":{"family":"x86_64"},"major":3,"minor":12,"patch":9,
            "url":"https://x/cpython-3.12.9-x86_64-gnu.tar.gz","sha256":"p3129"},
          "cpython-3.12.50-linux-x86_64-gnu-debug": {"name":"cpython","os":"linux","libc":"gnu",
            "arch":{"family":"x86_64"},"major":3,"minor":12,"patch":50,
            "url":"https://x/cpython-3.12.50-debug-full.tar.zst","sha256":"ZSTD"},
          "cpython-3.12.99-linux-x86_64-musl": {"name":"cpython","os":"linux","libc":"musl",
            "arch":{"family":"x86_64"},"major":3,"minor":12,"patch":99,
            "url":"https://x/MUSL-MUST-NOT-BE-PICKED.tar.gz","sha256":"MUSL"},
          "cpython-3.13.5a1-linux-x86_64-gnu": {"name":"cpython","os":"linux","libc":"gnu",
            "arch":{"family":"x86_64"},"major":3,"minor":13,"patch":5,
            "url":"https://x/ALPHA-MUST-NOT-BE-PICKED.tar.gz","sha256":"ALPHA"},
          "cpython-3.13.1-linux-x86_64-gnu": {"name":"cpython","os":"linux","libc":"gnu",
            "arch":{"family":"x86_64"},"major":3,"minor":13,"patch":1,
            "url":"https://x/cpython-3.13.1-x86_64-gnu.tar.gz","sha256":"p3131"}
        }"#;

    /// A partial pin selects the newest matching patch — `3.12` must not jump to
    /// 3.13, and must not stop at 3.12.9.
    #[test]
    fn python_resolves_a_partial_pin_to_its_newest_patch() {
        let got = resolve(
            Language::Python,
            "3.12",
            ".python-version",
            None,
            Arch::X64,
            &fetcher(PYTHON_INDEX_JSON),
        )
        .expect("resolve");
        assert_eq!(got.version, "3.12.13");
        assert_eq!(tarball(&got).1, "p31213");
        // A newer `.tar.zst` debug build must not win: we unpack gzip only, and
        // choosing it fails as "invalid gzip header" long after the resolve.
        assert_ne!(tarball(&got).1, "ZSTD", "picked a zstd variant");
    }

    /// The python image is glibc. A musl asset would resolve cleanly and then
    /// fail to exec — the same trap as the .NET RID.
    ///
    /// Mutation-checked: dropping the `libc == "gnu"` filter picks MUSL.
    #[test]
    fn python_never_picks_a_musl_asset() {
        let got = resolve(
            Language::Python,
            "3.12.13",
            ".python-version",
            None,
            Arch::X64,
            &fetcher(PYTHON_INDEX_JSON),
        )
        .expect("resolve");
        assert_eq!(tarball(&got).1, "p31213");
        assert_ne!(tarball(&got).1, "MUSL", "picked the musl decoy");
    }

    /// uv's index carries prereleases whose numeric major/minor/patch are
    /// IDENTICAL to a final's — `cpython-3.15.0b3` reports 3/15/0 — so only the
    /// key distinguishes them. Comparing versions alone selected Python
    /// 3.15.0a1 on a real unpinned repo, which then indexed 1202 defs without
    /// complaint: a prerelease interpreter is not wrong enough to fail.
    ///
    /// Mutation-checked: dropping the key filter picks the ALPHA decoy, which is
    /// newer than every final in the fixture.
    #[test]
    fn python_never_picks_a_prerelease() {
        let got = resolve(
            Language::Python,
            LATEST,
            "<no pin file>",
            None,
            Arch::X64,
            &fetcher(PYTHON_INDEX_JSON),
        )
        .expect("resolve");
        assert_ne!(tarball(&got).1, "ALPHA", "picked a prerelease");
        assert_eq!(got.version, "3.13.1", "newest FINAL, not newest overall");
    }

    #[test]
    fn python_reports_an_unmatched_pin_with_its_source() {
        let err = resolve(
            Language::Python,
            "3.99",
            "/ws/.python-version",
            None,
            Arch::X64,
            &fetcher(PYTHON_INDEX_JSON),
        )
        .expect_err("no such version");
        let msg = err.to_string();
        assert!(msg.contains("3.99"), "{msg}");
        assert!(msg.contains("/ws/.python-version"), "{msg}");
    }
}
