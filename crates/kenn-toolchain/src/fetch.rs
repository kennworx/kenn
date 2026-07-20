//! Fetching, verifying, and unpacking a toolchain archive.
//!
//! # Verify before unpack, not after
//!
//! The archive is downloaded to a file, hashed, and only unpacked once the hash
//! matches. Streaming straight into the staging directory would be cheaper on
//! disk, but then "verified" would mean "we wrote 600 MB and then checked" —
//! the bytes would already have been through the tar reader, which is the part
//! most worth not feeding untrusted input to.
//!
//! TLS authenticates the server; it says nothing about the bytes. Every vendor
//! publishes a hash next to the artifact, so there is no case where we cannot
//! check one.
//!
//! Only gzipped tarballs are handled — .NET, Swift, Go and Node all ship
//! `.tar.gz` for Linux, so a second decompressor would have no reachable case.

use std::io::Read;
use std::path::Path;

use sha2::{Sha256, Sha512};

/// A toolchain download is hundreds of megabytes over a link we do not control;
/// a stall must fail the run rather than hang the indexer forever.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The hash a vendor publishes for an artifact, tagged with its algorithm.
///
/// Not every vendor uses SHA-256. Measured across the six toolchains we
/// provision: Rust, Go, Node and python-build-standalone publish SHA-256, but
/// **.NET publishes SHA-512** (`sdks[].files[].hash` is 128 hex chars, and the
/// `.sha256` sidecar 404s). Hard-coding one algorithm would have meant either
/// dropping verification for .NET or silently comparing a SHA-256 against a
/// SHA-512 and always failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Digest<'a> {
    Sha256(&'a str),
    Sha512(&'a str),
}

impl<'a> Digest<'a> {
    fn expected(self) -> &'a str {
        match self {
            Digest::Sha256(h) | Digest::Sha512(h) => h,
        }
    }

    fn algorithm(self) -> &'static str {
        match self {
            Digest::Sha256(_) => "sha256",
            Digest::Sha512(_) => "sha512",
        }
    }
}

/// What to fetch, and the digest it must have — both taken from the vendor's
/// release metadata.
///
/// On URLs: Rust, .NET and python-build-standalone publish absolute URLs. Go and
/// Node publish only the *filename*, which still beats inventing one — the
/// filename is authoritative and only the fixed base is ours. Swift publishes
/// neither, which is called out where its resolver lives.
#[derive(Debug, Clone, Copy)]
pub struct Artifact<'a> {
    pub url: &'a str,
    pub digest: Digest<'a>,
    /// Leading path components to drop, like `tar --strip-components`. Most
    /// toolchain tarballs wrap everything in one versioned directory.
    pub strip_components: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("fetching {url}: {source}")]
    Http {
        url: String,
        #[source]
        source: Box<ureq::Error>,
    },
    #[error("fetching {url}: server returned HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("fetching {url}: {source}")]
    Io {
        url: String,
        #[source]
        source: std::io::Error,
    },
    /// The bytes are not what the vendor published. Nothing has been unpacked.
    #[error("{url}: {algorithm} mismatch: expected {expected}, got {actual}")]
    Checksum {
        url: String,
        algorithm: &'static str,
        expected: String,
        actual: String,
    },
    #[error("unpacking {url}: {source}")]
    Unpack {
        url: String,
        #[source]
        source: std::io::Error,
    },
    /// A tar entry resolved outside the destination — a path-traversal archive.
    #[error("unpacking {url}: entry {entry} escapes the destination directory")]
    Escapes { url: String, entry: String },
}

/// Download `artifact`, verify its checksum, and unpack it into `dest`.
///
/// `dest` is a staging directory owned by the caller: on any error here it is
/// discarded, so a failed fetch never reaches the cache.
pub fn fetch_verified(artifact: Artifact<'_>, dest: &Path) -> Result<(), FetchError> {
    // The archive lands beside the staging tree, on the same filesystem, and is
    // removed once unpacked.
    let archive_path = dest.join(".archive.tar.gz");
    download(artifact.url, &archive_path)?;
    verify(artifact.url, &archive_path, artifact.digest)?;

    let file = std::fs::File::open(&archive_path).map_err(|source| FetchError::Io {
        url: artifact.url.to_string(),
        source,
    })?;
    unpack_tar_gz(artifact.url, file, dest, artifact.strip_components)?;

    // Best effort: the staging tree is discarded wholesale on any failure, and
    // renamed into the cache on success, so a leftover archive would only waste
    // space in the cache.
    drop(std::fs::remove_file(&archive_path));
    Ok(())
}

fn download(url: &str, to: &Path) -> Result<(), FetchError> {
    let agent = ureq::Agent::config_builder()
        .timeout_recv_body(Some(READ_TIMEOUT))
        .build()
        .new_agent();

    let response = agent.get(url).call().map_err(|e| FetchError::Http {
        url: url.to_string(),
        source: Box::new(e),
    })?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(FetchError::Status {
            url: url.to_string(),
            status,
        });
    }

    let io_err = |source| FetchError::Io {
        url: url.to_string(),
        source,
    };
    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(to).map_err(io_err)?;
    std::io::copy(&mut reader, &mut file).map_err(io_err)?;
    Ok(())
}

/// Check a downloaded file against the vendor's published hash.
///
/// An empty `expected` is a failure, not a skip: "the vendor did not publish a
/// hash" must never silently degrade into "install it anyway". Split out from
/// [`fetch_verified`] so this — the security-critical step — is testable without
/// a network.
fn verify(url: &str, path: &Path, digest: Digest<'_>) -> Result<(), FetchError> {
    let expected = digest.expected();
    let actual = match digest {
        Digest::Sha256(_) => hash_file::<Sha256>(url, path)?,
        Digest::Sha512(_) => hash_file::<Sha512>(url, path)?,
    };
    if expected.is_empty() || !actual.eq_ignore_ascii_case(expected) {
        return Err(FetchError::Checksum {
            url: url.to_string(),
            algorithm: digest.algorithm(),
            expected: if expected.is_empty() {
                "<none published>".to_string()
            } else {
                expected.to_string()
            },
            actual,
        });
    }
    Ok(())
}

/// Lowercase hex digest of a file, read in chunks so a 600 MB archive is never
/// resident in memory.
fn hash_file<D: sha2::Digest>(url: &str, path: &Path) -> Result<String, FetchError> {
    let io_err = |source| FetchError::Io {
        url: url.to_string(),
        source,
    };
    let mut file = std::fs::File::open(path).map_err(io_err)?;
    let mut hasher = D::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(io_err)?;
        if n == 0 {
            break;
        }
        // `get(..n)` rather than `buf[..n]`: `Read::read` contracts to return at
        // most `buf.len()`, but the slice form would panic rather than error if
        // an implementation ever broke that.
        let Some(chunk) = buf.get(..n) else {
            return Err(FetchError::Io {
                url: url.to_string(),
                source: std::io::Error::other("reader returned more bytes than requested"),
            });
        };
        hasher.update(chunk);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // `from_digit` is infallible for a nibble in radix 16; `extend` over the
        // Option keeps that fact from needing an unwrap.
        s.extend(char::from_digit(u32::from(b >> 4), 16));
        s.extend(char::from_digit(u32::from(b & 0x0f), 16));
    }
    s
}

/// Unpack a gzipped tar into `dest`, dropping `strip_components` leading path
/// segments from every entry.
fn unpack_tar_gz<R: Read>(
    url: &str,
    reader: R,
    dest: &Path,
    strip_components: usize,
) -> Result<(), FetchError> {
    let unpack_err = |source| FetchError::Unpack {
        url: url.to_string(),
        source,
    };

    let decoder = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(true);

    for entry in archive.entries().map_err(unpack_err)? {
        let mut entry = entry.map_err(unpack_err)?;
        let path = entry.path().map_err(unpack_err)?.into_owned();

        let Some(relative) = strip(&path, strip_components) else {
            // Fewer components than we strip: the wrapper directory itself,
            // which has no content to place.
            continue;
        };

        // A downloaded tarball is untrusted input even after a checksum match —
        // the hash proves it is what the vendor published, not that it is safe.
        //
        // Validate the RELATIVE path's components, not the joined path. A
        // lexical `dest.join(x).starts_with(dest)` is not a containment check:
        // `Path::starts_with` compares components without normalizing, so
        // `dest/../escaped` starts with `dest` and sails through.
        if !is_contained(&relative) {
            return Err(FetchError::Escapes {
                url: url.to_string(),
                entry: path.display().to_string(),
            });
        }
        let target = dest.join(&relative);

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(unpack_err)?;
        }
        entry.unpack(&target).map_err(unpack_err)?;
    }

    Ok(())
}

/// Whether `relative` stays inside the directory it will be joined to: made only
/// of normal segments, with no `..` and no absolute-path root.
fn is_contained(relative: &Path) -> bool {
    use std::path::Component;
    relative
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Drop the first `n` components of `path`, or `None` when nothing would remain.
fn strip(path: &Path, n: usize) -> Option<std::path::PathBuf> {
    let mut components = path.components();
    for _ in 0..n {
        components.next()?;
    }
    let rest: std::path::PathBuf = components.collect();
    if rest.as_os_str().is_empty() {
        None
    } else {
        Some(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, *contents)
                .expect("append");
        }
        gzip(&builder.into_inner().expect("finish tar"))
    }

    /// `tar::Builder` refuses to write a `..` path, so a traversal archive has
    /// to be assembled from raw 512-byte blocks — which is exactly what a
    /// hostile publisher would do.
    fn traversal_tar_gz(name: &str, contents: &[u8]) -> Vec<u8> {
        let mut header = [b'\0'; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        // mode, uid, gid
        header[100..107].copy_from_slice(b"000644 ");
        header[108..115].copy_from_slice(b"000000 ");
        header[116..123].copy_from_slice(b"000000 ");
        // size and mtime, octal, NUL-terminated
        let size = format!("{:011o}\0", contents.len());
        header[124..136].copy_from_slice(size.as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[156] = b'0'; // typeflag: regular file
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        // Checksum is computed with the checksum field read as spaces.
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|b| u32::from(*b)).sum();
        let chksum = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(chksum.as_bytes());

        let mut tar = header.to_vec();
        tar.extend_from_slice(contents);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
        tar.extend_from_slice(&[0u8; 1024]); // end-of-archive
        gzip(&tar)
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(bytes).expect("gzip");
        enc.finish().expect("finish gzip")
    }

    #[test]
    fn unpacks_entries_into_the_destination() {
        let archive = tar_gz(&[("bin/dotnet", b"binary"), ("LICENSE", b"text")]);
        let tmp = tempfile::tempdir().expect("tempdir");

        unpack_tar_gz("test://x", archive.as_slice(), tmp.path(), 0).expect("unpack");

        assert_eq!(
            std::fs::read_to_string(tmp.path().join("bin/dotnet")).unwrap(),
            "binary"
        );
    }

    #[test]
    fn strip_components_drops_the_wrapper_directory() {
        let archive = tar_gz(&[("go/bin/go", b"binary"), ("go/VERSION", b"go1.24.0")]);
        let tmp = tempfile::tempdir().expect("tempdir");

        unpack_tar_gz("test://x", archive.as_slice(), tmp.path(), 1).expect("unpack");

        assert!(tmp.path().join("bin/go").is_file(), "wrapper must be gone");
        assert!(!tmp.path().join("go").exists());
    }

    /// A checksum match proves provenance, not safety. An entry that climbs out
    /// of the destination must be refused, not written.
    ///
    /// Mutation-checked: dropping the `starts_with(dest)` guard lets the entry
    /// through and writes `escaped` outside the destination.
    #[test]
    fn a_traversing_entry_is_refused() {
        let archive = traversal_tar_gz("../escaped", b"pwned");
        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).expect("mkdir");

        let err = unpack_tar_gz("test://x", archive.as_slice(), &dest, 0)
            .expect_err("traversal must be refused");

        assert!(matches!(err, FetchError::Escapes { .. }), "{err}");
        assert!(
            !tmp.path().join("escaped").exists(),
            "nothing may be written outside the destination"
        );
    }

    #[test]
    fn a_truncated_archive_is_an_error_not_a_partial_success() {
        let archive = tar_gz(&[("bin/dotnet", b"binary")]);
        let tmp = tempfile::tempdir().expect("tempdir");

        unpack_tar_gz("test://x", &archive[..archive.len() / 2], tmp.path(), 0)
            .expect_err("a truncated archive must fail");
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = tmp.path().join("abc");
        std::fs::write(&f, b"abc").expect("write");
        assert_eq!(
            hash_file::<Sha256>("test://x", &f).expect("hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// The bytes must be what the vendor published. TLS authenticates the
    /// server, not the payload.
    ///
    /// Mutation-checked: replacing the comparison with `Ok(())` makes both of
    /// these pass a corrupted artifact through to the unpacker.
    #[test]
    fn a_mismatched_checksum_is_refused() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = tmp.path().join("artifact");
        std::fs::write(&f, b"abc").expect("write");

        let err = verify("test://x", &f, Digest::Sha256(&"0".repeat(64)))
            .expect_err("mismatch must be refused");
        assert!(matches!(err, FetchError::Checksum { .. }), "{err}");
        // The real hash is reported, so the failure is diagnosable.
        assert!(err.to_string().contains("ba7816bf"), "{err}");

        verify(
            "test://x",
            &f,
            Digest::Sha256("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        )
        .expect("a matching hash verifies");
    }

    /// "No hash published" must fail, not skip. A vendor page that stops serving
    /// hashes would otherwise silently turn verification off everywhere.
    #[test]
    fn an_absent_published_checksum_is_refused() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = tmp.path().join("artifact");
        std::fs::write(&f, b"abc").expect("write");

        let err =
            verify("test://x", &f, Digest::Sha256("")).expect_err("an absent hash must be refused");
        assert!(matches!(err, FetchError::Checksum { .. }), "{err}");
    }

    /// .NET publishes SHA-512, not SHA-256 — `sdks[].files[].hash` is 128 hex
    /// chars and the `.sha256` sidecar 404s. Verified against the standard
    /// SHA-512("abc") vector.
    ///
    /// Mutation-checked: hashing with Sha256 for both arms makes this fail, and
    /// the wrong-algorithm case below guards against comparing across families.
    #[test]
    fn sha512_is_supported_for_vendors_that_publish_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = tmp.path().join("artifact");
        std::fs::write(&f, b"abc").expect("write");

        verify(
            "test://x",
            &f,
            Digest::Sha512(
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
                 2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
        )
        .expect("sha512 must verify");

        // A SHA-256 digest offered as SHA-512 must not accidentally pass.
        let err = verify(
            "test://x",
            &f,
            Digest::Sha512("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        )
        .expect_err("a sha256 value must not verify as sha512");
        assert!(err.to_string().contains("sha512"), "{err}");
    }

    /// Vendors publish hex in either case; a valid artifact must not be rejected
    /// over presentation.
    #[test]
    fn checksum_comparison_is_case_insensitive() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = tmp.path().join("artifact");
        std::fs::write(&f, b"abc").expect("write");

        verify(
            "test://x",
            &f,
            Digest::Sha256("BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"),
        )
        .expect("uppercase hex must verify");
    }

    #[test]
    fn strip_returns_none_when_nothing_would_remain() {
        assert!(strip(Path::new("go"), 1).is_none());
        assert_eq!(
            strip(Path::new("go/bin/go"), 1).unwrap(),
            Path::new("bin/go")
        );
    }
}
