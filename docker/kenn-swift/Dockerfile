# syntax=docker/dockerfile:1
# kenn-swift (Swift) indexer. See docker/README.md for the shared design, and for
# why Swift's toolchain is provisioned host-side rather than by the entrypoint.
#
# The noble tags are pinned EXPLICITLY, not left to `swift:6.3-slim`, because
# this image's distro is the one the other five follow: the provisioned toolchain
# needs glibc >= its build distro, and swift's vendor image is the only base we
# do not choose. If upstream retags, the drift must be visible here.
#
# gcc AND libc6-dev are what the toolchain owes the LINK step: Scrt1.o comes from
# the latter, crtbeginS.o/crtendS.o from the former. Without gcc, swiftc
# type-checks fine and fails only at link, which reads as a project error.

# Two container-specific hazards, both handled here:
#   1. RUNTIME — kenn-swift spawns `swift build` via posix_spawn (ProcessRunner),
#      NOT Foundation.Process, whose run()+pipe+waitUntilExit path deadlocks in
#      swift:6.x containers.
#   2. BUILD-TIME — swift-index-store's Package.swift spawns Foundation.Process
#      during MANIFEST EVALUATION to locate the toolchain lib dir (same deadlock).
#      We overwrite that manifest with one reading KENN_TOOLCHAIN_LIB instead.
FROM swift:6.3-noble AS sidecar
# Pinned to indexers/kenn-swift/Package.resolved's revision.
ARG SWIFT_INDEX_STORE_REV=7edb9a64e084ed64f83b84fb9269d3d1a20c0687
WORKDIR /src
COPY indexers/kenn-swift /src/kenn-swift
RUN git clone https://github.com/MobileNativeFoundation/swift-index-store /src/swift-index-store \
 && git -C /src/swift-index-store checkout "$SWIFT_INDEX_STORE_REV"
# Renamed on the way in; see that file's header for why it is not Package.swift
# on disk.
COPY docker/kenn-swift/swift-index-store.Package.swift /src/swift-index-store/Package.swift
# Point kenn-swift at the local patched copy, and drop the pinned resolution so
# SPM re-resolves the path dependency cleanly.
RUN sed -i 's#\.package(url: "https://github.com/MobileNativeFoundation/swift-index-store", branch: "main")#.package(path: "../swift-index-store")#' /src/kenn-swift/Package.swift \
 && rm -f /src/kenn-swift/Package.resolved
WORKDIR /src/kenn-swift
RUN export KENN_TOOLCHAIN_LIB="$(dirname "$(dirname "$(readlink -f "$(command -v swiftc)")")")/lib" \
 && swift build -c release \
 && cp .build/release/kenn-swift /usr/local/bin/kenn-swift

# swift:*-slim is the OS plus the Swift RUNTIME and no compiler. The runtime is
# payload, not leftovers: kenn-swift is itself a Swift program and links
# libswiftCore. The compiler arrives from the cache volume, and its own `swiftc`
# resolves resources relative to its path there, so the two do not collide.
FROM swift:6.3-noble-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
         git ca-certificates gcc libc6-dev binutils \
         libncurses6 libxml2 libcurl4 zlib1g libsqlite3-0 libedit2 libpython3.12 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=entrypoint /kenn-toolchain /usr/local/bin/kenn-toolchain
COPY --from=sidecar /usr/local/bin/kenn-swift /usr/local/bin/kenn-swift
# kenn-swift LINKS the toolchain's index-store library, which the slim base does
# not ship. Without it the binary is present and exits 127 — "command not found"
# for a command that is right there, naming neither the library nor the reason.
COPY --from=sidecar /usr/lib/libIndexStore.so* /usr/lib/
WORKDIR /

ENTRYPOINT ["kenn-toolchain", "swift", "--"]
