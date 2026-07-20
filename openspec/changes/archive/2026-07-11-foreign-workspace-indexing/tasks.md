## 1. Workspace targeting

- [x] 1.1 Add `short = 'w'` to the global `--workspace` arg (`crates/kenn-cli/src/main.rs:36`).
      Verify: `command_tree_is_valid` still passes (arg-name uniqueness holds with `-w`);
      `cli_smoke::short_w_is_an_alias_for_long_workspace` asserts `-w <dir> init` creates
      `<dir>/.kenn` from an unrelated cwd, proving it drives the same global arg.

## 2. Detection

- [x] 2.1 New `crates/kenn-cli/src/init/detect.rs`: one table per language holding its
      marker predicate, source globs, version-probe argv, and install hint. This is new
      code — only `driver/rust.rs:50` and `driver/go.rs:47` have marker-shaped discovery;
      python keys on configured `targets` and typescript/csharp/swift are `JsonlIndexer`s
      with no `discover_units`.
      Verify: table-driven tests over temp dirs — a bare dir yields nothing; a repo with
      `go.mod` + `tsconfig.json` yields both.

- [x] 2.2 Single recursive marker walk, pruning the union of the per-language
      `DEFAULT_EXCLUDES` `pub const`s in `kenn-config` (`language/{rust,go,python,
      csharp,typescript,swift,markdown,text}.rs`). No config is read — `init` runs before
      one exists.
      Verify: `services/api/go.mod` detects Go; `vendor/x/go.mod` alone does not;
      `target/**/Cargo.toml` alone does not.

- [x] 2.3 Version probe: spawn the language's probe argv, require exit 0. Spawn failure
      or non-zero exit ⇒ unavailable.

      All three in-house sidecars answer `--version` bare (no name prefix) on stdout,
      exit 0, without loading the toolchain they index — verified on built binaries:
      `kenn-dotnet` (MSBuild registration moved into the `index` action, so it answers
      with no .NET SDK reachable), `kenn-ts` (previously threw
      `ERR_PARSE_ARGS_UNKNOWN_OPTION`), `kenn-swift` (previously exited 2 with usage).
      Third-party probes — `rust-analyzer --version`, `scip-go --version`,
      `scip-python --version` — are NOT verified; confirm each before enabling it.

      Verify: a stub `rust-analyzer` on `PATH` exiting 1 classifies Rust as degraded,
      not enabled — this is the Homebrew-rustup-shim failure mode.
      No timeout: `std::process::Command` has none, and a `--version` hang is not a real
      scenario (§2). Revisit only if a probe is observed to block.

- [x] 2.4 Classify each detected language as `Enabled` or `Degraded { command, hint }`.
      Verify: with a stub PATH, a language flips between the two classifications.

## 3. Config authoring

- [x] 3.1 Build a `LanguageConfig` from the classification: `enabled = true` and **no
      `command` key** for `Enabled` (the default already resolves on `PATH`); for each
      `Degraded`, append its source globs to `[language.text] include` and set
      `enabled = true`. Built-in producers (markdown, CSS, HTML) enabled when their file
      types are present.
      Verify: a Rust+Go workspace with a broken `scip-go` yields `[language.rust]
      enabled = true` with no `command`, and `**/*.go` in the text fallback.

- [x] 3.2 Write `[language.text] excludes` as the union of `TextConfig::DEFAULT_EXCLUDES`
      and each degraded language's `DEFAULT_EXCLUDES`. A user-supplied `excludes` list
      REPLACES the defaults, so the union must be written explicitly.
      Verify: degrading Go emits `vendor/**` and `**/testdata/**`; an index run over a
      fixture with a populated `vendor/` chunks zero files from it.

- [x] 3.3 Extend the 2.1 table with per-language test globs. Seed `[tests] paths` from
      the **enabled** languages' globs, and only when the existing `tests.paths` is empty.
      Degraded languages contribute nothing — `text/ingest.rs:112` hardcodes
      `test: false`, so their globs would be inert. When a language is enabled but
      `tests.paths` is already non-empty, report the globs instead of adding them.
      Verify: fresh init on a Rust+Go workspace writes both languages' globs and no
      others; `paths = ["custom/**"]` survives `--force` untouched; a degraded Go
      contributes nothing. Guard the regression directly — `Config::default().tests.paths`
      is empty and `[tests] paths` is authoritative with no fallback
      (`tests_config.rs:8`), so an unseeded render silently disables all test detection.

- [x] 3.4 Author with **`toml_edit`** (`0.25`, `+spec-1.1.0` lineage — shares
      `toml_datetime` with the workspace `toml 1.1.2`, no duplicate). Do NOT serialize a
      typed `Config`: `toml::to_string(&Config::default())` emits every default
      (`command = ["rust-analyzer"]`, all excludes, `[mcp]`, …), which violates 3.1's
      "no `command` key" and freezes defaults. Instead parse the commented starter
      template into a `toml_edit::DocumentMut` and mutate keys in place — flip
      `[language.X].enabled`, add degraded globs to `[language.text]`, set `[tests] paths`
      — leaving every doc-comment and untouched key intact.
      Verify: the rendered doc reparses via `Config::from_toml` under
      `deny_unknown_fields`; a fresh init still carries the template's `# command = …`
      doc-comments.

- [x] 3.5 Keep `assets/starter_kenn.toml` as the zero-detection fallback (empty repo)
      and as the source of the commented documentation stanzas.
      Verify: `kenn init` in an empty temp dir still writes a parseable config.

## 4. Report

- [x] 4.1 Print one line per considered language — enabled / degraded / absent — plus an
      install hint per failing probe, and a trailing summary of what was written. No stdin
      reads, no TTY branch.
      Verify: run with stdin closed, assert no block and a success exit code even when
      every language degraded; snapshot the report for a mixed workspace.

## 5. Merge, force, and the broken-config landmine

- [x] 5.1 Add `--force` to `Command::Init`. Without it, keep the never-overwrite behavior
      of `cmd_init.rs:21-26` and name the flag in the message.
      Verify: existing test `idempotent_on_already_initialized` still passes.

- [x] 5.2 `--force` against a parseable config: parse it to a `toml_edit::DocumentMut`
      and merge **at the key level within `[language.*]`** — set `enabled`, add
      `command`/globs where detection calls for them, and NEVER remove or overwrite a
      sibling key the user set (`max_threads`, `low_priority`, `projects`, per-language
      `excludes`). This is NOT "replace the `language` section": those user keys live
      inside `[language.X]`, so a section replace would wipe them. Non-language sections
      pass through untouched. `toml_edit` preserves comments, so no `.bak` is needed on
      this path.
      Verify: a config with `[tests] paths`, `[layout] derived_root`, `[metrics]`, rust
      `max_threads`, and markdown `excludes` retains all of them, comments included, after
      `--force`. Regression-guard `[tests] paths` specifically — authoritative with no
      fallback (`tests_config.rs:8`), so losing it silently disables all test detection.

- [x] 5.3 Short-circuit `init` so a malformed `<workspace>/kenn.toml` cannot brick it.
      `Config::load_or_default` (`config.rs:63`) errors at `main.rs:266`, before
      `dispatch_command` at `main.rs:306`; `completions` and `cc-hook` already
      short-circuit earlier (`main.rs:234-245`). On a parse error: warn, resolve the
      layout against `Config::default()`, and under `--force` back up and fully replace
      while reporting that non-language settings were discarded.
      Verify: a workspace whose `kenn.toml` has an unknown field makes `kenn status` fail
      and `kenn init` succeed; `kenn init --force` repairs it and `kenn status` then succeeds.

## 6. End-to-end

- [x] 6.1 CLI integration test (`crates/kenn-cli/tests/`): fixture repo → `init -w` →
      assert the written config → `index -w` → assert a non-empty snapshot, using only
      built-in producers so the test needs no external toolchain.
      Verify: passes on a machine with no language indexers installed.

- [x] 6.2 Degraded-path integration test: fixture with `go.mod`, a `vendor/` tree, and no
      `scip-go` on `PATH` → `init -w` → `index -w` → `find semantic` returns a chunk from a
      first-party `.go` file and none from `vendor/`.
      Verify: the run succeeds with no language indexer on `PATH`.

## 7. Gates

- [x] 7.1 `cargo clippy --workspace --all-targets` — zero warnings (pedantic is on).
- [x] 7.2 `just crap-ci` — the classifier is branch-heavy; split the per-language table
      into small tested helpers rather than baselining one large function.
- [x] 7.3 `cargo fmt --all` last, after 7.1 and 7.2 are green.
