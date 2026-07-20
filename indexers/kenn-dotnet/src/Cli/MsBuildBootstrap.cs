using System.ComponentModel;
using System.Reflection;
using System.Text;
using System.Text.Json;
using Microsoft.Build.Locator;

namespace Kenn.Dotnet.Cli;

/// <summary>
/// Registers the MSBuild toolset that Roslyn's <c>MSBuildWorkspace</c> loads
/// during indexing, and reports this tool's own version.
///
/// Registration lives here rather than in <c>Program.cs</c>'s top-level
/// statements so that <c>--version</c> and <c>--help</c> answer on a machine
/// with no .NET SDK. A caller probing whether C# is indexable must be able to
/// ask without the probe itself requiring the very toolchain it asks about.
///
/// Registration must still happen before the JIT resolves any MSBuild type.
/// That ordering is enforced on the *consumer* side: <see cref="IndexCommand.Run"/>
/// is marked <c>NoInlining</c> so its body, which constructs the Roslyn
/// workspace, can never be inlined into the frame that calls
/// <see cref="TryRegister"/>.
/// </summary>
internal static class MsBuildBootstrap
{
    /// <summary>
    /// The assembly's informational version: the same string <c>--version</c>
    /// prints, so the wire's <c>tool_version</c> and the version probe can
    /// never disagree.
    /// </summary>
    public static string ToolVersion { get; } =
        typeof(MsBuildBootstrap).Assembly
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
        ?? typeof(MsBuildBootstrap).Assembly.GetName().Version?.ToString()
        ?? "unknown";

    /// <summary>
    /// Replace every non-ASCII character with '?'.
    ///
    /// The missing-toolchain diagnostic is the only output a user without a
    /// .NET SDK ever sees, and it embeds an exception message the OS may have
    /// localized. On a Windows console under a non-UTF-8 code page those bytes
    /// render as mojibake, so we degrade them to something legible instead.
    /// </summary>
    public static string ToAscii(string text)
    {
        foreach (var ch in text)
        {
            if (ch >= 128)
            {
                return Rebuild(text);
            }
        }

        return text;

        static string Rebuild(string text)
        {
            var sb = new StringBuilder(text.Length);
            foreach (var ch in text)
            {
                sb.Append(ch < 128 ? ch : '?');
            }

            return sb.ToString();
        }
    }

    /// <summary>
    /// The one-line, ASCII, console-safe explanation for a locator failure.
    ///
    /// The whole string is degraded, not just the exception's message: a
    /// typographic character typed into the surrounding literal would reach a
    /// non-UTF-8 console exactly as an OS-localized one would.
    /// </summary>
    public static string FormatReason(Exception ex, string? startDir = null) => ToAscii(
        $"kenn-dotnet: no usable MSBuild instance ({ex.GetType().Name}): "
        + $"{ex.Message.ReplaceLineEndings(" ")}. "
        + LocatorAdvice(startDir ?? Directory.GetCurrentDirectory()));

    /// <summary>
    /// What to actually DO about a locator failure.
    ///
    /// The default advice ("install the SDK") is wrong — and actively
    /// misleading — in the most common container/CI case: the SDK IS installed
    /// and on PATH, but a <c>global.json</c> pins a version it does not satisfy,
    /// so hostfxr refuses to select it and MSBuildLocator sees no toolset. That
    /// surfaces only as "Error while calling hostfxr", which tells nobody
    /// anything. When a pin is present, name it instead.
    /// </summary>
    internal static string LocatorAdvice(string startDir)
    {
        var pin = FindSdkPin(startDir);
        if (pin is null)
        {
            return "C# indexing requires the .NET SDK; install it from "
                + "https://dotnet.microsoft.com/download and ensure 'dotnet' is on PATH.";
        }

        var roll = string.IsNullOrEmpty(pin.Value.RollForward)
            ? string.Empty
            : $" with rollForward '{pin.Value.RollForward}'";
        return $"{pin.Value.Path} pins .NET SDK '{pin.Value.Version}'{roll}, "
            + "and no installed SDK satisfies it (run 'dotnet --list-sdks' to see what is present). "
            + "Install that SDK, index with an image whose SDK major matches the pin, "
            + "or relax 'rollForward' in global.json.";
    }

    /// <summary>
    /// The nearest <c>global.json</c> SDK pin at or above <paramref name="startDir"/>.
    ///
    /// Mirrors the SDK resolver's own search: walk upward and stop at the FIRST
    /// global.json, whether or not it carries an sdk.version — a nearer file
    /// without a pin shadows a farther one that has it.
    /// </summary>
    internal static (string Path, string Version, string RollForward)? FindSdkPin(string startDir)
    {
        for (var dir = new DirectoryInfo(startDir); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, "global.json");
            if (!File.Exists(candidate))
            {
                continue;
            }

            try
            {
                using var doc = JsonDocument.Parse(File.ReadAllText(candidate));
                if (doc.RootElement.ValueKind == JsonValueKind.Object
                    && doc.RootElement.TryGetProperty("sdk", out var sdk)
                    && sdk.ValueKind == JsonValueKind.Object)
                {
                    var version = sdk.TryGetProperty("version", out var v)
                        ? v.GetString() ?? string.Empty
                        : string.Empty;
                    var roll = sdk.TryGetProperty("rollForward", out var r)
                        ? r.GetString() ?? string.Empty
                        : string.Empty;
                    if (version.Length > 0)
                    {
                        return (candidate, version, roll);
                    }
                }
            }
            catch (JsonException)
            {
                // Malformed global.json: the SDK resolver will complain about it
                // in its own words; do not editorialize here.
            }

            return null;
        }

        return null;
    }

    /// <summary>
    /// Locate and register the default MSBuild instance.
    ///
    /// Returns <c>false</c> and sets <paramref name="reason"/> to an ASCII,
    /// single-line explanation when no usable instance exists. The caller owns
    /// reporting it; this method writes nothing.
    /// </summary>
    /// <param name="startDir">
    /// Where to begin the <c>global.json</c> search when registration fails, so
    /// the failure can name the pin that caused it. Defaults to the process's
    /// current directory, which is the workspace when kenn drives this tool.
    /// </param>
    public static bool TryRegister(out string reason, string? startDir = null)
    {
        reason = string.Empty;
        if (MSBuildLocator.IsRegistered)
        {
            return true;
        }

        try
        {
            MSBuildLocator.RegisterDefaults();
            return true;
        }
        // MSBuildLocator discovers .NET Core toolsets by spawning `dotnet --info`.
        // A missing SDK surfaces as InvalidOperationException; a `dotnet` that
        // exists but cannot be executed surfaces as Win32Exception or IOException
        // from Process.Start. Anything outside this set is a bug in us or in the
        // locator, and must not be disguised as a missing toolchain.
        catch (Exception ex) when (ex is InvalidOperationException
                                      or Win32Exception
                                      or IOException
                                      or PlatformNotSupportedException)
        {
            reason = FormatReason(ex, startDir);
            return false;
        }
    }
}
