using System.Text;
using System.Text.RegularExpressions;

namespace Kenn.Dotnet.Indexing;

/// <summary>
/// Matches workspace-relative paths against a set of glob patterns.
///
/// Glob semantics (a subset of standard `globset`/`fast-glob`):
///   • `*`   — matches any run of characters except `/`
///   • `**`  — matches any run of characters including `/`
///   • `?`   — matches one character
///   • everything else is literal (with regex metacharacters escaped)
///
/// Path matching is case-insensitive (Windows + mixed-case `.Test/` folder
/// conventions rely on this). Empty pattern list matches nothing.
/// </summary>
internal sealed class TestPathMatcher
{
    private readonly Regex? _combined;

    public TestPathMatcher(IReadOnlyList<string> patterns)
    {
        if (patterns.Count == 0)
        {
            _combined = null;
            return;
        }
        var sb = new StringBuilder("^(?:");
        for (int i = 0; i < patterns.Count; i++)
        {
            if (i > 0) sb.Append('|');
            sb.Append(GlobToRegex(patterns[i]));
        }
        sb.Append(")$");
        _combined = new Regex(sb.ToString(), RegexOptions.IgnoreCase | RegexOptions.CultureInvariant | RegexOptions.Compiled);
    }

    public bool IsMatch(string relativePath)
        => _combined is not null && _combined.IsMatch(relativePath);

    private static string GlobToRegex(string glob)
    {
        var sb = new StringBuilder(glob.Length + 8);
        int i = 0;
        while (i < glob.Length)
        {
            char c = glob[i];
            if (c == '*')
            {
                if (i + 1 < glob.Length && glob[i + 1] == '*')
                {
                    sb.Append(".*");
                    i += 2;
                    // Swallow an optional trailing slash so `**/foo` matches `foo` too.
                    if (i < glob.Length && glob[i] == '/') i++;
                    continue;
                }
                sb.Append("[^/]*");
                i++;
                continue;
            }
            if (c == '?')
            {
                sb.Append("[^/]");
                i++;
                continue;
            }
            // Escape any regex metacharacter.
            if ("\\.+()[]{}|^$".IndexOf(c) >= 0)
            {
                sb.Append('\\');
            }
            sb.Append(c);
            i++;
        }
        return sb.ToString();
    }
}
