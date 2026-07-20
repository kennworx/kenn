using System.Collections.Immutable;
using System.Text;
using Microsoft.CodeAnalysis;

namespace Kenn.Dotnet.Indexing;

/// <summary>
/// Builds the wire `key` — language-naked, intra-package, native C#
/// source notation. Consumer assembles `pub_id` as `lang:key` from
/// `MetaFrame.language`; the package lives on `SymbolFrame.pkg`, never
/// embedded here.
///
/// Examples:
///   namespace : `Models.Order`
///   type      : `Models.List&lt;T&gt;`
///   method    : `Models.Order#Save&lt;T&gt;(int, ref T)`
///   member    : `Models.Order#Name`
///
/// Parameter types: special-type names (`int`/`string`), otherwise
/// fully-qualified; ref kinds prefix `ref `/`out `/`in `. C# 12's
/// `ref readonly` is unhandled (Roslyn 4.7).
/// </summary>
internal static class PubId
{
    private static readonly SymbolDisplayFormat ParamTypeFormat = new(
        globalNamespaceStyle: SymbolDisplayGlobalNamespaceStyle.Omitted,
        typeQualificationStyle: SymbolDisplayTypeQualificationStyle.NameAndContainingTypesAndNamespaces,
        genericsOptions: SymbolDisplayGenericsOptions.IncludeTypeParameters,
        miscellaneousOptions: SymbolDisplayMiscellaneousOptions.UseSpecialTypes);

    public static string? ForNamespace(StringBuilder buf, INamespaceSymbol ns)
    {
        if (ns.IsGlobalNamespace) return null;
        buf.Clear();
        AppendNamespacePath(buf, ns);
        return buf.ToString();
    }

    public static string ForType(StringBuilder buf, INamedTypeSymbol type)
    {
        buf.Clear();
        AppendTypePath(buf, type);
        return buf.ToString();
    }

    public static string ForMember(StringBuilder buf, ISymbol member)
    {
        buf.Clear();
        var container = member.ContainingType;
        if (container is not null)
        {
            AppendTypePath(buf, container);
        }
        else if (member.ContainingNamespace is { IsGlobalNamespace: false } ns)
        {
            AppendNamespacePath(buf, ns);
        }
        buf.Append('#').Append(member.Name);
        if (member is IMethodSymbol method)
        {
            AppendTypeParams(buf, method.TypeParameters);
            buf.Append('(');
            AppendParameterList(buf, method);
            buf.Append(')');
        }
        return buf.ToString();
    }

    private static void AppendTypePath(StringBuilder buf, INamedTypeSymbol type)
    {
        if (type.ContainingType is { } parent)
        {
            AppendTypePath(buf, parent);
            buf.Append('.');
        }
        else if (type.ContainingNamespace is { IsGlobalNamespace: false } ns)
        {
            AppendNamespacePath(buf, ns);
            buf.Append('.');
        }
        buf.Append(type.Name);
        AppendTypeParams(buf, type.TypeParameters);
    }

    /// <summary>
    /// Render a generic-parameter list using the type parameters' source
    /// names: `&lt;T&gt;`, `&lt;TKey, TValue&gt;`. Empty list emits nothing.
    /// Used both for type definitions and for method generic signatures
    /// so the wire `key` matches native C# source.
    /// </summary>
    private static void AppendTypeParams(StringBuilder buf, ImmutableArray<ITypeParameterSymbol> typeParams)
    {
        if (typeParams.IsDefaultOrEmpty || typeParams.Length == 0) return;
        buf.Append('<');
        for (var i = 0; i < typeParams.Length; i++)
        {
            if (i > 0) buf.Append(", ");
            buf.Append(typeParams[i].Name);
        }
        buf.Append('>');
    }

    /// <summary>
    /// Walk the namespace chain outermost → innermost, dot-separating, by
    /// recursing on the parent before appending this segment. No
    /// intermediate <see cref="List{T}"/>; writes directly to the buffer.
    /// </summary>
    private static void AppendNamespacePath(StringBuilder buf, INamespaceSymbol ns)
    {
        if (ns.IsGlobalNamespace) return;
        if (ns.ContainingNamespace is { IsGlobalNamespace: false } parent)
        {
            AppendNamespacePath(buf, parent);
            buf.Append('.');
        }
        buf.Append(ns.Name);
    }

    private static void AppendParameterList(StringBuilder buf, IMethodSymbol method)
    {
        if (method.Parameters.IsDefaultOrEmpty || method.Parameters.Length == 0) return;
        var first = true;
        foreach (var p in method.Parameters)
        {
            if (!first) buf.Append(", ");
            first = false;
            switch (p.RefKind)
            {
                case RefKind.Ref: buf.Append("ref "); break;
                case RefKind.Out: buf.Append("out "); break;
                case RefKind.In: buf.Append("in "); break;
            }
            buf.Append(p.Type.ToDisplayString(ParamTypeFormat));
        }
    }
}
