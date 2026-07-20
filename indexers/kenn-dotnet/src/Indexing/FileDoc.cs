using System.Collections.Generic;
using System.Linq;
using System.Text;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Text;

namespace Kenn.Dotnet.Indexing;

/// <summary>
/// Extracts a C# file's comment trivia for the file-level doc carried on
/// <c>FileFrame.Doc</c>. C# does not surface plain <c>//</c> / <c>/* */</c>
/// comments through <c>GetDocumentationCommentXml()</c> (and namespaces
/// have no doc XML at all), so the file header and namespace-leading
/// comments are only reachable via syntax trivia.
/// </summary>
internal static class FileDoc
{
    /// <summary>
    /// One entry per contiguous comment <em>block</em>, in source order,
    /// drawn from two slots: the leading trivia of the compilation unit's
    /// first token (the file header), and each namespace declaration's
    /// leading trivia. Consecutive single-line comments are coalesced into
    /// one block (joined by newlines); a blank line between them breaks the
    /// block, and a block comment is its own entry. Coalescing keeps a
    /// multiline <c>//</c> license header as a single entry so the consumer
    /// can drop it whole — only the first line of such a header carries the
    /// copyright/license marker. Entries are deduplicated by span so a
    /// comment directly above a namespace with no preceding usings (first
    /// token == the <c>namespace</c> keyword) is not counted twice. Returns
    /// null when the file has no such comments. No filtering —
    /// license-boilerplate removal is a consumer concern.
    /// </summary>
    public static string[]? Extract(SyntaxTree tree)
    {
        var root = (CompilationUnitSyntax)tree.GetRoot();
        var entries = new List<string>();
        var seen = new HashSet<TextSpan>();

        void Collect(SyntaxTriviaList trivia)
        {
            var block = new StringBuilder();
            var eolRun = 0;

            void FlushBlock()
            {
                if (block.Length > 0)
                {
                    entries.Add(block.ToString());
                    block.Clear();
                }
            }

            foreach (var t in trivia)
            {
                switch (t.Kind())
                {
                    case SyntaxKind.SingleLineCommentTrivia:
                        if (!seen.Add(t.Span))
                        {
                            continue;
                        }
                        // A blank line (>= 2 end-of-lines since the last
                        // comment) ends the block; a single newline keeps it.
                        if (block.Length > 0)
                        {
                            if (eolRun >= 2)
                            {
                                FlushBlock();
                            }
                            else
                            {
                                block.Append('\n');
                            }
                        }
                        block.Append(t.ToString());
                        eolRun = 0;
                        break;

                    case SyntaxKind.MultiLineCommentTrivia:
                        if (!seen.Add(t.Span))
                        {
                            continue;
                        }
                        // A block comment stands on its own.
                        FlushBlock();
                        entries.Add(t.ToString());
                        eolRun = 0;
                        break;

                    case SyntaxKind.EndOfLineTrivia:
                        eolRun++;
                        break;

                        // Whitespace (indentation) is ignored and does not break
                        // a block.
                }
            }

            FlushBlock();
        }

        Collect(root.GetFirstToken().LeadingTrivia);
        foreach (var ns in root.DescendantNodes().OfType<BaseNamespaceDeclarationSyntax>())
        {
            Collect(ns.GetLeadingTrivia());
        }

        return entries.Count > 0 ? entries.ToArray() : null;
    }
}
