//! `kenn-toolchain` — the ENTRYPOINT of every indexer image.
//!
//! Usage: `kenn-toolchain <language> -- <indexer> [args…]`
//!
//! Provisions the workspace's pinned toolchain into the mounted cache, exports
//! the language's toolchain-root env var, then execs `<indexer>`.
//!
//! Diagnostics go to stderr only: three of the six indexers stream JSONL on
//! stdout and kenn parses it, so a stray byte there would corrupt a frame and
//! look like an indexer bug.

use std::path::Path;
use std::process::Command;

use kenn_toolchain::pin::Language;
use kenn_toolchain::resolve::Arch;
use kenn_toolchain::run;

fn main() -> std::process::ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let Some((languages, rest)) = split_args(&args) else {
        eprintln!("kenn-toolchain: usage: kenn-toolchain <language> -- <indexer> [args...]");
        return std::process::ExitCode::from(2);
    };

    let Some(mut command) = exec_command(&rest) else {
        eprintln!("kenn-toolchain: no indexer to run");
        return std::process::ExitCode::from(2);
    };

    match provision_all(&languages, &workspace_dir(), &mut command) {
        Ok(path) => command.env("PATH", path),
        Err(e) => {
            // Fatal and named. Falling through to the indexer with no toolchain
            // is what produced zero files at exit 0.
            eprintln!("kenn-toolchain: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    exec_indexer(command)
}

/// The workspace is the working directory: kenn's docker runtime mounts it at
/// its own absolute path and sets `-w` to it.
fn workspace_dir() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf())
}

/// Provision every requested toolchain, exporting each one's root var onto
/// `command` and returning the `PATH` they should run with.
///
/// `PATH` is threaded through the loop rather than re-read from the environment
/// each time, so a second toolchain does not overwrite the first one's entry.
fn provision_all(
    languages: &[Language],
    workspace: &Path,
    command: &mut Command,
) -> Result<std::ffi::OsString, String> {
    let mut path = std::env::var_os("PATH").unwrap_or_default();
    for &language in languages {
        let outcome = run::provision(
            language,
            workspace,
            Arch::host(),
            &mut std::io::stderr(),
            &run::http_text,
            &run::http_install,
        )
        .map_err(|e| e.to_string())?;
        path = apply_toolchain(language, outcome.path(), command, path);
    }
    Ok(path)
}

/// Point `command` at one provisioned toolchain: its root var where the language
/// needs one, and its bin directory on `PATH` — without which the indexer's
/// `dotnet`/`go`/`node` spawns find nothing, the toolchain no longer being in
/// the image. A toolchain with no path (not pinned, or no cache mounted) leaves
/// the environment exactly as the indexer would have seen it anyway.
fn apply_toolchain(
    language: Language,
    root: Option<&Path>,
    command: &mut Command,
    path: std::ffi::OsString,
) -> std::ffi::OsString {
    let Some(root) = root else { return path };
    if let Some(var) = toolchain_root_var(language) {
        command.env(var, root);
    }
    prepend_path(&run::toolchain_bin(language, root), Some(&path))
}

/// REPLACE this process rather than spawning a child. As an image ENTRYPOINT
/// this runs as PID 1, and PID 1 has to reap orphaned grandchildren — which a
/// spawn-and-wait parent does not do. scip-go drives `go/packages`, which shells
/// out to `go list`; under a non-reaping PID 1 those calls fail and scip-go emits
/// a VALID BUT EMPTY index, reporting "Visiting Project Files [2/2]" as if it
/// worked. Measured on a real repo: 195 bytes spawned versus 268,900 exec'd,
/// same environment and same output path.
///
/// exec also makes signals and exit status reach the indexer directly, which is
/// what a caller of `docker run` expects.
fn exec_indexer(mut command: Command) -> std::process::ExitCode {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let err = command.exec();
        eprintln!("kenn-toolchain: cannot exec the indexer: {err}");
        std::process::ExitCode::FAILURE
    }
    #[cfg(not(unix))]
    match command.status() {
        Ok(s) => u8::try_from(s.code().unwrap_or(1)).map_or(
            std::process::ExitCode::FAILURE,
            std::process::ExitCode::from,
        ),
        Err(e) => {
            eprintln!("kenn-toolchain: cannot exec the indexer: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// `<language> [--] <indexer> [args…]`.
fn split_args(args: &[std::ffi::OsString]) -> Option<(Vec<Language>, Vec<std::ffi::OsString>)> {
    let (first, rest) = args.split_first()?;
    // A comma-separated list, because one image can need more than one
    // toolchain: scip-python is a Node application that analyses Python, so it
    // needs node to RUN and python to INSPECT. A list keeps that in the
    // ENTRYPOINT where it is visible, rather than hidden in a special case.
    let languages: Option<Vec<Language>> = first.to_str()?.split(',').map(parse_language).collect();
    let languages = languages.filter(|l: &Vec<Language>| !l.is_empty())?;
    // Tolerate the conventional `--` separator so the entrypoint reads the same
    // whether or not a caller uses one.
    let rest = match rest.split_first() {
        Some((sep, tail)) if sep == "--" => tail,
        _ => rest,
    };
    (!rest.is_empty()).then(|| (languages, rest.to_vec()))
}

fn parse_language(s: &str) -> Option<Language> {
    Some(match s {
        "dotnet" | "csharp" => Language::Dotnet,
        "rust" => Language::Rust,
        "go" => Language::Go,
        "python" => Language::Python,
        "node" => Language::Node,
        "swift" => Language::Swift,
        "typescript" | "ts" => Language::TypeScript,
        _ => return None,
    })
}

/// The env var each toolchain is found through. `None` where the indexer needs
/// no pointer because being on `PATH` is enough.
fn toolchain_root_var(language: Language) -> Option<&'static str> {
    match language {
        Language::Dotnet => Some("DOTNET_ROOT"),
        Language::Go => Some("GOROOT"),
        Language::Rust => Some("RUSTUP_HOME"),
        Language::Python | Language::Node | Language::Swift | Language::TypeScript => None,
    }
}

/// `PATH` with the provisioned toolchain's bin directory prepended, so the
/// toolchain wins over anything of the same name already in the image.
fn prepend_path(toolchain_bin: &Path, existing: Option<&std::ffi::OsStr>) -> std::ffi::OsString {
    let mut path = std::ffi::OsString::from(toolchain_bin);
    if let Some(existing) = existing.filter(|e| !e.is_empty()) {
        path.push(":");
        path.push(existing);
    }
    path
}

/// `argv` is non-empty by construction — [`split_args`] returns `None` for an
/// empty tail — but say so with a pattern rather than an index, so a future
/// caller cannot turn it into a panic.
fn exec_command(argv: &[std::ffi::OsString]) -> Option<Command> {
    let (program, indexer_args) = argv.split_first()?;
    let mut command = Command::new(program);
    command.args(indexer_args);
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_toolchain::run::Outcome;

    fn os(v: &[&str]) -> Vec<std::ffi::OsString> {
        v.iter().map(Into::into).collect()
    }

    #[test]
    fn splits_language_from_the_indexer_argv() {
        let (lang, rest) =
            split_args(&os(&["go", "--", "scip-go", "--output", "x.scip"])).expect("parse");
        assert_eq!(lang, vec![Language::Go]);
        assert_eq!(rest, os(&["scip-go", "--output", "x.scip"]));
    }

    /// The `--` is optional; both spellings must reach the same indexer argv, or
    /// an image's ENTRYPOINT would silently pass `--` to the indexer.
    #[test]
    fn the_separator_is_optional() {
        let with = split_args(&os(&["rust", "--", "rust-analyzer"])).expect("parse");
        let without = split_args(&os(&["rust", "rust-analyzer"])).expect("parse");
        assert_eq!(with.1, without.1);
        assert_eq!(with.1, os(&["rust-analyzer"]));
    }

    /// One image can need more than one toolchain: scip-python is a Node
    /// application that analyses Python, so it needs node to RUN and python to
    /// INSPECT. A partly-unknown list must be rejected whole rather than
    /// silently provisioning the half it recognised.
    #[test]
    fn a_comma_separated_list_provisions_several_toolchains() {
        let (langs, rest) =
            split_args(&os(&["python,node", "--", "node", "idx.js"])).expect("parse");
        assert_eq!(langs, vec![Language::Python, Language::Node]);
        assert_eq!(rest, os(&["node", "idx.js"]));

        assert!(
            split_args(&os(&["python,bogus", "--", "x"])).is_none(),
            "an unknown entry must reject the whole list"
        );
    }

    #[test]
    fn csharp_and_dotnet_name_the_same_language() {
        assert_eq!(parse_language("csharp"), Some(Language::Dotnet));
        assert_eq!(parse_language("dotnet"), Some(Language::Dotnet));
        assert_eq!(parse_language("nonsense"), None);
    }

    #[test]
    fn a_missing_indexer_is_a_usage_error() {
        assert!(split_args(&os(&["go"])).is_none());
        assert!(split_args(&os(&["go", "--"])).is_none());
        assert!(split_args(&[]).is_none());
    }

    /// Every language kenn provisions must have a decided answer here, so adding
    /// one cannot silently inherit "no env var" and leave the toolchain unfound.
    #[test]
    fn every_language_has_a_decided_toolchain_root() {
        assert_eq!(toolchain_root_var(Language::Dotnet), Some("DOTNET_ROOT"));
        assert_eq!(toolchain_root_var(Language::Go), Some("GOROOT"));
        assert_eq!(toolchain_root_var(Language::Rust), Some("RUSTUP_HOME"));
        assert_eq!(toolchain_root_var(Language::Swift), None);
    }

    /// `Outcome::NoCache` and `NotPinned` carry no path, so nothing is exported
    /// and the indexer runs exactly as it would have without this entrypoint.
    #[test]
    fn outcomes_without_a_toolchain_export_nothing() {
        assert!(Outcome::NoCache.path().is_none());
        assert!(Outcome::NotPinned.path().is_none());
    }

    /// The provisioned toolchain must come FIRST on PATH. kenn-dotnet spawns
    /// `dotnet restore` by name and `MSBuildLocator` spawns `dotnet --info`; with
    /// the SDK gone from the image, losing this means those spawns find nothing
    /// — or worse, find a different toolchain than the one we resolved.
    #[test]
    fn the_provisioned_toolchain_leads_the_path() {
        let path = prepend_path(
            Path::new("/kenn-toolchains/dotnet/9.0.308"),
            Some(std::ffi::OsStr::new("/usr/local/bin:/usr/bin")),
        );
        assert_eq!(
            path,
            std::ffi::OsString::from("/kenn-toolchains/dotnet/9.0.308:/usr/local/bin:/usr/bin")
        );
    }

    /// An empty or absent PATH must not yield a leading/trailing colon — an
    /// empty PATH entry means "the current directory" to execvp, which would
    /// silently make the workspace searched for executables.
    #[test]
    fn an_absent_path_does_not_produce_an_empty_entry() {
        for existing in [None, Some(std::ffi::OsStr::new(""))] {
            let path = prepend_path(Path::new("/tc/bin"), existing);
            assert_eq!(path, std::ffi::OsString::from("/tc/bin"), "{existing:?}");
        }
    }

    fn env_of(command: &Command, key: &str) -> Option<String> {
        command.get_envs().find_map(|(k, v)| {
            (k == key).then(|| v.unwrap_or_default().to_string_lossy().into_owned())
        })
    }

    /// A language with a root var gets BOTH the var and the PATH entry. Losing
    /// either is the same failure: `MSBuildLocator` spawns `dotnet --info` by
    /// name and reads `DOTNET_ROOT`, and with the SDK gone from the image an
    /// unset one means it finds nothing.
    #[test]
    fn a_provisioned_toolchain_exports_its_root_and_leads_the_path() {
        let mut command = Command::new("kenn-dotnet");
        let root = Path::new("/kenn-toolchains/dotnet/9.0.308");
        let path = apply_toolchain(
            Language::Dotnet,
            Some(root),
            &mut command,
            "/usr/bin".into(),
        );
        assert_eq!(
            env_of(&command, "DOTNET_ROOT").as_deref(),
            Some(root.to_str().unwrap())
        );
        assert!(
            path.to_string_lossy().starts_with(root.to_str().unwrap()),
            "toolchain must lead PATH, got {path:?}"
        );
        assert!(path.to_string_lossy().ends_with("/usr/bin"));
    }

    /// Swift has no root var — being on PATH is enough — so nothing may be
    /// exported for it, but the PATH entry must still land.
    #[test]
    fn a_language_without_a_root_var_still_reaches_the_path() {
        let mut command = Command::new("kenn-swift");
        let path = apply_toolchain(
            Language::Swift,
            Some(Path::new("/kenn-toolchains/swift/6.3")),
            &mut command,
            "/usr/bin".into(),
        );
        assert_eq!(command.get_envs().count(), 0, "swift exports no root var");
        assert!(path
            .to_string_lossy()
            .contains("/kenn-toolchains/swift/6.3"));
    }

    /// No toolchain (not pinned, or no cache mounted) must leave the environment
    /// untouched rather than exporting an empty var — an empty `DOTNET_ROOT` is
    /// worse than an absent one, because the SDK lookup stops rather than
    /// falling back.
    #[test]
    fn an_unprovisioned_toolchain_changes_nothing() {
        let mut command = Command::new("kenn-dotnet");
        let path = apply_toolchain(Language::Dotnet, None, &mut command, "/usr/bin".into());
        assert_eq!(command.get_envs().count(), 0);
        assert_eq!(path, std::ffi::OsString::from("/usr/bin"));
    }
}
