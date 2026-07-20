import Foundation

// CLI: kenn-swift index --workspace <dir> [--projects <Package.swift>]... [--skip-build]
var argv = Array(CommandLine.arguments.dropFirst())

// Answered before `index` is required, so a caller can probe whether this
// indexer is runnable without handing it a workspace. Printed bare, matching
// `kenn-dotnet --version` and `kenn-ts --version`.
if argv.first == "--version" {
    print(toolVersion)
    exit(0)
}
guard argv.first == "index" else {
    logError(usageText)
    exit(2)
}
argv.removeFirst()

let parsedArgs: IndexArgs
do {
    parsedArgs = try parseIndexArgs(argv, defaultWorkspace: FileManager.default.currentDirectoryPath)
} catch let err as CliError {
    logError(err.message)
    logError(usageText)
    exit(2)
} catch {
    // Top-level code lets an uncaught error trap the process. Any error is a
    // usage error here, not a crash to dump on the caller.
    logError("\(error)")
    logError(usageText)
    exit(2)
}

let workspaceArg = parsedArgs.workspace
let projects = parsedArgs.projects
let skipBuild = parsedArgs.skipBuild
let storeOverride = parsedArgs.storeOverride
let platform = parsedArgs.platform

// Resolve to the POSIX realpath so workspace-prefix checks match the
// compiler's recorded paths (macOS `/tmp`/`/var` → `/private/...`).
let workspaceRoot = canonicalWorkspaceRoot(workspaceArg)

// Explicit `--projects` (Package.swift / .xcodeproj / .xcworkspace) classified by
// extension; otherwise discover both SwiftPM and Xcode projects under the root.
let discovered: [SwiftProject] =
    projects.isEmpty
    ? Provisioning.discoverProjects(root: workspaceRoot)
    : projects.map { Provisioning.classify($0) }

let sink = Sink()
sink.write([
    "type": "meta", "v": 1, "project_root": "file://\(workspaceRoot)",
    "tool": "kenn-swift", "tool_version": toolVersion, "language": "swift", "ts": iso8601Now(),
])

let indexer = Indexer(workspaceRoot: workspaceRoot, sink: sink)
if let store = storeOverride {
    // Read a store produced by any build system (e.g. an Xcode DerivedData
    // `Index.noindex/DataStore`). The reader is build-system agnostic — only
    // discovery/build differ. `--workspace` still scopes which files count.
    // The store is trusted: staleness is reported, never skipped, because
    // source mtimes routinely postdate an externally built store.
    indexer.collect(storePath: store, staleness: .warnOnly)
} else {
    for project in discovered {
        let provisioned = Provisioning.ensureStore(
            for: project, skipBuild: skipBuild, platform: platform, workspaceRoot: workspaceRoot)
        // Provisioning failures (failed build, missing store) go on the wire
        // so the consumer degrades the unit report instead of a silent Success.
        for err in provisioned.errors {
            sink.countError()
            sink.write([
                "type": "error", "severity": "error", "source": err.source,
                "message": err.message, "path": err.path,
            ])
        }
        if let storePath = provisioned.store {
            // Staleness policy per provisioning outcome: a successful
            // in-process build wrote every unit fresh (no check needed); a
            // FAILED build falls back to an old store whose stale units
            // must be dropped; `--skip-build` trusts the store (warn only).
            let staleness: Indexer.StalenessMode =
                skipBuild
                ? .warnOnly
                : provisioned.errors.contains(where: { $0.source == "build" }) ? .skip : .off
            indexer.collect(storePath: storePath, staleness: staleness)
        }
    }
}
indexer.emit()

sink.write([
    "type": "end",
    "stats": [
        "files": sink.fileCount, "symbols": sink.symbolCount,
        "edges": sink.edgeCount, "errors": sink.errorCount,
    ],
    "ts": iso8601Now(),
])
sink.flush()
