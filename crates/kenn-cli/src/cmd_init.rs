use std::path::{Path, PathBuf};

use anyhow::Result;
use kenn_config::Config;
use kenn_store::{Layout, Store};
use toml_edit::DocumentMut;

use crate::exit::ExitCodes;
use crate::init::{author, detect, report};

/// Starter `kenn.toml` — the base document authoring mutates, and the
/// zero-detection fallback for an empty repo. Lives in
/// `assets/starter_kenn.toml` so it can be edited with TOML syntax
/// highlighting; included at compile time via `include_str!`.
const STARTER_TOML: &str = include_str!("../assets/starter_kenn.toml");

/// `kenn init`. Detects the languages in `workspace` and writes a `kenn.toml`
/// that fits. Does its own tolerant config load and layout resolution, so it
/// runs even when an existing `kenn.toml` fails to parse — the case that would
/// otherwise brick every command before dispatch.
pub fn run(workspace: &Path, config_path: &Path, force: bool, docker: bool) -> Result<ExitCodes> {
    let raw = std::fs::read_to_string(config_path).ok();
    // Parse once; the classification and the layout config share the result.
    let parsed = raw.as_deref().map(Config::from_toml);

    // Broken config without --force: do nothing — don't even create `.kenn/` —
    // and signal non-zero. Nothing was written and every other command is still
    // unusable, so a scripted `init && index` stops here with the actionable
    // hint instead of failing later on the config load with none.
    if !force && matches!(parsed, Some(Err(_))) {
        eprintln!(
            "kenn: {} does not parse as a kenn config",
            config_path.display()
        );
        eprintln!("kenn: re-run with --force to back it up and replace it");
        return Ok(ExitCodes::Generic);
    }

    // Resolve the layout against the parsed config when we have one, else the
    // defaults — a broken config's `[layout]`/`[vectors]` can't be trusted.
    // Borrowed, not cloned; `Layout::resolve` only reads a couple of sections.
    let default_cfg = Config::default();
    let config_for_layout = match &parsed {
        Some(Ok(c)) => c,
        _ => &default_cfg,
    };
    let layout = Layout::resolve(config_for_layout, workspace)?;
    let store_existed = layout.committed_root().exists();
    Store::open(layout)?;

    // Decide the docker fallback only once we're past the broken-config guard
    // above, so a scripted `init && index` on an unparseable config bails on the
    // actionable hint without first probing the daemon. `--docker` routes a
    // language whose local toolchain is missing to a container image (task 5.1)
    // instead of degrading to text — but only when the daemon is runnable;
    // otherwise report and fall back to the degrade path (5.2). The `docker &&`
    // short-circuits the probe so a plain `kenn init` never spawns `docker info`.
    let daemon_up = docker && kenn_indexer::docker::daemon_available();
    let (containerize, docker_error) = containerize_decision(docker, daemon_up);
    if let Some(msg) = docker_error {
        eprintln!("{msg}");
    }

    match parsed {
        None => write_fresh(workspace, config_path, "initialized", containerize),
        Some(Ok(_)) if !force => {
            let verb = if store_existed {
                "already initialized at"
            } else {
                "initialized"
            };
            println!("{verb} {}", workspace.display());
            println!("re-run with --force to detect languages and merge them in");
            Ok(ExitCodes::Ok)
        }
        Some(Ok(_)) => {
            let raw = raw.expect("a parsed config implies the file was read");
            merge_into_existing(workspace, config_path, &raw, containerize)
        }
        // `Some(Err(_)) && !force` returned above, so this is force-only.
        Some(Err(_)) => {
            // Copy (not rename) to the backup: the primary file stays intact
            // until the atomic replacement below succeeds, so a failed write
            // never leaves the workspace with no config.
            let bak = backup_path(config_path);
            std::fs::copy(config_path, &bak)?;
            eprintln!(
                "kenn: {} did not parse; backed up to {}",
                config_path.display(),
                bak.display()
            );
            eprintln!("kenn: non-language settings from the old file were not preserved");
            write_fresh(workspace, config_path, "reinitialized", containerize)
        }
    }
}

/// Decide whether `init` should containerize a language whose local toolchain is
/// missing (task 5.1), and — when `--docker` was requested but the daemon isn't
/// runnable — the actionable message to print before falling back to the normal
/// degrade-to-text report (task 5.2). Pure over `(opt_in, daemon_up)` so the
/// three cases are unit-testable without a real daemon.
fn containerize_decision(docker_opt_in: bool, daemon_up: bool) -> (bool, Option<&'static str>) {
    match (docker_opt_in, daemon_up) {
        (false, _) => (false, None),
        (true, true) => (true, None),
        (true, false) => (
            false,
            Some(
                "kenn: --docker requested but the docker daemon is not runnable; \
                 falling back to text for languages whose local toolchain is missing",
            ),
        ),
    }
}

/// Author a fresh config from the starter template.
fn write_fresh(
    workspace: &Path,
    config_path: &Path,
    verb: &str,
    containerize: bool,
) -> Result<ExitCodes> {
    let classified = detect::detect_and_classify(workspace, containerize);
    let mut doc: DocumentMut = STARTER_TOML.parse()?;
    author::apply(&mut doc, &classified);
    write_atomic(config_path, &doc.to_string())?;
    println!("{verb} {}", workspace.display());
    println!("{}", report::render(&classified));
    Ok(ExitCodes::Ok)
}

/// Merge detection into a parseable existing config, preserving every key the
/// user set. `toml_edit` mutates in place, so `max_threads`, custom `command`s,
/// and non-language sections all survive.
fn merge_into_existing(
    workspace: &Path,
    config_path: &Path,
    raw: &str,
    containerize: bool,
) -> Result<ExitCodes> {
    let classified = detect::detect_and_classify(workspace, containerize);
    let mut doc: DocumentMut = raw.parse()?;
    author::apply(&mut doc, &classified);
    write_atomic(config_path, &doc.to_string())?;
    println!("updated {}", config_path.display());
    println!("{}", report::render(&classified));
    Ok(ExitCodes::Ok)
}

/// Write `content` to `path` atomically: a sibling temp file, then a rename over
/// `path` (atomic on the same filesystem). A failed or interrupted write never
/// truncates or removes the existing file.
fn write_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = {
        let mut p = path.as_os_str().to_os_string();
        p.push(".tmp");
        PathBuf::from(p)
    };
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// A non-existing backup path for `config_path`: `<path>.bak`, or `.bak.1`,
/// `.bak.2`, … if earlier backups are already present. Appends the suffix
/// rather than replacing the extension (so `kenn.toml` → `kenn.toml.bak`), and
/// never overwrites an existing backup.
fn backup_path(config_path: &Path) -> PathBuf {
    let base = {
        let mut p = config_path.as_os_str().to_os_string();
        p.push(".bak");
        PathBuf::from(p)
    };
    if !base.exists() {
        return base;
    }
    (1..10_000)
        .map(|n| {
            let mut p = base.as_os_str().to_os_string();
            p.push(format!(".{n}"));
            PathBuf::from(p)
        })
        .find(|p| !p.exists())
        .unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fresh_workspace_creates_dir_and_config() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("kenn.toml");
        let code = run(dir.path(), &cfg, false, false).unwrap();
        assert!(matches!(code, ExitCodes::Ok));
        assert!(dir.path().join(".kenn").is_dir());
        assert!(cfg.is_file());
        assert!(std::fs::read_to_string(&cfg)
            .unwrap()
            .contains("[workspace]"));
    }

    #[test]
    fn existing_config_is_untouched_without_force() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("kenn.toml");
        std::fs::write(&cfg, "[tests]\npaths = [\"keep/**\"]\n").unwrap();
        let before = std::fs::read_to_string(&cfg).unwrap();
        run(dir.path(), &cfg, false, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            before,
            "init without --force must not modify an existing config"
        );
    }

    #[test]
    fn force_merges_and_preserves_user_keys() {
        let dir = TempDir::new().unwrap();
        // Markdown is a built-in producer (no version probe), so its enablement
        // is deterministic — this test needs no toolchain on PATH.
        std::fs::write(dir.path().join("notes.md"), "# notes\n").unwrap();
        let cfg = dir.path().join("kenn.toml");
        // A user config with a custom per-language key and a non-language section.
        std::fs::write(
            &cfg,
            "[language.rust]\nenabled = false\nmax_threads = 7\n\n[language.markdown]\nenabled = false\n\n[metrics]\nregression_threshold_pct = 42\n",
        )
        .unwrap();

        run(dir.path(), &cfg, true, false).unwrap();

        let merged = Config::from_toml(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert!(
            merged.language.markdown.enabled,
            "detection merged the markdown enable in"
        );
        assert_eq!(
            merged.language.rust.max_threads,
            Some(7),
            "a user key on an untouched language survives"
        );
        assert_eq!(
            merged.metrics.regression_threshold_pct, 42,
            "non-language section survives"
        );
    }

    #[test]
    fn broken_config_survives_and_force_repairs_it() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("kenn.toml");
        std::fs::write(&cfg, "this is not = = valid toml [[[\n").unwrap();

        // Without --force: nothing is done — no `.kenn/`, config untouched — and
        // the exit is non-zero so a scripted init/index stops here.
        let code = run(dir.path(), &cfg, false, false).unwrap();
        assert!(
            matches!(code, ExitCodes::Generic),
            "broken + no force ⇒ non-zero"
        );
        assert!(
            !dir.path().join(".kenn").exists(),
            "broken + no force must not create store scaffolding"
        );
        Config::from_toml(&std::fs::read_to_string(&cfg).unwrap())
            .expect_err("the broken config is left in place, still unparseable");

        // With --force: backed up and replaced with a parseable config.
        let code = run(dir.path(), &cfg, true, false).unwrap();
        assert!(
            matches!(code, ExitCodes::Ok),
            "a repaired config is success"
        );
        assert!(
            dir.path().join("kenn.toml.bak").is_file(),
            "the broken file is backed up"
        );
        Config::from_toml(&std::fs::read_to_string(&cfg).unwrap())
            .expect("the replacement config parses");
    }

    #[test]
    fn containerize_decision_covers_the_three_cases() {
        // No opt-in ⇒ never containerize, no error (daemon state irrelevant).
        assert_eq!(containerize_decision(false, true), (false, None));
        assert_eq!(containerize_decision(false, false), (false, None));
        // Opt-in + daemon up ⇒ containerize, no error.
        assert_eq!(containerize_decision(true, true), (true, None));
        // Opt-in + daemon down ⇒ do NOT containerize, and report (5.2 fallback).
        let (containerize, err) = containerize_decision(true, false);
        assert!(!containerize, "docker absent must not containerize");
        assert!(
            err.is_some_and(|m| m.contains("--docker") && m.contains("not runnable")),
            "an actionable message is printed before the degrade fallback"
        );
    }

    #[test]
    fn backup_path_never_overwrites_an_existing_backup() {
        let dir = TempDir::new().unwrap();
        let cfg = dir.path().join("kenn.toml");
        assert_eq!(backup_path(&cfg), dir.path().join("kenn.toml.bak"));
        std::fs::write(dir.path().join("kenn.toml.bak"), "old").unwrap();
        assert_eq!(backup_path(&cfg), dir.path().join("kenn.toml.bak.1"));
    }
}
