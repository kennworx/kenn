#!/usr/bin/env bash
# Render the Homebrew formulas for kenn's three sidecar indexers from the
# per-platform archives a release actually published (indexer-formulas D6).
#
# Usage: render-sidecar-formulas.sh <version> <artifact-dir> <out-dir> [sidecar...]
#   <artifact-dir> holds  <sidecar>-<target>.tar.gz  and its  .tar.gz.sha256
#   <out-dir>      receives  kenn-ts.rb / kenn-dotnet.rb / kenn-swift.rb
#   [sidecar...]   optional filter — render only these (default: all three). Each
#                  sidecar's CI job renders just its OWN formula (D5 independence),
#                  so it must not fail on another sidecar's absent checksums.
#
# D6: checksums are READ from the published archives, never recomputed, and
# every one is validated in THIS shell BEFORE a single formula is written — so a
# missing or empty checksum exits non-zero and leaves nothing behind. The bug
# this guards against put the check inside `$(...)`, where `exit 1` kills only
# the subshell: it printed an error, rendered an empty sha256, and exited 0.
set -euo pipefail

version="${1:?version}"
artdir="${2:?artifact dir}"
outdir="${3:?output dir}"
shift 3 || true
repo="kennworx/kenn"
all_sidecars="kenn-ts kenn-dotnet kenn-swift"
sidecars="${*:-$all_sidecars}"
for s in $sidecars; do
  case " $all_sidecars " in *" $s "*) ;; *) echo "render: unknown sidecar '$s'" >&2; exit 2 ;; esac
done

# Which platform targets each sidecar ships for. kenn-swift is macOS-arm64 only
# (D3: its libIndexStore dependency has no Linux story outside the docker image).
targets_for() {
  case "$1" in
    kenn-ts | kenn-dotnet) echo "aarch64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu" ;;
    kenn-swift) echo "aarch64-apple-darwin" ;;
    *) echo "render: unknown sidecar '$1'" >&2; exit 2 ;;
  esac
}

desc_for() {
  case "$1" in
    kenn-ts) echo "kenn's TypeScript/JavaScript indexer" ;;
    kenn-dotnet) echo "kenn's C# indexer (self-contained)" ;;
    kenn-swift) echo "kenn's Swift indexer" ;;
  esac
}

# kenn-ts -> KennTs (Homebrew derives the class name from the file name).
# awk, not `sed \U` — the latter is GNU-only and silently mangles on BSD sed.
class_for() { echo "$1" | awk -F- '{for(i=1;i<=NF;i++) printf "%s%s", toupper(substr($i,1,1)), substr($i,2)}'; }

sha_of() {
  # Print the 64-hex digest from a `.sha256`, or exit the WHOLE script if it is
  # missing/empty/malformed. Called bare (not in `$(...)`), so this exit is real.
  local f="$1" h
  [ -f "$f" ] || { echo "render: missing checksum: $f" >&2; exit 1; }
  h="$(awk 'NR==1 {print $1}' "$f")"
  [[ "$h" =~ ^[0-9a-f]{64}$ ]] || { echo "render: invalid/empty checksum in $f: '$h'" >&2; exit 1; }
  printf '%s' "$h"
}

# ── Phase 1: validate every checksum up front, writing nothing. ──────────────
# `sha_of` exits on the first bad one, so control never reaches Phase 2 with a
# hole. Stash the validated digests keyed by "<sidecar> <target>".
digests=""
for s in $sidecars; do
  for t in $(targets_for "$s"); do
    d="$(sha_of "$artdir/$s-$t.tar.gz.sha256")"
    digests="$digests$s $t $d"$'\n'
  done
done

digest() { awk -v s="$1" -v t="$2" '$1==s && $2==t {print $3; exit}' <<<"$digests"; }

# ── Phase 2: render. All digests are known-good, so this cannot half-write. ──
mkdir -p "$outdir"
emit_platform_block() { # <sidecar> <target>  -> url+sha256 lines
  local s="$1" t="$2"
  printf '      url "https://github.com/%s/releases/download/v%s/%s-%s.tar.gz"\n' "$repo" "$version" "$s" "$t"
  printf '      sha256 "%s"\n' "$(digest "$s" "$t")"
}

for s in $sidecars; do
  {
    printf 'class %s < Formula\n' "$(class_for "$s")"
    printf '  desc "%s"\n' "$(desc_for "$s")"
    printf '  homepage "https://github.com/%s"\n' "$repo"
    printf '  version "%s"\n' "$version"
    printf '  license "MIT OR Apache-2.0"\n\n'

    printf '  on_macos do\n    on_arm do\n'
    emit_platform_block "$s" "aarch64-apple-darwin"
    printf '    end\n  end\n'

    if [ "$s" != "kenn-swift" ]; then
      printf '\n  on_linux do\n    on_arm do\n'
      emit_platform_block "$s" "aarch64-unknown-linux-gnu"
      printf '    end\n    on_intel do\n'
      emit_platform_block "$s" "x86_64-unknown-linux-gnu"
      printf '    end\n  end\n'
    fi

    printf '\n  def install\n    bin.install "%s"\n  end\nend\n' "$s"
  } > "$outdir/$s.rb"
  echo "render: wrote $outdir/$s.rb"
done
