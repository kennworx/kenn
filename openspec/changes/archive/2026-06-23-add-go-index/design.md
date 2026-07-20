# Design — add-go-index

## Context

Everything below the producer is already done and verified:

```
  PRODUCER     driver/go.rs ScipGo ......... ✗  (this change)
               config.language.go gating .... ✗  (this change)
  CONFIG       GoConfig ..................... ✗  (this change)
  ─────────────────────────────────────────────────────────────
  TRANSFORM    transformer_for(Go)→GoTransformer ........ ✓
               language_from_scip("go"/"Go"), …_path(".go") ✓
  MODEL        Language::Go (go:, .go, go.mod/go.sum) ..... ✓
               id/go.rs GoTransformer (gomod descriptors) . ✓  ← tests match real scip-go output
               kind_from_scip_go_kind ..................... ✓  ← canonical SCIP kind mapper
```

So this change is purely the producer + config, slotting into the existing `ScipDriver` trait.

## Validated scip-go contract

From `github.com/sourcegraph/scip-go` (`cmd/scip-go/main.go` flag definitions):

```
scip-go index --output <file>   # -o; default index.scip; '-' = stdout
              --module-root <dir>     # directory containing go.mod (default: search cwd upward)
              --module-path <path>    # override module path inferred from go.mod
              --module-version <ver>
              --go-version <go1.x.y>
              --quiet / -q            # silence stdout/stderr
              --skip-tests            # do not index *_test.go
              [package-patterns...]   # positional; default ./...
```

- **Symbol format**: `scip-go gomod <pkg> <ver> <descriptor>` — `manager = "gomod"`, descriptor uses `/` namespace, `#` type, `.` term. This already matches `id/go.rs` (`MANAGER = "gomod"`, scheme `scip-go`) and its passing tests.
- **Build requirement** (validated on helm, real timing): scip-go runs `go list -mod=readonly -m -json` and loads packages via `go/packages`. **kenn assumes the project is already built before indexing** — same hands-off posture as rust-analyzer/Swift. kenn does NOT run `go build`/`go mod download`. If the toolchain or deps are missing, scip-go exits non-zero and the unit is reported `Unavailable`/failed, not silently skipped.
  - **Why this matters**: a *cold* module makes scip-go appear to hang — `go/packages` compiles missing dependencies on the fly. Measured on helm (`helm.sh/helm/v4`, full k8s dep tree): cold scip-go ran 12+ min (killed); after `go build ./...` (6:53 one-time) the **same scip-go run took 4.8 s** and produced 53,185 first-party symbols. So the requirement is a *warm build cache*, not merely downloaded deps. This is the user's responsibility (build as normal before `kenn index`); the driver just documents it and surfaces the non-zero exit if the module isn't ready.

## Decision 0 — fix `GoTransformer` (the transform layer is NOT correct as-is)

Validated against real `scip-go` 0.2.4 output and its own `internal/symbols/composer.go` + `composer_test.go`. scip-go composes every symbol as `scip.Symbol{ Scheme:"scip-go", Package, Descriptors }`:

```
  scip-go gomod   <Package.Name>   <Package.Version>   <Descriptors...>
                  = pkg.Module.Path                    Descriptors[0] = obj.Pkg().Path()
                  (the MODULE)                         (the FULL package path) as a Namespace,
                                                       backtick-escaped when it has `/` or `.`,
                                                       then Type(#) / Term(.) / Method().
```

The package path lives in the **descriptor**, not the `Package.Name` field — and the two genuinely differ (composer_test.go:154: module `example.com/project`, package `example.com/lib`). The current `scip_to_public` seeds `native` with `head.package` (the module) then appends each namespace, so:

| real scip-go symbol | current transformer | correct |
|---|---|---|
| `…conc … \`…conc/pool\`/Pool#` | `go:…conc/…conc/pool.Pool` ❌ dup | `go:github.com/sourcegraph/conc/pool.Pool` |
| `…project 1.0.0 \`…lib\`/Version.` | `go:…project/…lib.Version` ❌ wrong pkg | `go:example.com/lib.Version` |

**Fix**: build the public id from the descriptor segments alone — the first `Namespace` IS the package path; do not prepend `head.package`. This matches the *stated* intent in `id/go.rs`'s own doc comment (`go:package_path.Symbol`); the implementation just used the module as `package_path`. The module/version is metadata (the version already isn't in the id). For stdlib the descriptor namespace is the short import path (`context`), so dropping the module yields the natural `go:context.Context.Done` — correct.

The descriptor PARSER needs no change: kenn's `read_name`/`trim_backticks` already consume backtick-quoted names containing `/` and `.` and handle doubled-backtick escapes. Only `scip_to_public`'s reconstruction changes, plus replacing the two fictional tests with scip-go's ground-truth cases (the old tests encoded a relative-descriptor format scip-go does not emit). See tasks §0.

## Decision 1 — discovery: one unit per `go.mod`

scip-go is module-scoped: `--module-root` points at a single `go.mod`. A monorepo with several modules needs one invocation per module (cf. dotnet's one-unit-per-`.sln`). This is a new discovery shape for a `ScipDriver` (Rust is workspace-wide, Python is single-unit-at-root), but trivially expressed with the existing `walk_for_language` helper filtering for the `go.mod` filename.

```
discover_units:
  if config targets non-empty → one unit per configured dir (verified to contain go.mod)   [future-proof, optional]
  else → walk_for_language(Go); for each file named `go.mod`, emit one unit:
            Unit { identifier: "go-<idx>", path: <dir containing go.mod> }
  no go.mod found → zero units, scip-go NOT spawned
```

`walk_for_language` already prunes `[workspace].excludes` + `[language.go].excludes` at recursion time, so `vendor/` and `testdata/` modules cost zero IO.

**Alternatives considered.** (a) Single invocation at workspace root relying on scip-go's upward `go.mod` search — rejected: misses sibling modules in a monorepo, and `./...` from the wrong root either errors or under-indexes. (b) Config-only module list (no walk) — rejected: most repos are single-module; zero-config discovery is the common case. A `targets` field can be added later if a user needs to pin a subset; not in scope now (§2 simplicity).

## Decision 2 — invocation: explicit `--module-root`, distinct output per unit

```
run_unit(unit):
  out = make_scip_output_path(workspace, &unit.identifier)   # "go-0", "go-1" → no collision
  scip-go [command[1..]] index --output <out> --module-root <unit.path> --quiet
  NotFound(command[0]) → ScipOutcome::Unavailable { "scip-go launcher `<tok>` not found on PATH" }
  non-zero exit       → ScipOutcome::Unavailable { last stderr line }
  success             → ScipOutcome::Scip { path: out, report }
```

Pass `--module-root <abs>` explicitly rather than relying on `current_dir`, mirroring how Python passes `--cwd`/`--target-only`. Each unit gets its own slug so multi-module `.scip` files don't overwrite each other; the existing multi-run merge stitches them.

## Decision 3 — Go-specific excludes

Default `[language.go].excludes`: `["vendor/**", "**/vendor/**", "**/testdata/**"]`.

- `vendor/` holds vendored dependency source — a `vendor/.../go.mod` is not a first-party module and must not become its own unit.
- `testdata/` is the Go convention for fixture files, which are frequently deliberately non-buildable; a stray `go.mod` under `testdata/` would break a module-scoped run.

Per the existing convention (`RustConfig::DEFAULT_EXCLUDES`), a user-supplied `excludes` REPLACES the default fully (`excludes = []` opts out).

## Decision 4 — external binary, not a sidecar

Swift/.NET got `kenn-swift`/`kenn-dotnet` sidecars because no adequate first-party SCIP indexer existed. Go has a mature, maintained first-party one (`sourcegraph/scip-go`) emitting exactly the `scip-go gomod` symbols `GoTransformer` already parses. So Go follows the scip-python/rust-analyzer pattern: an external, user-installable binary named via the `command` token vector — not a new sidecar crate. This keeps the change to a driver + config block.

## Decision 5 — SCIP edge-pipeline fixes (general, surfaced by Go)

Validating Go on helm exposed that kenn produced only 682 edges for 5,592 symbols. Decoding `helm.scip` with the scip bindings pinned the cause to three latent bugs in the SCIP edge pipeline — none Go-specific, all triggered because scip-go follows the SCIP spec's conventions where scip-python/typescript diverge. Each fix is language-agnostic and strictly more correct.

1. **FROM-attribution: body, not name** (`edge.rs::DocumentDefIndex`). The def index keyed each definition by its name-token `range`. A reference at line 50 is never inside the name identifier of `func Foo` at line 45, so `smallest_enclosing` returned `None` for all 134,574 helm references (scip-go puts `enclosing_range` only on definitions — 3,186 of them — never on references; scip-python stamps it on references, which is why kenn worked for it). Fix: index by `occ.enclosing_range` (the body) when present, falling back to `occ.range`.

2. **Edge kind: target descriptor, not role** (`edge.rs::classify_edge_kind`). scip-go tags every reference `ReadAccess`, and the classifier mapped any read → `field_access`, collapsing calls/type-uses. Fix: a read is `field_access` only when the target is a data symbol (`target_is_data_symbol`: a `.`-term that is not a `()`-callable); methods (`().`) and types (`#`) fall through to `TypeUse`, which `refine_with_kind_hints` promotes (`().` → `Calls`). After (helm): 14,875 calls / 13,594 type_use / 12,465 field_access / 643 implements — 41,577 total (60×).

3. **External-stub pub_id transformed** (`naming.rs::build_stub_from_scip`). A referenced-but-undefined external becomes a drained stub whose `pub_id` was the raw descriptor (`context/Background().`). Interning is keyed by SCIP string, so only true externals (stdlib/deps) kept the bad id. Fix: run the symbol through `transformer_for(lang).scip_to_public`, descriptor as fallback → `go:context.Background`.

These could be a separate change, but they are recorded here because Go validation is what surfaced and proved them. They carry their own tests (`def_index_uses_enclosing_range_for_body_containment`, `read_access_classified_by_target_descriptor`, `external_stub_pub_id_is_transformed_not_raw_descriptor`) and regress no other language.

## Non-goals

- No `go mod download` / build orchestration by kenn (caller's environment responsibility).
- No `[language.go].targets` sub-module pinning yet (add when a real multi-module user needs a subset).
- No new MCP behaviour — the empty-snapshot `config-disabled`/`configured-but-empty` diagnostics from `kenn-python-support` already cover a Go-disabled or Go-enabled-but-empty workspace once `go` is a known `[language.*]` key.
