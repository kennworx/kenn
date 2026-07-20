using Microsoft.CodeAnalysis;

namespace Kenn.Dotnet.Indexing;

internal static class SymbolFilter
{
    /// <summary>
    /// Locals are NEVER emitted as symbol records; the global namespace is
    /// also skipped. Same exclusion list any Roslyn-based indexer needs.
    /// </summary>
    public static bool IsLocalSymbol(ISymbol sym)
    {
        if (sym.Kind is SymbolKind.Local
            or SymbolKind.RangeVariable
            or SymbolKind.TypeParameter
            or SymbolKind.Discard)
        {
            return true;
        }
        if (sym is IMethodSymbol { MethodKind: MethodKind.LocalFunction or MethodKind.AnonymousFunction })
        {
            return true;
        }
        // Anonymous types / lambdas have empty names. The global namespace
        // also has an empty name, so exclude that here too.
        if (sym.Name.Length == 0)
        {
            return true;
        }
        return false;
    }

    public static bool IsInSource(ISymbol sym) =>
        sym.Locations.Any(l => l.IsInSource);
}
