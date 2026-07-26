---
id: fnd_04e5cb3e-b5fb-44dd-80e1-c31df38daa61
tags:
- directive
- polarity:do
- docker
parent_ids: []
created_at: 2026-07-23T16:59:56.880693Z
---
Host-side Swift toolchain provisioning (provision_swift_from_image, crates/kenn-indexer/src/docker.rs) MUST be idempotent AND visible. It runs on EVERY index preflight and its copy script leads with 'rm -rf {dest}', so: (1) PROBE FIRST and skip when already staged — busybox 'test -x {dest}/usr/bin/swiftc' (the tiny helper the module already uses; never pull the multi-GB swift:<tag> image just to check). Without the skip, a warm toolchain is wiped and re-copied (~2 GB) every run, and a version whose base image was evicted re-pays the multi-GB docker pull. (2) ANNOUNCE before the pull and INHERIT the child's stdio ('.status()', not '.output()') so 'docker pull' progress shows. A silent multi-GB provision behind a captured pipe is indistinguishable from a hang. THE TRAP: 'swift docker hangs' is almost always this provisioning latency, NOT the Foundation.Process deadlock (that is build-tooling only and already worked around via posix_spawn ProcessRunner + the KENN_TOOLCHAIN_LIB manifest patch) — it was misdiagnosed as a deadlock twice. When diagnosing, run 'docker run --rm -v kenn-toolchains:/t busybox du -sh /t/*/swift/*' and check whether swift:<ver> is being pulled. DEFERRED optimization: reuse a compatible provisioned toolchain (6.3 satisfies a 6.0 swift-tools-version MINIMUM) to skip the pull entirely — needs coordinated host + kenn-toolchain/src/run.rs best-available>=pin selection.