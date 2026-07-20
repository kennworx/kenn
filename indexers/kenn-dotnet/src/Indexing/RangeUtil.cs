using Microsoft.CodeAnalysis;

namespace Kenn.Dotnet.Indexing;

internal static class RangeUtil
{
    /// <summary>
    /// 4-int range (start_line, start_col, end_line, end_col), 0-based.
    /// Returned as a value tuple — no per-call array allocation.
    /// </summary>
    public static Range? FromLocation(Location? loc)
    {
        if (loc is null || !loc.IsInSource) return null;
        var span = loc.GetMappedLineSpan();
        return (
            span.StartLinePosition.Line,
            span.StartLinePosition.Character,
            span.EndLinePosition.Line,
            span.EndLinePosition.Character);
    }

    public static Range? FromSyntaxNode(SyntaxNode node) =>
        FromLocation(node.GetLocation());
}
