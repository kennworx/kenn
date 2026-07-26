//! `kenn docker-cache` — list and remove kenn's Docker cache volumes.
//!
//! The docker runtime creates per-repository dependency volumes
//! (`kenn-deps-<hash>`) and per-worktree build volumes (`kenn-build-<hash>`),
//! each labelled `kenn.managed` (enumeration) and, when bound to a directory,
//! `kenn.workspace=<dir>` (orphan-binding). This command reads no `kenn.toml`:
//! it operates purely on those labels and the current worktree root.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Subcommand;
use kenn_indexer::docker::{
    self, daemon_available, list_managed_volumes, remove_volume, ManagedVolume, RemoveOutcome,
    VolumeKind,
};

use crate::exit::ExitCodes;

#[derive(Debug, Subcommand)]
pub enum DockerCacheAction {
    /// List kenn's cache volumes with kind, binding, on-disk existence, and
    /// in-use state.
    Ls {
        /// Emit machine-readable JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Remove cache volumes. With no flag, removes only the current worktree's
    /// build volume. The scope flags are mutually exclusive.
    Clean {
        /// Remove every volume whose bound directory no longer exists (the
        /// teardown reaper: reclaims a dropped worktree's build volume and a
        /// deleted repository's deps volume).
        #[arg(long, group = "scope")]
        orphans: bool,
        /// Remove every kenn cache volume, regardless of binding.
        #[arg(long, group = "scope")]
        all: bool,
        /// Remove the build volume for an existing worktree at this path.
        #[arg(long, value_name = "PATH", group = "scope")]
        workspace: Option<PathBuf>,
        /// Remove the whole machine-wide toolchain volume. It is bound to no
        /// directory, so `--orphans` never reaps it; this is how it goes.
        #[arg(long, group = "scope")]
        toolchains: bool,
        /// Remove one provisioned toolchain — `<language>` or
        /// `<language>@<version>` — leaving the volume and its others intact.
        #[arg(long, value_name = "LANG[@VERSION]", group = "scope")]
        toolchain: Option<String>,
    },
}

/// The resolved clean scope (exactly one).
#[derive(Debug, PartialEq, Eq)]
enum CleanScope {
    Current,
    Orphans,
    All,
    Workspace(PathBuf),
    /// The whole toolchain volume.
    Toolchains,
    /// One provisioned toolchain: a language, and optionally one version of it.
    Toolchain(String, Option<String>),
}

pub fn run(action: DockerCacheAction) -> Result<ExitCodes> {
    match action {
        DockerCacheAction::Ls { json } => run_ls(json),
        DockerCacheAction::Clean {
            orphans,
            all,
            workspace,
            toolchains,
            toolchain,
        } => run_clean(&resolve_scope(
            orphans, all, workspace, toolchains, toolchain,
        )),
    }
}

/// Map the mutually-exclusive clean flags to a scope (none → current worktree).
fn resolve_scope(
    orphans: bool,
    all: bool,
    workspace: Option<PathBuf>,
    toolchains: bool,
    toolchain: Option<String>,
) -> CleanScope {
    if orphans {
        CleanScope::Orphans
    } else if all {
        CleanScope::All
    } else if toolchains {
        CleanScope::Toolchains
    } else if let Some(spec) = toolchain {
        let (lang, version) = parse_toolchain_spec(&spec);
        CleanScope::Toolchain(lang, version)
    } else if let Some(path) = workspace {
        CleanScope::Workspace(path)
    } else {
        CleanScope::Current
    }
}

/// `dotnet` → whole language; `dotnet@9.0.308` → that one version.
fn parse_toolchain_spec(spec: &str) -> (String, Option<String>) {
    match spec.split_once('@') {
        Some((lang, version)) => (lang.to_string(), Some(version.to_string())),
        None => (spec.to_string(), None),
    }
}

fn run_ls(json: bool) -> Result<ExitCodes> {
    if !daemon_available() {
        bail!("docker is not available (`docker info` failed) — is Docker running?");
    }
    let vols = list_managed_volumes().map_err(|e| anyhow::anyhow!(e))?;
    // The toolchain volume is the largest thing kenn puts on disk, and unlike
    // the others its size is not one number but a set of independently
    // reclaimable toolchains. Listing it opaquely would leave a user with
    // gigabytes and no way to see what they are for.
    let toolchains = if vols.iter().any(|v| v.kind == VolumeKind::Toolchain) {
        docker::list_toolchains().unwrap_or_default()
    } else {
        Vec::new()
    };
    let sizes = docker::volume_sizes();
    if json {
        println!("{}", render_json(&vols, &toolchains, &sizes));
    } else {
        print!("{}", render_table(&vols, &toolchains, &sizes));
    }
    Ok(ExitCodes::Ok)
}

fn run_clean(scope: &CleanScope) -> Result<ExitCodes> {
    if !daemon_available() {
        // Teardown-safe: never fail the caller when docker is down.
        println!("docker is not available; nothing reclaimed");
        return Ok(ExitCodes::Ok);
    }
    match scope {
        // Removing INSIDE the toolchain volume is not a volume removal, so it
        // does not go through the name-based path.
        CleanScope::Toolchain(language, version) => {
            Ok(clean_toolchain(language, version.as_deref()))
        }
        _ => clean_volumes(scope),
    }
}

/// How one toolchain removal is reported. A whole language and a single version
/// read the same way, so the label carries the distinction.
fn toolchain_label(language: &str, version: Option<&str>) -> String {
    version.map_or_else(|| language.to_string(), |v| format!("{language}@{v}"))
}

fn clean_toolchain(language: &str, version: Option<&str>) -> ExitCodes {
    let label = toolchain_label(language, version);
    match docker::remove_toolchain(language, version) {
        RemoveOutcome::Failed(e) => {
            eprintln!("{label}: FAILED — {e}");
            ExitCodes::Generic
        }
        other => {
            println!("{label}: {}", describe(&other));
            ExitCodes::Ok
        }
    }
}

fn clean_volumes(scope: &CleanScope) -> Result<ExitCodes> {
    let targets = select_targets(scope)?;
    if targets.is_empty() {
        println!("no matching kenn cache volumes");
        return Ok(ExitCodes::Ok);
    }
    Ok(if remove_all(&targets) {
        ExitCodes::Generic
    } else {
        ExitCodes::Ok
    })
}

/// Remove each target, reporting the outcome. Returns whether any removal hit a
/// genuine Docker failure (absent / in-use are reported non-errors).
fn remove_all(targets: &[String]) -> bool {
    let mut failed = false;
    for name in targets {
        match remove_volume(name) {
            RemoveOutcome::Failed(e) => {
                eprintln!("{name}: FAILED — {e}");
                failed = true;
            }
            other => println!("{name}: {}", describe(&other)),
        }
    }
    failed
}

/// The volume names a scope targets. `Current`/`Workspace` derive the name from a
/// directory (no listing); `Orphans`/`All` enumerate the managed volumes.
fn select_targets(scope: &CleanScope) -> Result<Vec<String>> {
    match scope {
        CleanScope::Current => Ok(vec![docker::build_volume_name(&current_worktree_root()?)]),
        CleanScope::Workspace(path) => Ok(vec![docker::build_volume_name(path)]),
        CleanScope::Orphans => managed_names(ManagedVolume::is_orphan),
        CleanScope::All => managed_names(|_| true),
        CleanScope::Toolchains => Ok(vec![docker::TOOLCHAIN_VOLUME.to_string()]),
        // Handled before this point: it removes a directory inside the volume,
        // not the volume itself.
        CleanScope::Toolchain(..) => Ok(Vec::new()),
    }
}

/// Names of the managed volumes matching `keep`, enumerated via docker labels.
fn managed_names(keep: impl Fn(&ManagedVolume) -> bool) -> Result<Vec<String>> {
    let vols = list_managed_volumes().map_err(|e| anyhow::anyhow!(e))?;
    Ok(vols
        .into_iter()
        .filter(|v| keep(v))
        .map(|v| v.name)
        .collect())
}

/// The current worktree root from cwd — the directory a build volume binds to.
fn current_worktree_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(kenn_store::git::work_dir(&cwd).unwrap_or(cwd))
}

/// A human-readable phrase for a non-failure removal outcome.
fn describe(outcome: &RemoveOutcome) -> &'static str {
    match outcome {
        RemoveOutcome::Removed => "removed",
        RemoveOutcome::NotFound => "nothing to remove",
        RemoveOutcome::InUse => "skipped (in use)",
        RemoveOutcome::Failed(_) => "failed",
    }
}

#[derive(serde::Serialize)]
struct VolumeRow<'a> {
    name: &'a str,
    kind: &'a str,
    /// Docker's on-disk size string, or `None` when `docker system df` did not
    /// report this volume (added key; existing consumers ignore it).
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<String>,
    bound_dir: Option<String>,
    exists: Option<bool>,
    in_use: bool,
}

fn to_rows<'a>(vols: &'a [ManagedVolume], sizes: &HashMap<String, String>) -> Vec<VolumeRow<'a>> {
    vols.iter()
        .map(|v| VolumeRow {
            name: &v.name,
            kind: v.kind.label(),
            size: sizes.get(&v.name).cloned(),
            bound_dir: v.bound_dir.as_ref().map(|d| d.display().to_string()),
            exists: v.bound_dir.as_ref().map(|d| d.exists()),
            in_use: v.in_use,
        })
        .collect()
}

#[derive(serde::Serialize)]
struct ToolchainRow<'a> {
    /// Carried so a mixed-arch host is legible: the cache holds one tree per
    /// architecture, so the same language+version can appear twice, and without
    /// this the two rows are indistinguishable duplicates.
    arch: &'a str,
    language: &'a str,
    version: &'a str,
    size_kb: u64,
}

fn render_json(
    vols: &[ManagedVolume],
    toolchains: &[docker::ProvisionedToolchain],
    sizes: &HashMap<String, String>,
) -> String {
    // Stays a top-level ARRAY of volumes: `--json` is a machine-readable
    // interface and reshaping the root would break every existing consumer.
    // The toolchains nest under the volume that holds them, which is where they
    // belong anyway.
    let rows: Vec<_> = to_rows(vols, sizes)
        .into_iter()
        .map(|row| {
            let nested: Vec<ToolchainRow<'_>> = if row.kind == VolumeKind::Toolchain.label() {
                toolchains
                    .iter()
                    .map(|t| ToolchainRow {
                        arch: &t.arch,
                        language: &t.language,
                        version: &t.version,
                        size_kb: t.size_kb,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            (row, nested)
        })
        .map(|(row, nested)| VolumeRowWithToolchains {
            row,
            toolchains: nested,
        })
        .collect();
    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
}

#[derive(serde::Serialize)]
struct VolumeRowWithToolchains<'a> {
    #[serde(flatten)]
    row: VolumeRow<'a>,
    /// Empty for every volume but the toolchain one, so existing consumers that
    /// ignore unknown keys are unaffected.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    toolchains: Vec<ToolchainRow<'a>>,
}

/// Kilobytes as a human-readable size. Toolchains run to hundreds of megabytes,
/// so raw kB would be unreadable in the exact case this exists to make legible.
#[expect(
    clippy::cast_precision_loss,
    reason = "display only, and a toolchain would need to exceed 4 PB before an \
              f64 lost a digit that a one-decimal size string could show"
)]
fn human_kb(kb: u64) -> String {
    if kb >= 1024 * 1024 {
        format!("{:.1} GB", kb as f64 / (1024.0 * 1024.0))
    } else if kb >= 1024 {
        format!("{:.0} MB", kb as f64 / 1024.0)
    } else {
        format!("{kb} kB")
    }
}

fn render_table(
    vols: &[ManagedVolume],
    toolchains: &[docker::ProvisionedToolchain],
    sizes: &HashMap<String, String>,
) -> String {
    if vols.is_empty() {
        return "no kenn cache volumes\n".to_string();
    }
    // Table 1 — the volumes, each with its size and binding. `\t` was used
    // before, but a tab stop misaligns the moment a name crosses it, so pad to
    // the widest cell instead.
    let mut vol_rows = vec![row(["VOLUME", "KIND", "SIZE", "BINDING"])];
    for v in vols {
        let mut binding = v.bound_dir.as_ref().map_or_else(
            || "shared".to_string(),
            |d| {
                let state = if d.exists() { "exists" } else { "MISSING" };
                format!("{} ({state})", d.display())
            },
        );
        if v.in_use {
            binding.push_str(" [in-use]");
        }
        vol_rows.push(vec![
            v.name.clone(),
            v.kind.label().to_string(),
            sizes
                .get(&v.name)
                .cloned()
                .unwrap_or_else(|| "?".to_string()),
            binding,
        ]);
    }
    let mut out = align_rows(&vol_rows);
    // Table 2 — what is inside the toolchain volume, each with the exact
    // `--toolchain` spec needed to reclaim it. A separate table because these
    // are sub-trees of one volume, not volumes; blank line between the two.
    if !toolchains.is_empty() {
        let mut tc_rows = vec![row(["TOOLCHAIN", "ARCH", "SIZE"])];
        for t in toolchains {
            tc_rows.push(vec![
                format!("{}@{}", t.language, t.version),
                t.arch.clone(),
                human_kb(t.size_kb),
            ]);
        }
        out.push('\n');
        out.push_str(&align_rows(&tc_rows));
    }
    out
}

/// One header/label row from string literals.
fn row<const N: usize>(cells: [&str; N]) -> Vec<String> {
    cells.iter().map(|s| (*s).to_string()).collect()
}

/// Render rows as a fixed-width table: every column left-aligned to its widest
/// cell (header included) with two spaces between columns. The last column is
/// never trailing-padded, so no line carries dangling whitespace.
fn align_rows(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for r in rows {
        for (w, cell) in widths.iter_mut().zip(r) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for r in rows {
        let last = r.len().saturating_sub(1);
        for (i, (cell, w)) in r.iter().zip(&widths).enumerate() {
            out.push_str(cell);
            if i != last {
                for _ in 0..w - cell.chars().count() + 2 {
                    out.push(' ');
                }
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(name: &str, kind: VolumeKind, bound: Option<&str>, in_use: bool) -> ManagedVolume {
        ManagedVolume {
            name: name.to_string(),
            kind,
            bound_dir: bound.map(PathBuf::from),
            in_use,
        }
    }

    #[test]
    fn resolve_scope_maps_the_mutually_exclusive_flags() {
        assert_eq!(
            resolve_scope(true, false, None, false, None),
            CleanScope::Orphans
        );
        assert_eq!(
            resolve_scope(false, true, None, false, None),
            CleanScope::All
        );
        assert_eq!(
            resolve_scope(false, false, Some(PathBuf::from("/x")), false, None),
            CleanScope::Workspace(PathBuf::from("/x"))
        );
        assert_eq!(
            resolve_scope(false, false, None, false, None),
            CleanScope::Current
        );
    }

    fn tc(language: &str, version: &str, size_kb: u64) -> docker::ProvisionedToolchain {
        docker::ProvisionedToolchain {
            arch: "arm64".to_string(),
            language: language.to_string(),
            version: version.to_string(),
            size_kb,
        }
    }

    #[test]
    fn resolve_scope_maps_the_toolchain_flags() {
        assert_eq!(
            resolve_scope(false, false, None, true, None),
            CleanScope::Toolchains
        );
        assert_eq!(
            resolve_scope(false, false, None, false, Some("dotnet".into())),
            CleanScope::Toolchain("dotnet".to_string(), None)
        );
        assert_eq!(
            resolve_scope(false, false, None, false, Some("dotnet@9.0.308".into())),
            CleanScope::Toolchain("dotnet".to_string(), Some("9.0.308".to_string()))
        );
    }

    /// `--toolchains` removes the volume; `--toolchain X` removes a directory
    /// INSIDE it and must never resolve to a volume name — deleting the whole
    /// volume when the user asked for one toolchain would cost every other
    /// workspace its cache.
    #[test]
    fn a_single_toolchain_never_targets_the_volume() {
        assert_eq!(
            select_targets(&CleanScope::Toolchains).unwrap(),
            vec!["kenn-toolchains".to_string()]
        );
        assert!(
            select_targets(&CleanScope::Toolchain("dotnet".into(), None))
                .unwrap()
                .is_empty()
        );
    }

    /// Two separated tables: the volumes (name, kind, size, binding), then the
    /// toolchains inside the toolchain volume with the `--toolchain` spec to
    /// reclaim each. A blank line divides them.
    #[test]
    fn the_table_lists_volumes_then_toolchains() {
        let vols = vec![
            vol("kenn-deps-a", VolumeKind::Deps, Some("/x"), false),
            vol("kenn-toolchains", VolumeKind::Toolchain, None, false),
        ];
        let sizes = HashMap::from([("kenn-deps-a".to_string(), "906.7MB".to_string())]);
        let table = render_table(&vols, &[tc("dotnet", "9.0.308", 423_616)], &sizes);
        // Volumes table: header + a deps row carrying its size.
        assert!(table.contains("VOLUME"), "volumes header: {table}");
        assert!(table.contains("kenn-deps-a"), "{table}");
        assert!(table.contains("906.7MB"), "deps size shown: {table}");
        // Toolchains table: its own header, the reclaim spec, and human size.
        assert!(table.contains("TOOLCHAIN"), "toolchains header: {table}");
        assert!(table.contains("dotnet@9.0.308"), "{table}");
        assert!(table.contains("414 MB"), "human-readable size: {table}");
        // The blank line proves they are two tables, not one nested block.
        assert!(table.contains("\n\n"), "blank line between tables: {table}");
    }

    /// The `align` fix: a narrow cell is padded to its column's width, so the
    /// next column starts at the same offset on every row. Mutation-checked —
    /// dropping the padding puts the second column at index 1, not 10.
    #[test]
    fn align_rows_pads_columns_to_a_common_width() {
        let out = align_rows(&[row(["A", "x"]), row(["longname", "y"])]);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[0].find('x'),
            Some(10),
            "narrow row padded: {:?}",
            lines[0]
        );
        assert_eq!(
            lines[1].find('y'),
            Some(10),
            "wide row aligned: {:?}",
            lines[1]
        );
        assert!(!lines[0].ends_with(' '), "no trailing pad: {:?}", lines[0]);
    }

    /// `--json` is a machine-readable interface: it must stay a top-level array
    /// of volumes. Reshaping the root would break every existing consumer.
    #[test]
    fn json_stays_an_array_and_nests_toolchains() {
        let vols = vec![
            vol("kenn-deps-a", VolumeKind::Deps, Some("/x"), false),
            vol("kenn-toolchains", VolumeKind::Toolchain, None, false),
        ];
        let sizes = HashMap::from([("kenn-deps-a".to_string(), "906.7MB".to_string())]);
        let json = render_json(&vols, &[tc("swift", "6.3.3", 1_800_000)], &sizes);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(parsed.is_array(), "root must stay an array: {json}");
        assert_eq!(parsed[0]["size"], "906.7MB", "deps size in json: {json}");
        // The pre-existing per-volume keys are untouched.
        assert_eq!(parsed[0]["name"], "kenn-deps-a");
        assert_eq!(parsed[0]["kind"], "deps");
        assert!(
            parsed[0].get("toolchains").is_none(),
            "absent on non-toolchain volumes: {json}"
        );
        assert_eq!(parsed[1]["toolchains"][0]["language"], "swift");
        assert_eq!(parsed[1]["toolchains"][0]["version"], "6.3.3");
        assert_eq!(parsed[1]["toolchains"][0]["size_kb"], 1_800_000);
    }

    #[test]
    fn human_kb_scales() {
        assert_eq!(human_kb(512), "512 kB");
        assert_eq!(human_kb(423_616), "414 MB");
        assert_eq!(human_kb(1_800_000), "1.7 GB");
    }

    #[test]
    fn describe_covers_the_non_failure_outcomes() {
        assert_eq!(describe(&RemoveOutcome::Removed), "removed");
        assert_eq!(describe(&RemoveOutcome::NotFound), "nothing to remove");
        assert_eq!(describe(&RemoveOutcome::InUse), "skipped (in use)");
    }

    #[test]
    fn table_marks_missing_and_in_use() {
        let vols = vec![
            vol(
                "kenn-deps-a",
                VolumeKind::Deps,
                Some("/no/such/dir/xyz"),
                false,
            ),
            vol("kenn-shared", VolumeKind::Other, None, true),
        ];
        let table = render_table(&vols, &[], &HashMap::new());
        assert!(table.contains("kenn-deps-a"));
        assert!(table.contains("deps"));
        assert!(
            table.contains("MISSING"),
            "gone dir marked MISSING: {table}"
        );
        assert!(table.contains("shared"), "unbound shows shared: {table}");
        assert!(table.contains("[in-use]"), "in-use marked: {table}");
    }

    #[test]
    fn json_carries_kind_binding_and_flags() {
        let vols = vec![vol(
            "kenn-build-a",
            VolumeKind::Build,
            Some("/no/such/dir/xyz"),
            false,
        )];
        let sizes = HashMap::from([("kenn-build-a".to_string(), "15.4MB".to_string())]);
        let json = render_json(&vols, &[], &sizes);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let row = &parsed[0];
        assert_eq!(row["name"], "kenn-build-a");
        assert_eq!(row["kind"], "build");
        assert_eq!(row["size"], "15.4MB");
        assert_eq!(row["exists"], false);
        assert_eq!(row["in_use"], false);
        assert_eq!(row["bound_dir"], "/no/such/dir/xyz");
    }
}
