import Foundation
import SwiftParser
import SwiftSyntax

/// Per-declaration body extents recovered from a Swift source file with the
/// official parser (SwiftSyntax). libIndexStore reports each definition at its
/// name-token location but carries no extent — so `range` collapses to a single
/// line. This maps the 1-based line of a declaration's name token to the whole
/// declaration span (attributes through the closing brace, *excluding* a leading
/// doc comment — the same "declaration node span" the C# producer emits via
/// Roslyn). It is the Swift analog of Roslyn (C#) and the TS compiler API (TS).
struct BodyExtents {
    /// name-token line (1-based) → `(bodyStartLine, bodyEndLine)`, 1-based.
    private let byNameLine: [Int: (Int, Int)]

    init(source: String, fileName: String) {
        let tree = Parser.parse(source: source)
        let collector = Collector(converter: SourceLocationConverter(fileName: fileName, tree: tree))
        collector.walk(tree)
        byNameLine = collector.byNameLine
    }

    /// The declaration extent whose name token is on `nameLine`, if any.
    func extent(nameLine: Int) -> (start: Int, end: Int)? {
        byNameLine[nameLine].map { (start: $0.0, end: $0.1) }
    }
}

/// Walks the syntax tree recording `(nameLine → span)` for every declaration
/// kind kenn-swift emits as a node. Non-declaration nodes and locals are
/// ignored; a def whose line finds no entry keeps its single-line name span.
private final class Collector: SyntaxVisitor {
    let converter: SourceLocationConverter
    var byNameLine: [Int: (Int, Int)] = [:]

    init(converter: SourceLocationConverter) {
        self.converter = converter
        super.init(viewMode: .sourceAccurate)
    }

    /// Record `node`'s span keyed by `name`'s line. `positionAfterSkipping-
    /// LeadingTrivia` starts at the first attribute or the declaration keyword
    /// (dropping the leading doc comment/whitespace); `endPositionBefore-
    /// TrailingTrivia` ends at the closing brace. On a name-line collision
    /// (two names on one line) the smallest — most specific — span wins.
    private func record(name: TokenSyntax, node: some SyntaxProtocol) {
        let nameLine = converter.location(for: name.positionAfterSkippingLeadingTrivia).line
        let start = converter.location(for: node.positionAfterSkippingLeadingTrivia).line
        let end = converter.location(for: node.endPositionBeforeTrailingTrivia).line
        if let existing = byNameLine[nameLine], existing.1 - existing.0 <= end - start {
            return
        }
        byNameLine[nameLine] = (start, end)
    }

    override func visit(_ n: ClassDeclSyntax) -> SyntaxVisitorContinueKind {
        record(name: n.name, node: n)
        return .visitChildren
    }
    override func visit(_ n: StructDeclSyntax) -> SyntaxVisitorContinueKind {
        record(name: n.name, node: n)
        return .visitChildren
    }
    override func visit(_ n: EnumDeclSyntax) -> SyntaxVisitorContinueKind {
        record(name: n.name, node: n)
        return .visitChildren
    }
    override func visit(_ n: ProtocolDeclSyntax) -> SyntaxVisitorContinueKind {
        record(name: n.name, node: n)
        return .visitChildren
    }
    override func visit(_ n: ActorDeclSyntax) -> SyntaxVisitorContinueKind {
        record(name: n.name, node: n)
        return .visitChildren
    }
    override func visit(_ n: FunctionDeclSyntax) -> SyntaxVisitorContinueKind {
        record(name: n.name, node: n)
        return .visitChildren
    }
    override func visit(_ n: TypeAliasDeclSyntax) -> SyntaxVisitorContinueKind {
        record(name: n.name, node: n)
        return .visitChildren
    }
    override func visit(_ n: AssociatedTypeDeclSyntax) -> SyntaxVisitorContinueKind {
        record(name: n.name, node: n)
        return .visitChildren
    }
    override func visit(_ n: InitializerDeclSyntax) -> SyntaxVisitorContinueKind {
        record(name: n.initKeyword, node: n)
        return .visitChildren
    }
    override func visit(_ n: SubscriptDeclSyntax) -> SyntaxVisitorContinueKind {
        record(name: n.subscriptKeyword, node: n)
        return .visitChildren
    }
    override func visit(_ n: VariableDeclSyntax) -> SyntaxVisitorContinueKind {
        for binding in n.bindings {
            if let pattern = binding.pattern.as(IdentifierPatternSyntax.self) {
                record(name: pattern.identifier, node: n)
            }
        }
        return .visitChildren
    }
    override func visit(_ n: EnumCaseDeclSyntax) -> SyntaxVisitorContinueKind {
        for element in n.elements {
            record(name: element.name, node: n)
        }
        return .visitChildren
    }
}
