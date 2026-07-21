using Kenn.Dotnet.Wire;

namespace Kenn.Dotnet.Cli;

/// <summary>
/// Parsed `index` subcommand options. Populated by System.CommandLine in
/// <see cref="Program"/>; consumed by <see cref="IndexCommand"/> and the
/// downstream walker. Every field is `required` so any newly added option
/// forces a compile-time update at the parsing call site.
/// </summary>
public sealed class IndexOptions
{
    /// <summary>Workspace root. Defaults to the current working directory.</summary>
    public required DirectoryInfo Workspace { get; init; }

    /// <summary>
    /// Explicit `.sln`/`.csproj` files to load. When empty, the loader
    /// discovers projects under <see cref="Workspace"/>.
    /// </summary>
    public required List<FileInfo> Projects { get; init; }

    /// <summary>File-path glob include filters (workspace-relative). Defaults to `**`.</summary>
    public required List<string> Include { get; init; }

    /// <summary>File-path glob exclude filters, applied after <see cref="Include"/>.</summary>
    public required List<string> Exclude { get; init; }

    /// <summary>
    /// Glob patterns marking workspace-relative paths as test files. Symbols
    /// defined in matching files get <c>test = true</c> on the wire so the
    /// store / analyzer can split user-test from user-live god-node sets.
    /// When empty, no files are tagged as tests.
    /// </summary>
    public required List<string> TestGlobs { get; init; }

    /// <summary>
    /// Regexes matched against a C# project's assembly name. A project whose
    /// assembly matches any is test code — every symbol in it emits
    /// <c>test = true</c> — alongside the framework-reference check. Fits a repo
    /// whose test assemblies share a naming convention (e.g. all end in
    /// <c>Test</c>). Empty = matched by nothing.
    /// </summary>
    public List<string> TestAssemblyRegexes { get; init; } = new();

    /// <summary>
    /// Skip the `dotnet restore` pass before opening the workspace. Set this
    /// when the caller has already restored (e.g. CI) or when offline.
    /// </summary>
    public required bool SkipRestore { get; init; }

    /// <summary>Timeout in milliseconds for the `dotnet restore` step.</summary>
    public required int RestoreTimeoutMs { get; init; }

    /// <summary>
    /// When a project fails to load because its (possibly nested) global.json
    /// pins an SDK that is not installed, install that SDK on demand via
    /// `kenn-toolchain provision-sdk` and retry. Off by default: it reaches the
    /// network at index time, and with it off an unsatisfiable pin stays the
    /// named, terminal failure it is today.
    /// </summary>
    public bool ProvisionSdk { get; init; }

    /// <summary>
    /// Stdout flush threshold in bytes. The JSONL sink buffers frames and
    /// flushes when buffered bytes reach this value (or <see cref="FlushFrames"/>).
    /// Larger values reduce syscall overhead at the cost of latency.
    /// </summary>
    public required int FlushBytes { get; init; }

    /// <summary>Stdout flush threshold in frames; companion to <see cref="FlushBytes"/>.</summary>
    public required int FlushFrames { get; init; }

    /// <summary>
    /// Cap on the number of project walks run concurrently within one
    /// invocation. Defaults to <see cref="Environment.ProcessorCount"/>.
    /// `1` produces a strictly serial walk (deterministic frame order;
    /// useful for debugging and snapshot comparisons).
    /// </summary>
    public required int MaxParallelism { get; init; }

    /// <summary>
    /// Comma-separated allowlist of edge kinds to emit (e.g. `calls,defined_in`).
    /// `null` or empty emits all edge kinds. Parsed lazily into
    /// <see cref="EdgeKindAllowlist"/>.
    /// </summary>
    public required string? EdgeKinds { get; init; }

    /// <summary>
    /// Lazily-parsed view of <see cref="EdgeKinds"/> as an <see cref="EdgeKind"/>
    /// set. `null` means "no restriction" (emit every edge kind). Unknown
    /// strings are silently dropped.
    /// </summary>
    public IReadOnlySet<EdgeKind>? EdgeKindAllowlist =>
        string.IsNullOrWhiteSpace(EdgeKinds)
            ? null
            : EdgeKinds.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                .Select(s => EdgeKindExtensions.TryParseWireString(s.ToLowerInvariant()))
                .Where(k => k is not null)
                .Select(k => k!.Value)
                .ToHashSet();
}
