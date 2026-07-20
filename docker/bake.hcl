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
#   docker buildx bake -f docker/bake.hcl                 # everything, local
#   docker buildx bake -f docker/bake.hcl csharp          # one image
#   docker buildx bake -f docker/bake.hcl --push default  # publish
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

function "platforms" {
  params = []
  result = PLATFORMS == "" ? [] : split(",", PLATFORMS)
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
  cache-from = ["type=gha,scope=entrypoint"]
  cache-to   = ["type=gha,scope=entrypoint,mode=max"]
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
  cache-from = ["type=gha,scope=csharp"]
  cache-to   = ["type=gha,scope=csharp,mode=max"]
}

target "typescript" {
  inherits   = ["_image"]
  dockerfile = "docker/kenn-typescript/Dockerfile"
  contexts   = { entrypoint = "target:entrypoint" }
  tags       = ["${REGISTRY}/kenn-typescript:${TAG}"]
  cache-from = ["type=gha,scope=typescript"]
  cache-to   = ["type=gha,scope=typescript,mode=max"]
}

target "go" {
  inherits   = ["_image"]
  dockerfile = "docker/kenn-go/Dockerfile"
  contexts   = { entrypoint = "target:entrypoint" }
  tags       = ["${REGISTRY}/kenn-go:${TAG}"]
  cache-from = ["type=gha,scope=go"]
  cache-to   = ["type=gha,scope=go,mode=max"]
}

target "rust" {
  inherits   = ["_image"]
  dockerfile = "docker/kenn-rust/Dockerfile"
  contexts   = { entrypoint = "target:entrypoint" }
  tags       = ["${REGISTRY}/kenn-rust:${TAG}"]
  cache-from = ["type=gha,scope=rust"]
  cache-to   = ["type=gha,scope=rust,mode=max"]
}

target "python" {
  inherits   = ["_image"]
  dockerfile = "docker/kenn-python/Dockerfile"
  contexts   = { entrypoint = "target:entrypoint" }
  tags       = ["${REGISTRY}/kenn-python:${TAG}"]
  cache-from = ["type=gha,scope=python"]
  cache-to   = ["type=gha,scope=python,mode=max"]
}

target "swift" {
  inherits   = ["_image"]
  dockerfile = "docker/kenn-swift/Dockerfile"
  contexts   = { entrypoint = "target:entrypoint" }
  tags       = ["${REGISTRY}/kenn-swift:${TAG}"]
  cache-from = ["type=gha,scope=swift"]
  cache-to   = ["type=gha,scope=swift,mode=max"]
}
