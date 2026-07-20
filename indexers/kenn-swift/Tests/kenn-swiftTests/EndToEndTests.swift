import XCTest

@testable import kenn_swift

/// End-to-end: build the committed `Fixtures/SampleApp` SwiftPM package, read
/// its index store, and assert the emitted wire carries the expected symbols
/// and edges (calls / implements / overrides, extension members folded onto the
/// extended type). Slow (runs `swift build`) but the real integration contract.
final class EndToEndTests: XCTestCase {
    /// Parsed frames keyed for assertions.
    private struct Indexed {
        let keyById: [Int: String]
        let symbols: [[String: Any]]
        let edges: [(src: String, dst: String, kind: String)]

        func hasSymbol(_ key: String) -> Bool { keyById.values.contains(key) }
        func hasEdge(_ src: String, _ kind: String, _ dst: String) -> Bool {
            edges.contains { $0.src == src && $0.dst == dst && $0.kind == kind }
        }
    }

    private func runIndexer() throws -> Indexed {
        let fixtureDir = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Fixtures/SampleApp").path
        // POSIX realpath — the compiler records `/private/...` realpaths in
        // units; `resolvingSymlinksInPath()` would STRIP `/private` and the
        // workspace prefix check would then drop every unit on checkouts
        // under $TMPDIR//tmp (same fix as main.swift).
        let root = canonicalWorkspaceRoot(fixtureDir)

        guard let storePath = Provisioning.ensureSwiftPMStore(packageDir: root, skipBuild: false).store
        else {
            throw XCTSkip("could not produce index store for fixture (toolchain unavailable)")
        }

        let objects = try runReaderObjects(root: root, storePath: storePath)

        var keyById: [Int: String] = [:]
        var symbols: [[String: Any]] = []
        for obj in objects {
            guard let type = obj["type"] as? String else { continue }
            if type == "symbol" || type == "stub" {
                if let id = obj["id"] as? Int, let key = obj["key"] as? String { keyById[id] = key }
                if type == "symbol" { symbols.append(obj) }
            }
        }
        let edges: [(src: String, dst: String, kind: String)] = objects.compactMap { obj in
            guard obj["type"] as? String == "edge",
                let s = obj["source"] as? Int, let t = obj["target"] as? Int,
                let k = obj["edge_kind"] as? String
            else { return nil }
            return (keyById[s] ?? "?\(s)", keyById[t] ?? "?\(t)", k)
        }
        return Indexed(keyById: keyById, symbols: symbols, edges: edges)
    }

    func testEmitsSymbolsAndSemanticEdges() throws {
        let ix = try runIndexer()

        // Symbols (kinds + extension folding).
        XCTAssertTrue(ix.hasSymbol("SampleApp.Order"), "struct Order")
        XCTAssertTrue(ix.hasSymbol("SampleApp.Persistable"), "protocol → interface")
        XCTAssertTrue(
            ix.hasSymbol("SampleApp.Order.describe()"),
            "extension member folded onto the extended type")

        // Edges (directions verified empirically).
        XCTAssertTrue(
            ix.hasEdge("SampleApp.Cart.checkout()", "calls", "SampleApp.Order.save()"), "calls")
        XCTAssertTrue(
            ix.hasEdge("SampleApp.Order", "implements", "SampleApp.Persistable"), "conformance")
        XCTAssertTrue(
            ix.hasEdge("SampleApp.Derived", "implements", "SampleApp.Base"), "inheritance")
        XCTAssertTrue(
            ix.hasEdge("SampleApp.Derived.run()", "overrides", "SampleApp.Base.run()"), "override")
    }

    func testEmitsBodyExtentFromSwiftSyntax() throws {
        let ix = try runIndexer()
        // `struct Order` spans Order.swift lines 5–10 (1-based); wire is 0-based.
        // libIndexStore alone gives only the name line — SwiftSyntax recovers
        // the whole declaration span.
        let order = ix.symbols.first { $0["key"] as? String == "SampleApp.Order" }
        let body = order?["body"] as? [Int]
        let range = order?["range"] as? [Int]
        XCTAssertEqual(body?.count, 4, "Order carries a 4-int body span")
        XCTAssertEqual(body?[0], 4, "body start (0-based) = `struct Order` on line 5")
        XCTAssertEqual(body?[2], 9, "body end (0-based) = closing brace on line 10")
        XCTAssertEqual(range?[0], range?[2], "name span is single-line")
        XCTAssertNotEqual(body?[0], body?[2], "body span is multi-line, unlike the name span")
    }

    func testProtocolKindAndExtensionParent() throws {
        let ix = try runIndexer()
        let persistable = ix.symbols.first { $0["key"] as? String == "SampleApp.Persistable" }
        XCTAssertEqual(persistable?["kind"] as? String, "interface")

        // `describe` lives in OrderExt.swift but parents to the canonical Order.
        let describe = ix.symbols.first { $0["key"] as? String == "SampleApp.Order.describe()" }
        let order = ix.symbols.first { $0["key"] as? String == "SampleApp.Order" }
        XCTAssertNotNil(describe)
        XCTAssertEqual(describe?["parent"] as? Int, order?["id"] as? Int)
    }
}
