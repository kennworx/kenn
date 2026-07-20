using Kenn.Dotnet.Wire;
using Microsoft.CodeAnalysis;

namespace Kenn.Dotnet.Indexing;

/// <summary>
/// Map a Roslyn <see cref="ISymbol"/> to the wire <see cref="SymKind"/>
/// taxonomy. Defaults to <see cref="SymKind.Symbol"/> for anything we
/// don't classify more specifically.
/// </summary>
internal static class KindMap
{
    public static SymKind For(ISymbol sym) => sym switch
    {
        INamespaceSymbol => SymKind.Namespace,
        INamedTypeSymbol t => t.TypeKind switch
        {
            TypeKind.Class => SymKind.Class,
            TypeKind.Struct => SymKind.Struct,
            TypeKind.Interface => SymKind.Interface,
            TypeKind.Enum => SymKind.Enum,
            TypeKind.Delegate => SymKind.Delegate,
            TypeKind.Module => SymKind.Module,
            _ => SymKind.Type,
        },
        IMethodSymbol m => m.MethodKind switch
        {
            MethodKind.Constructor or MethodKind.StaticConstructor => SymKind.Constructor,
            MethodKind.Destructor => SymKind.Destructor,
            MethodKind.PropertyGet or MethodKind.PropertySet => SymKind.Accessor,
            MethodKind.EventAdd or MethodKind.EventRemove => SymKind.Accessor,
            _ => SymKind.Method,
        },
        IFieldSymbol { ContainingType.TypeKind: TypeKind.Enum } => SymKind.EnumMember,
        IFieldSymbol f => f.IsConst ? SymKind.Const : SymKind.Field,
        IPropertySymbol => SymKind.Property,
        IEventSymbol => SymKind.Event,
        _ => SymKind.Symbol,
    };
}
