> **Sequencing note**: §1–§3 form one indivisible commit. Between adding `GoConfig` (§1) and the wire-up (§3), `cmd_index.rs` / `workflow.rs` won't reference Go yet but the workspace still compiles; the driver (§2) and config (§1) are independent until §3 joins them. Keep §1–§3 in one working-tree pass before clippy/tests. §4 (verify) lands with the same commit. §0 is independent of the rest and can land/verify on its own.

## 0. Fix `GoTransformer` to match real scip-go output (PREREQUISITE)

> Validated against real `scip-go` 0.2.4 and its own `internal/symbols/composer_test.go`: every symbol's first `Namespace` descriptor is the FULL package path (`obj.Pkg().Path()`, backtick-escaped when it contains `/` or `.`), while the `Package.Name` field is the MODULE path — and the two differ (composer_test.go:154: module `example.com/project`, package `example.com/lib`). The current `scip_to_public` seeds `native` with `head.package` (the module) and appends each namespace, which DUPLICATES the package path for first-party symbols and emits the wrong package when module≠package.

- [x] 0.1 In `crates/kenn-model/src/id/go.rs`, rewrite `scip_to_public` to build the native id from the DESCRIPTORS ALONE: the first `Namespace` descriptor IS the package path → start `native` from it (do NOT prepend `head.package`); append any further `Namespace` with `/`, and each `Type`/`Term`/`Method` with `.`. The module field (`head.package`/version) is metadata, not part of the id. Keep `parse_public` unchanged.
- [x] 0.2 Replace the two fictional tests (`package_method`, `package_top_level_term`) with scip-go's 8 ground-truth cases from `composer_test.go`, asserting the corrected mapping, e.g.:
  - `scip-go gomod example.com/lib v1.0.0 \`example.com/lib\`/MyStruct#` → `go:example.com/lib.MyStruct`
  - `scip-go gomod example.com/lib v1.0.0 \`example.com/lib\`/Server#Start().` → `go:example.com/lib.Server.Start`
  - `scip-go gomod example.com/lib v1.0.0 \`example.com/lib\`/Config#Name.` → `go:example.com/lib.Config.Name`
  - `scip-go gomod example.com/project 1.0.0 \`example.com/lib\`/Version.` → `go:example.com/lib.Version` (module ≠ package; module ignored)
  - plus a real first-party case from `conc`: `…conc … \`github.com/sourcegraph/conc/pool\`/Pool#New().` → `go:github.com/sourcegraph/conc/pool.Pool.New`
  - plus a stdlib case: `scip-go gomod github.com/golang/go/src go1.20 context/Context#Done().` → `go:context.Context.Done`
- [x] 0.3 Confirm `cross-language` id round-trip tests in `crates/kenn-model/tests/id_cross_language.rs` still pass (or update the Go fixture to the corrected form if one exists).
- [x] 0.4 `cargo test -p kenn-model` green; `cargo clippy -p kenn-model --all-targets` clean.

## 1. `[language.go]` config block

- [x] 1.1 Create `crates/kenn-config/src/language/go.rs` with `pub struct GoConfig { enabled: bool (default false), command: Vec<String> (default ["scip-go"]), excludes: Vec<String> }`, `#[serde(default, deny_unknown_fields)]`, mirroring `rust.rs`. Add `GoConfig::DEFAULT_EXCLUDES = &["vendor/**", "**/vendor/**", "**/testdata/**"]`; user-supplied `excludes` REPLACES the default (`excludes = []` opts out). Reject `command = []` consistent with the other languages' load-time validation.
- [x] 1.2 In `crates/kenn-config/src/language/mod.rs`: `mod go;`, `pub use go::GoConfig;`, and `#[serde(default)] pub go: GoConfig,` on `LanguageConfig`.
- [x] 1.3 Re-export `GoConfig` from `crates/kenn-config/src/lib.rs` alongside `PythonConfig` etc.
- [x] 1.4 Per-language tests in `go.rs`: defaults (`enabled=false`, `command=["scip-go"]`, excludes = the three globs), empty-`command` rejection, `excludes = []` opt-out.

## 2. `ScipGo` driver

- [x] 2.1 Create `crates/kenn-indexer/src/driver/go.rs` with `pub struct ScipGo { command: Vec<String> }` (default `vec!["scip-go".into()]`) and `impl ScipDriver for ScipGo`, modelled on `python.rs`.
- [x] 2.2 `language_id()` → `"go"`; `command()` → `PathBuf::from(self.command.first()...)`.
- [x] 2.3 `discover_units`: walk `walk_for_language(workspace, Language::Go)`; for every file named `go.mod`, push `Unit { identifier: format!("go-{idx}"), path: <parent dir of go.mod> }`. No `go.mod` → empty `Vec`. (Walk already prunes Go excludes, so `vendor/`/`testdata/` modules are skipped at recursion.)
- [x] 2.4 `run_unit`: `out = make_scip_output_path(workspace, &unit.identifier)`; spawn `Command::new(command[0]).args(command[1..]).args(["index", "--module-root", &unit.path, "--output", &out, "--quiet"])`; capture stderr. `NotFound` → `ScipOutcome::Unavailable` with `"scip-go launcher \`<tok>\` not found on PATH"`; non-zero exit → `ScipOutcome::Unavailable` with last stderr line; success → `ScipOutcome::Scip { path: out, report }`.
- [x] 2.5 Register in `crates/kenn-indexer/src/driver/mod.rs`: `mod go;` and `pub use go::ScipGo;`.
- [x] 2.6 Driver unit tests in `mod.rs` (mirror the Python set): `scip_go_discovers_one_unit_per_gomod`, `scip_go_discovers_no_units_without_gomod`, `scip_go_skips_vendor_and_testdata_modules`, `scip_go_returns_unavailable_when_binary_missing`.

## 3. Wire-up at both driver-construction sites

- [x] 3.1 In `crates/kenn-cli/src/cmd_index.rs`: add `ScipGo` to the `kenn_indexer::driver::{…}` import; add `.with_language_excludes(Language::Go, &config.language.go.excludes)?` to the workspace builder chain (alongside Rust/TS/C#/Python/Swift); in `build_driver`, add `if config.language.go.enabled { runner = runner.with_scip_driver(ScipGo { command: config.language.go.command.clone() }); }`.
- [x] 3.2 Same driver wire-up in `crates/kenn-indexer/src/workflow.rs` (import + `if config.language.go.enabled { … with_scip_driver(ScipGo …) }`).
- [x] 3.3 Update `crates/kenn-cli/src/starter_kenn.toml` (`kenn init`): add a `[language.go]` block matching the rust/python pattern — `enabled = false` (visible), one commented `# command = ["scip-go"]`, and a one-line note that scip-go needs a buildable module (deps fetched).

## 4. Verification — quality gates and end-to-end

- [x] 4.1 `cargo clippy --workspace --all-targets` — zero warnings (CLAUDE.md §5); narrow `#[allow]` only where a pedantic flag is intentional.
- [x] 4.2 `cargo test -p kenn-config -p kenn-indexer -p kenn-cli` — new tests pass, existing tests unaffected.
- [x] 4.3 `just crap-ci` — no regression / no new over-threshold function on `driver/go.rs` or `language/go.rs`; refresh baseline only if pre-existing entries shift (surface it, don't paper over — CLAUDE.md §6).
- [x] 4.4 Install scip-go (`go install github.com/sourcegraph/scip-go/cmd/scip-go@latest`); pick a small buildable Go module under `tmp/` (clone one if none present, `go mod download` it). `cargo build -p kenn-cli`; write a `kenn.toml` with `[language.go] enabled = true`; run `build/kenn index <module>` teeing to `tmp/go-index.log`.
- [x] 4.5 `build/kenn status` against the module reports non-zero `documents`/`symbols`/`definitions`, `status = ok`, no failed projects. Record actual counts in the commit message; a zero-edge result is a Go-transform bug to investigate, not a verification pass.
- [x] 4.6 Through the kenn MCP tools (ask user to reload kenn-mcp first per the memory note): `get_workspace_overview` lists `go`, and `search_symbols`/`find_symbol` returns `go:` results from the module.
- [x] 4.7 Multi-module sanity: a workspace with two `go.mod` files yields two units (assert via discovery test in §2.6; spot-check on a real two-module layout if available).
- [x] 4.8 **Real-data scale check on helm** (deps already cached under `tmp/go-validate/helm`, ~1.2 GB): `build/kenn index tmp/go-validate/helm` with `[language.go] enabled = true`, teeing to `tmp/helm-index.log`. Confirm `go:` symbols land with sane package paths (NO duplicated `…/github.com/...github.com/...` segments — the §0 regression signal), spot-check a known helm symbol (e.g. `go:helm.sh/helm/v3/pkg/action.Install.Run`) via `find_symbol`, and record symbol/edge counts in the commit message. A duplicated-package id here means §0 didn't take.
- [x] 4.9 `cargo fmt --all` as the final pre-commit step (CLAUDE.md §7).

## 5. SCIP edge-pipeline fixes (discovered during helm validation)

> These are general SCIP-pipeline fixes, not Go-specific — they correct latent bugs that only surfaced with a SCIP indexer (scip-go) that follows the spec's enclosing-range/role conventions. On helm they took the edge graph from 682 → 41,577 edges. Landable independently of §0–§4.

- [x] 5.1 **FROM-attribution by body, not name.** `DocumentDefIndex::from_document` (`edge.rs`) indexed each definition by its name-token `range`, which never contains a reference — so `smallest_enclosing` returned `None` for every reference whose indexer (scip-go) puts `enclosing_range` only on definitions. Fix: index by `occ.enclosing_range` (the body) when present, falling back to `occ.range`. Test `def_index_uses_enclosing_range_for_body_containment`.
- [x] 5.2 **Edge-kind from target descriptor, not role.** scip-go tags every reference `ReadAccess`, so `classify_edge_kind` collapsed all edges to `field_access`. Fix: a read is `field_access` only when the target is a data symbol (`target_is_data_symbol`: a `.`-term that isn't `()`-callable); method/type targets fall through to `TypeUse` for `refine_with_kind_hints` to promote. Test `read_access_classified_by_target_descriptor`. Result on helm: 14,875 calls / 13,594 type_use / 12,465 field_access / 643 implements.
- [x] 5.3 **External-stub pub_id transformed.** `build_stub_from_scip` (`naming.rs`) stored the raw SCIP descriptor as a stub's `pub_id` (`context/Background().`); for genuine externals (referenced, never defined in-workspace) the stub is the final record. Fix: run the symbol through `transformer_for(lang).scip_to_public`, descriptor as fallback. Test `external_stub_pub_id_is_transformed_not_raw_descriptor`. helm: 0 raw-descriptor pub_ids remain; edge target now `go:context.Background`.
- [x] 5.4 Gates: `cargo clippy -p kenn-indexer --all-targets` clean, full `kenn-indexer` suite green, `just crap-ci` PASSED, `cargo fmt --all`. No rust/python/typescript classification regressed (all language-agnostic, strictly-more-correct fixes).
