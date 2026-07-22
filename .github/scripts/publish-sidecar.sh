#!/usr/bin/env bash
# Publish one sidecar to the release dist created, then render + push its
# Homebrew formula (indexer-formulas D4/D5/D6). Each sidecar job calls this for
# ITS sidecar only, so a failure here costs one formula, not the others.
#
# Usage: publish-sidecar.sh <sidecar> <version> <tag> <artifact-dir>
#   <artifact-dir> holds this sidecar's <sidecar>-<target>.tar.gz + .tar.gz.sha256
# Env: GH_TOKEN (release read/upload, repo-scoped), HOMEBREW_TAP_TOKEN (tap push).
set -euo pipefail

sidecar="${1:?sidecar}"; version="${2:?version}"; tag="${3:?tag}"; artdir="${4:?artifact dir}"
tap="kennworx/homebrew-tap"

# 1. Wait for the release. dist creates it on the same tag but may still be
#    building — cross-workflow ordering is not expressible, so poll, do not
#    assume (D4). ~40 min ceiling: once Windows joined the matrix the release
#    build runs ~25 min, and the old 20 min ceiling lost the race by ~4 min.
#    Then fail loudly rather than hang.
for i in $(seq 1 120); do
  if gh release view "$tag" >/dev/null 2>&1; then break; fi
  echo "publish($sidecar): waiting for release $tag ($i/120)"; sleep 20
done
gh release view "$tag" >/dev/null || { echo "publish($sidecar): release $tag never appeared" >&2; exit 1; }

# 2. Upload this sidecar's archives + checksums to that release.
gh release upload "$tag" \
  "$artdir/$sidecar"-*.tar.gz "$artdir/$sidecar"-*.tar.gz.sha256 --clobber

# 3. Render this sidecar's formula from the uploaded checksums (D6: read, never
#    recompute; the renderer validates before it writes).
outdir="$(mktemp -d)"
"$(dirname "$0")/render-sidecar-formulas.sh" "$version" "$artdir" "$outdir" "$sidecar"

# 4. Push the formula to the tap. Three jobs may push concurrently to different
#    files in the same repo, so rebase-and-retry on a non-fast-forward.
work="$(mktemp -d)"
git clone --depth 1 "https://x-access-token:${HOMEBREW_TAP_TOKEN}@github.com/${tap}.git" "$work"
cp "$outdir/$sidecar.rb" "$work/Formula/$sidecar.rb"
cd "$work"
git config user.name "kenn-release"
git config user.email "kenn-release@users.noreply.github.com"
git add "Formula/$sidecar.rb"
git commit -m "$sidecar $version" || { echo "publish($sidecar): formula unchanged"; exit 0; }
for i in 1 2 3 4 5; do
  if git push; then echo "publish($sidecar): pushed $sidecar $version"; exit 0; fi
  echo "publish($sidecar): push rejected, rebasing ($i/5)"; git pull --rebase origin HEAD; sleep "$i"
done
echo "publish($sidecar): tap push failed after retries" >&2; exit 1
