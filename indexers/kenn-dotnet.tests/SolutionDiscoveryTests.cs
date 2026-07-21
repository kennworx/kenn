using Kenn.Dotnet.Cli;
using Kenn.Dotnet.Indexing;
using Xunit;

namespace Kenn.Dotnet.Tests;

/// <summary>
/// Guards which solution/project files kenn-dotnet discovers when none are
/// passed explicitly. The `.slnx` case is the one that bit: a repo shipping
/// only the newer XML solution format (Newtonsoft.Json) was discovered as
/// nothing, so the whole workspace indexed zero files at exit 0.
/// </summary>
public class SolutionDiscoveryTests
{
    private static IndexOptions OptionsFor(DirectoryInfo ws) => new()
    {
        Workspace = ws,
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

    private static DirectoryInfo TempDirWith(params string[] fileNames)
    {
        var dir = Directory.CreateTempSubdirectory("kenn-slnx-test");
        foreach (var name in fileNames)
        {
            File.WriteAllText(Path.Combine(dir.FullName, name), "");
        }
        return dir;
    }

    [Theory]
    [InlineData("App.slnx")]
    [InlineData("App.sln")]
    [InlineData("App.csproj")]
    public void DiscoversSolutionAndProjectFiles(string fileName)
    {
        var ws = TempDirWith(fileName);
        try
        {
            var found = SolutionLoader.DiscoverProjectFiles(OptionsFor(ws));
            Assert.Contains(found, f => f.Name == fileName);
        }
        finally
        {
            ws.Delete(recursive: true);
        }
    }

    /// A repo with ONLY a `.slnx` — the regression case — must discover it.
    [Fact]
    public void ASlnxOnlyRepoIsNotEmpty()
    {
        var ws = TempDirWith("Newtonsoft.Json.slnx");
        try
        {
            var found = SolutionLoader.DiscoverProjectFiles(OptionsFor(ws));
            Assert.Single(found);
            Assert.Equal(".slnx", found[0].Extension);
        }
        finally
        {
            ws.Delete(recursive: true);
        }
    }

    /// A non-project file must not be discovered — the filter is real, not
    /// "anything in the directory".
    [Fact]
    public void IgnoresUnrelatedFiles()
    {
        var ws = TempDirWith("README.md", "notes.txt");
        try
        {
            var found = SolutionLoader.DiscoverProjectFiles(OptionsFor(ws));
            Assert.Empty(found);
        }
        finally
        {
            ws.Delete(recursive: true);
        }
    }
}
