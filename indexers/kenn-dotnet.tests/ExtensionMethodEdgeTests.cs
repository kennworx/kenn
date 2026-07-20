using System.Text.Json;
using Kenn.Dotnet.Cli;
using Kenn.Dotnet.Indexing;
using Kenn.Dotnet.Wire;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.Extensions.Logging.Abstractions;
using Xunit;

namespace Kenn.Dotnet.Tests;

/// <summary>
/// Coverage for the `extends_type` edge: a C# extension method gains an edge
/// to the type it extends (the receiver), so the type's extension methods are
/// discoverable *from* the type — while keeping its `defined_in` to the holder
/// static class. Drives an in-memory <see cref="CSharpCompilation"/> through
/// <see cref="IndexerCore.IndexCompilationAsync"/> and inspects the JSONL wire.
/// </summary>
public class ExtensionMethodEdgeTests
{
    private const string Source = """
        namespace Shop;

        public class Order { public int Id; }

        public static class OrderExtensions
        {
            public static void Touch(this Order o) { }
            public static T FirstOr<T>(this System.Collections.Generic.IEnumerable<T> xs, T fallback) => fallback;
        }

        public static class Other
        {
            public static void Plain(Order o) { }
        }
        """;

    private static List<JsonElement> IndexToFrames()
    {
        var workspace = new DirectoryInfo(
            Path.Combine(Path.GetTempPath(), "kenn-ext-" + Guid.NewGuid().ToString("N")));
        workspace.Create();
        var filePath = Path.Combine(workspace.FullName, "Fixture.cs");
        var tree = CSharpSyntaxTree.ParseText(Source, path: filePath);

        // Reference the whole running framework so `IEnumerable<T>` / `object`
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

    private static Dictionary<uint, string> KeyById(List<JsonElement> frames)
    {
        var map = new Dictionary<uint, string>();
        foreach (var f in frames)
        {
            // Both full symbols and ref-stubs (the receiver types referenced by
            // an edge before/without being walked as a definition) carry `key`.
            var type = f.GetProperty("type").GetString();
            if ((type == "symbol" || type == "stub") && f.TryGetProperty("key", out var k))
            {
                map[f.GetProperty("id").GetUInt32()] = k.GetString()!;
            }
        }
        return map;
    }

    private static List<(string? Source, string? Target)> EdgesOfKind(List<JsonElement> frames, string kind)
    {
        var keyById = KeyById(frames);
        string? KeyOf(uint id) => keyById.TryGetValue(id, out var k) ? k : null;
        return frames
            .Where(f => f.GetProperty("type").GetString() == "edge"
                        && f.GetProperty("edge_kind").GetString() == kind)
            .Select(f => (KeyOf(f.GetProperty("source").GetUInt32()),
                          KeyOf(f.GetProperty("target").GetUInt32())))
            .ToList();
    }

    [Fact]
    public void ExtensionMethodEmitsExtendsTypeToReceiver()
    {
        var edges = EdgesOfKind(IndexToFrames(), "extends_type");

        // Touch(this Order) → Order.
        Assert.Contains(edges, e => e.Source is not null && e.Source.Contains("#Touch(") && e.Target == "Shop.Order");
        // FirstOr<T>(this IEnumerable<T>) → the open generic IEnumerable<>.
        Assert.Contains(edges, e => e.Source is not null && e.Source.Contains("#FirstOr")
                                    && e.Target is not null && e.Target.Contains("IEnumerable"));
        // Plain(Order) is an ordinary method, not an extension → no edge.
        Assert.DoesNotContain(edges, e => e.Source is not null && e.Source.Contains("#Plain("));
        // Exactly the two extension methods produced an edge.
        Assert.Equal(2, edges.Count);
    }

    [Fact]
    public void ExtensionMethodKeepsDefinedInHolder()
    {
        var edges = EdgesOfKind(IndexToFrames(), "defined_in");

        // The extension method still belongs to its holder static class, not Order.
        Assert.Contains(edges, e => e.Source is not null && e.Source.Contains("#Touch(")
                                    && e.Target == "Shop.OrderExtensions");
    }
}
