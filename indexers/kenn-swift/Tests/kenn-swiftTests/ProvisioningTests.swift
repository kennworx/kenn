import XCTest

@testable import kenn_swift

/// Pure-logic coverage for project discovery/classification — the
/// `xcodebuild`-free parts of `Provisioning` (the build/store paths need a real
/// toolchain and are exercised by manual app runs, not the suite).
final class ProvisioningTests: XCTestCase {
    func testClassifyByExtension() {
        guard case .xcode(let proj) = Provisioning.classify("/x/App.xcodeproj") else {
            return XCTFail("`.xcodeproj` should classify as xcode")
        }
        XCTAssertEqual(proj, "/x/App.xcodeproj")

        guard case .xcode = Provisioning.classify("/x/App.xcworkspace") else {
            return XCTFail("`.xcworkspace` should classify as xcode")
        }
        guard case .swiftpm(let manifest) = Provisioning.classify("/x/Package.swift") else {
            return XCTFail("Package.swift should classify as swiftpm")
        }
        XCTAssertEqual(manifest, "/x/Package.swift")
    }

    func testDiscoverFindsBothKindsPrefersWorkspaceSkipsBundlesAndBuild() throws {
        let fm = FileManager.default
        let root = fm.temporaryDirectory.appendingPathComponent("kenn-disc-\(UUID().uuidString)")
        try fm.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: root) }

        func touch(_ rel: String) {
            let url = root.appendingPathComponent(rel)
            try? fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            fm.createFile(atPath: url.path, contents: Data())
        }
        touch("Package.swift")  // top-level SwiftPM package
        touch("App.xcodeproj/project.pbxproj")  // co-located project…
        touch("App.xcworkspace/contents.xcworkspacedata")  // …and workspace (should win)
        touch(".build/checkouts/Dep/Package.swift")  // dependency — must be skipped
        touch(".kenn/local/xcode-dd/App-abc/SourcePackages/X/Package.swift")  // derived — skipped

        let projects = Provisioning.discoverProjects(root: root.path)
        let swiftpm = projects.compactMap { p -> String? in
            if case .swiftpm(let m) = p { return m } else { return nil }
        }
        let xcode = projects.compactMap { p -> String? in
            if case .xcode(let c) = p { return c } else { return nil }
        }

        XCTAssertEqual(swiftpm.count, 1, "only the top-level Package.swift, not the .build/derived ones")
        XCTAssertTrue(swiftpm[0].hasSuffix("/Package.swift"))
        XCTAssertFalse(swiftpm[0].contains("/.build/"))
        XCTAssertEqual(xcode.count, 1, "one container per dir — .xcworkspace wins over .xcodeproj")
        XCTAssertTrue(xcode[0].hasSuffix("App.xcworkspace"))
    }

    /// SwiftPM `.build` is redirected to a per-package scratch dir ONLY in docker
    /// (`KENN_SWIFT_SCRATCH` set); native returns nil → default `.build` (the macOS
    /// toolchain breaks on `--scratch-path` + prepare-for-indexing).
    func testSwiftScratchIsDockerScopedByEnv() {
        let old = ProcessInfo.processInfo.environment["KENN_SWIFT_SCRATCH"]
        defer {
            if let old { setenv("KENN_SWIFT_SCRATCH", old, 1) } else { unsetenv("KENN_SWIFT_SCRATCH") }
        }
        setenv("KENN_SWIFT_SCRATCH", "/kenn-build/swift", 1)
        let docker = Provisioning.swiftScratch(packageDir: "/ws/MyPkg")
        XCTAssertNotNil(docker)
        XCTAssertTrue(
            docker!.hasPrefix("/kenn-build/swift/MyPkg-"),
            "docker: <volume>/<base>-<hash>, got \(docker ?? "nil")")
        unsetenv("KENN_SWIFT_SCRATCH")
        XCTAssertNil(
            Provisioning.swiftScratch(packageDir: "/ws/MyPkg"),
            "native: nil → default .build (no --scratch-path)")
    }

    func testMissingStoreYieldsProvisionError() throws {
        let fm = FileManager.default
        let root = fm.temporaryDirectory.appendingPathComponent("kenn-nostore-\(UUID().uuidString)")
        try fm.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: root) }

        let result = Provisioning.ensureSwiftPMStore(packageDir: root.path, skipBuild: true)
        XCTAssertNil(result.store)
        XCTAssertEqual(result.errors.count, 1)
        XCTAssertEqual(result.errors[0].source, "store")
        XCTAssertEqual(result.errors[0].path, root.path)
    }

    func testFailedBuildYieldsBuildProvisionError() throws {
        // A broken MANIFEST fails manifest evaluation in every build mode —
        // a mere type error in a source file no longer fails the build, since
        // provisioning tries `--experimental-prepare-for-indexing` first,
        // which tolerates compile errors by design.
        let root = try makeSwiftPackage(
            name: "Broken",
            sources: ["Broken/B.swift": "public struct X {}\n"],
            manifest: #"""
                // swift-tools-version:5.9
                import PackageDescription
                let package = Package(name: "Broken", targets: [ THIS IS NOT SWIFT
                """#)
        defer { try? FileManager.default.removeItem(at: root) }

        let result = Provisioning.ensureSwiftPMStore(packageDir: root.path, skipBuild: false)
        XCTAssertTrue(
            result.errors.contains { $0.source == "build" && $0.path == root.path },
            "a failed `swift build` must yield a wire-bound build error")
    }
}
