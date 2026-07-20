using Kenn.Dotnet.Indexing;
using Xunit;

namespace Kenn.Dotnet.Tests;

/// <summary>
/// Unit coverage for <see cref="DocXml.Normalize"/> — turning Roslyn's
/// <c>&lt;member&gt;</c> XML doc comments into plain prose. Mirrors the real
/// tag shapes seen in a C# corpus: summary, param/returns, see cref to FQNs,
/// paramref, list/c, inheritdoc, entities, and malformed input.
/// </summary>
public class DocXmlTests
{
    private static string Member(string inner) => $"<member name=\"T:Test\">{inner}</member>";

    [Fact]
    public void SummaryBecomesPlainProse()
    {
        var doc = DocXml.Normalize(Member("<summary>Holds the order.</summary>"));
        Assert.Equal("Holds the order.", doc);
    }

    [Fact]
    public void StripsMemberEnvelopeAndTags()
    {
        var doc = DocXml.Normalize(Member("<summary>Base asset name.</summary>"));
        Assert.NotNull(doc);
        Assert.DoesNotContain("<", doc!);
        Assert.DoesNotContain("member", doc!);
        Assert.DoesNotContain("summary", doc!);
    }

    [Fact]
    public void SeeCrefRendersBareTypeName()
    {
        var doc = DocXml.Normalize(Member(
            "<summary>Returns the absolute value of the specified <see cref=\"T:System.Decimal\"/>.</summary>"));
        Assert.NotNull(doc);
        Assert.Contains("absolute value of the specified", doc!);
        Assert.Contains("Decimal", doc!);
        Assert.DoesNotContain("System.Decimal", doc!);
        Assert.DoesNotContain("T:", doc!);
    }

    [Fact]
    public void SeeCrefMethodDropsNamespaceAndArgs()
    {
        var doc = DocXml.Normalize(Member("<summary>See <see cref=\"M:Acme.Foo.Bar(System.Int32)\"/>.</summary>"));
        Assert.NotNull(doc);
        Assert.Contains("Bar", doc!);
        Assert.DoesNotContain("Acme", doc!);
        Assert.DoesNotContain("Int32", doc!);
    }

    [Fact]
    public void ParamAndReturnsTextIsKept()
    {
        var doc = DocXml.Normalize(Member(
            "<summary>Absolute value.</summary><param name=\"value\">The input value.</param><returns>The absolute value.</returns>"));
        Assert.NotNull(doc);
        Assert.Contains("Absolute value.", doc!);
        Assert.Contains("The input value.", doc!);
        Assert.Contains("The absolute value.", doc!);
        Assert.DoesNotContain("param", doc!);
    }

    [Fact]
    public void SeeLangwordRendersTheKeyword()
    {
        var doc = DocXml.Normalize(Member("<summary>Returns <see langword=\"true\"/> on success.</summary>"));
        Assert.NotNull(doc);
        Assert.Contains("Returns true on success.", doc!);
    }

    [Fact]
    public void ParamRefRendersTheName()
    {
        var doc = DocXml.Normalize(Member("<summary>Copy <paramref name=\"src\"/> next to it.</summary>"));
        Assert.NotNull(doc);
        Assert.Contains("Copy src next to it.", doc!);
    }

    [Fact]
    public void ListItemsAndInlineCodeAreFlattened()
    {
        var doc = DocXml.Normalize(Member(
            "<summary>Filter:<list type=\"bullet\"><item><c>UserId</c> exact match;</item><item><c>Email</c> partial match.</item></list></summary>"));
        Assert.NotNull(doc);
        Assert.Contains("Filter:", doc!);
        Assert.Contains("UserId", doc!);
        Assert.Contains("exact match;", doc!);
        Assert.Contains("Email", doc!);
        Assert.DoesNotContain("<list", doc!);
        Assert.DoesNotContain("<c>", doc!);
    }

    [Fact]
    public void DecodesXmlEntities()
    {
        var doc = DocXml.Normalize(Member("<summary>true if a &lt; b &amp; c &gt; d.</summary>"));
        Assert.Equal("true if a < b & c > d.", doc);
    }

    [Fact]
    public void InheritDocOnlyYieldsNull()
    {
        Assert.Null(DocXml.Normalize(Member("<inheritdoc />")));
        Assert.Null(DocXml.Normalize(Member("<inheritdoc/>")));
    }

    [Fact]
    public void MalformedXmlYieldsNull()
    {
        Assert.Null(DocXml.Normalize("<summary>unterminated"));
        Assert.Null(DocXml.Normalize("not xml at all <<<"));
    }

    [Fact]
    public void NullOrWhitespaceYieldsNull()
    {
        Assert.Null(DocXml.Normalize(null));
        Assert.Null(DocXml.Normalize(""));
        Assert.Null(DocXml.Normalize("   \n  "));
    }

    [Fact]
    public void MultiParagraphCollapsesToSingleSpacedProse()
    {
        var doc = DocXml.Normalize(Member(
            "<summary><para>Private key for a hot wallet.</para><para>Returned in the <c>master_seed</c> field.</para></summary>"));
        Assert.NotNull(doc);
        Assert.Contains("Private key for a hot wallet.", doc!);
        Assert.Contains("Returned in the master_seed field.", doc!);
        Assert.DoesNotContain("  ", doc!); // whitespace fully collapsed
    }
}
