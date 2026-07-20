# syntax=docker/dockerfile:1
# scip-go indexer. See docker/README.md for the shared design.
#
# No Go toolchain: `go.mod` names it and the entrypoint provisions it.
# GOTOOLCHAIN stays `local` so the go command cannot second-guess that pin —
# `auto` only ever switches UPWARD, which guarantees "at least the pin" rather
# than "exactly the pin".

# scip-go moved from github.com/sourcegraph/scip-go to github.com/scip-code/scip-go.
#
# Pinned to $BUILDPLATFORM and cross-compiled, so a multi-arch build never
# emulates this stage. Under QEMU it does not merely run slowly: `go install`
# fetches from proxy.golang.org, and emulated crypto is slow enough that the TLS
# handshake TIMES OUT (measured: 50s, then failure) — a network error that is
# really an emulation cost.
#
# The output path is then normalized by LOOKING for it, because `go install`
# moves it: $GOPATH/bin/ when GOARCH matches the host, $GOPATH/bin/$GOOS_$GOARCH/
# when it does not — so either hardcoded path is wrong on one of the two
# platforms. GOBIN would pin it and is rejected outright: "cannot install
# cross-compiled binaries when GOBIN is set".
#
# CGO_ENABLED=0 EXPLICITLY, so both architectures get the same binary shape.
# Left to its default it flips on GOARCH: the cross build disables cgo and comes
# out static, the native build leaves it on and comes out dynamically linked
# against the BUILDER's glibc (measured — amd64 static, arm64 dynamic, from this
# same stage). Two published arches differing in linkage is an arch-specific bug
# waiting to be found by whoever runs the other one.
FROM --platform=$BUILDPLATFORM golang:1-bookworm AS sidecar
ARG TARGETARCH
RUN CGO_ENABLED=0 GOOS=linux GOARCH="$TARGETARCH" \
    go install github.com/scip-code/scip-go/cmd/scip-go@latest \
 && mkdir -p /out \
 && cp "$(find /go/bin -type f -name scip-go | head -1)" /out/scip-go

# gcc and libc6-dev are payload, not build leftovers: scip-go compiles the TARGET
# via go/packages, and a cgo dependency needs a C compiler and headers. The
# image's libc is part of the compilation environment handed to the target.
FROM ubuntu:noble
RUN apt-get update \
    && apt-get install -y --no-install-recommends git gcc libc6-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=entrypoint /kenn-toolchain /usr/local/bin/kenn-toolchain
COPY --from=sidecar /out/scip-go /usr/local/bin/scip-go
ENV GOPATH=/go \
    GOTOOLCHAIN=local
WORKDIR /

ENTRYPOINT ["kenn-toolchain", "go", "--"]
