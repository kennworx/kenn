using System.CommandLine;
using System.Runtime.CompilerServices;
using Kenn.Dotnet.Indexing;
using Kenn.Dotnet.Wire;
using Microsoft.Build.Locator;
using Microsoft.Extensions.Logging;

namespace Kenn.Dotnet.Cli;

/// <summary>
/// The `index` subcommand: declares its options, parses them into
/// <see cref="IndexOptions"/>, and runs <see cref="IndexerCore"/> to
/// stream JSONL frames on stdout. <see cref="Build"/> assembles the
/// fully-wired <see cref="Command"/> for <see cref="RootCommand"/> in
/// <c>Program.cs</c>; <see cref="Run"/> is the underlying entry point
/// (also reusable from tests).
/// </summary>
public static class IndexCommand
{
    /// <summary>
    /// Build the `index` Command with all options and the action handler
    /// wired up. The caller owns <paramref name="loggerFactory"/>'s
    /// lifecycle.
    /// </summary>
    public static Command Build(ILoggerFactory loggerFactory)
    {
        var workspaceOpt = new Option<DirectoryInfo>("--workspace")
        {
            Description = "Workspace root (default: cwd)",
            DefaultValueFactory = _ => new DirectoryInfo(Directory.GetCurrentDirectory()),
        };

        var projectsOpt = new Option<List<FileInfo>>("--projects")
        {
            Description = "Explicit .sln/.csproj paths. If empty, discovered under --workspace.",
            DefaultValueFactory = _ => new List<FileInfo>(),
            AllowMultipleArgumentsPerToken = true,
            Arity = ArgumentArity.ZeroOrMore,
        };

        var includeOpt = new Option<List<string>>("--include")
        {
            Description = "Glob patterns of files to include (default: all)",
            DefaultValueFactory = _ => new List<string>(),
            AllowMultipleArgumentsPerToken = true,
            Arity = ArgumentArity.ZeroOrMore,
        };

        var excludeOpt = new Option<List<string>>("--exclude")
        {
            Description = "Glob patterns of files to exclude",
            DefaultValueFactory = _ => new List<string>(),
            AllowMultipleArgumentsPerToken = true,
            Arity = ArgumentArity.ZeroOrMore,
        };

        var testGlobOpt = new Option<List<string>>("--test-glob")
        {
            Description = "Glob patterns marking test files; symbols in matching files emit test=true",
            DefaultValueFactory = _ => new List<string>(),
            AllowMultipleArgumentsPerToken = true,
            Arity = ArgumentArity.ZeroOrMore,
        };

        var testAssemblyRegexOpt = new Option<List<string>>("--test-assembly-regex")
        {
            Description = "Regexes matched against a project's assembly name; a match marks the whole project as test",
            DefaultValueFactory = _ => new List<string>(),
            AllowMultipleArgumentsPerToken = true,
            Arity = ArgumentArity.ZeroOrMore,
        };

        var skipRestoreOpt = new Option<bool>("--skip-restore")
        {
            Description = "Skip running `dotnet restore` before indexing",
            DefaultValueFactory = _ => false,
        };

        var restoreTimeoutOpt = new Option<int>("--restore-timeout-ms")
        {
            Description = "Timeout in ms for `dotnet restore`",
            DefaultValueFactory = _ => 300_000,
        };

        var flushBytesOpt = new Option<int>("--flush-bytes")
        {
            Description = "Producer flushes stdout when buffered bytes exceed this",
            DefaultValueFactory = _ => 1 << 20, // 1 MiB
        };

        var flushFramesOpt = new Option<int>("--flush-frames")
        {
            Description = "Producer flushes stdout when buffered frame count exceeds this",
            DefaultValueFactory = _ => 4096,
        };

        var edgeKindsOpt = new Option<string?>("--edge-kinds")
        {
            Description = "Comma-separated allowlist of edge kinds (default: all). "
                        + "e.g. defined_in,calls,implements",
        };

        var maxParallelismOpt = new Option<int>("--max-parallelism")
        {
            Description = "Cap on concurrent project walks. Default: ProcessorCount. "
                        + "Pass 1 for a strictly serial (deterministic) walk.",
            DefaultValueFactory = _ => Environment.ProcessorCount,
        };

        var cmd = new Command("index", "Index a C# workspace and stream JSONL on stdout")
        {
            workspaceOpt,
            projectsOpt,
            includeOpt,
            excludeOpt,
            testGlobOpt,
            testAssemblyRegexOpt,
            skipRestoreOpt,
            restoreTimeoutOpt,
            flushBytesOpt,
            flushFramesOpt,
            edgeKindsOpt,
            maxParallelismOpt,
        };

        cmd.SetAction(async (parseResult, ct) =>
        {
            var opts = new IndexOptions
            {
                Workspace = parseResult.GetValue(workspaceOpt)!,
                Projects = parseResult.GetValue(projectsOpt) ?? new(),
                Include = parseResult.GetValue(includeOpt) ?? new(),
                Exclude = parseResult.GetValue(excludeOpt) ?? new(),
                TestGlobs = parseResult.GetValue(testGlobOpt) ?? new(),
                TestAssemblyRegexes = parseResult.GetValue(testAssemblyRegexOpt) ?? new(),
                SkipRestore = parseResult.GetValue(skipRestoreOpt),
                RestoreTimeoutMs = parseResult.GetValue(restoreTimeoutOpt),
                FlushBytes = parseResult.GetValue(flushBytesOpt),
                FlushFrames = parseResult.GetValue(flushFramesOpt),
                EdgeKinds = parseResult.GetValue(edgeKindsOpt),
                MaxParallelism = Math.Max(1, parseResult.GetValue(maxParallelismOpt)),
            };

            var logger = loggerFactory.CreateLogger("Kenn.Dotnet.Cli.IndexCommand");
            using var sink = JsonlSink.OpenStdout(opts.FlushBytes, opts.FlushFrames);

            // Register MSBuild here, in a frame that references no MSBuild type.
            // `Run`, which does, is NoInlining, so its body cannot be hoisted
            // into this frame and its types cannot load before we return.
            //
            // A failure carries its reason onto the wire, not just stderr: every
            // other failure path in `Run` emits meta+error+end, and the pipeline
            // reads the reason from the stream.
            // Search for a global.json pin from the WORKSPACE, not the process's
            // cwd: a locator failure is usually the indexed repo pinning an SDK
            // this toolchain does not have, and the two dirs differ whenever the
            // caller passes --workspace.
            if (!MsBuildBootstrap.TryRegister(out var reason, opts.Workspace?.FullName))
            {
                // Console.Error, not the logger: `KENN_DOTNET_LOG` sets the
                // logger's minimum level, so `LogError` is silenced at Critical
                // and above. This is the only explanation a user without a .NET
                // SDK ever gets; it must not be suppressible. Exactly once here,
                // and once on the wire below.
                Console.Error.WriteLine(reason);
                WriteStartupFailure(sink, opts, reason);
                return 1;
            }

            return await Run(opts, sink, loggerFactory, logger, ct);
        });

        return cmd;
    }

    /// <summary>
    /// Emit a self-contained meta+error+end triple for a failure that happened
    /// before <see cref="Run"/> could open the stream, so the consumer sees the
    /// reason on the wire rather than only on stderr.
    /// </summary>
    private static void WriteStartupFailure(JsonlSink sink, IndexOptions opts, string message)
    {
        sink.Write(new MetaFrame
        {
            ProjectRoot = new Uri(new Uri("file://"), opts.Workspace.FullName).ToString(),
            Tool = "kenn-dotnet",
            ToolVersion = MsBuildBootstrap.ToolVersion,
            Language = "csharp",
        });
        sink.Write(new ErrorFrame { Severity = "error", Source = "indexer", Message = message });
        sink.Write(new EndFrame { Stats = new EndStats { Files = 0, Symbols = 0, Edges = 0, Errors = 1 } });
        sink.Flush();
    }

    /// <summary>
    /// Indexing entry point. Used by <see cref="Build"/> and reusable from tests.
    ///
    /// PRECONDITION: <see cref="MsBuildBootstrap.TryRegister"/> must have
    /// succeeded first. This is the method that loads MSBuild types, via
    /// <see cref="IndexerCore"/>'s Roslyn workspace.
    ///
    /// The compiler already isolates those loads in this method's async state
    /// machine, so they cannot land in the caller's frame today. <c>NoInlining</c>
    /// makes that explicit rather than incidental: it is what preserves the
    /// invariant should this method ever become synchronous. The guard below is
    /// therefore best-effort — a JIT that resolved an MSBuild type before the
    /// first statement ran would surface a load failure instead of this message.
    /// </summary>
    [MethodImpl(MethodImplOptions.NoInlining)]
    public static async Task<int> Run(
        IndexOptions opts,
        JsonlSink sink,
        ILoggerFactory loggerFactory,
        ILogger logger,
        CancellationToken ct)
    {
        if (!MSBuildLocator.IsRegistered)
        {
            throw new InvalidOperationException(
                "IndexCommand.Run requires MsBuildBootstrap.TryRegister() to have succeeded first; "
                + "registering after an MSBuild type has loaded is too late.");
        }

        logger.LogInformation("workspace={Workspace} projects={ProjectCount}",
            opts.Workspace.FullName, opts.Projects.Count);

        // Roslyn 4.7's MSBuildWorkspace launches an out-of-process BuildHost
        // (Microsoft.CodeAnalysis.Workspaces.MSBuild.BuildHost.dll) and reads
        // its stdout via async pipes backed by AF_UNIX socket pairs on macOS.
        // The child outlives kenn-dotnet on crashes and even on some clean
        // exits — it gets reparented to PID 1 and keeps holding the socket
        // pair. The next kenn-dotnet run then collides with that lingering
        // socket state inside Socket.ctor and itself crashes with
        // AccessViolationException. We pre-emptively SIGKILL any existing
        // BuildHost.dll at startup (the DLL is uniquely Roslyn's, no other
        // tool uses it), and arm a ProcessExit/UnhandledException fallback
        // for sweeping our own children.
        BuildHostGuard.Install(logger);
        BuildHostGuard.KillAllExisting(logger);

        sink.Write(new MetaFrame
        {
            ProjectRoot = new Uri(new Uri("file://"), opts.Workspace.FullName).ToString(),
            Tool = "kenn-dotnet",
            ToolVersion = MsBuildBootstrap.ToolVersion,
            Language = "csharp",
        });

        var coreLogger = loggerFactory.CreateLogger("Kenn.Dotnet.Indexing");
        var core = new IndexerCore(opts, sink, coreLogger);
        EndStats stats;
        try
        {
            stats = await core.RunAsync(ct);
        }
        catch (Exception ex)
        {
            sink.Write(new ErrorFrame
            {
                Severity = "error",
                Source = "indexer",
                Message = ex.Message.Replace("\r\n", " ").Replace('\n', ' ').Replace('\r', ' '),
            });
            logger.LogDebug(ex, "Indexing failed");
            stats = new EndStats { Files = 0, Symbols = 0, Edges = 0, Errors = 1 };
        }

        sink.Write(new EndFrame { Stats = stats });
        sink.Flush();

        // Synchronous sweep of our own BuildHost child(ren) before returning
        // — `MSBuildWorkspace.Dispose()` plus the ProcessExit hook should be
        // enough on paper, but in practice Dispose returns before the child
        // has actually exited and the runtime tears down before ProcessExit
        // gets a chance to run pgrep. Killing here, while we still have a
        // managed thread to spawn `pgrep` on, is reliable.
        BuildHostGuard.KillOurChildren(Environment.ProcessId, logger);

        logger.LogInformation("done files={Files} symbols={Symbols} edges={Edges} errors={Errors}",
            stats.Files, stats.Symbols, stats.Edges, stats.Errors);
        // Exit 0 if we produced any useful frames, regardless of soft
        // errors (per-file parse failures, MSBuild diagnostics elevated
        // to error severity, etc.). The Rust side already records every
        // error frame via the JSONL stream; reflecting that in the exit
        // code as well caused the pipeline to mark the whole unit
        // Partial and made aggregation skip otherwise-valid data.
        //
        // Exit 1 only on hard startup failure: the catch-block above
        // sets Files = 0 / Symbols = 0 / Edges = 0 / Errors = 1, which
        // matches the "no work done at all" branch.
        var producedAnything = stats.Files > 0 || stats.Symbols > 0 || stats.Edges > 0;
        return producedAnything ? 0 : 1;
    }
}
