using System.Reflection;
using Kenn.Dotnet.Cli;
using Xunit;

namespace Kenn.Dotnet.Tests;

/// <summary>
/// Guards the contract that lets `kenn-dotnet --version` answer on a machine
/// with no .NET SDK. `kenn init` uses that probe to decide whether C# is
/// indexable at all, so a regression here silently degrades every C# workspace
/// to the generic text fallback.
/// </summary>
public class MsBuildBootstrapTests
{
    // NOTE: the end-to-end invariant — `--version` exits 0 with no .NET SDK
    // reachable — cannot be tested from here. Under `dotnet test` the indexer
    // runs via `dotnet exec`, and MSBuildLocator resolves an SDK from the
    // muxer's own `Environment.ProcessPath` no matter how the environment is
    // scrubbed. It only fails for the self-contained binary the test suite
    // never builds. That check lives in `just probe-smoke`, against the
    // artifacts in ./build. What remains here is what a unit test can honestly
    // assert: the JIT-ordering attribute, the version's single source, and the
    // ASCII degradation of an OS-localized diagnostic.

    /// <summary>
    /// MSBuildLocator requires registration to happen before the JIT resolves
    /// any MSBuild type. `Run` constructs Roslyn's MSBuildWorkspace, so if it
    /// were inlined into its caller — the frame that calls TryRegister — the
    /// JIT could load those types before registration ran.
    /// </summary>
    [Fact]
    public void Run_IsNoInlining_SoMsBuildTypesCannotLoadInTheRegisteringFrame()
    {
        var run = typeof(IndexCommand).GetMethod(
            nameof(IndexCommand.Run), BindingFlags.Public | BindingFlags.Static);

        Assert.NotNull(run);
        Assert.True(
            run!.MethodImplementationFlags.HasFlag(MethodImplAttributes.NoInlining),
            "IndexCommand.Run must be [MethodImpl(MethodImplOptions.NoInlining)] — it loads "
            + "MSBuild types and must not be inlined into the frame that registers MSBuild.");
    }

    /// <summary>
    /// `tool_version` on the wire and the string `--version` prints come from
    /// one source, so a consumer correlating them can never see a mismatch.
    /// </summary>
    [Fact]
    public void ToolVersion_ComesFromTheAssemblyInformationalVersion()
    {
        var expected = typeof(MsBuildBootstrap).Assembly
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion;

        Assert.False(string.IsNullOrWhiteSpace(MsBuildBootstrap.ToolVersion));
        Assert.NotEqual("unknown", MsBuildBootstrap.ToolVersion);
        if (expected is not null)
        {
            Assert.Equal(expected, MsBuildBootstrap.ToolVersion);
        }
    }

    /// <summary>
    /// The missing-toolchain diagnostic embeds an OS-localized exception
    /// message, so the literal being ASCII is not enough — the whole string is
    /// degraded before it reaches a console that may not be UTF-8.
    /// </summary>
    [Theory]
    [InlineData("plain ascii", "plain ascii")]
    [InlineData("em—dash", "em?dash")]
    [InlineData("Der Pfad „dotnet“ ist ungültig", "Der Pfad ?dotnet? ist ung?ltig")]
    [InlineData("", "")]
    public void ToAscii_ReplacesEveryNonAsciiCharacter(string input, string expected)
    {
        Assert.Equal(expected, MsBuildBootstrap.ToAscii(input));
    }

    [Fact]
    public void ToAscii_OutputIsAlwaysAscii()
    {
        const string localized = "無効なパス: dotnet — 見つかりません";
        Assert.All(MsBuildBootstrap.ToAscii(localized), ch => Assert.True(ch < 128));
    }

    [Fact]
    public void ToAscii_ReturnsTheSameInstanceWhenAlreadyAscii()
    {
        const string ascii = "kenn-dotnet: no usable MSBuild instance";
        Assert.Same(ascii, MsBuildBootstrap.ToAscii(ascii));
    }

    /// <summary>
    /// The whole diagnostic is degraded, not only the exception's message. A
    /// typographic character typed into the surrounding literal reaches a
    /// non-UTF-8 console exactly as an OS-localized one would.
    /// </summary>
    [Fact]
    public void FormatReason_IsAsciiEvenWhenTheExceptionMessageIsNot()
    {
        var reason = MsBuildBootstrap.FormatReason(
            new InvalidOperationException("Der Pfad „dotnet“ ist ungültig — 見つかりません"));

        Assert.All(reason, ch => Assert.True(ch < 128, $"non-ASCII '{ch}' in: {reason}"));
        Assert.Contains("InvalidOperationException", reason, StringComparison.Ordinal);
        Assert.Contains("requires the .NET SDK", reason, StringComparison.Ordinal);
    }

    /// <summary>A multi-line exception message must not break the one-line contract.</summary>
    [Fact]
    public void FormatReason_CollapsesNewlinesToOneLine()
    {
        var reason = MsBuildBootstrap.FormatReason(
            new IOException("line one\nline two\r\nline three"));

        Assert.DoesNotContain('\n', reason);
        Assert.DoesNotContain('\r', reason);
    }

    /// <summary>
    /// A global.json pinning an unavailable SDK must be NAMED in the failure.
    ///
    /// Real case this comes from: a workspace pinning "9.0.308" with
    /// rollForward "latestMinor", indexed by an image carrying SDK 10.0.302.
    /// latestMinor does not cross a major, so hostfxr selects nothing and
    /// MSBuildLocator reports only "Error while calling hostfxr" — while the
    /// old advice told the user to install the SDK and put `dotnet` on PATH,
    /// both of which were already true. The pin is the actionable fact.
    /// </summary>
    [Fact]
    public void FormatReason_NamesTheGlobalJsonPinInsteadOfBlamingPath()
    {
        var dir = Directory.CreateTempSubdirectory("kenn-globaljson");
        try
        {
            File.WriteAllText(
                Path.Combine(dir.FullName, "global.json"),
                """{ "sdk": { "version": "9.0.308", "rollForward": "latestMinor" } }""");

            var reason = MsBuildBootstrap.FormatReason(
                new InvalidOperationException("Error while calling hostfxr"),
                dir.FullName);

            Assert.Contains("9.0.308", reason);
            Assert.Contains("latestMinor", reason);
            Assert.Contains("global.json", reason);
            // The misleading advice must be gone: the SDK IS installed here.
            Assert.DoesNotContain("ensure 'dotnet' is on PATH", reason);
            Assert.DoesNotContain('\n', reason);
        }
        finally
        {
            dir.Delete(recursive: true);
        }
    }

    /// <summary>
    /// With no pin in scope, the original install-the-SDK advice is still the
    /// right answer — the diagnostic must not blame a global.json that is absent.
    /// </summary>
    [Fact]
    public void FormatReason_KeepsInstallAdviceWhenNoPinIsInScope()
    {
        // A temp dir has no global.json at or above it inside the temp root;
        // if one existed higher the assertion below would catch the confusion.
        var dir = Directory.CreateTempSubdirectory("kenn-nopin");
        try
        {
            var reason = MsBuildBootstrap.FormatReason(
                new InvalidOperationException("SDK not found"), dir.FullName);

            if (MsBuildBootstrap.FindSdkPin(dir.FullName) is null)
            {
                Assert.Contains("dotnet.microsoft.com/download", reason);
            }
        }
        finally
        {
            dir.Delete(recursive: true);
        }
    }

    /// <summary>
    /// The nearest global.json wins even when it carries no sdk pin — that is
    /// what the SDK resolver does, and reporting a farther pin would misdirect.
    /// </summary>
    [Fact]
    public void FindSdkPin_NearestGlobalJsonShadowsAFartherPin()
    {
        var root = Directory.CreateTempSubdirectory("kenn-shadow");
        try
        {
            File.WriteAllText(
                Path.Combine(root.FullName, "global.json"),
                """{ "sdk": { "version": "9.0.308" } }""");
            var nested = Directory.CreateDirectory(Path.Combine(root.FullName, "nested"));
            File.WriteAllText(
                Path.Combine(nested.FullName, "global.json"),
                """{ "msbuild-sdks": { "Some.Sdk": "1.0.0" } }""");

            Assert.Null(MsBuildBootstrap.FindSdkPin(nested.FullName));
            Assert.Equal("9.0.308", MsBuildBootstrap.FindSdkPin(root.FullName)!.Value.Version);
        }
        finally
        {
            root.Delete(recursive: true);
        }
    }
}
