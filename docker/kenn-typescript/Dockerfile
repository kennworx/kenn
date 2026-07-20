# syntax=docker/dockerfile:1
# kenn-ts (TypeScript) indexer. See docker/README.md for the shared design.
#
# Provisions nothing: kenn-ts is `bun build --compile` output with the runtime
# and the TypeScript compiler inside the binary, so no workspace can pin a
# toolchain for it. The entrypoint still runs, finds no pin, and execs — every
# image has the same shape.

# Pinned to $BUILDPLATFORM and cross-compiled, so a multi-arch build never
# emulates: only the tiny runtime layer differs per arch.
FROM --platform=$BUILDPLATFORM oven/bun:1 AS sidecar
ARG TARGETARCH
WORKDIR /src
COPY indexers /src
WORKDIR /src/kenn-ts
RUN bun install --frozen-lockfile \
    && case "$TARGETARCH" in \
         amd64) target=bun-linux-x64 ;; \
         arm64) target=bun-linux-aarch64 ;; \
         *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
       esac \
    && bun build src/main.ts --compile --target="$target" --outfile /kenn-ts

# `git` is payload: src/indexing/tsconfig.ts spawns `git worktree list`.
FROM ubuntu:noble
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates libstdc++6 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=entrypoint /kenn-toolchain /usr/local/bin/kenn-toolchain
COPY --from=sidecar /kenn-ts /usr/local/bin/kenn-ts
WORKDIR /

ENTRYPOINT ["kenn-toolchain", "typescript", "--"]
