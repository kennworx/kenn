# syntax=docker/dockerfile:1
# The provisioning entrypoint, built ONCE per platform and shared by every
# indexer image through a bake named context.
#
# Previously each of the six Dockerfiles compiled this itself: twelve Rust builds
# per publish (six images x two platforms), half of them under QEMU on an amd64
# runner. Parallel CI matrix jobs cannot dedupe that — they are separate build
# graphs. `docker buildx bake` puts them in ONE graph, so this is built once per
# platform and copied everywhere else.
#
# Bookworm (glibc 2.36) builds a binary that runs on the noble (2.39) runtime
# bases; the reverse does not hold, and there is no official `rust:*-noble`. The
# glibc floor of this stage must stay at or below the LOWEST floor of every image
# that copies the result, so raising it is not a cosmetic change: a too-new build
# lands as a binary that exists and will not exec, naming neither file nor reason.
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# Cache mounts, not COPY layers: the registry and target dirs are the whole cost
# of this stage. Scoped per arch so the two platforms do not share one target dir
# and invalidate each other.
ARG TARGETARCH
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,id=kenn-toolchain-target-${TARGETARCH} \
    cargo build --release -p kenn-toolchain --bin kenn-toolchain \
 && cp /src/target/release/kenn-toolchain /usr/local/bin/kenn-toolchain

# Scratch: this image exists only to be a COPY source. Nothing runs it.
FROM scratch
COPY --from=build /usr/local/bin/kenn-toolchain /kenn-toolchain
