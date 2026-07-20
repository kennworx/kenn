using Kenn.Dotnet.Indexing;
using Microsoft.CodeAnalysis.CSharp;
using Xunit;

namespace Kenn.Dotnet.Tests;

/// <summary>
/// Unit coverage for <see cref="FileDoc.Extract"/> — the file-header and
/// namespace-leading comment extraction. Asserts contiguous comments
/// coalesce into one block, a blank line breaks the block, a block comment
/// is its own entry, the file-header and namespace slots are both read, a
/// comment above a no-usings namespace is not double-counted, and a file
/// with no comments yields null.
/// </summary>
public class FileDocTests
{
    private static string[]? Extract(string source) =>
        FileDoc.Extract(CSharpSyntaxTree.ParseText(source));

    [Fact]
    public void ContiguousSlashLinesCoalesceIntoOneBlock()
    {
        var doc = Extract("// line one\n// line two\n// line three\nusing System;\nnamespace N { }");
        Assert.NotNull(doc);
        Assert.Single(doc!);
        Assert.Equal("// line one\n// line two\n// line three", doc![0]);
    }

    [Fact]
    public void BlockCommentIsOneEntryWithNewlinesKept()
    {
        var doc = Extract("/* header\n   second line */\nusing System;\nnamespace N { }");
        Assert.NotNull(doc);
        Assert.Single(doc!);
        Assert.Contains("\n", doc![0]);
        Assert.StartsWith("/* header", doc[0]);
    }

    [Fact]
    public void BlankLineBreaksHeaderFromPurposeComment()
    {
        var doc = Extract("// Copyright foo\n\n// Purpose note\nusing System;\nnamespace N { }");
        Assert.NotNull(doc);
        Assert.Equal(2, doc!.Length);
        Assert.Equal("// Copyright foo", doc[0]);
        Assert.Equal("// Purpose note", doc[1]);
    }

    [Fact]
    public void NamespaceLeadingCommentOnly()
    {
        var doc = Extract("using System;\n\n// namespace doc\nnamespace N { }");
        Assert.NotNull(doc);
        Assert.Single(doc!);
        Assert.Equal("// namespace doc", doc![0]);
    }

    [Fact]
    public void HeaderAndNamespaceBothCaptured()
    {
        var doc = Extract("// file header\nusing System;\n\n// namespace doc\nnamespace N { }");
        Assert.NotNull(doc);
        Assert.Equal(2, doc!.Length);
        Assert.Equal("// file header", doc[0]);
        Assert.Equal("// namespace doc", doc[1]);
    }

    [Fact]
    public void CommentAboveNamespaceWithNoUsingsIsNotDoubleCounted()
    {
        // The first token of the compilation unit IS the `namespace`
        // keyword, so the file-header slot and the namespace slot point at
        // the same trivia span — it must be emitted once.
        var doc = Extract("// header\nnamespace N { }");
        Assert.NotNull(doc);
        Assert.Single(doc!);
        Assert.Equal("// header", doc![0]);
    }

    [Fact]
    public void NoCommentsYieldsNull()
    {
        Assert.Null(Extract("using System;\nnamespace N { }"));
    }
}
