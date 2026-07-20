import IndexStore

/// Project an IndexStore `SymbolKind` onto the kenn wire `SymbolKind` string
/// (the set the consumer's `kind_from_str` accepts). Returns `nil` for kinds
/// kenn does not model as nodes (parameters, the `extension` symbol itself,
/// `using`/`concept`/comment tags) — design D4.
///
/// - `protocol` → `interface`, actors are reported as `class`, `subscript` is
///   an `instanceMethod` (→ `method`), `associatedtype` is a `typealias`
///   (→ `type`), operators are `function`s.
func wireKind(_ kind: SymbolKind) -> String? {
    switch kind {
    case .class: return "class"
    case .struct: return "struct"
    case .enum: return "enum"
    case .protocol: return "interface"
    case .instanceMethod, .classMethod, .staticMethod: return "method"
    case .function, .conversionFunction, .macro: return "function"
    case .constructor: return "constructor"
    case .destructor: return "destructor"
    case .instanceProperty, .classProperty, .staticProperty: return "property"
    case .enumConstant: return "enum_member"
    case .typealias: return "type"
    case .field: return "field"
    case .variable: return "symbol"
    case .module, .namespace: return "module"
    default:
        // parameter, extension, union, namespaceAlias, using, concept,
        // commentTag, unknown — not emitted as graph nodes.
        return nil
    }
}

/// True for the wire kind strings that denote a nominal type (extension hosts).
func isTypeKindStr(_ kind: String) -> Bool {
    switch kind {
    case "class", "struct", "enum", "interface", "type":
        return true
    default:
        return false
    }
}

/// True for synthesized/expansion names kenn does not model as nodes: accessors
/// referenced by name (`getter:x`/`setter:x`) and macro-expansion symbols (raw
/// mangled `$s…`, e.g. SwiftUI `#Preview`). Used to filter both definitions and
/// edge endpoints so neither leaks as a node or stub.
func isNoiseName(_ name: String) -> Bool {
    name.hasPrefix("getter:") || name.hasPrefix("setter:") || name.hasPrefix("$s")
}

/// True for accessor subkinds (get/set/willSet/didSet/read/modify/…). kenn does
/// not model accessors as independent nodes — they are suppressed and their
/// references fold away (mirrors kenn-dotnet's synthesized-accessor handling).
func isAccessor(_ subkind: SymbolSubkind) -> Bool {
    switch subkind {
    case .accessorGetter, .accessorSetter, .swiftAccessorWillSet, .swiftAccessorDidSet,
        .swiftAccessorAddressor, .swiftAccessorMutableAddressor, .swiftAccessorRead,
        .swiftAccessorModify, .swiftAccessorInit, .swiftAccessorBorrow, .swiftAccessorMutate:
        return true
    default:
        return false
    }
}

/// True for kinds whose label-bearing Swift name (`save(x:)`) should also be
/// surfaced as the `sig` field — callables where the argument labels are the
/// signature's readable part.
func isCallable(_ kind: SymbolKind) -> Bool {
    switch kind {
    case .instanceMethod, .classMethod, .staticMethod, .function, .conversionFunction,
        .constructor, .destructor:
        return true
    default:
        return false
    }
}
