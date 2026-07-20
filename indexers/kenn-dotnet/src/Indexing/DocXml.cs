using System.Text;
using System.Xml.Linq;

namespace Kenn.Dotnet.Indexing;

/// <summary>
/// Normalizes a C# XML documentation comment (as returned by Roslyn's
/// <c>GetDocumentationCommentXml()</c>) into plain prose for storage, lexical
/// search, and embedding. Strips the <c>&lt;member&gt;</c> envelope and all doc
/// tags, keeps the human text of prose elements (summary, remarks, param,
/// returns, …), renders inline reference tags (<c>see cref</c>, <c>paramref</c>)
/// as their bare names, decodes XML entities, and collapses whitespace.
///
/// A comment whose only content is <c>&lt;inheritdoc/&gt;</c> (no inline prose)
/// normalizes to <c>null</c> — the inherited doc is not resolved, so the symbol
/// is treated as undocumented. Malformed XML also yields <c>null</c> (raw markup
/// is never leaked downstream).
/// </summary>
internal static class DocXml
{
    public static string? Normalize(string? xml)
    {
        if (string.IsNullOrWhiteSpace(xml))
        {
            return null;
        }

        XElement root;
        try
        {
            root = XElement.Parse(xml);
        }
        catch (System.Xml.XmlException)
        {
            return null; // never leak raw markup downstream
        }

        var sb = new StringBuilder();
        AppendText(root, sb);
        var text = CollapseWhitespace(sb.ToString());
        return text.Length == 0 ? null : text;
    }

    private static void AppendText(XElement element, StringBuilder sb)
    {
        foreach (var node in element.Nodes())
        {
            switch (node)
            {
                case XText t:
                    sb.Append(t.Value);
                    break;
                case XElement e:
                    AppendElement(e, sb);
                    break;
                default:
                    break; // comments / PIs contribute nothing
            }
        }
    }

    private static void AppendElement(XElement e, StringBuilder sb)
    {
        switch (e.Name.LocalName)
        {
            case "inheritdoc":
                break; // inherited prose is not resolved → contributes nothing

            case "see":
            case "seealso":
            case "paramref":
            case "typeparamref":
                // Inline reference. Explicit link text wins; otherwise emit the
                // bare cref/parameter name (never the raw FQN with its prefix),
                // or the keyword for `<see langword="null"/>`-style references.
                if (e.Nodes().Any())
                {
                    AppendText(e, sb);
                }
                else
                {
                    var name = e.Attribute("cref")?.Value is { } cref
                        ? ShortCref(cref)
                        : e.Attribute("name")?.Value ?? e.Attribute("langword")?.Value;
                    if (!string.IsNullOrEmpty(name))
                    {
                        sb.Append(' ').Append(name).Append(' ');
                    }
                }
                break;

            default:
                // Prose/block element (summary, remarks, param, returns, list,
                // item, para, c, code, …): pad so adjacent sections don't fuse,
                // then recurse into its text.
                sb.Append(' ');
                AppendText(e, sb);
                sb.Append(' ');
                break;
        }
    }

    /// <summary>
    /// Reduces a doc cref ("T:System.Decimal", "M:Ns.Type.Method(args)") to its
    /// bare trailing identifier ("Decimal", "Method"). Leaves an already-bare
    /// name unchanged.
    /// </summary>
    private static string ShortCref(string cref)
    {
        var s = cref;
        if (s.Length > 1 && s[1] == ':')
        {
            s = s[2..]; // drop the "T:" / "M:" / "P:" / "!:" prefix
        }
        var paren = s.IndexOf('(');
        if (paren >= 0)
        {
            s = s[..paren]; // drop method argument list
        }
        var dot = s.LastIndexOf('.');
        if (dot >= 0)
        {
            s = s[(dot + 1)..]; // keep the last namespace/type segment
        }
        return s;
    }

    private static string CollapseWhitespace(string s)
    {
        var sb = new StringBuilder(s.Length);
        var prevSpace = false;
        foreach (var c in s)
        {
            if (char.IsWhiteSpace(c))
            {
                if (!prevSpace && sb.Length > 0)
                {
                    sb.Append(' ');
                }
                prevSpace = true;
            }
            else
            {
                sb.Append(c);
                prevSpace = false;
            }
        }
        while (sb.Length > 0 && sb[^1] == ' ')
        {
            sb.Length--;
        }
        return sb.ToString();
    }
}
