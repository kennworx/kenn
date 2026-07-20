using System.Text.Json;
using Kenn.Dotnet.Cli;
using Kenn.Dotnet.Indexing;
using Kenn.Dotnet.Wire;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.Extensions.Logging.Abstractions;
using Xunit;

namespace Kenn.Dotnet.Tests;

/// <summary>
/// Coverage for the optional `body` range on the `symbol` frame: the full
/// declaration span of the item (attributes + member body), distinct from
/// `range` (the name-identifier span). Drives an in-memory
/// <see cref="CSharpCompilation"/> through
/// <see cref="IndexerCore.IndexCompilationAsync"/> and inspects the JSONL wire.
/// </summary>
public class SymbolBodyRangeTests
{
    private const string Source = """
        namespace Shop;

        public class Widget
        {
            /// <summary>Adds one.</summary>
            [System.Obsolete]
            public int Compute(int x)
            {
                return x + 1;
            }
        }
        """;

    private static List<JsonElement> IndexToFrames()
    {
        var workspace = new DirectoryInfo(
            Path.Combine(Path.GetTempPath(), "kenn-body-" + Guid.NewGuid().ToString("N")));
        workspace.Create();
        var filePath = Path.Combine(workspace.FullName, "Fixture.cs");
        var tree = CSharpSyntaxTree.ParseText(Source, path: filePath);

        // Reference the whole running framework so `System.Obsolete` / `int`
        // resolve; the walk only emits symbols from the source assembly.
        var refs = ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!)
            .Split(Path.PathSeparator)
            .Where(p => p.Length > 0)
            .Select(p => (MetadataReference)MetadataReference.CreateFromFile(p))
            .ToList();

        var compilation = CSharpCompilation.Create(
            "ShopAsm",
            new[] { tree },
            refs,
            new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary));

        var opts = new IndexOptions
        {
            Workspace = workspace,
            Projects = new List<FileInfo>(),
            Include = new List<string>(),
            Exclude = new List<string>(),
            TestGlobs = new List<string>(),
            SkipRestore = true,
            RestoreTimeoutMs = 0,
            FlushBytes = 1 << 20,
            FlushFrames = 1 << 16,
            MaxParallelism = 1,
            EdgeKinds = null,
        };

        var stream = new MemoryStream();
        using (var sink = JsonlSink.OpenStream(stream, opts.FlushBytes, opts.FlushFrames))
        {
            var core = new IndexerCore(opts, sink, NullLogger.Instance);
            core.IndexCompilationAsync(compilation, "ShopAsm", "1.0.0", CancellationToken.None)
                .GetAwaiter().GetResult();
        }

        try { workspace.Delete(recursive: true); }
        catch (IOException) { /* best-effort temp cleanup */ }

        var frames = new List<JsonElement>();
        using var reader = new StreamReader(new MemoryStream(stream.ToArray()));
        string? line;
        while ((line = reader.ReadLine()) is not null)
        {
            if (line.Length == 0) continue;
            frames.Add(JsonDocument.Parse(line).RootElement.Clone());
        }
        return frames;
    }

    private static JsonElement SymbolFrame(List<JsonElement> frames, Func<JsonElement, bool> match) =>
        frames.Single(f => f.GetProperty("type").GetString() == "symbol" && match(f));

    private static (int Sl, int Sc, int El, int Ec) Range(JsonElement arr) =>
        (arr[0].GetInt32(), arr[1].GetInt32(), arr[2].GetInt32(), arr[3].GetInt32());

    [Fact]
    public void MethodBodySpansFullDeclaration()
    {
        var frames = IndexToFrames();
        var compute = SymbolFrame(frames, f => f.GetProperty("name").GetString() == "Compute");

        Assert.True(compute.TryGetProperty("body", out var bodyArr));
        var body = Range(bodyArr);
        var range = Range(compute.GetProperty("range"));

        // Independently parse the fixture to learn the method declaration
        // node's span — body must equal that node (attribute list → closing
        // brace), not the name identifier and not the inner block.
        var method = CSharpSyntaxTree.ParseText(Source)
            .GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>().Single();
        var span = method.GetLocation().GetMappedLineSpan();

        // Body starts at or above the name line (here strictly above, on the
        // `[Obsolete]` attribute) and ends on the method's closing brace.
        Assert.True(body.Sl <= range.Sl);
        Assert.Equal(span.StartLinePosition.Line, body.Sl);
        Assert.Equal(span.EndLinePosition.Line, body.El);
        Assert.True(body.El > range.Sl);
    }

    [Fact]
    public void ImplicitConstructorHasNoBody()
    {
        var frames = IndexToFrames();

        // Widget declares no constructor, so Roslyn synthesizes one with no
        // declaring syntax — its symbol frame carries a name `range` but the
        // `body` field must be omitted entirely.
        var ctor = SymbolFrame(frames, f => f.GetProperty("kind").GetString() == "constructor");

        Assert.True(ctor.TryGetProperty("range", out _));
        Assert.False(ctor.TryGetProperty("body", out _));
    }
}
