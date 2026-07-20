//! `recover` and the cross-fs / legacy-directory sweepers run on
//! indexer cold start.

use std::fs;
use std::time::SystemTime;

use crate::layout::Store;

use super::state::incomplete_runs;
use super::types::{RecoveryError, RecoveryReport};

/// Detect and clean up incomplete runs from a crashed indexer pass.
///
/// Per D1, a run directory without `meta.json` is incomplete. On
/// next indexer start, every such run is removed. `live` is
/// unchanged.
pub fn recover(store: &Store) -> Result<RecoveryReport, RecoveryError> {
    let mut report = RecoveryReport::default();
    for run in incomplete_runs(store) {
        fs::remove_dir_all(&run)?;
        report.deleted_incomplete_runs.push(run);
    }
    sweep_cross_fs_tmp(store, &mut report);
    sweep_stale_findings_dir(store);
    Ok(report)
}

/// Best-effort removal of a stale `<derived_root>/findings/` directory
/// left by a prior layout, where the findings Lance mirror lived at the
/// derived root instead of inside the run (D2 / §B). The mirror is now
/// derived per-run from the committed records, so this directory is dead
/// weight — no data loss, the `.kenn/findings/<id>.md` records remain
/// the source of truth. Failure is logged, never fatal.
fn sweep_stale_findings_dir(store: &Store) {
    let stale = store.layout().legacy_findings_dir();
    if !stale.is_dir() {
        return;
    }
    if let Err(e) = fs::remove_dir_all(&stale) {
        tracing::warn!(path = %stale.display(), error = %e, "failed to sweep stale findings dir");
    }
}

/// Cold-start sweep of the cross-fs fallback tmp dir (§5.7 / D8).
///
/// When `Layout::writer_tmp_dir(run_id)` falls back to
/// `<vectors_root>/.tmp/` because the vectors and derived roots are
/// on different filesystems, the per-run cleanup paths can't reach
/// the tmp dir. This sweep removes `*.tmp` files older than one hour
/// from that fallback directory at indexer start.
///
/// No-op in the common case (vectors and derived share a filesystem)
/// — failed-run cleanup already covers the per-run `tmp/`.
fn sweep_cross_fs_tmp(store: &Store, report: &mut RecoveryReport) {
    if store.layout().vectors_share_fs_with_derived() {
        return;
    }
    let tmp_dir = store.layout().vectors_root().join(".tmp");
    let Ok(read) = fs::read_dir(&tmp_dir) else {
        return;
    };
    let threshold = SystemTime::now() - std::time::Duration::from_secs(60 * 60);
    for entry in read.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.ends_with(".tmp") {
            continue;
        }
        let too_old = fs::metadata(entry.path())
            .and_then(|m| m.modified())
            .is_ok_and(|m| m < threshold);
        if too_old && fs::remove_file(entry.path()).is_ok() {
            report.swept_cross_fs_tmp_files += 1;
        }
    }
}
