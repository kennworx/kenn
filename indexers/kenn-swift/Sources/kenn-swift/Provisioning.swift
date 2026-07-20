import Foundation

/// A discovered Swift project and the build system that owns it.
enum SwiftProject {
    case swiftpm(manifest: String)  // path to a Package.swift
    case xcode(container: String)  // path to an .xcodeproj or .xcworkspace
}

/// A provisioning failure the caller surfaces on the JSONL wire as an
/// `ErrorFrame{severity: "error"}` — a failed build or a missing store.
/// stderr diagnostics are unchanged; the frame is additive so the consumer
/// can degrade the unit report instead of recording a silent Success.
struct ProvisionError {
    let source: String  // "build" | "store"
    let message: String
    let path: String  // package dir / Xcode container
}

enum Provisioning {
    /// Ensure an index store for a project and return its path, dispatching by
    /// build system. `store` is nil (logged) when no store is available;
    /// `errors` carries wire-bound provisioning failures either way.
    static func ensureStore(
        for project: SwiftProject, skipBuild: Bool, platform: String? = nil, workspaceRoot: String
    ) -> (store: String?, errors: [ProvisionError]) {
        switch project {
        case .swiftpm(let manifest):
            return ensureSwiftPMStore(packageDir: packageDir(from: manifest), skipBuild: skipBuild)
        case .xcode(let container):
            return ensureXcodeStore(
                container: container, skipBuild: skipBuild, platform: platform,
                workspaceRoot: workspaceRoot)
        }
    }

    /// The package directory for a `.swiftpm` entry: the parent of a
    /// `Package.swift` file, or the path itself if it is already a directory
    /// (tolerates `--projects path/to/PkgDir`).
    private static func packageDir(from manifest: String) -> String {
        if (manifest as NSString).lastPathComponent == "Package.swift" {
            return (manifest as NSString).deletingLastPathComponent
        }
        var isDir: ObjCBool = false
        if FileManager.default.fileExists(atPath: manifest, isDirectory: &isDir), isDir.boolValue {
            return manifest
        }
        return (manifest as NSString).deletingLastPathComponent
    }

    // MARK: - SwiftPM

    static func ensureSwiftPMStore(
        packageDir: String, skipBuild: Bool
    ) -> (store: String?, errors: [ProvisionError]) {
        let scratch = swiftScratch(packageDir: packageDir)
        let buildDir = scratch ?? (packageDir + "/.build")
        var errors: [ProvisionError] = []
        if !skipBuild, !runSwiftBuild(packageDir: packageDir, scratch: scratch) {
            logError("`swift build` failed for \(packageDir); reading any existing store")
            errors.append(
                ProvisionError(
                    source: "build",
                    message: "`swift build` failed; reading any existing store",
                    path: packageDir))
        }
        if let store = locateSwiftPMStore(buildDir: buildDir) {
            return (store, errors)
        }
        logError("no index store under \(buildDir); run `swift build` first (or unset skip_build)")
        errors.append(
            ProvisionError(
                source: "store",
                message: "no index store under the build dir; run `swift build` first (or unset skip_build)",
                path: packageDir))
        return (nil, errors)
    }

    /// The SwiftPM `.build` scratch directory, or `nil` to use the package's
    /// default `.build`. Set ONLY in docker (`KENN_SWIFT_SCRATCH` = the mounted
    /// per-worktree build-cache volume, keyed per package) so `.build` lands on
    /// the fast in-VM volume instead of the slow host bind mount. Native returns
    /// `nil`: `swift build --experimental-prepare-for-indexing --scratch-path` is
    /// broken on the macOS toolchain (a `chdir` error that poisons the fallback,
    /// dropping `calls` relations); the Linux container toolchain is unaffected.
    /// Native `.build` already persists in-tree and is excluded from indexing, so
    /// it needs no redirect.
    static func swiftScratch(packageDir: String) -> String? {
        guard let root = ProcessInfo.processInfo.environment["KENN_SWIFT_SCRATCH"],
            !root.isEmpty
        else { return nil }
        let base = (packageDir as NSString).lastPathComponent
        let hash = String(fnv1a64Hex(Data(packageDir.utf8)).prefix(8))
        return root + "/" + base + "-" + hash
    }

    /// SwiftPM writes the store under `<buildDir>/<triple>/<config>/index/store`,
    /// with a `<buildDir>/<config>` symlink. Try the symlinked paths, then triples.
    private static func locateSwiftPMStore(buildDir: String) -> String? {
        let fm = FileManager.default
        var candidates = [buildDir + "/debug/index/store", buildDir + "/release/index/store"]
        if let triples = try? fm.contentsOfDirectory(atPath: buildDir) {
            for triple in triples where triple.contains("-") {
                candidates.append(buildDir + "/\(triple)/debug/index/store")
                candidates.append(buildDir + "/\(triple)/release/index/store")
            }
        }
        return candidates.first { fm.fileExists(atPath: $0) }
    }

    /// Locate `name` on PATH — a portable `which`. `Process` needs a real
    /// executable path and `/usr/bin/env` does not exist on Windows.
    /// Mirrors execvp's skip-and-continue: non-files are rejected
    /// (`isExecutableFile` is true for DIRECTORIES via the search bit, so
    /// a directory named `swift` on PATH must not short-circuit), and the
    /// scan keeps going past unusable candidates. On Windows only `.exe`
    /// is accepted — `Process`/CreateProcessW cannot spawn `.cmd`/`.bat`
    /// shims directly, and the execute-bit probe is meaningless there.
    static func findOnPath(_ name: String) -> String? {
        #if os(Windows)
            let separator: Character = ";"
            let suffixes = ["", ".exe"]
        #else
            let separator: Character = ":"
            let suffixes = [""]
        #endif
        let fm = FileManager.default
        let path = ProcessInfo.processInfo.environment["PATH"] ?? ""
        for dir in path.split(separator: separator) where !dir.isEmpty {
            for suffix in suffixes {
                let candidate = String(dir) + "/" + name + suffix
                var isDir: ObjCBool = false
                guard fm.fileExists(atPath: candidate, isDirectory: &isDir), !isDir.boolValue
                else { continue }
                #if !os(Windows)
                    guard fm.isExecutableFile(atPath: candidate) else { continue }
                #endif
                return candidate
            }
        }
        return nil
    }

    private static func runSwiftBuild(packageDir: String, scratch: String?) -> Bool {
        guard let swift = findOnPath("swift") else {
            logError("`swift` not found on PATH; cannot build \(packageDir)")
            return false
        }
        // --scratch-path only in docker (Linux toolchain); never on macOS, where
        // it breaks --experimental-prepare-for-indexing (see swiftScratch).
        let scratchArgs = scratch.map { ["--scratch-path", $0] } ?? []
        // Prepare-for-indexing (what SourceKit-LSP background indexing uses):
        // error-tolerant — partial swiftmodules keep dependent targets compiling
        // even when a dependency has errors — and skips codegen, so it is fast.
        // `--build-tests` indexes test targets too (symbols tagged test = true).
        let prepare = {
            runProcess(
                swift,
                [
                    "build", "--experimental-prepare-for-indexing", "--build-tests",
                    "--package-path", packageDir,
                ] + scratchArgs)
        }
        let plain = {
            runProcess(
                swift, ["build", "--build-tests", "--package-path", packageDir] + scratchArgs)
        }
        // Build order is platform-dependent. On macOS, prepare-for-indexing yields
        // a COMPLETE index store (including `calls`), so run it first (fast,
        // error-tolerant) and fall back to a plain build only if the flag is
        // rejected. On Linux (swift 6.x — the docker runtime) prepare-for-indexing
        // OMITS call relations (they need codegen, which it skips), so run the
        // plain build first for a complete store, falling back to
        // prepare-for-indexing for error-tolerance — a non-compiling package still
        // yields symbols. See finding fnd_1681c1a2.
        #if os(macOS)
            return prepare() || plain()
        #else
            return plain() || prepare()
        #endif
    }

    // MARK: - Xcode

    /// Build an `.xcodeproj`/`.xcworkspace` with `xcodebuild` to a local
    /// derived-data dir (deterministic store location) and return the store at
    /// `<dd>/Index.noindex/DataStore`. The reader is shared with SwiftPM mode.
    static func ensureXcodeStore(
        container: String, skipBuild: Bool, platform: String? = nil, workspaceRoot: String
    ) -> (store: String?, errors: [ProvisionError]) {
        var errors: [ProvisionError] = []
        let derivedData = derivedDataDir(workspaceRoot: workspaceRoot, container: container)
        if !skipBuild {
            guard let scheme = resolveScheme(container: container) else {
                logError("no scheme found for \(container) (xcodebuild -list)")
                errors.append(
                    ProvisionError(
                        source: "build",
                        message: "no scheme found (xcodebuild -list)",
                        path: container))
                return (nil, errors)
            }
            let destination =
                destinationFor(platform: platform)
                ?? resolveDestination(container: container, scheme: scheme)
            if !runXcodebuild(
                container: container, scheme: scheme, destination: destination,
                derivedData: derivedData)
            {
                logError(
                    "`xcodebuild` failed for \(container) (scheme \(scheme), \(destination)); "
                        + "reading any existing store. If iOS: ensure the simulator runtime is "
                        + "installed (`xcodebuild -downloadPlatform iOS`).")
                errors.append(
                    ProvisionError(
                        source: "build",
                        message: "`xcodebuild` failed (scheme \(scheme), \(destination)); "
                            + "reading any existing store",
                        path: container))
            }
        }
        let store = derivedData + "/Index.noindex/DataStore"
        if FileManager.default.fileExists(atPath: store) {
            return (store, errors)
        }
        logError("no index store at \(store) for \(container)")
        errors.append(
            ProvisionError(
                source: "store",
                message: "no index store at \(store)",
                path: container))
        return (nil, errors)
    }

    private static func containerFlag(_ container: String) -> String {
        container.hasSuffix(".xcworkspace") ? "-workspace" : "-project"
    }

    /// Derived-data dir for an Xcode build, under kenn's local-artifacts area
    /// (`<workspace>/.kenn/local/xcode-dd/<name>-<hash>`) rather than inside the
    /// user's project tree. Keyed by container path so multiple projects don't
    /// collide; reused across runs so `xcodebuild` stays incremental.
    private static func derivedDataDir(workspaceRoot: String, container: String) -> String {
        let base = ((container as NSString).lastPathComponent as NSString).deletingPathExtension
        let hash = String(fnv1a64Hex(Data(container.utf8)).prefix(8))
        return workspaceRoot + "/.kenn/local/xcode-dd/" + base + "-" + hash
    }

    /// Pick a scheme: the one matching the container's base name, else the first
    /// shared scheme from `xcodebuild -list -json`.
    private static func resolveScheme(container: String) -> String? {
        guard
            let data = captureXcodebuild([
                "-list", "-json", "-skipPackageUpdates", containerFlag(container), container,
            ]),
            let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        let info = (obj["project"] as? [String: Any]) ?? (obj["workspace"] as? [String: Any])
        let schemes = info?["schemes"] as? [String] ?? []
        if schemes.isEmpty { return nil }
        let base = ((container as NSString).lastPathComponent as NSString).deletingPathExtension
        return schemes.first { $0 == base } ?? schemes.first
    }

    /// Map an explicit `--platform` value to an `xcodebuild` destination, or nil
    /// to fall back to auto-detection. Useful for multiplatform apps where
    /// auto-detection would pick a different platform than intended.
    private static func destinationFor(platform: String?) -> String? {
        switch platform?.lowercased() {
        case "macos", "macosx", "mac": return "generic/platform=macOS"
        case "ios", "iphonesimulator": return "generic/platform=iOS Simulator"
        case "tvos": return "generic/platform=tvOS Simulator"
        case "watchos": return "generic/platform=watchOS Simulator"
        case "visionos", "xros": return "generic/platform=visionOS Simulator"
        default: return nil
        }
    }

    /// Pick a build destination from the scheme's `SUPPORTED_PLATFORMS`: macOS
    /// if supported (no simulator needed), else the iOS simulator (no signing).
    private static func resolveDestination(container: String, scheme: String) -> String {
        if let data = captureXcodebuild([
            "-showBuildSettings", "-json", "-skipPackageUpdates", containerFlag(container),
            container, "-scheme", scheme,
        ]),
            let arr = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]],
            let settings = arr.first?["buildSettings"] as? [String: String],
            let platforms = settings["SUPPORTED_PLATFORMS"]
        {
            if platforms.contains("macosx") { return "generic/platform=macOS" }
            if platforms.contains("iphonesimulator") { return "generic/platform=iOS Simulator" }
            if platforms.contains("appletvsimulator") { return "generic/platform=tvOS Simulator" }
            if platforms.contains("watchsimulator") { return "generic/platform=watchOS Simulator" }
        }
        return "generic/platform=macOS"
    }

    private static func runXcodebuild(
        container: String, scheme: String, destination: String, derivedData: String
    ) -> Bool {
        runProcess(
            "/usr/bin/xcodebuild",
            [
                "build", containerFlag(container), container, "-scheme", scheme,
                "-destination", destination, "-derivedDataPath", derivedData,
                "-configuration", "Debug", "-skipPackageUpdates",
                "CODE_SIGNING_ALLOWED=NO", "COMPILER_INDEX_STORE_ENABLE=YES",
                // Xcode 15+ sandboxes run-script phases by default, which denies
                // CocoaPods' embed-frameworks `rsync` (a post-compile packaging
                // step we don't need for indexing). Disable it so Pods workspaces
                // build through to a complete index store.
                "ENABLE_USER_SCRIPT_SANDBOXING=NO",
            ])
    }

    // MARK: - Discovery

    /// Discover SwiftPM packages and Xcode projects under `root`. A directory's
    /// `.xcworkspace` wins over a co-located `.xcodeproj` (it aggregates it);
    /// build/VCS/derived dirs and bundle internals are skipped.
    static func discoverProjects(root: String) -> [SwiftProject] {
        var swiftpm: [String] = []
        var xcodeByDir: [String: String] = [:]
        let fm = FileManager.default
        guard let en = fm.enumerator(atPath: root) else { return [] }
        for case let rel as String in en {
            let leaf = (rel as NSString).lastPathComponent
            if leaf == ".build" || leaf == ".git" || leaf == ".kenn" || leaf == "DerivedData" {
                en.skipDescendants()
                continue
            }
            if leaf.hasSuffix(".xcodeproj") || leaf.hasSuffix(".xcworkspace") {
                en.skipDescendants()  // a bundle, not a dir to recurse
                let dir = (rel as NSString).deletingLastPathComponent
                let full = root + "/" + rel
                // .xcworkspace wins; don't let a later .xcodeproj overwrite it.
                if leaf.hasSuffix(".xcworkspace") || xcodeByDir[dir] == nil {
                    xcodeByDir[dir] = full
                }
                continue
            }
            if leaf == "Package.swift" {
                swiftpm.append(root + "/" + rel)
            }
        }
        return swiftpm.map { .swiftpm(manifest: $0) }
            + xcodeByDir.values.sorted().map { .xcode(container: $0) }
    }

    /// Classify an explicit `--projects` path by extension.
    static func classify(_ path: String) -> SwiftProject {
        if path.hasSuffix(".xcodeproj") || path.hasSuffix(".xcworkspace") {
            return .xcode(container: path)
        }
        return .swiftpm(manifest: path)
    }

    // MARK: - Process helpers

    /// Run a subprocess with output → stderr (stdout is the JSONL channel).
    /// Delegates to [`ProcessRunner`], which uses `posix_spawn` on POSIX to
    /// dodge the `Foundation.Process` container deadlock.
    private static func runProcess(_ launchPath: String, _ args: [String]) -> Bool {
        ProcessRunner.run(launchPath, args)
    }

    /// Run `xcodebuild <args>` capturing stdout (for `-list`/`-showBuildSettings`
    /// JSON); stderr is forwarded. Returns nil on non-zero exit.
    private static func captureXcodebuild(_ args: [String]) -> Data? {
        ProcessRunner.capture("/usr/bin/xcodebuild", args)
    }
}
