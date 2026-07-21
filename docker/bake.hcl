# Build every indexer image in ONE build graph.
#
# Why bake rather than six `docker build` calls (or six CI matrix jobs): each
# image needs the same provisioning entrypoint, and separate builds cannot share
# it — six images x two platforms was TWELVE Rust compiles per publish, half of
# them under QEMU on an amd64 runner. Here `entrypoint` is built ONCE per
# platform and referenced by every image through a named context, so the graph
# compiles Rust twice instead of twelve times.
#
# One entrypoint, not two, because every image is noble. That was not always so:
# csharp and typescript were alpine, which meant a musl build alongside the glibc
# one. Six of this change's bugs were libc mismatches — a glibc .NET RID on
# alpine, a musl entrypoint on debian, rust-analyzer shipping gnu-only, Node
# publishing no musl build at all — and each presented as "exists but will not
# exec", naming neither the file nor the reason. Unifying costs about 450MB
# across those two images (measured: csharp 187->407MB, typescript 178->404MB)
# and removes the entire class.
#
# Noble specifically because swift's vendor image is noble-based and that is the
# one base we do not choose. Matching it everywhere else keeps a single glibc
# floor (2.39) instead of a debian/ubuntu split.
#
# Not auto-discovered from the repo root, so it is always passed explicitly:
#
#   docker buildx bake -f docker/bake.hcl --load                 # everything, local (into `docker images`)
#   docker buildx bake -f docker/bake.hcl --load csharp          # one image, local
#   CACHE=gha docker buildx bake -f docker/bake.hcl --push default   # publish (CI)
#
# `context = "."` is relative to the INVOCATION directory, not this file, so
# every command above runs from the repository root.

variable "REGISTRY" {
  default = "ghcr.io/kennworx"
}

variable "TAG" {
  default = "local"
}

variable "PLATFORMS" {
  # Local builds default to the host arch only; CI overrides with both.
  default = ""
}

variable "CACHE" {
  # External build-cache backend. Empty locally: a local build carries no gha
  # token, and `cache-to type=gha` is a HARD error without one, not a skipped
  # warning — so a hardcoded gha cache makes the one local command unrunnable.
  # CI sets `CACHE=gha` to restore the cross-run layer cache.
  default = ""
}

function "platforms" {
  params = []
  result = PLATFORMS == "" ? [] : split(",", PLATFORMS)
}

# gha cache lines only when CI asked for them; empty (no external cache) locally.
function "cache_from" {
  params = [scope]
  result = CACHE == "gha" ? ["type=gha,scope=${scope}"] : []
}

function "cache_to" {
  params = [scope]
  result = CACHE == "gha" ? ["type=gha,scope=${scope},mode=max"] : []
}

group "default" {
  targets = ["csharp", "typescript", "go", "rust", "python", "swift"]
}

# ---------------------------------------------------------------- entrypoints

target "entrypoint" {
  context    = "."
  dockerfile = "docker/kenn-toolchain/Dockerfile"
  platforms  = platforms()
  # Deliberately UNTAGGED. This is a COPY source pulled in through
  # `contexts = { entrypoint = "target:entrypoint" }`, never something a user
  # pulls, and `bake default` builds it as a dependency without pushing it — so
  # a tag here only produces a merge step that fails on an image nobody wrote.
  cache-from = cache_from("entrypoint")
  cache-to   = cache_to("entrypoint")
}

# ---------------------------------------------------------------------- images

target "_image" {
  context   = "."
  platforms = platforms()
}

target "csharp" {
  inherits   = ["_image"]
  dockerfile = "docker/kenn-csharp/Dockerfile"
  contexts   = { entrypoint = "target:entrypoint" }
  tags       = ["${REGISTRY}/kenn-csharp:${TAG}"]
  cache-from = cache_from("csharp")
  cache-to   = cache_to("csharp")
}

target "typescript" {
  inherits   = ["_image"]
  dockerfile = "docker/kenn-typescript/Dockerfile"
  contexts   = { entrypoint = "target:entrypoint" }
  tags       = ["${REGISTRY}/kenn-typescript:${TAG}"]
  cache-from = cache_from("typescript")
  cache-to   = cache_to("typescript")
}

target "go" {
  inherits   = ["_image"]
  dockerfile = "docker/kenn-go/Dockerfile"
  contexts   = { entrypoint = "target:entrypoint" }
  tags       = ["${REGISTRY}/kenn-go:${TAG}"]
  cache-from = cache_from("go")
  cache-to   = cache_to("go")
}

target "rust" {
  inherits   = ["_image"]
  dockerfile = "docker/kenn-rust/Dockerfile"
  contexts   = { entrypoint = "target:entrypoint" }
  tags       = ["${REGISTRY}/kenn-rust:${TAG}"]
  cache-from = cache_from("rust")
  cache-to   = cache_to("rust")
}

target "python" {
  inherits   = ["_image"]
  dockerfile = "docker/kenn-python/Dockerfile"
  contexts   = { entrypoint = "target:entrypoint" }
  tags       = ["${REGISTRY}/kenn-python:${TAG}"]
  cache-from = cache_from("python")
  cache-to   = cache_to("python")
}

target "swift" {
  inherits   = ["_image"]
  dockerfile = "docker/kenn-swift/Dockerfile"
  contexts   = { entrypoint = "target:entrypoint" }
  tags       = ["${REGISTRY}/kenn-swift:${TAG}"]
  cache-from = cache_from("swift")
  cache-to   = cache_to("swift")
}
