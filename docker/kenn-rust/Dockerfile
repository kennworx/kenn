# syntax=docker/dockerfile:1
# rust-analyzer indexer. See docker/README.md for the shared design.
#
# No Rust toolchain, only `rustup` and `rust-analyzer`. `rust-toolchain.toml`
# names the toolchain and the entrypoint drives rustup to install it.
#
# rustup rather than fetching tarballs ourselves: a Rust toolchain is four
# component bundles that rustup merges into a sysroot per the channel manifest,
# and it verifies each against the same signed manifest we would be reading.
# `--profile minimal` plus rust-src is what rust-analyzer needs — cargo for
# `cargo metadata`, rustc's sysroot, and std sources.

# The standalone binary, not `apt`/`apk`'s: distro packages depend on rust-src
# and drag the whole toolchain back into the image. Upstream publishes gnu only.
FROM ubuntu:noble AS analyzer
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
 && case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu ;; \
      arm64) triple=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && curl -fsSL -o /tmp/ra.gz \
      "https://github.com/rust-lang/rust-analyzer/releases/latest/download/rust-analyzer-${triple}.gz" \
 && gunzip -c /tmp/ra.gz > /usr/local/bin/rust-analyzer \
 && chmod +x /usr/local/bin/rust-analyzer

# gcc and libc6-dev are payload: build scripts and proc-macro expansion link
# native code. `git` is payload: cargo fetches VCS dependencies.
FROM ubuntu:noble
RUN apt-get update \
    && apt-get install -y --no-install-recommends git gcc libc6-dev ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --default-toolchain none \
    && cp /root/.cargo/bin/rustup /usr/local/bin/rustup && rm -rf /root/.cargo /root/.rustup
COPY --from=entrypoint /kenn-toolchain /usr/local/bin/kenn-toolchain
COPY --from=analyzer /usr/local/bin/rust-analyzer /usr/local/bin/rust-analyzer
WORKDIR /

ENTRYPOINT ["kenn-toolchain", "rust", "--"]
