//! End-to-end CLI smoke for `kenn`.
//!
//! Each test runs the binary in a child process so `RocksDB` locks are
//! released cleanly between writer and reader operations. Tests use a
//! unique `--workspace` (`TempDir`) and bypass git ancestry by *not*
//! initializing a git repo — the staleness check then becomes a no-op
//! (non-git workspace → always-mismatching key).

use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn ci(workspace: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    let mut cmd = Command::cargo_bin("kenn").expect("locate kenn binary");
    cmd.arg("--workspace").arg(workspace).args(args);
    cmd.assert()
}

#[test]
fn completions_emits_script_for_each_shell() {
    // `completions` is context-free — no --workspace, no .kenn/. It
    // walks the clap Command tree and writes to stdout.
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let out = Command::cargo_bin("kenn")
            .unwrap()
            .args(["completions", shell])
            .env_remove("CLAUDE_PROJECT_DIR")
            .output()
            .unwrap();
        assert!(out.status.success(), "{shell} exited non-zero");
        assert!(
            out.stdout.len() > 1024,
            "{shell} produced suspiciously short output ({} bytes)",
            out.stdout.len()
        );
    }
}

#[test]
fn cli_rejects_unknown_subcommand() {
    let out = Command::cargo_bin("kenn")
        .unwrap()
        .arg("bogus")
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn cli_help_lists_four_subcommands() {
    let out = Command::cargo_bin("kenn")
        .unwrap()
        .arg("--help")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    for sub in ["init", "index", "status", "rollback"] {
        assert!(
            stdout.contains(sub),
            "help output missing `{sub}`:\n{stdout}"
        );
    }
}

/// `documents` count from `kenn status --json` for `workspace`. `status` reads
/// the persisted snapshot and never spawns the embedder, so this is CI-safe.
fn snapshot_documents(workspace: &Path) -> u64 {
    let out = ci(workspace, &["status", "--json"]).success();
    let json: serde_json::Value =
        serde_json::from_slice(&out.get_output().stdout).expect("status --json parses");
    json.get("meta")
        .and_then(|m| m.get("documents"))
        .and_then(serde_json::Value::as_u64)
        .expect("meta.documents present")
}

#[test]
fn init_then_index_produces_a_snapshot_with_builtin_producers_only() {
    // Markdown is a built-in producer: no external toolchain, so this runs
    // anywhere. init detects it, index builds a snapshot, status sees documents.
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("README.md"),
        "# Widgets\n\nProse about the widget subsystem.\n",
    )
    .unwrap();

    ci(dir.path(), &["init"]).success();
    ci(dir.path(), &["index"]).success();
    assert!(
        snapshot_documents(dir.path()) >= 1,
        "a markdown-only repo must produce a non-empty snapshot"
    );
}

#[test]
fn degraded_language_indexes_first_party_source_not_vendored() {
    // A Go repo whose indexer is absent degrades to the text fallback, which
    // must index the first-party .go files and skip vendor/. PATH is scrubbed on
    // init so `scip-go` is guaranteed absent regardless of the dev's machine.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("go.mod"), "module x\n").unwrap();
    std::fs::write(dir.path().join("a.go"), "package main\n").unwrap();
    std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
    std::fs::write(dir.path().join("pkg/b.go"), "package pkg\n").unwrap();
    std::fs::create_dir_all(dir.path().join("vendor/dep")).unwrap();
    std::fs::write(dir.path().join("vendor/dep/c.go"), "package dep\n").unwrap();

    // An empty PATH dir OUTSIDE the workspace, so the fixture contains only what
    // the assertion reasons about.
    let empty_path = TempDir::new().unwrap();
    Command::cargo_bin("kenn")
        .expect("kenn binary")
        .env("PATH", empty_path.path()) // no scip-go anywhere → Go degrades
        .args(["-w", dir.path().to_str().unwrap(), "init"])
        .assert()
        .success();
    ci(dir.path(), &["index"]).success();

    // Two first-party .go files indexed; vendor/dep/c.go excluded.
    assert_eq!(
        snapshot_documents(dir.path()),
        2,
        "text fallback indexes first-party .go and excludes vendor/"
    );
}

#[test]
fn init_survives_and_repairs_an_unparseable_config() {
    // The landmine: `Config::load_or_default` runs before dispatch, so a
    // malformed kenn.toml bricks EVERY command — including init — unless init
    // is short-circuited ahead of that load. `status` proves the brick;
    // `init` proves the escape.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("kenn.toml"), "garbage = = [[[ not toml\n").unwrap();

    // A normal command fails to even load the config.
    Command::cargo_bin("kenn")
        .expect("kenn binary")
        .arg("-w")
        .arg(dir.path())
        .arg("status")
        .assert()
        .failure();

    // init runs anyway, and --force repairs the file.
    Command::cargo_bin("kenn")
        .expect("kenn binary")
        .args(["-w", dir.path().to_str().unwrap(), "init", "--force"])
        .assert()
        .success();

    // The repaired config now loads, so status works.
    Command::cargo_bin("kenn")
        .expect("kenn binary")
        .args(["-w", dir.path().to_str().unwrap(), "status"])
        .assert()
        .success();
    assert!(
        dir.path().join("kenn.toml.bak").is_file(),
        "the broken config was backed up"
    );
}

#[test]
fn short_w_is_an_alias_for_long_workspace() {
    // `-w <dir> init` must target <dir>, exactly as `--workspace <dir>` does:
    // it creates <dir>/.kenn even when the process cwd is elsewhere. If `-w`
    // were not wired to the same global arg, init would fall back to cwd and
    // <dir>/.kenn would never appear.
    let dir = TempDir::new().unwrap();
    Command::cargo_bin("kenn")
        .expect("locate kenn binary")
        .arg("-w")
        .arg(dir.path())
        .arg("init")
        .assert()
        .success();
    assert!(
        dir.path().join(".kenn").is_dir(),
        "-w must target the given workspace, not cwd"
    );
}

#[test]
fn init_then_status_on_fresh_workspace() {
    let dir = TempDir::new().unwrap();
    ci(dir.path(), &["init"]).success();
    assert!(dir.path().join(".kenn").is_dir());
    assert!(dir.path().join("kenn.toml").is_file());

    ci(dir.path(), &["init"]).success(); // idempotent
    let out = ci(dir.path(), &["status"]).success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(stdout.contains("uninitialized") || stdout.contains("no live snapshot"));
}

#[test]
fn rollback_with_no_previous_errors() {
    let dir = TempDir::new().unwrap();
    ci(dir.path(), &["init"]).success();
    let assert = ci(dir.path(), &["rollback", "--yes"]);
    let out = assert.failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr);
    assert!(
        stderr.contains("no previous snapshot"),
        "stderr was: {stderr}"
    );
}

#[test]
fn end_to_end_index_no_drivers() {
    // With no language drivers configured (csharp present in default config
    // but no .sln to discover), `index` produces an empty snapshot. This
    // exercises the full chain: begin_indexing → writer open + schema
    // apply → run_pipeline (no work) → publish → status.
    let dir = TempDir::new().unwrap();
    // Override default csharp.enabled = true to avoid kenn-dotnet detection
    // varying by host. Empty config → all defaults except [language.csharp]
    // override.
    std::fs::write(
        dir.path().join("kenn.toml"),
        "[language.csharp]\nenabled = false\n",
    )
    .unwrap();

    ci(dir.path(), &["init"]).success();
    ci(dir.path(), &["index"]).success();

    let out = ci(dir.path(), &["status"]).success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    assert!(
        stdout.contains("snapshot:"),
        "status missing snapshot line:\n{stdout}"
    );
    assert!(
        stdout.contains("documents=0"),
        "expected zero counts on empty workspace:\n{stdout}"
    );

    // A second index against an unchanged non-git workspace should still
    // run (non-git → always-mismatching key) and publish a new snapshot.
    ci(dir.path(), &["index"]).success();
}

#[test]
fn query_groups_render_on_empty_index() {
    // The groups that return data on an empty snapshot (`overview` reports the
    // empty state; the findings store is independent of the code index) must
    // render in both TOON (default) and JSON.
    let dir = TempDir::new().unwrap();
    ci(dir.path(), &["init"]).success();
    ci(dir.path(), &["index"]).success();

    for case in [&["overview"][..], &["findings", "get", "fnd_nope"][..]] {
        let toon = ci(dir.path(), case).success();
        assert!(
            !toon.get_output().stdout.is_empty(),
            "{case:?} produced empty TOON"
        );
        let json: Vec<&str> = case.iter().copied().chain(["--json"]).collect();
        let out = ci(dir.path(), &json).success();
        serde_json::from_slice::<serde_json::Value>(&out.get_output().stdout)
            .unwrap_or_else(|e| panic!("{case:?} --json is not valid JSON: {e}"));
    }
}

#[test]
fn every_non_embedding_leaf_runs_without_panicking() {
    // Exercises every non-embedding leaf's dispatch arm end-to-end (clap parse
    // → ServerState build → bootstrap → tool call → render/error). On an empty
    // snapshot the code-graph tools return a clean `EMPTY_SNAPSHOT` error; the
    // contract asserted here is only that nothing panics. The embedding-backed
    // leaves (`find` bare/symbols, `findings search|add|merge`, `directives
    // --query`) are omitted — they'd load the model, which needs a daemon.
    let dir = TempDir::new().unwrap();
    ci(dir.path(), &["init"]).success();
    ci(dir.path(), &["index"]).success();

    let leaves: &[&[&str]] = &[
        &["find", "symbol", "Nope"],
        &["find", "at-location", "src/x.rs", "1"],
        &["find", "similar", "rs:Nope"],
        &["find", "usages", "Nope"],
        &["list", "callers", "rs:Nope"],
        &["list", "callees", "rs:Nope"],
        &["list", "implementers", "rs:Nope"],
        &["list", "overrides", "rs:Nope"],
        &["list", "usages", "rs:Nope"],
        &["list", "correspondences", "rs:Nope"],
        &["list", "in-scope", "rs:Nope"],
        &["list", "module-files", "rs:Nope"],
        &["list", "imports", "rs:Nope", "--direction", "both"],
        &["check", "links"],
        &["check", "css"],
        &["check", "findings"],
        &["findings", "directives", "src/x.rs"],
        &["findings", "predecessors", "fnd_nope"],
        &["findings", "successors", "fnd_nope"],
        &[
            "findings", "touch", "fnd_nope", "--op", "attach", "--anchor", "src/x.rs",
        ],
        &["get", "symbol", "rs:Nope"],
        &["get", "source", "rs:Nope"],
    ];
    for case in leaves {
        let out = ci(dir.path(), case).get_output().clone();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("panic") && !stderr.contains("RUST_BACKTRACE"),
            "{case:?} panicked:\n{stderr}"
        );
    }
}

fn git(dir: &Path, args: &[&str]) {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("run git");
}

#[test]
fn worktree_status_shows_fallback_from_parent() {
    let repo = TempDir::new().unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    git(
        repo.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(repo.path(), &["config", "user.name", "test"]);
    git(repo.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.path().join("README"), b"x").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "init"]);

    // Disable csharp so empty workspace publishes cleanly.
    std::fs::write(
        repo.path().join("kenn.toml"),
        "[language.csharp]\nenabled = false\n",
    )
    .unwrap();
    ci(repo.path(), &["init"]).success();
    ci(repo.path(), &["index"]).success();

    let wt_dir = TempDir::new().unwrap();
    let wt_path = wt_dir.path().join("feature-x");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "feature-x",
            wt_path.to_str().unwrap(),
        ],
    );

    let assert = ci(&wt_path, &["status"]).success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        stdout.contains("fallback: parent"),
        "expected fallback label in status output:\n{stdout}"
    );
}

// ── workspace-resolution chain (kenn mcp) ──────────────────────────
//
// Each test launches `kenn mcp` with controlled env / args, sends
// stdin EOF immediately (`write_stdin("")`), and asserts the
// startup-log line that records which source produced the bound
// workspace. The MCP server reads the source line BEFORE the
// dispatch loop starts, so these tests don't depend on the loop
// completing — only on the eprintln in `main.rs` firing.
//
// Output `assert_cmd`-style: no panic on non-zero exit, since
// MCP exits non-zero on stdin EOF (separate lance shutdown quirk).

fn mcp_startup(args: &[&str]) -> assert_cmd::Command {
    let mut cmd = Command::cargo_bin("kenn").expect("locate kenn binary");
    cmd.args(args).write_stdin("");
    cmd
}

#[test]
fn workspace_discovery_cli_flag_wins() {
    let dir = TempDir::new().unwrap();
    // Even with CLAUDE_PROJECT_DIR pointing elsewhere, --workspace wins.
    let out = mcp_startup(&["--workspace", dir.path().to_str().unwrap(), "mcp"])
        .env("CLAUDE_PROJECT_DIR", "/tmp")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("source=cli-flag"),
        "expected source=cli-flag, stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(dir.path().to_str().unwrap()),
        "expected the temp path in source line, stderr:\n{stderr}"
    );
}

#[test]
fn workspace_discovery_honors_claude_project_dir() {
    let dir = TempDir::new().unwrap();
    let out = mcp_startup(&["mcp"])
        .env("CLAUDE_PROJECT_DIR", dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("source=claude-project-dir"),
        "expected source=claude-project-dir, stderr:\n{stderr}"
    );
}

#[test]
fn workspace_discovery_rejects_bogus_claude_project_dir() {
    // Run from a TempDir so the fall-through doesn't land in some
    // git repo on the host machine (that would still bind cleanly,
    // but the warning line is what we care about here).
    let dir = TempDir::new().unwrap();
    let out = mcp_startup(&["mcp"])
        .env("CLAUDE_PROJECT_DIR", "/does/not/exist/kenn-sentinel")
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("CLAUDE_PROJECT_DIR") && stderr.contains("is not an existing directory"),
        "expected fall-through warning, stderr:\n{stderr}"
    );
    // After the warning, the chain proceeds — should land on git-toplevel or cwd.
    assert!(
        stderr.contains("source=git-toplevel") || stderr.contains("source=cwd"),
        "expected fall-through to git-toplevel/cwd, stderr:\n{stderr}"
    );
    // And the reason field names why we fell through.
    assert!(
        stderr.contains("reason=claude-project-dir-invalid"),
        "expected reason=claude-project-dir-invalid, stderr:\n{stderr}"
    );
}

#[test]
fn workspace_discovery_no_env_no_flag_emits_reason() {
    // No --workspace, no CLAUDE_PROJECT_DIR: fall through to
    // git-toplevel or cwd with `reason=no-claude-project-dir`.
    let dir = TempDir::new().unwrap();
    let out = mcp_startup(&["mcp"])
        .env_remove("CLAUDE_PROJECT_DIR")
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("source=git-toplevel") || stderr.contains("source=cwd"),
        "expected git-toplevel or cwd source, stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("reason=no-claude-project-dir"),
        "expected reason=no-claude-project-dir, stderr:\n{stderr}"
    );
}

/// The atlas axes answer from a real snapshot, including the tables axis.
///
/// Exercises `dispatch_axis`, which nothing else reaches: the router's coverage
/// came entirely from the non-axis arms, so adding an axis command tripped the
/// complexity gate on a function no test had ever run.
///
/// Markdown-only so it needs no external toolchain. The tables axis is empty
/// here and that is the point — an axis with nothing to show must still answer.
#[test]
fn the_atlas_axes_answer_on_a_built_snapshot() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("README.md"),
        "# Widgets\n\nProse about the widget subsystem.\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("schema.sql"),
        "CREATE TABLE widgets (id INT);\nSELECT id FROM widgets;\n",
    )
    .unwrap();
    ci(dir.path(), &["init"]).success();
    // `kenn init` leaves SQL opt-in; the axis must answer either way.
    ci(dir.path(), &["index"]).success();

    for axis in ["packages", "domains", "contracts", "documents", "tables"] {
        let out = ci(dir.path(), &[axis, "--json"])
            .success()
            .get_output()
            .clone();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("items"), "{axis} answered: {text}");
    }
}

/// Naming a table returns its references rather than the listing — the drill-in
/// half of the axis, and the arm that carries the grouping.
#[test]
fn naming_a_table_returns_its_reference_sites() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("README.md"), "# db\n").unwrap();
    std::fs::write(
        dir.path().join("schema.sql"),
        "CREATE TABLE widgets (id INT);\nSELECT id FROM widgets;\n",
    )
    .unwrap();
    ci(dir.path(), &["init"]).success();
    // The template already carries a `[language.sql]` section, opt-in and off;
    // appending a second one is a duplicate-key parse error, so flip the flag.
    let cfg = dir.path().join("kenn.toml");
    let text = std::fs::read_to_string(&cfg).unwrap();
    let text = text.replacen(
        "[language.sql]\nenabled = false",
        "[language.sql]\nenabled = true",
        1,
    );
    assert!(
        text.contains("[language.sql]\nenabled = true"),
        "flag flipped"
    );
    std::fs::write(&cfg, text).unwrap();
    ci(dir.path(), &["index", "--force"]).success();

    let listing = ci(dir.path(), &["tables", "--json"])
        .success()
        .get_output()
        .clone();
    let listing = String::from_utf8_lossy(&listing.stdout).to_string();
    assert!(
        listing.contains("widgets"),
        "the table is listed: {listing}"
    );

    let named = ci(dir.path(), &["tables", "sql:widgets", "--json"])
        .success()
        .get_output()
        .clone();
    let named = String::from_utf8_lossy(&named.stdout).to_string();
    assert!(
        named.contains("declares"),
        "naming it returns its sites, with what each does: {named}"
    );
}
