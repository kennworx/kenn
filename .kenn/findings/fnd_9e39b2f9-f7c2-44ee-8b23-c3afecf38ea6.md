---
id: fnd_9e39b2f9-f7c2-44ee-8b23-c3afecf38ea6
tags:
- directive
- polarity:do
- swift
- supersedes:fnd_02c529c1-dca7-4fd4-8f35-1404d792e45e
parent_ids:
- fnd_02c529c1-dca7-4fd4-8f35-1404d792e45e
created_at: 2026-07-17T10:38:01.271648Z
---
Swift `.build` caching (task #42) RESOLVED by docker-scoping. `swift build --experimental-prepare-for-indexing --scratch-path <dir>` is broken ONLY on the macOS toolchain (Apple swiftlang-6.3.3): a `chdir` error that poisons the plain-build fallback and drops `calls` relations. The Linux container toolchain (swift:6.3) handles --scratch-path fine — verified a complete store (21 units, 413 records) and the indexer relocating .build onto the mounted volume. So: pass --scratch-path ONLY in docker, gated on the KENN_SWIFT_SCRATCH env var (workflow.rs sets it to the per-worktree build-cache volume via the launch build_cache tuple); native returns nil → default .build (which already persists + is excluded from indexing). swiftScratch + runSwiftBuild in Provisioning.swift. Do NOT pass --scratch-path on the native/macOS path. SEPARATE pre-existing gap (NOT caused by caching): the Linux/docker kenn-swift emits NO `calls` edges (only implements/imports/overrides) with OR without scratch — macOS prepare-for-indexing yields calls, the swift:6.3 Linux one does not. Tracked separately.