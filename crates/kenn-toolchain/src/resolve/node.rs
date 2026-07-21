use super::common::{Arch, Install, ResolveError, Resolved, LATEST};

const NODE_INDEX: &str = "https://nodejs.org/dist/index.json";
const NODE_DIST: &str = "https://nodejs.org/dist/";

/// Node ships glibc builds only — musl exists solely on
/// unofficial-builds.nodejs.org, whose retention and architecture coverage we do
/// not control. That is why the python image is glibc-based.
pub(super) fn resolve_node(
    pin: &str,
    pin_source: &str,
    arch: Arch,
    fetch_text: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Resolved, ResolveError> {
    let meta = |url: &str, message: String| ResolveError::Metadata {
        language: "node",
        url: url.to_string(),
        message,
    };
    let body = fetch_text(NODE_INDEX).map_err(|m| meta(NODE_INDEX, m))?;
    let index: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| meta(NODE_INDEX, e.to_string()))?;

    // Entries are newest-first. An unpinned workspace takes the newest LTS
    // rather than the newest release: `lts` is a codename string on LTS lines
    // and `false` otherwise, and a current-but-not-LTS Node is a worse default
    // for running a third-party indexer.
    let entry = index.as_array().into_iter().flatten().find(|r| {
        let version = r.get("version").and_then(serde_json::Value::as_str);
        if pin == LATEST {
            r.get("lts").is_some_and(serde_json::Value::is_string)
        } else {
            version == Some(format!("v{}", pin.trim_start_matches('v')).as_str())
        }
    });
    let (Some(entry), Some(version)) = (
        entry,
        entry
            .and_then(|e| e.get("version"))
            .and_then(serde_json::Value::as_str),
    ) else {
        return Err(ResolveError::NoMatch {
            language: "node",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: String::new(),
        });
    };
    // `files` lists platforms, not filenames — presence check only.
    let has_platform = entry
        .get("files")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|f| f.iter().any(|v| v.as_str() == Some(arch.node_platform())));
    if !has_platform {
        return Err(ResolveError::NoMatch {
            language: "node",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: format!(" — {version} publishes no {} build", arch.node_platform()),
        });
    }

    // The checksum lives in a separate per-version manifest, and its left column
    // is the AUTHORITATIVE filename — so the URL is a fixed base joined to a
    // published name rather than a name we invented.
    let sums_url = format!("{NODE_DIST}{version}/SHASUMS256.txt");
    let sums = fetch_text(&sums_url).map_err(|m| meta(&sums_url, m))?;
    let want = format!("node-{version}-{}.tar.gz", arch.node_platform());
    let sha = sums.lines().find_map(|line| {
        let (sha, name) = line.split_once("  ")?;
        (name.trim() == want).then(|| sha.to_string())
    });
    let Some(sha) = sha else {
        return Err(ResolveError::NoMatch {
            language: "node",
            pin: pin.to_string(),
            pin_source: pin_source.to_string(),
            detail: format!(" — {want} is absent from SHASUMS256.txt"),
        });
    };

    Ok(Resolved {
        version: version.trim_start_matches('v').to_string(),
        install: Install::Tarball {
            url: format!("{NODE_DIST}{version}/{want}"),
            digest_hex: sha,
            digest_is_sha512: false,
            // Wrapped in `node-v<ver>-<platform>/`.
            strip_components: 1,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pin::Language;
    use crate::resolve::resolve;
    use crate::resolve::testutil::tarball;

    const NODE_INDEX_JSON: &str = r#"[
          {"version":"v24.4.0","files":["linux-x64","linux-arm64"],"lts":false},
          {"version":"v22.20.0","files":["linux-x64","linux-arm64"],"lts":"Jod"},
          {"version":"v20.19.0","files":["linux-x64"],"lts":"Iron"}
        ]"#;
    const NODE_SHASUMS: &str = concat!(
        "aaaa  node-v22.20.0-linux-x64.tar.xz\n",
        "1111  node-v22.20.0-linux-x64.tar.gz\n",
        "2222  node-v22.20.0-linux-arm64.tar.gz\n"
    );

    #[expect(
        clippy::unnecessary_wraps,
        reason = "must match the fetch_text signature it is passed as"
    )]
    fn node_fetcher(url: &str) -> Result<String, String> {
        Ok(if url.contains("SHASUMS") {
            NODE_SHASUMS
        } else {
            NODE_INDEX_JSON
        }
        .to_string())
    }

    #[test]
    fn node_takes_the_gz_checksum_for_the_right_platform() {
        let got = resolve(
            Language::Node,
            "22.20.0",
            "<none>",
            None,
            Arch::Arm64,
            &node_fetcher,
        )
        .expect("resolve");
        assert_eq!(got.version, "22.20.0");
        assert_eq!(tarball(&got).1, "2222");
        assert!(tarball(&got)
            .0
            .ends_with("node-v22.20.0-linux-arm64.tar.gz"));
        // `.tar.xz` shares the platform and sorts first in SHASUMS256.txt;
        // matching on the platform alone would take its checksum.
        assert_ne!(tarball(&got).1, "aaaa");
    }

    /// Unpinned Node takes the newest LTS, not the newest release: a
    /// current-but-not-LTS Node is a worse host for a third-party indexer.
    #[test]
    fn node_defaults_to_the_newest_lts_not_the_newest_release() {
        let got = resolve(
            Language::Node,
            LATEST,
            "<none>",
            None,
            Arch::X64,
            &node_fetcher,
        )
        .expect("resolve");
        assert_eq!(got.version, "22.20.0", "24.4.0 is newer but not LTS");
    }
}
