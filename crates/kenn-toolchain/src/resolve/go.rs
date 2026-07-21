use super::common::{Arch, Install, ResolveError, Resolved, LATEST};

const GO_METADATA: &str = "https://go.dev/dl/?mode=json&include=all";
/// Go publishes only a filename, so the base is ours. It is a fixed constant
/// rather than a per-artifact guess, and the filename itself is authoritative.
const GO_DOWNLOAD_BASE: &str = "https://go.dev/dl/";

pub(super) fn resolve_go(
    pin: &str,
    pin_source: &str,
    arch: Arch,
    fetch_text: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Resolved, ResolveError> {
    let body = fetch_text(GO_METADATA).map_err(|message| ResolveError::Metadata {
        language: "go",
        url: GO_METADATA.to_string(),
        message,
    })?;
    let doc: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| ResolveError::Metadata {
            language: "go",
            url: GO_METADATA.to_string(),
            message: e.to_string(),
        })?;

    // go.dev spells versions with a `go` prefix; our pins are normalized without.
    let want = format!("go{}", pin.trim_start_matches("go"));
    let release = doc.as_array().into_iter().flatten().find(|r| {
        if pin == LATEST {
            // The list is newest-first, so the first stable entry is the latest.
            r.get("stable").and_then(serde_json::Value::as_bool) == Some(true)
        } else {
            r.get("version").and_then(serde_json::Value::as_str) == Some(want.as_str())
        }
    });

    let file = release.and_then(|r| {
        r.get("files")?.as_array()?.iter().find(|f| {
            f.get("kind").and_then(serde_json::Value::as_str) == Some("archive")
                && f.get("os").and_then(serde_json::Value::as_str) == Some("linux")
                && f.get("arch").and_then(serde_json::Value::as_str) == Some(arch.go_arch())
        })
    });

    let (Some(file), Some(filename), Some(sha256)) = (
        file,
        file.and_then(|f| f.get("filename"))
            .and_then(serde_json::Value::as_str),
        file.and_then(|f| f.get("sha256"))
            .and_then(serde_json::Value::as_str),
    ) else {
        return Err(ResolveError::NoMatch {
            language: "go",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: if release.is_some() {
                format!(
                    " — release exists but publishes no linux/{} archive",
                    arch.go_arch()
                )
            } else {
                String::new()
            },
        });
    };
    let _ = file;

    Ok(Resolved {
        // Under LATEST the pin is a sentinel, so the CONCRETE version has to
        // come from the release we picked — the cache key must never be
        // "*latest*", or every new release would silently reuse the old install.
        version: release
            .and_then(|r| r.get("version"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(pin)
            .trim_start_matches("go")
            .to_string(),
        install: Install::Tarball {
            url: format!("{GO_DOWNLOAD_BASE}{filename}"),
            digest_hex: sha256.to_string(),
            digest_is_sha512: false,
            // The tarball wraps everything in a `go/` directory.
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

    const GO_JSON: &str = r#"[
          {"version":"go1.26.5","stable":true,"files":[
            {"filename":"go1.26.5.linux-arm64.tar.gz","os":"linux","arch":"arm64",
             "version":"go1.26.5","sha256":"fe4789e9","size":1,"kind":"archive"}]},
          {"version":"go1.24.5","stable":true,"files":[
            {"filename":"go1.24.5.src.tar.gz","os":"","arch":"","version":"go1.24.5",
             "sha256":"src000","size":1,"kind":"source"},
            {"filename":"go1.24.5.linux-amd64.tar.gz","os":"linux","arch":"amd64",
             "version":"go1.24.5","sha256":"amd64hash","size":1,"kind":"archive"},
            {"filename":"go1.24.5.linux-arm64.tar.gz","os":"linux","arch":"arm64",
             "version":"go1.24.5","sha256":"arm64hash","size":1,"kind":"archive"}]}
        ]"#;

    #[test]
    fn go_selects_the_archive_for_the_pinned_version_and_arch() {
        let got = resolve(
            Language::Go,
            "1.24.5",
            "go.mod",
            None,
            Arch::Arm64,
            &fetcher(GO_JSON),
        )
        .expect("resolve");
        assert_eq!(got.version, "1.24.5");
        assert_eq!(
            tarball(&got).0,
            "https://go.dev/dl/go1.24.5.linux-arm64.tar.gz"
        );
        assert_eq!(tarball(&got).1, "arm64hash");
        assert!(!tarball(&got).2);
        assert_eq!(tarball(&got).3, 1);

        let amd = resolve(
            Language::Go,
            "1.24.5",
            "go.mod",
            None,
            Arch::X64,
            &fetcher(GO_JSON),
        )
        .expect("resolve");
        assert_eq!(tarball(&amd).1, "amd64hash");
    }

    /// `kind: source` is not a toolchain. Selecting it would download something
    /// that unpacks fine and then fails much later as a missing `bin/go`.
    #[test]
    fn go_ignores_the_source_archive() {
        let got = resolve(
            Language::Go,
            "1.24.5",
            "go.mod",
            None,
            Arch::X64,
            &fetcher(GO_JSON),
        )
        .expect("resolve");
        assert!(
            tarball(&got).0.ends_with("linux-amd64.tar.gz"),
            "{}",
            tarball(&got).0
        );
    }

    /// An unmatched pin must name the pin AND where it came from — "not found"
    /// alone sends people hunting in the wrong file.
    #[test]
    fn go_reports_the_pin_and_its_source_when_nothing_matches() {
        let err = resolve(
            Language::Go,
            "1.99.0",
            "/ws/go.mod",
            None,
            Arch::X64,
            &fetcher(GO_JSON),
        )
        .expect_err("no such version");
        let msg = err.to_string();
        assert!(msg.contains("1.99.0"), "{msg}");
        assert!(msg.contains("/ws/go.mod"), "{msg}");
    }

    #[test]
    fn go_accepts_a_pin_written_with_the_go_prefix() {
        let got = resolve(
            Language::Go,
            "go1.24.5",
            "go.mod",
            None,
            Arch::X64,
            &fetcher(GO_JSON),
        )
        .expect("resolve");
        assert_eq!(got.version, "1.24.5", "the prefix is normalized away");
    }
}
