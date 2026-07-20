import XCTest

@testable import kenn_swift

/// swift-prepare-for-indexing: a non-compiling package still yields a full
/// index (prepare-first provisioning), the plain-build fallback engages when
/// the flag is rejected, and stale units are skipped and reported.
final class ErrorToleranceTests: XCTestCase {
    /// The regression behind `realPath`: `resolvingSymlinksInPath()` strips
    /// `/private`, diverging from the compiler's recorded unit paths. The
    /// production normalization must yield the realpath form.
    func testCanonicalWorkspaceRootUsesCompilerPathForm() throws {
        #if os(macOS)
            XCTAssertEqual(
                canonicalWorkspaceRoot("/tmp"), "/private/tmp",
                "must resolve to the realpath form the compiler records")
            XCTAssertEqual(
                URL(fileURLWithPath: "/private/tmp").resolvingSymlinksInPath().path, "/tmp",
                "Foundation strips /private — the reason resolvingSymlinksInPath is unusable here")
        #endif
        // Nonexistent paths pass through untouched.
        XCTAssertEqual(canonicalWorkspaceRoot("/nonexistent/kenn-x"), "/nonexistent/kenn-x")
    }

    /// A type error in a dependency target must not cost index coverage: the
    /// prepare-for-indexing build compiles both targets, and the wire carries
    /// symbols for the broken file and the dependent target alike.
    func testNonCompilingPackageIsFullyIndexed() throws {
        let root = try makeSwiftPackage(
            name: "tol",
            sources: [
                "LibA/A.swift": "public struct A {\n    public init() {}\n"
                    + "    public func value() -> Int { return brokenReference }\n}\n",
                "AppB/B.swift": "import LibA\n\npublic struct B {\n    public init() {}\n"
                    + "    public func use() -> Int { A().value() }\n}\n",
            ],
            deps: ["AppB": ["LibA"]])
        defer { try? FileManager.default.removeItem(at: root) }
        let rootPath = realPath(root)

        let result = Provisioning.ensureSwiftPMStore(packageDir: rootPath, skipBuild: false)
        if result.errors.contains(where: { $0.source == "build" }) {
            throw XCTSkip("toolchain lacks --experimental-prepare-for-indexing")
        }
        let storePath = try XCTUnwrap(result.store)

        let keys = symbolKeys(try runReaderObjects(root: rootPath, storePath: storePath))
        XCTAssertTrue(keys.contains("LibA.A"), "broken file still indexed: \(keys)")
        XCTAssertTrue(keys.contains("LibA.A.value()"), "member of the broken type")
        XCTAssertTrue(keys.contains("AppB.B"), "dependent target not skipped")
        XCTAssertTrue(keys.contains("AppB.B.use()"), "member of the dependent target")
    }

    #if !os(Windows)
    /// When the toolchain rejects `--experimental-prepare-for-indexing`, the
    /// plain `swift build` fallback runs. Uses a fake `swift` on PATH that
    /// fails the prepare invocation and records both calls. POSIX-only:
    /// the shim is a `/bin/sh` script installed via `setenv(PATH)`.
    func testPrepareRejectedFallsBackToPlainBuild() throws {
        let fm = FileManager.default
        let fakeDir = fm.temporaryDirectory.appendingPathComponent(
            "kenn-fake-\(UUID().uuidString)")
        try fm.createDirectory(at: fakeDir, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: fakeDir) }
        let log = fakeDir.appendingPathComponent("calls.log").path
        let script = """
            #!/bin/sh
            printf '%s\\n' "$*" >> "\(log)"
            case "$*" in *--experimental-prepare-for-indexing*) exit 1;; esac
            exit 0
            """
        let fakeSwift = fakeDir.appendingPathComponent("swift")
        try script.write(to: fakeSwift, atomically: true, encoding: .utf8)
        try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: fakeSwift.path)

        let oldPath = ProcessInfo.processInfo.environment["PATH"] ?? "/usr/bin:/bin"
        setenv("PATH", fakeDir.path + ":" + oldPath, 1)
        defer { setenv("PATH", oldPath, 1) }

        let pkgDir = fm.temporaryDirectory.appendingPathComponent(
            "kenn-fb-\(UUID().uuidString)")
        try fm.createDirectory(at: pkgDir, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: pkgDir) }
        let result = Provisioning.ensureSwiftPMStore(packageDir: pkgDir.path, skipBuild: false)

        let calls = String(decoding: try Data(contentsOf: URL(fileURLWithPath: log)), as: UTF8.self)
            .split(separator: "\n").map(String.init)
        #if os(macOS)
            // Prepare-first: the shim rejects prepare (exit 1) → plain fallback.
            XCTAssertEqual(calls.count, 2, "prepare attempt then plain fallback: \(calls)")
            XCTAssertTrue(calls[0].contains("--experimental-prepare-for-indexing"))
            XCTAssertFalse(calls[1].contains("--experimental-prepare-for-indexing"))
        #else
            // Linux plain-first: the shim's plain build exits 0, so prepare is
            // never reached (calls come from a full build, not prepare-for-indexing).
            XCTAssertEqual(calls.count, 1, "plain build first, succeeds: \(calls)")
            XCTAssertFalse(calls[0].contains("--experimental-prepare-for-indexing"))
        #endif
        // The build exited 0 → not reported failed (the missing store is, separately).
        XCTAssertFalse(result.errors.contains { $0.source == "build" })
    }
    #endif

    /// Staleness policy per mode: after a FAILED in-process build (.skip) a
    /// unit older than its source is dropped and reported; on trusted-store
    /// reads (.warnOnly — `--skip-build`/`--store`) it is kept and reported,
    /// so a store older than a fresh checkout never empties the index.
    func testStaleUnitSkippedAfterFailedBuildButKeptWhenTrusted() throws {
        let root = try makeSwiftPackage(
            name: "stale",
            sources: [
                "Lib/Fresh.swift": "public struct FreshType {}\n",
                "Lib/Aging.swift": "public struct AgingType {}\n",
            ])
        defer { try? FileManager.default.removeItem(at: root) }
        let rootPath = realPath(root)

        let result = Provisioning.ensureSwiftPMStore(packageDir: rootPath, skipBuild: false)
        guard let storePath = result.store else {
            throw XCTSkip("could not produce index store (toolchain unavailable)")
        }

        // Make Aging.swift newer than its unit without rebuilding.
        let aging = rootPath + "/Sources/Lib/Aging.swift"
        try FileManager.default.setAttributes(
            [.modificationDate: Date().addingTimeInterval(60)], ofItemAtPath: aging)

        func storeWarning(_ objects: [[String: Any]]) -> String {
            objects.first {
                $0["type"] as? String == "error" && $0["severity"] as? String == "warning"
                    && $0["source"] as? String == "store"
            }?["message"] as? String ?? ""
        }

        // .skip — failed-build fallback: stale unit dropped + reported.
        let skipped = try runReaderObjects(
            root: rootPath, storePath: storePath, staleness: .skip)
        let skippedKeys = symbolKeys(skipped)
        XCTAssertTrue(skippedKeys.contains("Lib.FreshType"), "fresh unit still emitted")
        XCTAssertFalse(skippedKeys.contains("Lib.AgingType"), "stale unit skipped")
        XCTAssertTrue(
            storeWarning(skipped).contains("skipped 1 stale"),
            "skip mode reports the drop: \(storeWarning(skipped))")

        // .warnOnly — trusted store: stale unit KEPT + reported, no blackout.
        let kept = try runReaderObjects(
            root: rootPath, storePath: storePath, staleness: .warnOnly)
        let keptKeys = symbolKeys(kept)
        XCTAssertTrue(keptKeys.contains("Lib.FreshType"))
        XCTAssertTrue(
            keptKeys.contains("Lib.AgingType"),
            "trusted-store read keeps stale units — no empty index on old artifacts")
        XCTAssertTrue(
            storeWarning(kept).contains("older than their sources"),
            "warn-only mode still reports: \(storeWarning(kept))")

        // .off — nothing reported, everything emitted.
        let off = try runReaderObjects(root: rootPath, storePath: storePath, staleness: .off)
        XCTAssertTrue(symbolKeys(off).contains("Lib.AgingType"))
        XCTAssertEqual(storeWarning(off), "", "off mode runs no check")

        // Systematic skew: with EVERY source newer than the store (fresh
        // checkout over an old store / CI cache restore), .skip must not
        // empty the index — the ratio guard keeps mtime-stale units and
        // says so.
        try FileManager.default.setAttributes(
            [.modificationDate: Date().addingTimeInterval(60)],
            ofItemAtPath: rootPath + "/Sources/Lib/Fresh.swift")
        let skew = try runReaderObjects(root: rootPath, storePath: storePath, staleness: .skip)
        let skewKeys = symbolKeys(skew)
        XCTAssertTrue(skewKeys.contains("Lib.FreshType"), "ratio guard keeps units: \(skewKeys)")
        XCTAssertTrue(skewKeys.contains("Lib.AgingType"))
        XCTAssertTrue(
            storeWarning(skew).contains("systematic mtime skew"),
            "downgrade is reported: \(storeWarning(skew))")

        // Deleted source: unambiguous even on a trusted read — the unit is
        // dropped in EVERY checking mode (emitting it would pair an
        // empty-bytes content_hash with ranges into a gone file). Last
        // scenario: destructive.
        try FileManager.default.removeItem(atPath: aging)
        let deleted = try runReaderObjects(
            root: rootPath, storePath: storePath, staleness: .warnOnly)
        let deletedKeys = symbolKeys(deleted)
        XCTAssertFalse(
            deletedKeys.contains("Lib.AgingType"),
            "deleted-source units are skipped on trusted reads too")
        XCTAssertTrue(deletedKeys.contains("Lib.FreshType"))
        XCTAssertTrue(
            storeWarning(deleted).contains("deleted sources"),
            "deletion is reported distinctly: \(storeWarning(deleted))")
    }
}
