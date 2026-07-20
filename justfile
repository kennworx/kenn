# Build the kenn CLI → ./build/kenn. profile=debug (default) or release.
build-cli profile="debug":
  @case "{{profile}}" in release|debug) ;; *) echo "invalid profile: {{profile}} (use release|debug)" >&2; exit 1 ;; esac
  @cargo build {{ if profile == "release" { "--release" } else { "" } }} -p kenn
  @mkdir -p build
  # Copy (not symlink) so each worktree's `build/kenn` is independent of
  # other worktrees that rebuild the shared cargo `target/`.
  @cp "$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/{{profile}}/kenn" build/kenn
  # macOS: `cp` invalidates the cargo binary's ad-hoc code signature;
  # AMFI then SIGKILLs the binary at exec (exit 137, no output). Re-sign
  # in place. No-op on non-macOS (codesign isn't on PATH; `|| true`).
  @if [ "$(uname)" = "Darwin" ]; then codesign --force --sign - build/kenn 2>/dev/null || true; fi
  @ls -lh build/kenn

# Map (just-os, just-arch) pairs to .NET RuntimeIdentifier strings.
host_rid := if os() + "-" + arch() == "macos-aarch64" { "osx-arm64" } \
       else if os() + "-" + arch() == "macos-x86_64"  { "osx-x64"   } \
       else if os() + "-" + arch() == "linux-aarch64" { "linux-arm64" } \
       else if os() + "-" + arch() == "linux-x86_64"  { "linux-x64"   } \
       else if os() + "-" + arch() == "windows-x86_64"{ "win-x64"     } \
       else { error("unsupported host: " + os() + "/" + arch()) }

# Build self-contained single-file ./build/kenn-dotnet for the given runtime id. Default: host platform.
build-indexer-dotnet rid=host_rid:
  @dotnet publish indexers/kenn-dotnet -c Release -r {{ rid }} --nologo
  @mkdir -p build
  @cp indexers/kenn-dotnet/bin/Release/net10.0/{{ rid }}/publish/kenn-dotnet build/kenn-dotnet

# Build the Swift index-store sidecar → ./build/kenn-swift (release).
build-indexer-swift:
  @swift build -c release --package-path indexers/kenn-swift
  @mkdir -p build
  @cp indexers/kenn-swift/.build/release/kenn-swift build/kenn-swift
  @ls -lh build/kenn-swift

# Run the kenn-swift integration test suite (builds the fixture; slow).
test-indexer-swift:
  @swift test --package-path indexers/kenn-swift

# Build self-contained single-file ./build/kenn-ts via `bun build --compile`.
build-indexer-ts:
  @cd indexers/kenn-ts && bun install
  @mkdir -p build
  @bun build indexers/kenn-ts/src/main.ts --compile --outfile build/kenn-ts
  @ls -lh build/kenn-ts

# Install kenn + available language indexers into a bin dir on PATH (default ~/.local/bin).
install prefix="~/.local/bin" profile="release":
  #!/usr/bin/env bash
  set -euo pipefail
  dest="{{prefix}}"; dest="${dest/#\~/$HOME}"
  mkdir -p "$dest"
  echo "installing kenn → $dest"
  # Core CLI — the only always-required binary (query + orchestration).
  just build-cli {{profile}} >/dev/null
  install -m 755 build/kenn "$dest/kenn"; echo "  ✓ kenn         (core)"
  # Language indexers: build + install each whose toolchain is available.
  if command -v dotnet >/dev/null 2>&1; then just build-indexer-dotnet >/dev/null && install -m 755 build/kenn-dotnet "$dest/kenn-dotnet" && echo "  ✓ kenn-dotnet  (C#)"; else echo "  – kenn-dotnet  skipped — no dotnet SDK"; fi
  if command -v bun    >/dev/null 2>&1; then just build-indexer-ts     >/dev/null && install -m 755 build/kenn-ts     "$dest/kenn-ts"     && echo "  ✓ kenn-ts      (TypeScript)"; else echo "  – kenn-ts      skipped — no bun"; fi
  if command -v swift  >/dev/null 2>&1; then just build-indexer-swift  >/dev/null && install -m 755 build/kenn-swift  "$dest/kenn-swift"  && echo "  ✓ kenn-swift   (Swift)"; else echo "  – kenn-swift   skipped — no swift toolchain"; fi
  # macOS: install/cp invalidates the ad-hoc code signature; AMFI SIGKILLs the
  # binary at exec otherwise. Re-sign each installed binary in place.
  if [ "$(uname)" = "Darwin" ]; then for b in kenn kenn-dotnet kenn-ts kenn-swift; do [ -f "$dest/$b" ] && codesign --force --sign - "$dest/$b" 2>/dev/null || true; done; fi
  command -v rust-analyzer >/dev/null 2>&1 || echo "  ! rust-analyzer not on PATH — needed to index Rust"
  case ":$PATH:" in *":$dest:"*) ;; *) echo "  ! $dest is not on PATH — add:  export PATH=\"$dest:\$PATH\"";; esac
  echo "done."

# Run the kenn-dotnet xunit test project.
test-indexer-dotnet:
  @dotnet test indexers/kenn-dotnet.tests/kenn-dotnet.tests.csproj --nologo

# Every sidecar test suite, plus the artifact-level probe contract.
test-indexers: test-indexer-dotnet test-indexer-swift probe-smoke index-stability
  @cd indexers/kenn-ts && bun test

# Stress kenn-dotnet index for abort regressions (8 runs, ~4s each). Requires a .NET SDK.
index-stability: build-indexer-dotnet
  #!/usr/bin/env bash
  set -euo pipefail
  # `kenn-dotnet index` must not abort. Draining `dotnet restore`'s redirected
  # pipes with CopyToAsync corrupts an ArrayPool buffer on macOS and kills the run
  # with a fatal AccessViolationException — measured at 6 aborts in 12 runs before
  # SolutionLoader switched to blocking reads. The property is probabilistic, so no
  # unit test can assert it: 8 runs miss a 50% regression with probability 0.4%.
  if ! command -v dotnet >/dev/null 2>&1; then echo "  - no .NET SDK; skipping"; exit 0; fi
  mkdir -p tmp
  aborts=0
  for i in $(seq 1 8); do
    ./build/kenn-dotnet index --workspace . \
      --projects indexers/kenn-dotnet/kenn-dotnet.csproj \
      >/dev/null 2>"tmp/stability-$i.err" || true
    if grep -q "Fatal error" "tmp/stability-$i.err"; then aborts=$((aborts + 1)); fi
  done
  if [ "$aborts" -ne 0 ]; then
    echo "  ! kenn-dotnet index aborted on $aborts of 8 runs"
    grep -m1 "Fatal error" tmp/stability-*.err | head -1
    exit 1
  fi
  echo "  ok kenn-dotnet index: 0 aborts / 8 runs"

# Assert every sidecar handles an unreachable toolchain without crashing.
probe-smoke: build-indexer-dotnet build-indexer-ts build-indexer-swift
  #!/usr/bin/env bash
  set -euo pipefail
  # `kenn init` probes each indexer this way to decide whether the language is
  # indexable; a non-zero exit silently degrades it to the generic text fallback.
  # Builds the binaries first: this cannot live in the xunit suite, because under
  # `dotnet test` the indexer runs via `dotnet exec` and MSBuildLocator resolves an
  # SDK from the muxer's own path no matter how the environment is scrubbed. Only
  # the self-contained binary can be probed.
  # A hermetic, empty workspace: `tmp/` is the repo's general scratch dir, and a
  # stray Package.swift or .csproj left there would change what the probe indexes.
  mkdir -p tmp
  ws=$(mktemp -d "${TMPDIR:-/tmp}/kenn-probe.XXXXXX")
  trap 'rm -rf "$ws"' EXIT
  fail=0
  probe() {  # tool -> exit 0, a version on stdout, no toolchain complaint
    local tool="$1" bin="build/$1" out code err
    out=$(env -i PATH= HOME="$HOME" "./$bin" --version 2>"tmp/probe-$tool.err") && code=0 || code=$?
    err=$(cat "tmp/probe-$tool.err")
    if [ "$code" -ne 0 ]; then echo "  ! $tool --version exited $code (want 0)"; return 1; fi
    if ! printf '%s' "$out" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+'; then
      echo "  ! $tool --version printed '$out', want a version"; return 1
    fi
    # Warnings on stderr are tolerated (dyld, deprecations). A toolchain
    # complaint is not: the probe must never need the toolchain it probes.
    if printf '%s' "$err" | grep -Eqi 'no usable|not found|requires the'; then
      echo "  ! $tool --version complained about its toolchain: $err"; return 1
    fi
    echo "  ok $tool --version -> $out"
  }
  for tool in kenn-dotnet kenn-ts kenn-swift; do probe "$tool" || fail=1; done

  # Indexing without a toolchain must degrade, not crash: a signal death or an
  # empty stream is what `kenn init`'s degrade path cannot distinguish from
  # success. kenn-dotnet must name the cause; the other two must at least emit
  # a well-formed stream and exit cleanly.
  if env -i PATH= HOME="$HOME" ./build/kenn-dotnet index --workspace "$ws" >tmp/probe-index.out 2>tmp/probe-index.err; then
    echo "  ! kenn-dotnet index succeeded with no SDK reachable"; fail=1
  elif ! grep -q "no usable MSBuild instance" tmp/probe-index.err; then
    echo "  ! kenn-dotnet index did not report the missing toolchain"; fail=1
  elif [ "$(grep -c "no usable MSBuild instance" tmp/probe-index.err)" -ne 1 ]; then
    # A raw Console.Error.WriteLine beside a logger call printed the same
    # sentence twice, and the Rust side then appended the stderr tail next to
    # the error frame carrying it a third time.
    echo "  ! kenn-dotnet repeated the reason on stderr"; fail=1
  elif ! grep -q '"type":"error"' tmp/probe-index.out; then
    echo "  ! kenn-dotnet index reported the failure on stderr but not on the wire"; fail=1
  else
    echo "  ok kenn-dotnet index reports the missing toolchain once, on stderr and the wire"
  fi

  # The diagnostic must not be suppressible. Routing it through ILogger made
  # `KENN_DOTNET_LOG=Critical` silence it: exit 1, zero bytes of explanation.
  env -i PATH= HOME="$HOME" KENN_DOTNET_LOG=Critical ./build/kenn-dotnet index --workspace "$ws" \
    >/dev/null 2>tmp/probe-quiet.err || true
  if ! grep -q "no usable MSBuild instance" tmp/probe-quiet.err; then
    echo "  ! KENN_DOTNET_LOG silenced the missing-toolchain diagnostic"; fail=1
  else
    echo "  ok the diagnostic survives KENN_DOTNET_LOG=Critical"
  fi

  for tool in kenn-ts kenn-swift; do
    if ! env -i PATH= HOME="$HOME" "./build/$tool" index --workspace "$ws" >"tmp/probe-$tool.out" 2>/dev/null; then
      echo "  ! $tool index crashed with no toolchain (want a clean degrade)"; fail=1
    elif ! head -1 "tmp/probe-$tool.out" | grep -q '"type":"meta"\|"type": *"meta"'; then
      echo "  ! $tool index emitted no meta frame"; fail=1
    else
      echo "  ok $tool index degrades cleanly"
    fi
  done
  exit $fail

# Build one indexer image locally (tagged :local), e.g. `just build-image kenn-go`.
# Images are named by language (kenn-<lang>), matching the published names.
build-image name="default":
  #!/usr/bin/env bash
  set -euo pipefail
  # One bake graph for every image. The provisioning entrypoint is a shared
  # target, so it compiles ONCE per platform instead of once per image — six
  # separate `docker build` runs cannot dedupe that.
  # `default` builds all six; pass a target name for one.
  case "{{name}}" in
    default|csharp|typescript|go|rust|python|swift)
      docker buildx bake -f docker/bake.hcl --load "{{name}}" ;;
    *)
      # Never tag an arbitrary name into the kennworx namespace: a catch-all
      # would stamp org identity on any stray target, producing an image
      # indistinguishable from a published one and one `docker push` away
      # from actually being published as ours.
      echo "build-image: refusing unknown target" >&2
      echo "  expected: default (all), or one of csharp typescript go rust python swift" >&2
      exit 1 ;;
  esac

# End-to-end smoke for the docker indexer runtime. Skips cleanly without docker.
docker-index-smoke: build-cli
  #!/usr/bin/env bash
  set -euo pipefail
  if ! docker info >/dev/null 2>&1; then echo "  - docker unavailable; skipping"; exit 0; fi
  docker build -q -t kenn-ra-smoke:local docker/kenn-rust >/dev/null
  # Under the repo's ./tmp (below /Users, which Docker Desktop shares) — the mac
  # default $TMPDIR (/var/folders) is NOT shared, so the same-path mount is empty
  # there. Being nested in kenn's cargo workspace also exercises the nesting fix:
  # only the crate is mounted, so no ancestor Cargo.toml reaches the container.
  mkdir -p tmp
  root=$(mktemp -d "$PWD/tmp/kenn-docker-smoke.XXXXXX")
  trap 'rm -rf "$root"' EXIT
  ws="$root/repo"
  mkdir -p "$ws/src"
  printf '[package]\nname = "smoke"\nversion = "0.0.0"\nedition = "2021"\n' > "$ws/Cargo.toml"
  printf 'fn helper(x: i32) -> i32 { x + 1 }\nfn main() { println!("{}", helper(41)); }\n' > "$ws/src/main.rs"
  # init creates the .kenn store; then pin rust to the docker runtime + image.
  ./build/kenn init -w "$ws" >/dev/null
  printf '[language.rust]\nenabled = true\nruntime = "docker"\nimage = "kenn-ra-smoke:local"\n' > "$ws/kenn.toml"
  ./build/kenn index -w "$ws" --force
  scip=$(find "$ws/.kenn" -name rust.scip -size +0c | head -1)
  [ -n "$scip" ] || { echo "  ! no non-empty rust.scip — rust did not index in docker"; exit 1; }
  owner=$(stat -f '%Su' "$scip" 2>/dev/null || stat -c '%U' "$scip")
  [ "$owner" = "$(id -un)" ] || { echo "  ! rust.scip owned by $owner, not $(id -un) — --user/chown broken"; exit 1; }
  echo "  ok docker-index-smoke: rust indexed in a container, output host-owned"
  # ── python: the image must actually INDEX a project, not merely answer
  # `--help`. scip-python 0.6.6 shells out to `pip list` (env eval) and `git`
  # (version) at index time; the Dockerfile bundles both. This is the assertion
  # that would have caught the "python3 but no pip" regression.
  docker build -q -t kenn-scippy-smoke:local docker/kenn-python >/dev/null
  pyws="$root/pyrepo"
  mkdir -p "$pyws"
  printf '[project]\nname = "smoke_py"\nversion = "0.0.0"\n' > "$pyws/pyproject.toml"
  printf 'def add(a, b):\n    return a + b\n\n\ndef double(x):\n    return add(x, x)\n' > "$pyws/mod.py"
  ./build/kenn init -w "$pyws" >/dev/null
  printf '[language.python]\nenabled = true\nruntime = "docker"\nimage = "kenn-scippy-smoke:local"\nproject_name = "smoke_py"\nproject_version = "0.0.0"\n' > "$pyws/kenn.toml"
  pyout=$(./build/kenn index -w "$pyws" --force 2>&1); echo "$pyout"
  echo "$pyout" | grep -Eq 'python.*(failed|indexed 0 files)' && { echo "  ! python did not index in docker (pip/git missing from the image?)"; exit 1; }
  pyscip=$(find "$pyws/.kenn" -name 'python*.scip' -size +0c | head -1)
  [ -n "$pyscip" ] || { echo "  ! no non-empty python scip — python did not index in docker"; exit 1; }
  echo "  ok docker-index-smoke: python indexed in a container (pip + git present)"

# Run the kenn-embed integration test against real EmbeddingGemma weights (macOS only). First run downloads ~300MB GGUF.
embed-smoke:
  @cargo test -p kenn-embed --test llama_integration -- --ignored --nocapture

# Clippy the whole workspace with the pedantic lints (see CLAUDE.md).
clippy:
  @cargo clippy --workspace --all-targets

# Generate a cargo-crap CRAP report (cyclomatic complexity x test coverage).
crap crate="":
  #!/usr/bin/env bash
  set -euo pipefail
  mkdir -p tmp
  # Homebrew's rustc ships no llvm-tools component; if a standalone
  # llvm-profdata is on PATH, point cargo-llvm-cov at it.
  if command -v llvm-profdata >/dev/null 2>&1; then
    export LLVM_COV="$(command -v llvm-cov)"
    export LLVM_PROFDATA="$(command -v llvm-profdata)"
  fi
  # Excludes: examples/benches/tests are runnable scaffolding, not
  # production code — keep them out of the gate.
  EXCLUDES=(--exclude '**/examples/**' --exclude '**/benches/**' --exclude '**/tests/**')
  if [ -n "{{ crate }}" ]; then
    cargo llvm-cov --package "{{ crate }}" --lcov --output-path tmp/lcov.info
    cargo crap --path "crates/{{ crate }}" --lcov tmp/lcov.info "${EXCLUDES[@]}"
  else
    cargo llvm-cov --workspace --lcov --output-path tmp/lcov.info
    cargo crap --workspace --lcov tmp/lcov.info "${EXCLUDES[@]}"
  fi

# CI gate: workspace coverage + CRAP against the committed baseline.
crap-ci:
  #!/usr/bin/env bash
  set -euo pipefail
  mkdir -p tmp
  if command -v llvm-profdata >/dev/null 2>&1; then
    export LLVM_COV="$(command -v llvm-cov)"
    export LLVM_PROFDATA="$(command -v llvm-profdata)"
  fi
  # MUST stay in sync with `threshold` in `.cargo-crap.toml`. We
  # hardcode rather than parse the TOML to avoid yet another dep
  # (`tomlq`) on the CI host. The gate verifies the two match below.
  THRESHOLD=30
  CONFIG_THRESHOLD=$(grep -E '^threshold[[:space:]]*=' .cargo-crap.toml | sed -E 's/[^0-9]+//g')
  if [ "$THRESHOLD" != "$CONFIG_THRESHOLD" ]; then
    echo "FATAL: gate THRESHOLD=$THRESHOLD differs from .cargo-crap.toml threshold=$CONFIG_THRESHOLD"
    echo "Update both or the gate will silently disagree with the report."
    exit 2
  fi
  EXCLUDES=(--exclude '**/examples/**' --exclude '**/benches/**' --exclude '**/tests/**')
  # `RUST_TEST_THREADS=1` — `kenn-store/tests/hybrid_search.rs` shares
  # the process-wide `init_shared_embedder` daemon at 127.0.0.1:41873.
  # Under llvm-cov the embedder is slow enough that parallel tests
  # race on init/release; serializing test threads keeps the suite
  # reliable. `serial_test` covers the same suite under plain
  # `cargo test`, but llvm-cov instrumentation needs the bigger hammer.
  RUST_TEST_THREADS=1 cargo llvm-cov --workspace --lcov --output-path tmp/lcov.info
  cargo crap --workspace --lcov tmp/lcov.info "${EXCLUDES[@]}" \
    --baseline crap-baseline.json --format json --output tmp/crap-delta.json
  # Predicate: any "regressed" entry, OR any "new" entry whose CRAP
  # exceeds the threshold.
  BAD=$(jq --argjson t "$THRESHOLD" \
    '[.entries[] | select(.status == "regressed" or (.status == "new" and .crap > $t))] | length' \
    tmp/crap-delta.json)
  if [ "$BAD" -gt 0 ]; then
    echo "CRAP gate FAILED: $BAD offending entries (regressed or new-over-threshold)"
    jq --argjson t "$THRESHOLD" \
      '.entries | map(select(.status == "regressed" or (.status == "new" and .crap > $t))) | sort_by(-.crap) | .[] | {status, function, file, line, crap, cyclomatic, coverage}' \
      tmp/crap-delta.json
    exit 1
  fi
  echo "CRAP gate PASSED: no regressions, no new over-threshold functions"

# Regenerate the committed CRAP baseline (`crap-baseline.json`).
crap-baseline:
  #!/usr/bin/env bash
  set -euo pipefail
  mkdir -p tmp
  if command -v llvm-profdata >/dev/null 2>&1; then
    export LLVM_COV="$(command -v llvm-cov)"
    export LLVM_PROFDATA="$(command -v llvm-profdata)"
  fi
  THRESHOLD=30
  EXCLUDES=(--exclude '**/examples/**' --exclude '**/benches/**' --exclude '**/tests/**')
  # Match `crap-ci`: see note there for why test threads are serialized.
  RUST_TEST_THREADS=1 cargo llvm-cov --workspace --lcov --output-path tmp/lcov.info
  cargo crap --workspace --lcov tmp/lcov.info "${EXCLUDES[@]}" \
    --format json --output tmp/crap-full.json
  WS_ROOT="$(pwd)"
  jq --argjson t "$THRESHOLD" --arg ws "$WS_ROOT/" \
    '.entries |= (map(select(.crap > $t)) | map(.file |= sub("^" + $ws; "")))' \
    tmp/crap-full.json > crap-baseline.json
  KEPT=$(jq '.entries | length' crap-baseline.json)
  echo "Wrote crap-baseline.json with $KEPT over-threshold entries"

# List Rust source files larger than 30k.
large-files:
  #!/usr/bin/env bash
  set -euo pipefail
  found=$(find crates -type f -name '*.rs' -size +30k)
  if [ -z "$found" ]; then
    echo "(no hand-written source file over 30k)"
  else
    # `wc -lc` → lines + bytes; sort by size (bytes), print both.
    echo "$found" | xargs wc -lc | grep -v ' total$' | sort -rnk2 \
      | awk '{printf "  %6s lines  %7.1f KB  %s\n",$1,$2/1024,$3}'
  fi

# Clean ./tmp directory.
tmp:
  @rm tmp/*
  @mkdir -p tmp

# Remove all build artifacts: ./tmp/, ./build/, cargo target, dotnet bin/obj.
clean:
  @rm tmp/* build/*
  @mkdir -p tmp build
  @rm -rf indexers/kenn-dotnet*/bin indexers/kenn-dotnet*/obj
