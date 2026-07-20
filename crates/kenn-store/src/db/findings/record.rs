//! Finding record files — the committed source of truth (committed-findings D1).
//!
//! Each finding is one Markdown file, `.kenn/findings/<id>.md`: immutable
//! YAML frontmatter (`id`, `tags`, `parent_ids`, `created_at`) followed by the
//! prose body. The body **is** the finding's `text` — it is the embedding
//! source, written byte-for-byte so the text fingerprint matches the in-memory
//! `Finding.text` (embeddings reuse). The `embedding` is **not** in the record:
//! it is derived from the findings vector sidecar (D3), never committed.
//!
//! The `fnd_<uuid>` id is the filename, so a record is write-once and
//! uniquely named — two branches' findings `git merge` as a plain union
//! of files, with no conflict and no binary-union healing.

use std::fs::{self, DirEntry, File};
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::api::types::{DbError, Finding};

/// The immutable YAML frontmatter of a `.kenn/findings/<id>.md` record — a
/// finding's authored fields *except* the prose, which is the body. The
/// `embedding` is intentionally absent; it is reconciled from the findings
/// vector sidecar on rebuild (D3).
#[derive(Debug, Serialize, Deserialize)]
struct FrontMatter {
    /// `"fnd_"` + a hyphenated `UUIDv4` — also the file's name.
    id: String,
    /// Free-form classification + `supersedes:` / `tombstone:` markers.
    #[serde(default)]
    tags: Vec<String>,
    /// Provenance edges — code-graph node ids and/or finding ids.
    #[serde(default)]
    parent_ids: Vec<String>,
    /// Creation time (UTC) — serialized as an RFC 3339 string.
    created_at: crate::clock::Timestamp,
}

impl FrontMatter {
    /// Project a [`Finding`]'s frontmatter fields — the prose (`text`) and the
    /// derived `embedding` are dropped.
    fn from_finding(finding: &Finding) -> Self {
        Self {
            id: finding.id.clone(),
            tags: finding.tags.clone(),
            parent_ids: finding.parent_ids.clone(),
            created_at: finding.created_at,
        }
    }
}

/// Serialize a finding to its Markdown record text: `---\n<yaml>---\n<body>`.
/// The body is the finding's `text`, written verbatim so the text fingerprint
/// is stable across the json→md migration.
fn to_markdown(finding: &Finding) -> Result<String, DbError> {
    let front = FrontMatter::from_finding(finding);
    let yaml = serde_yaml_ng::to_string(&front)
        .map_err(|e| DbError::Backend(format!("serialize finding {}: {e}", finding.id)))?;
    // `serde_yaml_ng` emits a bare mapping (no `---` doc marker); strip one
    // defensively so the framing is always exactly one `---` pair.
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    // `serde_yaml_ng` terminates its output with a newline; ensure it so the
    // closing `---` always sits on its own line (and `from_markdown`'s
    // `\n---\n` split holds) even if that serializer behavior ever changes.
    let sep = if yaml.ends_with('\n') { "" } else { "\n" };
    Ok(format!("---\n{yaml}{sep}---\n{}", finding.text))
}

/// Parse a Markdown record's bytes into a [`Finding`] (`embedding` left
/// `None`). Returns `None` if the frontmatter framing or YAML is malformed —
/// the caller skips it with a warning rather than failing the open.
fn from_markdown(bytes: &[u8]) -> Option<Finding> {
    let text = std::str::from_utf8(bytes).ok()?;
    let rest = text.strip_prefix("---\n")?;
    // The closing delimiter is the first `\n---\n` after the opening; our
    // frontmatter never contains one, and the body after it is verbatim even
    // if the prose itself contains a `---` line. `split_once` avoids manual
    // byte indexing into the string.
    let (front_yaml, body) = rest.split_once("\n---\n")?;
    let front: FrontMatter = serde_yaml_ng::from_str(front_yaml).ok()?;
    Some(Finding {
        id: front.id,
        text: body.to_owned(),
        embedding: None,
        tags: front.tags,
        parent_ids: front.parent_ids,
        created_at: front.created_at,
    })
}

/// Write `finding` to its `<id>.md` record under `dir`, atomically:
/// serialize to `<dir>/.tmp/<id>.md`, fsync that file, then rename it
/// onto `<dir>/<id>.md`. The record is write-once — the `fnd_<uuid>`
/// id never collides — so a crash leaves at most a stray file under
/// `.tmp/`, never a torn record. The caller fsyncs `dir` once after a
/// batch via [`fsync_dir`].
///
/// Why `.tmp/` is a subdir of `dir` rather than the records dir itself:
/// `dir` (`.kenn/findings/`) is tracked in git, so a crashed `.md.tmp`
/// sibling shows up as untracked debris. Tucking the in-flight writes
/// into a `.tmp/` subdir lets a single `findings/.tmp/` gitignore entry
/// cover every transient artifact. The subdir is a child of the
/// records dir, so `fs::rename` stays same-filesystem and atomic by
/// construction — this holds even when `derived_root` is configured
/// to a sync-backed location outside the repo (Dropbox/NFS/etc.), where
/// any tmp parented on the derived side would suffer sync round-trips.
pub(super) fn write_record(dir: &Path, finding: &Finding) -> Result<(), DbError> {
    let markdown = to_markdown(finding)?;
    let final_path = dir.join(format!("{}.md", finding.id));
    // `.tmp/` was created at `FindingsStore::open` time via
    // [`ensure_tmp_dir`] — no per-write `create_dir_all` here.
    let tmp_path = dir.join(".tmp").join(format!("{}.md", finding.id));

    let mut file = File::create(&tmp_path).map_err(DbError::Io)?;
    file.write_all(markdown.as_bytes()).map_err(DbError::Io)?;
    file.sync_all().map_err(DbError::Io)?;
    drop(file);
    fs::rename(&tmp_path, &final_path).map_err(DbError::Io)?;
    Ok(())
}

/// Create the `<dir>/.tmp/` write-staging subdir. Idempotent — safe to
/// call once at `FindingsStore::open` time. Hoisting this out of the
/// per-record path saves one `create_dir_all` syscall per finding in a
/// `flush` batch.
pub(super) fn ensure_tmp_dir(dir: &Path) -> Result<(), DbError> {
    fs::create_dir_all(dir.join(".tmp")).map_err(DbError::Io)
}

/// Best-effort one-shot sweep of pre-`.tmp/`-subdir `<id>.md.tmp`
/// debris in `dir`. Older builds parked the write-staging file as a
/// sibling of the final record; a crash mid-write left it behind
/// as a `git status` annoyance. After this sweep ran once on a repo
/// post-upgrade, no further legacy files can be produced. Errors are
/// logged at debug, not surfaced — the store opens fine either way.
pub(super) fn sweep_legacy_tmp(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".md.tmp") {
            let path = entry.path();
            if let Err(e) = fs::remove_file(&path) {
                tracing::debug!(
                    "skipping legacy findings tmp sweep for {}: {e}",
                    path.display()
                );
            }
        }
    }
}

/// fsync `dir` so the record renames durably reach disk — the commit
/// point of a `flush` (D4).
pub(super) fn fsync_dir(dir: &Path) -> Result<(), DbError> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(DbError::Io)
}

/// Load every finding record directly under `dir` — the `*.md` files,
/// not the `vectors/` sidecar subdir nor any in-flight `.tmp/` writes.
///
/// Returns the parsed findings (each `embedding` left `None`) plus the
/// staleness signature — the record-file **count** and the **newest
/// record mtime** — computed in the same `read_dir` pass so they
/// describe exactly the set that was read. An unparseable or unreadable
/// record is skipped with a warning rather than failing the open (D4
/// defense-in-depth), but it still counts toward the signature so a
/// permanently-broken record does not force an endless rebuild.
pub(super) fn read_records(dir: &Path) -> Result<(Vec<Finding>, usize, SystemTime), DbError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), 0, SystemTime::UNIX_EPOCH))
        }
        Err(e) => return Err(DbError::Io(e)),
    };

    let mut findings = Vec::new();
    let mut count = 0;
    let mut newest = SystemTime::UNIX_EPOCH;
    for entry in entries {
        let entry = entry.map_err(DbError::Io)?;
        if !is_record_file(&entry)? {
            continue;
        }
        count += 1;
        if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
            newest = newest.max(mtime);
        }
        let path = entry.path();
        match fs::read(&path) {
            Ok(bytes) => match from_markdown(&bytes) {
                Some(finding) => findings.push(finding),
                None => {
                    tracing::warn!("skipping unparseable finding record {}", path.display());
                }
            },
            Err(e) => {
                tracing::warn!("skipping unreadable finding record {}: {e}", path.display());
            }
        }
    }
    // A deterministic rebuild order — record files are uuid-named.
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    Ok((findings, count, newest))
}

/// Read a single committed record by id (`<dir>/<id>.md`). `None` when the
/// file is absent or unparseable. Lets a bounded result set be resolved
/// without scanning the whole corpus.
pub(super) fn read_record(dir: &Path, id: &str) -> Option<Finding> {
    let bytes = fs::read(dir.join(format!("{id}.md"))).ok()?;
    from_markdown(&bytes)
}

/// Whether a directory entry is a finding record — a regular file whose
/// extension is `md`. This excludes the `.tmp/` subdir that holds in-flight
/// writes and the `vectors/` sidecar subdirectory.
fn is_record_file(entry: &DirEntry) -> Result<bool, DbError> {
    if !entry.file_type().map_err(DbError::Io)?.is_file() {
        return Ok(false);
    }
    Ok(Path::new(&entry.file_name())
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md")))
}

#[cfg(test)]
mod tests {
    use super::{ensure_tmp_dir, read_records, sweep_legacy_tmp, write_record};
    use crate::api::types::Finding;

    fn finding(id: &str, text: &str) -> Finding {
        Finding {
            id: id.to_owned(),
            text: text.to_owned(),
            embedding: Some(vec![0.5_f32; 4]),
            tags: vec!["invariant".to_owned()],
            parent_ids: vec!["rust:m::f".to_owned()],
            created_at: time::OffsetDateTime::from_unix_timestamp(1_700_000_000)
                .unwrap()
                .into(),
        }
    }

    /// Mirror `FindingsStore::open`'s pre-call setup so unit tests can
    /// exercise `write_record` without standing up the full store.
    fn prep(dir: &std::path::Path) {
        ensure_tmp_dir(dir).unwrap();
    }

    #[test]
    fn record_round_trips_without_the_embedding() {
        let dir = tempfile::tempdir().unwrap();
        prep(dir.path());
        write_record(dir.path(), &finding("fnd_a", "a durable fact")).unwrap();

        let (findings, count, _) = read_records(dir.path()).unwrap();
        assert_eq!(count, 1);
        let got = findings.first().expect("one finding");
        assert_eq!(got.id, "fnd_a");
        assert_eq!(got.text, "a durable fact");
        assert_eq!(got.tags, vec!["invariant".to_owned()]);
        assert_eq!(got.parent_ids, vec!["rust:m::f".to_owned()]);
        // The authored embedding is not committed — it is derived.
        assert!(got.embedding.is_none(), "the record omits the embedding");
    }

    /// A multi-line body containing a `---` line round-trips intact — the
    /// closing frontmatter delimiter is the first one, the body is verbatim.
    #[test]
    fn body_with_dashes_and_newlines_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        prep(dir.path());
        let text = "line one\n\n---\nnot frontmatter\n**Why:** because.";
        write_record(dir.path(), &finding("fnd_md", text)).unwrap();

        let (findings, _, _) = read_records(dir.path()).unwrap();
        assert_eq!(findings.first().unwrap().text, text);
    }

    /// Positive layout check: `write_record` lands `<id>.md` in `dir`
    /// and leaves nothing behind in `<dir>/.tmp/`.
    #[test]
    fn write_record_lands_final_file_and_clears_staging() {
        let dir = tempfile::tempdir().unwrap();
        prep(dir.path());
        write_record(dir.path(), &finding("fnd_x", "hello")).unwrap();

        assert!(
            dir.path().join("fnd_x.md").is_file(),
            "final record must exist at <dir>/<id>.md"
        );
        let staging = dir.path().join(".tmp");
        assert!(staging.is_dir(), ".tmp/ staging subdir must exist");
        let leftover: Vec<_> = std::fs::read_dir(&staging).unwrap().flatten().collect();
        assert!(
            leftover.is_empty(),
            ".tmp/ must be empty after a successful write — leftover: {:?}",
            leftover
                .iter()
                .map(std::fs::DirEntry::file_name)
                .collect::<Vec<_>>()
        );
    }

    /// `sweep_legacy_tmp` removes pre-`.tmp/`-subdir-layout `*.md.tmp`
    /// debris while leaving real records and the staging subdir.
    #[test]
    fn sweep_removes_only_legacy_tmp_debris() {
        let dir = tempfile::tempdir().unwrap();
        prep(dir.path());
        write_record(dir.path(), &finding("fnd_keep", "real")).unwrap();
        std::fs::write(dir.path().join("fnd_old.md.tmp"), b"orphan").unwrap();
        std::fs::write(dir.path().join("note.txt"), b"unrelated").unwrap();

        sweep_legacy_tmp(dir.path());

        assert!(
            dir.path().join("fnd_keep.md").is_file(),
            "real record survives the sweep"
        );
        assert!(
            !dir.path().join("fnd_old.md.tmp").exists(),
            "legacy *.md.tmp must be removed"
        );
        assert!(
            dir.path().join("note.txt").is_file(),
            "non-matching files survive"
        );
        assert!(
            dir.path().join(".tmp").is_dir(),
            "the staging subdir is left alone"
        );
    }

    #[test]
    fn read_skips_an_unparseable_record_and_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        prep(dir.path());
        write_record(dir.path(), &finding("fnd_ok", "good")).unwrap();
        // A corrupt record — skipped with a warning, not a hard error.
        std::fs::write(dir.path().join("fnd_bad.md"), b"not a frontmatter doc").unwrap();
        // The `vectors/` sidecar subdir is never read as a record.
        std::fs::create_dir(dir.path().join("vectors")).unwrap();

        let (findings, count, _) = read_records(dir.path()).unwrap();
        // Both `*.md` files count; only the parseable one inflates.
        assert_eq!(count, 2, "both .md files count toward staleness");
        assert_eq!(findings.len(), 1, "the corrupt record is skipped");
        assert_eq!(findings[0].id, "fnd_ok");
    }
}
