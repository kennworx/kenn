import Foundation
import IndexStore

#if canImport(Darwin)
    import Darwin
#elseif canImport(Glibc)
    import Glibc
#endif

/// A definition collected in pass 1.
private struct DefInfo {
    let name: String
    let kindStr: String
    let callable: Bool
    let fileId: UInt32
    /// Absolute path of the file the definition is in — parsed once with
    /// SwiftSyntax to recover the body extent (see `BodyExtents`).
    let filePath: String
    let line: Int
    let parentUsr: String?
    let module: String
    let isTest: Bool
}

/// One collected edge (resolved to ids at emit time).
private struct PendingEdge {
    let srcUsr: String
    let dstUsr: String
    let kind: String
}

/// Walks one or more index stores and emits the kenn JSONL wire on `sink`.
/// Symbols are interned by USR across all stores in the run; a single
/// `MetaFrame`/`EndFrame` bracket the whole run (emitted by the caller).
final class Indexer {
    private let workspaceRoot: String
    private let sink: Sink

    // Interning + collected state (shared across all stores in the run).
    private var fileIds: [String: UInt32] = [:] // abs path -> file id
    private var defs: [String: DefInfo] = [:] // usr -> definition
    private var seenNames: [String: String] = [:] // usr -> symbol name (any sighting)
    private var pendingEdges: [PendingEdge] = []
    private var edgeDedup: Set<String> = []
    // Module → imported-module name pairs (`from\u{1}to`), from `import` module
    // references. Emitted as `imports` edges between synthetic module nodes.
    private var moduleImports: Set<String> = []
    // Defined nominal-type USRs, longest first — used to resolve an extension
    // member's `s:e:…` parent onto the canonical type by prefix match (D3).
    private var typeUsrsByLen: [String] = []
    private var packageIds: [String: UInt32] = [:] // module name -> package id

    // Emit-time id assignment.
    private var symIds: [String: UInt32] = [:] // usr -> wire id (def or stub)
    private var keyCache: [String: String] = [:]
    private var keyVisiting: Set<String> = [] // cycle guard for keyFor recursion
    private var usedKeys: [String: String] = [:] // key -> usr (collision detection)
    // SwiftSyntax body-extent cache: absolute file path -> parsed declaration
    // spans (see `BodyExtents`). Each def-bearing file is parsed once.
    private var extentCache: [String: BodyExtents] = [:]

    init(workspaceRoot: String, sink: Sink) {
        self.workspaceRoot = workspaceRoot
        self.sink = sink
    }

    // MARK: - Pass 1: collect from a store

    /// How the reader treats units older than their main source file.
    enum StalenessMode {
        /// Drop stale units and report them — the store is a fallback read
        /// after an in-process build FAILED, so stale ranges describe code
        /// that no longer exists.
        case skip
        /// Keep stale units but report them — the caller opted out of
        /// building (`--skip-build`, `--store`): the store is trusted, and
        /// mtimes routinely postdate it (fresh checkout of a CI artifact),
        /// so skipping would empty the index.
        case warnOnly
        /// No check — the sidecar just built this store successfully, so
        /// every unit is fresh by construction.
        case off
    }

    func collect(storePath: String, staleness: StalenessMode) {
        let store: IndexStore
        do {
            store = try IndexStore(path: storePath)
        } catch {
            sink.countError()
            sink.write([
                "type": "error", "severity": "error", "source": "kenn-swift",
                "message": "open index store \(storePath): \(error)",
            ])
            return
        }

        var staleness = staleness
        if staleness != .off, !FileManager.default.fileExists(atPath: storePath + "/v5/units") {
            // Unknown store layout: the mtime lookup below would silently
            // treat everything as fresh. Say so instead of pretending the
            // check ran.
            sink.write([
                "type": "error", "severity": "warning", "source": "store",
                "message": "staleness check unavailable — no v5/units directory in store",
                "path": storePath,
            ])
            staleness = .off
        }

        var sourceDates: [String: Date?] = [:]  // mainFile → mtime, memoized
        var missingSkipped = 0
        var mtimeStaleCount = 0
        var keptMtimeStale = false
        var staleExamples: [String] = []
        func noteStale(_ mainFile: String, missing: Bool) {
            if missing { missingSkipped += 1 } else { mtimeStaleCount += 1 }
            if staleExamples.count < 3 { staleExamples.append(mainFile) }
        }

        if staleness == .skip {
            // Single-parse pre-pass: buffer the fields ingest needs so the
            // ingest pass never re-creates UnitReaders (each is a full
            // unit-file parse), and classify freshness for the systematic-
            // skew guard. A fallback store older than a fresh checkout (CI
            // cache restore, archive/rsync/Docker copy) makes every unit
            // "mtime-stale" without a single real edit — dropping them all
            // would empty the index, so when strictly more than half of
            // the STILL-PRESENT sources are mtime-stale, keep them and say
            // so. Deleted-source units are excluded from that denominator
            // and always dropped: deletion is unambiguous.
            var buffered:
                [(mainFile: String, module: String, recordName: String?, freshness: UnitFreshness)] =
                    []
            for unit in store.units {
                guard let mainFile = workspaceMainFile(unit) else { continue }
                let freshness = unitFreshness(
                    storePath: storePath, unitName: unit.name, mainFile: mainFile,
                    sourceDates: &sourceDates)
                buffered.append((mainFile, unit.moduleName, unit.recordName, freshness))
            }
            let mtimeStale = buffered.filter { $0.freshness == .mtimeStale }.count
            let present = buffered.filter { $0.freshness != .missingSource }.count
            keptMtimeStale = mtimeStale * 2 > present
            for entry in buffered {
                switch entry.freshness {
                case .missingSource:
                    noteStale(entry.mainFile, missing: true)
                    continue
                case .mtimeStale:
                    noteStale(entry.mainFile, missing: false)
                    if !keptMtimeStale { continue }
                case .fresh:
                    break
                }
                ingestUnit(
                    store: store, mainFile: entry.mainFile, module: entry.module,
                    recordName: entry.recordName)
            }
        } else {
            for unit in store.units {
                guard let mainFile = workspaceMainFile(unit) else { continue }
                if staleness == .warnOnly {
                    switch unitFreshness(
                        storePath: storePath, unitName: unit.name, mainFile: mainFile,
                        sourceDates: &sourceDates)
                    {
                    case .missingSource:
                        // Deletion is unambiguous even on a trusted read —
                        // emitting the unit would pair an empty-bytes
                        // content_hash with ranges into a gone file.
                        noteStale(mainFile, missing: true)
                        continue
                    case .mtimeStale:
                        noteStale(mainFile, missing: false)  // kept: store trusted
                    case .fresh:
                        break
                    }
                }
                ingestUnit(
                    store: store, mainFile: mainFile, module: unit.moduleName,
                    recordName: unit.recordName)
            }
        }

        if let message = staleWarning(
            mode: staleness, keptMtimeStale: keptMtimeStale, missing: missingSkipped,
            mtime: mtimeStaleCount, examples: staleExamples)
        {
            // Surfaced as a warning frame (not an error): partial coverage,
            // the run is still useful. The consumer records it on the unit
            // report and `kenn status` / `kenn index` show it.
            sink.write([
                "type": "error", "severity": "warning", "source": "store",
                "message": message,
                "path": storePath,
            ])
        }
    }

    /// The unit's workspace-scoped main file, or nil when the unit is not
    /// ours to index (system/module units, out-of-workspace paths). One
    /// filter shared by every pass so staleness ratios and ingest always
    /// reason over the same unit set.
    private func workspaceMainFile(_ unit: UnitReader) -> String? {
        if unit.isSystem || unit.isModule { return nil }
        let mainFile = unit.mainFile
        if mainFile.isEmpty || !isUnderWorkspace(mainFile) { return nil }
        return mainFile
    }

    /// Read one unit's record and fold its occurrences into the collected
    /// state (files, defs, edges, imports).
    private func ingestUnit(store: IndexStore, mainFile: String, module: String, recordName: String?) {
        let fileId = ensureFile(mainFile)
        guard let recordName,
            let record = try? RecordReader(indexStore: store, recordName: recordName)
        else { return }

        record.forEach(occurrence: { [self] occ in
            let sym = occ.symbol
            let usr = sym.usr
            if usr.isEmpty { return }
            // A module reference (`import Foo`) — record `module imports Foo`.
            // Modules have no in-source definition, so they never become
            // symbol defs; capture the import edge and stop.
            if sym.kind == .module {
                if sym.name != module, !sym.name.isEmpty {
                    moduleImports.insert(module + "\u{1}" + sym.name)
                }
                return
            }
            // An extension is not a node. Swift gives extension members a
            // `childOf` parent of `s:e:<type-usr><member-mangling>` and the
            // extension occurrence itself carries no relations, so the
            // extended type is recovered later by prefix-matching the USR
            // against defined types (see `resolveParent`).
            if usr.hasPrefix("s:e:") || sym.kind == .extension { return }
            // Accessors / macro expansions are not nodes: skip their defs and
            // any edges touching them. The subkind check catches accessors
            // with an accessor subkind; `isNoiseName` catches name-only
            // accessors (`getter:`/`setter:`) and `#Preview` macro `$s…` nodes.
            if isAccessor(sym.subkind) || isNoiseName(sym.name) { return }
            seenNames[usr] = sym.name

            var parentUsr: String?
            occ.forEach(relation: { related, roles in
                let rUsr = related.usr
                // Skip relations to noise targets (external `getter:`/`setter:`
                // accessors, macro `$s…` symbols) so they don't become stubs.
                if rUsr.isEmpty || isNoiseName(related.name) { return }
                self.seenNames[rUsr] = related.name
                if roles.contains(.childOf) { parentUsr = rUsr }
                // `calledBy`: occ.symbol is called by `related` → related calls it.
                if roles.contains(.calledBy) { self.addEdge(rUsr, usr, "calls") }
                // `overrideOf`: occ.symbol overrides `related`.
                if roles.contains(.overrideOf) { self.addEdge(usr, rUsr, "overrides") }
                // `baseOf`: occ.symbol is a base/protocol of `related` →
                // `related` conforms to / inherits occ.symbol.
                if roles.contains(.baseOf) { self.addEdge(rUsr, usr, "implements") }
            })

            guard occ.roles.contains(.definition) else { return }
            guard let kindStr = wireKind(sym.kind) else { return }
            if defs[usr] == nil {
                defs[usr] = DefInfo(
                    name: sym.name, kindStr: kindStr, callable: isCallable(sym.kind),
                    fileId: fileId, filePath: mainFile, line: occ.location.line,
                    parentUsr: parentUsr, module: module, isTest: isTestPath(mainFile))
            }
        })
    }

    /// One human-readable line describing what the staleness pass did,
    /// or nil when nothing was stale. Deleted-source and mtime-stale
    /// units are reported separately — they mean different things.
    private func staleWarning(
        mode: StalenessMode, keptMtimeStale: Bool, missing: Int, mtime: Int, examples: [String]
    ) -> String? {
        if missing + mtime == 0 { return nil }
        var parts: [String] = []
        if missing > 0 {
            parts.append("skipped \(missing) unit(s) with deleted sources")
        }
        if mtime > 0 {
            if mode == .warnOnly {
                parts.append(
                    "kept \(mtime) unit(s) older than their sources (store trusted; rebuild to refresh)"
                )
            } else if keptMtimeStale {
                parts.append(
                    "kept \(mtime) unit(s) older than their sources — systematic mtime skew "
                        + "(fresh checkout over an old store?)")
            } else {
                parts.append("skipped \(mtime) stale unit(s) — source newer than index")
            }
        }
        return parts.joined(separator: "; ") + ": " + examples.joined(separator: ", ")
    }

    private enum UnitFreshness {
        case fresh
        /// The unit's main source file no longer exists — the unit
        /// describes deleted code.
        case missingSource
        /// The main source file's mtime is strictly newer than the unit
        /// file's — the unit may describe outdated code (or the source was
        /// merely touched; the caller weighs the systematic case).
        case mtimeStale
    }

    /// SourceKit-LSP's freshness rule, kept-on-equal: a unit is mtime-stale
    /// only when its main source file is STRICTLY newer than the unit file.
    /// Equal mtimes count as fresh on purpose — on coarse-mtime filesystems
    /// a source written and compiled within the same clock tick would
    /// otherwise be dropped right after a successful build. A unit file we
    /// cannot stat is treated as fresh so the reader degrades to today's
    /// behavior rather than dropping everything (the caller warns once when
    /// the whole units directory is absent).
    private func unitFreshness(
        storePath: String, unitName: String, mainFile: String,
        sourceDates: inout [String: Date?]
    ) -> UnitFreshness {
        let srcDate: Date?
        if let cached = sourceDates[mainFile] {
            srcDate = cached
        } else {
            srcDate = modificationDate(mainFile)
            sourceDates[mainFile] = srcDate
        }
        guard let srcDate else { return .missingSource }
        guard let unitDate = modificationDate(storePath + "/v5/units/" + unitName) else {
            return .fresh
        }
        return srcDate > unitDate ? .mtimeStale : .fresh
    }

    /// Bare `stat(2)` mtime on POSIX — `FileManager.attributesOfItem`
    /// builds a full attribute dictionary per call, which adds up over
    /// thousands of units. The mtime field is spelled `st_mtimespec` on
    /// Darwin and `st_mtim` on Glibc; other platforms (Windows) take the
    /// portable Foundation path.
    private func modificationDate(_ path: String) -> Date? {
        #if canImport(Darwin) || canImport(Glibc)
            var st = stat()
            guard stat(path, &st) == 0 else { return nil }
            #if canImport(Darwin)
                let ts = st.st_mtimespec
            #else
                let ts = st.st_mtim
            #endif
            let seconds = TimeInterval(ts.tv_sec)
            let nanos = TimeInterval(ts.tv_nsec) / 1_000_000_000
            return Date(timeIntervalSince1970: seconds + nanos)
        #else
            let attrs = try? FileManager.default.attributesOfItem(atPath: path)
            return attrs?[.modificationDate] as? Date
        #endif
    }

    // MARK: - Pass 2: emit

    /// Emit packages, files, symbols, stubs, and edges. Files were already
    /// emitted lazily in `ensureFile`; packages and symbols/edges are emitted
    /// here once collection across all stores is complete.
    func emit() {
        // Index defined nominal types by USR length (desc) for extension-parent
        // prefix resolution. The USR breaks length ties: `sorted` is not a stable
        // sort and its input is a Dictionary's keys, so comparing on `count`
        // alone left equal-length USRs in per-run order — and `resolveParent`
        // takes the FIRST prefix match, so which type an extension member
        // attached to could change between runs.
        typeUsrsByLen = defs.filter { isTypeKindStr($0.value.kindStr) }
            .keys.sorted { $0.count != $1.count ? $0.count > $1.count : $0 < $1 }

        // Assign a wire id + key to every definition, in USR order.
        //
        // `defs` is a Dictionary and Swift seeds its hasher PER PROCESS, so
        // iterating it visits USRs in a different order every run. `keyFor`
        // gives the unsalted key to whichever USR arrives first and salts the
        // rest, so that order decided identity: two runs of one binary over an
        // unchanged tree produced `ArgumentParser.Contained` at line 414 in one
        // and at line 441 in the other, with the loser carrying a `#<digest>`
        // suffix. Nine atlas documents differed between runs.
        //
        // Sorted by USR rather than by source position on purpose: a USR is a
        // stable identity, so inserting a declaration ABOVE a collision cannot
        // move the unsalted key onto a different symbol. A line-ordered sort
        // would be deterministic per run but still churn ids on unrelated edits.
        for usr in defs.keys.sorted() {
            _ = ensureSymId(usr)
        }
        // Stubs: edge endpoints that were never defined in-workspace.
        for edge in pendingEdges {
            for usr in [edge.srcUsr, edge.dstUsr] where defs[usr] == nil {
                _ = ensureStubId(usr)
            }
        }
        // Edges.
        for edge in pendingEdges {
            guard let s = symIds[edge.srcUsr], let t = symIds[edge.dstUsr], s != t else { continue }
            sink.countEdge()
            sink.write(["type": "edge", "edge_kind": edge.kind, "source": Int(s), "target": Int(t)])
        }
        // `imports` edges between synthetic module nodes. Sorted because
        // `moduleImports` is a Set: iterating it directly allocated module-stub
        // ids in per-run order, so `ArgumentParserUnitTests` and
        // `ArgumentParserExampleTests` swapped ids 5704/5705 between two runs of
        // one binary over an unchanged store. Ids are the wire identity, so every
        // consumer downstream inherited the churn.
        for pair in moduleImports.sorted() {
            let parts = pair.split(separator: "\u{1}", maxSplits: 1)
            guard parts.count == 2 else { continue }
            let from = ensureModuleNode(String(parts[0]))
            let to = ensureModuleNode(String(parts[1]))
            if from == to { continue }
            sink.countEdge()
            sink.write(["type": "edge", "edge_kind": "imports", "source": Int(from), "target": Int(to)])
        }
    }

    /// Intern a synthetic node for a module (modules have no in-source
    /// definition). Emitted as a `module`-kind stub keyed by name (`sw:<name>`),
    /// shared across `imports` edges and deduped by name.
    private func ensureModuleNode(_ name: String) -> UInt32 {
        let synthetic = "swiftmodule:" + name
        if let id = symIds[synthetic] { return id }
        let id = sink.allocId()
        symIds[synthetic] = id
        sink.write(["type": "stub", "id": Int(id), "kind": "module", "name": name, "key": name])
        return id
    }

    // MARK: - Emit helpers

    private func ensureFile(_ absPath: String) -> UInt32 {
        if let id = fileIds[absPath] { return id }
        let id = sink.allocId()
        fileIds[absPath] = id
        let rel = relativePath(absPath)
        let data = (try? Data(contentsOf: URL(fileURLWithPath: absPath))) ?? Data()
        sink.countFile()
        sink.write([
            "type": "file", "id": Int(id), "path": rel,
            "content_hash": fnv1a64Hex(data), "test": isTestPath(absPath),
        ])
        return id
    }

    private func ensurePackage(_ module: String) -> UInt32 {
        if let id = packageIds[module] { return id }
        let id = sink.allocId()
        packageIds[module] = id
        sink.write([
            "type": "package", "id": Int(id), "name": module, "manager": "swiftpm",
        ])
        return id
    }

    /// Emit a full SymbolFrame for a definition (idempotent by USR). Returns
    /// `nil` for a macro-expansion node (a `$s…`-keyed `#Preview` member), which
    /// is not a real source symbol — its edges then drop on the presence guard.
    private func ensureSymId(_ usr: String) -> UInt32? {
        if let id = symIds[usr] { return id }
        guard let def = defs[usr] else { return ensureStubId(usr) }
        let key = keyFor(usr)
        if key.hasPrefix("$s") { return nil }
        let id = sink.allocId()
        symIds[usr] = id // set before parent recursion to break cycles
        let pkgId = ensurePackage(def.module)
        let parentId = resolveParent(def.parentUsr).flatMap { ensureSymId($0) } ?? 0
        let line = max(def.line - 1, 0) // store is 1-based; wire is 0-based
        var frame: [String: Any] = [
            "type": "symbol", "id": Int(id), "pkg": Int(pkgId),
            "key": key, "kind": def.kindStr, "name": def.name,
            "file": Int(def.fileId), "range": [line, 0, line, 0],
            "test": def.isTest,
        ]
        // Full declaration span (attributes → closing brace) recovered with
        // SwiftSyntax, since libIndexStore carries no extent. Omitted when the
        // file won't parse or no declaration name lands on the def line — the
        // consumer then falls back to the single-line name span.
        if let body = bodyExtent(path: def.filePath, nameLine: def.line) {
            frame["body"] = body
        }
        if parentId != 0 { frame["parent"] = Int(parentId) }
        if def.callable { frame["sig"] = def.name } // label-bearing Swift name
        sink.countSymbol()
        sink.write(frame)
        return id
    }

    /// Emit a minimal StubFrame for a referenced-but-undefined symbol. Returns
    /// `nil` for a macro-expansion (`$s…`) node, as `ensureSymId` does.
    private func ensureStubId(_ usr: String) -> UInt32? {
        if let id = symIds[usr] { return id }
        let key = keyFor(usr)
        if key.hasPrefix("$s") { return nil }
        let id = sink.allocId()
        symIds[usr] = id
        let name = seenNames[usr] ?? usr
        sink.write([
            "type": "stub", "id": Int(id), "kind": "symbol", "name": name, "key": key,
        ])
        return id
    }

    /// The 0-based wire body span `[startLine, 0, endLine, 0]` for a definition
    /// whose name is on `nameLine` (1-based) in `path`, or `nil` when the file
    /// can't be read/parsed or no declaration name lands on that line. Parses
    /// each file once (cached).
    private func bodyExtent(path: String, nameLine: Int) -> [Int]? {
        let extents: BodyExtents
        if let cached = extentCache[path] {
            extents = cached
        } else {
            let src = (try? String(contentsOfFile: path, encoding: .utf8)) ?? ""
            extents = BodyExtents(source: src, fileName: path)
            extentCache[path] = extents
        }
        guard let e = extents.extent(nameLine: nameLine) else { return nil }
        return [max(e.start - 1, 0), 0, max(e.end - 1, 0), 0]
    }

    /// Compose a readable key from the parent chain (design D7). Module-rooted
    /// for top-level symbols; overloads with identical composed names are
    /// salted with a short USR digest.
    private func keyFor(_ usr: String) -> String {
        if let k = keyCache[usr] { return k }
        let baseName = defs[usr]?.name ?? seenNames[usr] ?? usr
        // Cycle guard: a parent chain that loops back (possible when an
        // extension parent resolves by prefix onto a descendant) would recurse
        // forever and overflow the stack. Break with the bare name.
        if !keyVisiting.insert(usr).inserted {
            return baseName
        }
        defer { keyVisiting.remove(usr) }
        let prefix: String
        if let resolved = resolveParent(defs[usr]?.parentUsr), resolved != usr {
            prefix = keyFor(resolved)
        } else if let module = defs[usr]?.module {
            prefix = module
        } else {
            prefix = ""
        }
        var key = prefix.isEmpty ? baseName : "\(prefix).\(baseName)"
        // Overload collision: same composed key, different USR → salt.
        if let owner = usedKeys[key], owner != usr {
            key += "#" + String(fnv1a64Hex(Data(usr.utf8)).prefix(6))
        }
        usedKeys[key] = usr
        keyCache[usr] = key
        return key
    }

    /// Resolve an extension member's `s:e:…` parent onto the canonical extended
    /// type: strip the `s:e:` marker and return the longest defined nominal-type
    /// USR that prefixes the remainder. Non-extension parents pass through; an
    /// extension on an undefined (external) type yields the raw USR (→ a stub).
    private func resolveParent(_ raw: String?) -> String? {
        guard let raw else { return nil }
        guard raw.hasPrefix("s:e:") else { return raw }
        let inner = String(raw.dropFirst(4))
        return typeUsrsByLen.first { inner.hasPrefix($0) } ?? raw
    }

    private func addEdge(_ srcUsr: String, _ dstUsr: String, _ kind: String) {
        if srcUsr.isEmpty || dstUsr.isEmpty || srcUsr == dstUsr { return }
        let dedup = "\(srcUsr)\u{1}\(dstUsr)\u{1}\(kind)"
        if edgeDedup.insert(dedup).inserted {
            pendingEdges.append(PendingEdge(srcUsr: srcUsr, dstUsr: dstUsr, kind: kind))
        }
    }

    // MARK: - Path helpers

    private func isUnderWorkspace(_ absPath: String) -> Bool {
        // In the workspace, but NOT inside a dependency tree — dependency sources
        // sit under `.build/checkouts` (SwiftPM), `.kenn/local/xcode-dd/.../
        // SourcePackages` (Xcode), or `Pods/` (CocoaPods), all nested under the
        // workspace root. Excluding `Pods/` keeps dependency handling consistent
        // across package managers (SPM deps are already skipped) — kenn indexes
        // the project, not its dependencies.
        absPath.hasPrefix(workspaceRoot)
            && !absPath.contains("/.build/")
            && !absPath.contains("/.kenn/")
            && !absPath.contains("/DerivedData/")
            && !absPath.contains("/Pods/")
    }

    private func relativePath(_ absPath: String) -> String {
        guard absPath.hasPrefix(workspaceRoot) else { return absPath }
        var rel = String(absPath.dropFirst(workspaceRoot.count))
        if rel.hasPrefix("/") { rel.removeFirst() }
        return rel
    }

    /// SwiftPM test-target convention: sources under a `Tests/` directory.
    private func isTestPath(_ absPath: String) -> Bool {
        relativePath(absPath).split(separator: "/").contains { $0 == "Tests" || $0.hasSuffix("Tests") }
    }
}
