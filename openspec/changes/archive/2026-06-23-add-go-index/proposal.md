## Why

`kenn index` produces an empty snapshot on Go-only workspaces today. Most of the model layer is already wired for Go: `kind_from_scip_go_kind` is the canonical SCIP kind mapper (rust-analyzer reuses it), `language_from_scip`/`language_from_path` recognise `go`/`.go`, and `Language::Go` already declares the `go:` prefix, `.go` extension, and `go.mod`/`go.sum` project files. The two live `scip-indexer` requirements that name Go (`*.go`/module roots for discovery; `Go → ["scip-go"]` for dispatch) are aspirational text with no scenarios and no implementation.

Two pieces are missing. **(1)** `GoTransformer` is wrong: validated against real `scip-go` 0.2.4 (and scip-go's own `composer_test.go`), every symbol's first descriptor is the FULL package path (`obj.Pkg().Path()`) while the package field is the MODULE path — and they differ. The current transformer prepends the module, duplicating the package path for first-party symbols (`go:…conc/…conc/pool.Pool`). The existing `id/go.rs` tests only covered a relative-descriptor format scip-go does not emit, so this never surfaced. **(2)** A **producer**: a `ScipGo` driver that spawns `scip-go` and hands the resulting `.scip` to the transform pipeline, plus the `[language.go]` config block to turn it on. This mirrors the `kenn-python-support` change, with the config-shape unification and MCP empty-snapshot diagnostics already in place from that change.

## What Changes

- **Fix `GoTransformer::scip_to_public`** to build the public id from the descriptor segments alone (the first `Namespace` descriptor is the package path), not by prepending the module field — and replace the two fictional `id/go.rs` tests with scip-go's ground-truth cases. Without this, every first-party Go symbol gets a duplicated/garbled id.
- Add a `ScipGo` SCIP driver that discovers Go modules by walking for `go.mod`, spawns `scip-go index --output <out> --module-root <module dir> --quiet` once per module, and returns each produced `.scip` for the existing transform pipeline to ingest.
- Add `[language.go]` to the kenn config (`enabled: bool` default `false`, `command: Vec<String>` default `["scip-go"]`, `excludes: Vec<String>`), matching the unified per-language shape.
- Wire the new driver into `cmd_index::build_driver` and `workflow.rs` alongside the existing five, gated on `config.language.go.enabled`.
- Add Go to the per-language exclude wiring (`with_language_excludes(Language::Go, …)`) and the `kenn init` starter `kenn.toml`.
- **Fix three latent SCIP edge-pipeline bugs** surfaced by validating Go on helm (general, not Go-specific — they bite any indexer that follows the SCIP spec's enclosing-range/role conventions rather than scip-python's): (1) FROM-attribution indexed defs by name range instead of body `enclosing_range`, so references attributed to nothing; (2) `classify_edge_kind` collapsed all `ReadAccess` refs to `field_access` instead of reading the target descriptor; (3) external-symbol stubs stored the raw SCIP descriptor as `pub_id` instead of the transformed `go:`/`rs:`/… id. Together these took helm's Go graph from 682 → 41,577 correctly-typed edges with canonical external ids.

## Capabilities

### New Capabilities

(none — Go is already named in the live `scip-indexer` capability's discovery and dispatch requirements; this change makes those rules concrete and adds the implementation)

### Modified Capabilities

- `scip-indexer`: make the Go discovery rule concrete (one indexable unit per `go.mod` module root, `vendor/`/`testdata/` skipped); add a `Go indexer dispatch via launcher command` requirement with scenarios for single-module, multi-module, missing-launcher, and the build-requirement caveat — mirroring the existing `Python indexer dispatch via launcher command` requirement.

## Impact

- **Code**: new `crates/kenn-indexer/src/driver/go.rs` (`ScipGo`); module registration in `driver/mod.rs`; new `crates/kenn-config/src/language/go.rs` (`GoConfig`) + `pub go` field in `language/mod.rs`; wire-up at `crates/kenn-cli/src/cmd_index.rs::build_driver` (driver + `with_language_excludes` + the no-languages guard) and `crates/kenn-indexer/src/workflow.rs::index_workspace` (driver + Go excludes); the two MCP language lists (`error.rs` `ConfigHint`, `orchestrate.rs` `config_expects_symbols`); the `id/go.rs` transformer fix; and the three edge-pipeline fixes in `crates/kenn-indexer/src/edge.rs` (`DocumentDefIndex`, `classify_edge_kind`) and `crates/kenn-indexer/src/transform/naming.rs` (`build_stub_from_scip`); starter `kenn.toml`.
- **Config**: additive `[language.go]` block — no breaking change (unlike the Python change, the unified shape already exists). Default `enabled = false`, so existing workspaces are unaffected.
- **External dependency**: `scip-go` (`github.com/sourcegraph/scip-go`). The user supplies the launcher (`["scip-go"]`, or an absolute path). scip-go shells to `go list`/`go/packages`, so the module must be buildable with deps available — the same "needs a real build" posture as rust-analyzer and Swift, and surfaced through the existing Tier-2 preflight on the launcher token.
- **No spec-level impact** on `indexing-orchestrator`, the data-model, or any store spec — `ScipGo` fits the existing `ScipDriver` trait and emits records the existing transform already handles end-to-end.
