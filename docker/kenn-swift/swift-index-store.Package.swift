// swift-tools-version:5.7
//
// Replacement manifest for the swift-index-store dependency, copied over
// upstream's `Package.swift` by docker/kenn-swift/Dockerfile.
//
// WHY: upstream's Linux `#else` branch spawns Foundation.Process ("which
// swiftc") during MANIFEST EVALUATION, which deadlocks in swift:6.x containers
// (the same Foundation.Process hazard kenn-swift avoids at runtime via its
// posix_spawn ProcessRunner). This copy reads the toolchain lib dir from
// KENN_TOOLCHAIN_LIB instead — no subprocess. Kept byte-for-byte identical to
// upstream except that branch.
//
// NOT named `Package.swift` on purpose: kenn's Swift discovery matches that
// exact basename, so a real `Package.swift` here would make kenn treat
// docker/kenn-swift as a package to index. The Dockerfile renames it on COPY.
//
// Pinned alongside SWIFT_INDEX_STORE_REV in the Dockerfile — if you bump that
// revision, re-diff this file against upstream's manifest.

import Foundation
import PackageDescription

// Patched for kenn-swift's container build: upstream spawns Foundation.Process
// here during manifest evaluation (deadlocks in swift:6.x containers). Read the
// toolchain lib dir from the environment instead — kenn-swift's Dockerfile
// exports KENN_TOOLCHAIN_LIB before `swift build`.
let toolchainLibDir = ProcessInfo.processInfo.environment["KENN_TOOLCHAIN_LIB"] ?? "/usr/lib"

let indexLinkerSettings: [LinkerSetting] = [
    .unsafeFlags(["-L\(toolchainLibDir)"]),
    .unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "\(toolchainLibDir)"]),
]

let swiftDemangleLinkerSettings: [LinkerSetting] = [
    .unsafeFlags(["-L\(toolchainLibDir)"]),
    .unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "\(toolchainLibDir)"]),
    .linkedLibrary("swiftDemangle"),
]

let package = Package(
    name: "IndexStore",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .library(name: "IndexStore", targets: ["IndexStore"]),
        .library(name: "CSwiftDemangle", targets: ["CSwiftDemangle"]),
        .library(name: "SwiftDemangle", targets: ["SwiftDemangle"]),
        .executable(name: "indexutil-export", targets: ["indexutil-export"]),
        .executable(name: "unnecessary-testable", targets: ["unnecessary-testable"]),
        .executable(name: "unused-imports", targets: ["unused-imports"]),
        .executable(name: "indexutil-annotate", targets: ["indexutil-annotate"]),
        .executable(name: "tycat", targets: ["tycat"]),
    ],
    targets: [
        .target(name: "CIndexStore"),
        .target(
            name: "IndexStore", dependencies: ["CIndexStore"], linkerSettings: indexLinkerSettings),
        .testTarget(name: "IndexStoreTests", dependencies: ["IndexStore"], exclude: ["BUILD"]),
        .target(
            name: "CSwiftDemangle",
            cxxSettings: [.headerSearchPath("PrivateHeaders/include")],
            linkerSettings: swiftDemangleLinkerSettings
        ),
        .target(name: "SwiftDemangle", dependencies: ["CSwiftDemangle"]),
        .testTarget(name: "SwiftDemangleTests", dependencies: ["SwiftDemangle"], exclude: ["BUILD"]),
        .executableTarget(name: "indexutil-export", dependencies: ["IndexStore"], exclude: ["BUILD"]),
        .executableTarget(
            name: "unnecessary-testable", dependencies: ["IndexStore"], exclude: ["BUILD"]),
        .executableTarget(name: "unused-imports", dependencies: ["IndexStore"], exclude: ["BUILD"]),
        .executableTarget(name: "indexutil-annotate", dependencies: ["IndexStore"], exclude: ["BUILD"]),
        .executableTarget(name: "tycat", dependencies: ["IndexStore"], exclude: ["BUILD"]),
    ],
    cxxLanguageStandard: .cxx17
)
