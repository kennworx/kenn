// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "kenn-swift",
    platforms: [.macOS(.v13)],
    dependencies: [
        // libIndexStore wrapper. Pinned by branch: the package marks IndexStore
        // with unsafe build flags, which SwiftPM forbids depending on by version.
        .package(url: "https://github.com/MobileNativeFoundation/swift-index-store", branch: "main"),
        // The official Swift parser — recovers declaration extents (the whole
        // `func`/`class` span) that libIndexStore's point-based occurrences
        // lack. The version range spans a Swift major so it tracks the
        // toolchain (resolves 603.x for Swift 6.3); prebuilt macro binaries
        // keep the build fast.
        .package(url: "https://github.com/swiftlang/swift-syntax.git", "600.0.0" ..< "700.0.0"),
    ],
    targets: [
        .executableTarget(
            name: "kenn-swift",
            dependencies: [
                .product(name: "IndexStore", package: "swift-index-store"),
                .product(name: "SwiftSyntax", package: "swift-syntax"),
                .product(name: "SwiftParser", package: "swift-syntax"),
            ]
        ),
        .testTarget(
            name: "kenn-swiftTests",
            dependencies: ["kenn-swift"]
        ),
    ]
)
