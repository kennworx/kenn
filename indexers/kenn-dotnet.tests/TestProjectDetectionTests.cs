using System.Xml.Linq;
using Kenn.Dotnet.Indexing;
using Xunit;

namespace Kenn.Dotnet.Tests;

/// <summary>
/// A C# project is a test project when it references a test framework or the VS
/// Test Platform (what makes <c>dotnet test</c> discover it), so every symbol in
/// it emits <c>test=true</c> — not just files named <c>*Test.cs</c>. Covers
/// <see cref="IndexerCore.IsTestFrameworkAssembly"/>, the assembly-name gate.
/// </summary>
public class TestProjectDetectionTests
{
    [Theory]
    [InlineData("xunit.core", true)]
    [InlineData("xunit.assert", true)]
    [InlineData("xunit", true)]
    [InlineData("xunit.runner.visualstudio", true)]
    [InlineData("nunit", true)]
    [InlineData("nunit.framework", true)]
    [InlineData("nunit3.testadapter", true)] // adapter — a project referencing it is a test project
    [InlineData("MSTest.TestFramework", true)]
    [InlineData("Microsoft.VisualStudio.TestPlatform.TestFramework", true)]
    [InlineData("Microsoft.TestPlatform.TestHost", true)]
    [InlineData("Newtonsoft.Json", false)]
    [InlineData("MyApp.Core", false)]
    [InlineData("System.Text.Json", false)]
    public void identifies_test_framework_assemblies(string assembly, bool expected)
    {
        Assert.Equal(expected, IndexerCore.IsTestFrameworkAssembly(assembly));
    }

    [Theory]
    [InlineData("nunit", true)]
    [InlineData("NUnit3TestAdapter", true)]
    [InlineData("xunit", true)]
    [InlineData("MSTest.TestFramework", true)]
    [InlineData("Microsoft.NET.Test.Sdk", true)] // Test.Sdk package id — no matching runtime assembly
    [InlineData("Microsoft.Extensions.Logging", false)]
    [InlineData("Newtonsoft.Json", false)]
    [InlineData("", false)]
    public void identifies_test_framework_packages(string package, bool expected)
    {
        Assert.Equal(expected, IndexerCore.IsTestFrameworkPackage(package));
    }

    [Theory]
    // <IsTestProject>true</IsTestProject> — the explicit SDK marker.
    [InlineData(
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><IsTestProject>true</IsTestProject></PropertyGroup></Project>",
        true)]
    // Value is parsed, not merely present: false must not read as a test project.
    [InlineData(
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><IsTestProject>false</IsTestProject></PropertyGroup></Project>",
        false)]
    // A PackageReference to a framework marks it even with no IsTestProject —
    // and versionless (central package management), the real-world shape.
    [InlineData(
        "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup><PackageReference Include=\"nunit\" /></ItemGroup></Project>",
        true)]
    [InlineData(
        "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup><PackageReference Include=\"Microsoft.NET.Test.Sdk\" /></ItemGroup></Project>",
        true)]
    // Production project: no test markers.
    [InlineData(
        "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup><PackageReference Include=\"Serilog\" /></ItemGroup></Project>",
        false)]
    // Descendants() spans EVERY ItemGroup: the framework ref is in the SECOND
    // group, after a production-only first group.
    [InlineData(
        "<Project Sdk=\"Microsoft.NET.Sdk\">"
        + "<ItemGroup><PackageReference Include=\"Serilog\" /></ItemGroup>"
        + "<ItemGroup><PackageReference Include=\"nunit\" /></ItemGroup></Project>",
        true)]
    // ...including a conditional ItemGroup — Conditions are not evaluated, the
    // element's presence is enough.
    [InlineData(
        "<Project Sdk=\"Microsoft.NET.Sdk\">"
        + "<ItemGroup Condition=\"'$(TargetFramework)'=='net8.0'\">"
        + "<PackageReference Include=\"xunit\" /></ItemGroup></Project>",
        true)]
    // Legacy csproj with the 2003 MSBuild namespace — LocalName matching must
    // still see IsTestProject through the xmlns.
    [InlineData(
        "<Project xmlns=\"http://schemas.microsoft.com/developer/msbuild/2003\"><PropertyGroup><IsTestProject>true</IsTestProject></PropertyGroup></Project>",
        true)]
    public void csproj_xml_declares_test(string csproj, bool expected)
    {
        Assert.Equal(expected, IndexerCore.XmlDeclaresTest(XDocument.Parse(csproj)));
    }

    [Theory]
    // A framework PackageReference is contagious across a project reference.
    [InlineData(
        "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup><PackageReference Include=\"nunit\" /></ItemGroup></Project>",
        true)]
    [InlineData(
        "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup><PackageReference Include=\"Microsoft.NET.Test.Sdk\" /></ItemGroup></Project>",
        true)]
    // <IsTestProject> is a self-marker, NOT contagious — a referrer of this
    // project is not automatically test code.
    [InlineData(
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><IsTestProject>true</IsTestProject></PropertyGroup></Project>",
        false)]
    [InlineData(
        "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup><PackageReference Include=\"Serilog\" /></ItemGroup></Project>",
        false)]
    public void framework_reference_is_contagious_istestproject_is_not(string csproj, bool expected)
    {
        Assert.Equal(expected, IndexerCore.XmlReferencesTestFramework(XDocument.Parse(csproj)));
    }

    [Fact]
    public void resolves_project_reference_paths_normalizing_backslashes()
    {
        var dir = Path.Combine(Path.GetTempPath(), "App");
        var doc = XDocument.Parse(
            "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>"
            + "<ProjectReference Include=\"..\\Lib\\Base.csproj\" />"
            + "<PackageReference Include=\"nunit\" />" // not a ProjectReference — must be ignored
            + "</ItemGroup></Project>");

        var paths = IndexerCore.ProjectReferencePaths(doc, dir).ToList();

        // Backslash separators resolve the same as forward slashes; without
        // normalization they'd be literal filename chars on macOS/Linux.
        Assert.Equal(
            new[] { Path.GetFullPath(Path.Combine(dir, "../Lib/Base.csproj")) },
            paths);
    }

    [Fact]
    public void splits_semicolon_separated_project_reference_list_and_trims()
    {
        var dir = Path.Combine(Path.GetTempPath(), "App");
        // MSBuild Include is a `;`-separated item list; entries may be padded.
        var doc = XDocument.Parse(
            "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>"
            + "<ProjectReference Include=\"..\\Lib\\A.csproj ; ..\\Lib\\B.csproj\" />"
            + "</ItemGroup></Project>");

        var paths = IndexerCore.ProjectReferencePaths(doc, dir).ToList();

        Assert.Equal(
            new[]
            {
                Path.GetFullPath(Path.Combine(dir, "../Lib/A.csproj")),
                Path.GetFullPath(Path.Combine(dir, "../Lib/B.csproj")),
            },
            paths);
    }

    [Fact]
    public void csproj_declares_test_via_project_reference_to_test_base()
    {
        var root = Path.Combine(Path.GetTempPath(), "kenn-prtest-" + Guid.NewGuid().ToString("N"));
        try
        {
            var baseDir = Path.Combine(root, "Lib", "Base");
            var appDir = Path.Combine(root, "App");
            Directory.CreateDirectory(baseDir);
            Directory.CreateDirectory(appDir);

            // Test-base: carries nunit, but is NOT <IsTestProject>.
            var basePath = Path.Combine(baseDir, "Base.csproj");
            File.WriteAllText(basePath,
                "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>"
                + "<PackageReference Include=\"nunit\" /></ItemGroup></Project>");

            // Referrer: no own test signal; reaches the framework only via the
            // ProjectReference to the test-base.
            var appPath = Path.Combine(appDir, "App.csproj");
            File.WriteAllText(appPath,
                "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>"
                + "<ProjectReference Include=\"..\\Lib\\Base\\Base.csproj\" /></ItemGroup></Project>");

            Assert.True(IndexerCore.CsprojDeclaresTest(appPath));

            // Control: a referrer whose only reference is a production library
            // is not test code.
            var libPath = Path.Combine(baseDir, "Prod.csproj");
            File.WriteAllText(libPath,
                "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>"
                + "<PackageReference Include=\"Serilog\" /></ItemGroup></Project>");
            var app2Path = Path.Combine(appDir, "App2.csproj");
            File.WriteAllText(app2Path,
                "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>"
                + "<ProjectReference Include=\"..\\Lib\\Base\\Prod.csproj\" /></ItemGroup></Project>");

            Assert.False(IndexerCore.CsprojDeclaresTest(app2Path));
        }
        finally
        {
            if (Directory.Exists(root)) Directory.Delete(root, recursive: true);
        }
    }

    [Fact]
    public void reuses_cached_csproj_result_within_a_pass()
    {
        var root = Path.Combine(Path.GetTempPath(), "kenn-cache-" + Guid.NewGuid().ToString("N"));
        try
        {
            var baseDir = Path.Combine(root, "Lib", "Base");
            var appDir = Path.Combine(root, "App");
            Directory.CreateDirectory(baseDir);
            Directory.CreateDirectory(appDir);

            var basePath = Path.Combine(baseDir, "Base.csproj");
            File.WriteAllText(basePath,
                "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>"
                + "<PackageReference Include=\"nunit\" /></ItemGroup></Project>");
            var appPath = Path.Combine(appDir, "App.csproj");
            File.WriteAllText(appPath,
                "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>"
                + "<ProjectReference Include=\"..\\Lib\\Base\\Base.csproj\" /></ItemGroup></Project>");

            var cache = new Dictionary<string, IndexerCore.CsprojTestInfo>();
            Assert.True(IndexerCore.CsprojDeclaresTest(appPath, cache));
            Assert.True(cache.ContainsKey(appPath)); // the project's own csproj was evaluated into the cache

            // Rewrite the test-base on disk to drop its framework. A re-read
            // would now yield false; the cached result keeps the pass consistent,
            // so the same cache still returns true — proving the base was cached.
            File.WriteAllText(basePath,
                "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>"
                + "<PackageReference Include=\"Serilog\" /></ItemGroup></Project>");
            Assert.True(IndexerCore.CsprojDeclaresTest(appPath, cache));

            // A fresh cache re-evaluates the changed base and sees no framework.
            Assert.False(IndexerCore.CsprojDeclaresTest(appPath, new Dictionary<string, IndexerCore.CsprojTestInfo>()));
        }
        finally
        {
            if (Directory.Exists(root)) Directory.Delete(root, recursive: true);
        }
    }
}
