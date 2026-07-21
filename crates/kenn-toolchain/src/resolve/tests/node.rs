//! `resolve` tests for node. Shared `tarball`/`fetcher` helpers come from
//! the parent test module.

use super::super::*;
use super::tarball;
use crate::pin::Language;

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
