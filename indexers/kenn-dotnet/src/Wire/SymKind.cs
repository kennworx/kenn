namespace Kenn.Dotnet.Wire;

/// <summary>
/// Wire <c>SymbolKind</c> taxonomy. Mirrors <c>indexers/frames.ts</c>.
///
/// Named <see cref="SymKind"/> rather than <c>SymbolKind</c> to avoid the
/// collision with <see cref="Microsoft.CodeAnalysis.SymbolKind"/> in files
/// that import both Roslyn and the wire namespace.
///
/// The wire serialization is the lowercase form returned by
/// <see cref="SymKindExtensions.ToWireString"/>; consumers parse the same
/// set of strings.
/// </summary>
public enum SymKind
{
    Namespace,
    Module,
    Class,
    Struct,
    Interface,
    Enum,
    EnumMember,
    Delegate,
    Type,
    Constructor,
    Destructor,
    Method,
    Function,
    Accessor,
    Property,
    Field,
    Const,
    Event,
    Symbol,
}

public static class SymKindExtensions
{
    public static string ToWireString(this SymKind k) => k switch
    {
        SymKind.Namespace => "namespace",
        SymKind.Module => "module",
        SymKind.Class => "class",
        SymKind.Struct => "struct",
        SymKind.Interface => "interface",
        SymKind.Enum => "enum",
        SymKind.EnumMember => "enum_member",
        SymKind.Delegate => "delegate",
        SymKind.Type => "type",
        SymKind.Constructor => "constructor",
        SymKind.Destructor => "destructor",
        SymKind.Method => "method",
        SymKind.Function => "function",
        SymKind.Accessor => "accessor",
        SymKind.Property => "property",
        SymKind.Field => "field",
        SymKind.Const => "const",
        SymKind.Event => "event",
        SymKind.Symbol => "symbol",
        _ => "symbol",
    };
}
