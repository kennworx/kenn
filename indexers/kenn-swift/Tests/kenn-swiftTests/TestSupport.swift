import Foundation

@testable import kenn_swift

/// POSIX realpath of a URL — the compiler records realpaths
/// (`/private/var/...`) in units, so tests building packages under the
/// temp dir must hand the Indexer the same form (see
/// `canonicalWorkspaceRoot`).
func realPath(_ url: URL) -> String {
    canonicalWorkspaceRoot(url.path)
}

/// Write a SwiftPM package under a fresh temp dir. `sources` maps
/// `TargetName/File.swift` to contents; targets are inferred from the
/// first path segment, `deps` names inter-target dependencies, and
/// `manifest` overrides the generated Package.swift (e.g. to test broken
/// manifests). Cleans up after itself if a write throws — the caller's
/// cleanup defer is registered only after this returns.
func makeSwiftPackage(
    name: String, sources: [String: String], deps: [String: [String]] = [:],
    manifest: String? = nil
) throws -> URL {
    let fm = FileManager.default
    let root = fm.temporaryDirectory.appendingPathComponent("kenn-pkg-\(UUID().uuidString)")
    do {
        var targets: [String] = []
        var seen = Set<String>()
        for (rel, contents) in sources {
            let url = root.appendingPathComponent("Sources/" + rel)
            try fm.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try contents.write(to: url, atomically: true, encoding: .utf8)
            let target = String(rel.split(separator: "/")[0])
            if seen.insert(target).inserted { targets.append(target) }
        }
        let targetDecls = targets.map { t in
            let d = (deps[t] ?? []).map { "\"\($0)\"" }.joined(separator: ", ")
            return ".target(name: \"\(t)\", dependencies: [\(d)], path: \"Sources/\(t)\")"
        }.joined(separator: ",\n    ")
        let manifestText =
            manifest
            ?? """
            // swift-tools-version:5.9
            import PackageDescription
            let package = Package(name: "\(name)", targets: [
                \(targetDecls)
            ])
            """
        try manifestText.write(
            to: root.appendingPathComponent("Package.swift"), atomically: true, encoding: .utf8)
        return root
    } catch {
        try? fm.removeItem(at: root)
        throw error
    }
}

/// Drive the `Indexer` over `storePath` and return the parsed JSONL wire
/// objects. Shared by the end-to-end and error-tolerance suites so the
/// Sink/Indexer drive-and-parse sequence lives in one place.
func runReaderObjects(
    root: String, storePath: String, staleness: Indexer.StalenessMode = .off
) throws -> [[String: Any]] {
    let outURL = FileManager.default.temporaryDirectory
        .appendingPathComponent("kenn-reader-\(UUID().uuidString).jsonl")
    FileManager.default.createFile(atPath: outURL.path, contents: nil)
    let handle = try FileHandle(forWritingTo: outURL)
    defer { try? FileManager.default.removeItem(at: outURL) }

    let sink = Sink(output: handle)
    let indexer = Indexer(workspaceRoot: root, sink: sink)
    indexer.collect(storePath: storePath, staleness: staleness)
    indexer.emit()
    sink.flush()
    try? handle.close()

    let text = String(decoding: try Data(contentsOf: outURL), as: UTF8.self)
    return text.split(separator: "\n").compactMap {
        try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: Any]
    }
}

/// Symbol keys present on a parsed wire.
func symbolKeys(_ objects: [[String: Any]]) -> Set<String> {
    Set(
        objects.filter { $0["type"] as? String == "symbol" }
            .compactMap { $0["key"] as? String })
}
