using System.Diagnostics;
using Kenn.Dotnet.Cli;
using Microsoft.Build.Construction;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.MSBuild;
using Microsoft.Extensions.Logging;

namespace Kenn.Dotnet.Indexing;

internal static class SolutionLoader
{
    public static List<FileInfo> DiscoverProjectFiles(IndexOptions opts)
    {
        if (opts.Projects.Count > 0) return opts.Projects;
        var dir = opts.Workspace.FullName;
        return Directory.EnumerateFiles(dir)
            .Where(f =>
            {
                var ext = Path.GetExtension(f);
                return ext.Equals(".sln", StringComparison.OrdinalIgnoreCase)
                    || ext.Equals(".csproj", StringComparison.OrdinalIgnoreCase);
            })
            .Select(p => new FileInfo(p))
            .ToList();
    }

    /// <summary>
    /// Run `dotnet restore` for <paramref name="project"/>. Async because
    /// the caller is async, the restore can take tens of seconds, and the
    /// sync form had three failure modes:
    ///   - ignored the cancellation token (ctrl-C wouldn't stop it),
    ///   - leaked the child process on timeout (we logged but didn't kill,
    ///     so the orphan re-parented to PID 1 and held AF_UNIX socket
    ///     state — the next kenn-dotnet run's `MSBuildWorkspace.BuildHost`
    ///     could then trip the documented `Socket.ctor` `AccessViolation`
    ///     race against that leftover pipe state),
    ///   - risked pipe-buffer deadlock — stdout/stderr were redirected but
    ///     never read, so a chatty restore would block the child on its
    ///     next write and our wait would time out forever after.
    /// </summary>
    public static async Task RunRestoreAsync(
        FileInfo project,
        IndexOptions opts,
        ILogger log,
        CancellationToken ct)
    {
        if (opts.SkipRestore) return;

        var args = project.Extension.Equals(".sln", StringComparison.OrdinalIgnoreCase)
            ? $"restore \"{project.FullName}\" /p:EnableWindowsTargeting=true"
            : "restore /p:EnableWindowsTargeting=true";

        log.LogInformation("$ dotnet {Args}", args);
        var psi = new ProcessStartInfo("dotnet", args)
        {
            WorkingDirectory = opts.Workspace.FullName,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        using var p = Process.Start(psi)!;
        // Drain stdout/stderr concurrently so the child can't deadlock on a
        // full pipe buffer. We discard the bytes — failures show up via
        // ExitCode and any explicit error frames.
        //
        // BLOCKING reads, each on its own thread (see StartBlockingDrain),
        // deliberately, not CopyToAsync. On macOS an async read of a redirected
        // pipe completes through SocketAsyncEngine's kqueue loop; a completion
        // landing after the buffer has gone back to the ArrayPool corrupts it,
        // and the process dies with a *fatal* AccessViolationException inside
        // PipeStream.ReadAsyncCore on a threadpool thread. Unrecoverable, and it
        // kills the whole index run.
        //
        // Measured on `kenn-dotnet index --workspace . --projects
        // kenn-dotnet.csproj`, same binary, same workload:
        //     CopyToAsync drains .... 2/12 runs, then 6/12 on a repeat
        //     blocking drains ....... 0 aborts / 32 runs
        //     --skip-restore ........ 0 aborts / 18 runs (never enters here)
        // `just probe-smoke` re-runs this workload and fails on any abort.
        //
        // This was long misattributed to a lingering Roslyn BuildHost (see
        // BuildHostGuard's class doc). It is not: `pgrep -f BuildHost` is empty
        // immediately before a crashing run, and the crash lands between the
        // "$ dotnet restore" log line and the exit-code check — inside this
        // method. A blocking read never enters the async completion path.
        var drainOut = StartBlockingDrain(p.StandardOutput.BaseStream);
        var drainErr = StartBlockingDrain(p.StandardError.BaseStream);

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeoutCts.CancelAfter(opts.RestoreTimeoutMs);
        try
        {
            await p.WaitForExitAsync(timeoutCts.Token);
            // Drain-side guard: a `dotnet restore` grandchild can outlive its
            // parent and keep the pipe-write end open, which would make
            // Task.WhenAll wait forever. Cap the post-exit drain; if it
            // overruns, the two reader threads stay blocked until the
            // grandchild closes the pipe or this process exits. They hold no
            // managed state and cannot corrupt anything — the reason to cap
            // rather than to abandon an async read.
            try { await Task.WhenAll(drainOut, drainErr).WaitAsync(TimeSpan.FromSeconds(5)); }
            catch (TimeoutException) { /* grandchild holds the pipe; bail */ }
        }
        catch (OperationCanceledException) when (!ct.IsCancellationRequested)
        {
            // Timeout (not caller cancellation): kill the orphan tree so
            // it doesn't keep holding NuGet/`obj` locks or AF_UNIX socket
            // state for the next run. Wait for the drains to finish (the
            // kill closes the pipes — they EOF instantly) so we don't
            // leave zombie reader tasks behind.
            log.LogWarning("dotnet restore did not finish within {Ms}ms; killing", opts.RestoreTimeoutMs);
            try { p.Kill(entireProcessTree: true); } catch { /* already gone */ }
            try { await Task.WhenAll(drainOut, drainErr); } catch { /* drain raced with kill */ }
            return;
        }
        if (p.ExitCode != 0)
        {
            log.LogWarning("dotnet restore exited with code {Code}", p.ExitCode);
        }
    }

    /// <summary>
    /// Drain <paramref name="stream"/> to nowhere with a blocking read, on a
    /// thread of its own. Synchronous by design — see <see cref="RunRestoreAsync"/>
    /// for why this must not be <c>CopyToAsync</c>.
    ///
    /// <see cref="TaskCreationOptions.LongRunning"/>, not <see cref="Task.Run"/>:
    /// the read blocks until the child's pipe reaches EOF, which the caller caps
    /// at <c>RestoreTimeoutMs</c> (five minutes by default). Parking two
    /// threadpool workers for that long would starve the Roslyn project loads
    /// running concurrently on the same pool, which grows by roughly one thread
    /// per second past the core count.
    ///
    /// The stream belongs to a <see cref="Process"/> the caller disposes. A read
    /// blocked at that moment ends by exception; that is the expected way out,
    /// not a failure to report.
    /// </summary>
    private static Task StartBlockingDrain(Stream stream) => Task.Factory.StartNew(
        () =>
        {
            try
            {
                stream.CopyTo(Stream.Null);
            }
            catch (IOException) { /* pipe closed under us */ }
            catch (ObjectDisposedException) { /* Process disposed while we blocked */ }
        },
        CancellationToken.None,
        TaskCreationOptions.LongRunning,
        TaskScheduler.Default);

    /// <summary>
    /// Load <paramref name="entry"/> (.sln or .csproj) into <paramref name="ws"/>,
    /// accumulating across calls. Bypasses
    /// <see cref="MSBuildWorkspace.OpenSolutionAsync"/> because it calls
    /// <c>ClearSolution()</c> internally — replacing prior projects rather than
    /// adding to them. Instead we parse the .sln ourselves to get .csproj paths,
    /// then call <see cref="MSBuildWorkspace.OpenProjectAsync"/> per unique
    /// path; OpenProjectAsync accumulates and Roslyn's MSBuildProjectLoader
    /// dedupes by canonical path. <paramref name="loadedPaths"/> tracks
    /// already-loaded paths across entries (case-insensitive) so transitive
    /// deps shared between .slns are loaded once.
    /// </summary>
    public static async Task LoadEntryIntoSharedWorkspaceAsync(
        MSBuildWorkspace ws,
        FileInfo entry,
        HashSet<string> loadedPaths,
        ILogger log,
        CancellationToken ct)
    {
        var ext = entry.Extension.ToLowerInvariant();

        // Fast path: the workspace is empty AND the entry is a .sln. Use
        // OpenSolutionAsync — one batched BuildHost call instead of N
        // per-csproj round-trips. Roslyn 4.7's BuildHost has a known AVE
        // race in System.Net.Sockets when it gets hit with many rapid
        // OpenProjectAsync calls, so we keep BuildHost interactions to a
        // minimum on the cold path.
        if (loadedPaths.Count == 0 && ext == ".sln")
        {
            var sln = await ws.OpenSolutionAsync(entry.FullName, cancellationToken: ct);
            foreach (var p in sln.Projects)
            {
                if (!string.IsNullOrEmpty(p.FilePath))
                {
                    loadedPaths.Add(Path.GetFullPath(p.FilePath));
                }
            }
            return;
        }

        // Slow path: expand to csproj paths, OpenProjectAsync each missing one.
        // OpenProjectAsync accumulates (unlike OpenSolutionAsync, which
        // ClearSolution()s), so prior loads are preserved.
        foreach (var csproj in ExpandToCsprojPaths(entry, log))
        {
            ct.ThrowIfCancellationRequested();
            var canon = Path.GetFullPath(csproj);
            if (!loadedPaths.Add(canon)) continue;
            try
            {
                await ws.OpenProjectAsync(canon, cancellationToken: ct);
                // Transitive ProjectReferences pulled in by OpenProjectAsync
                // are now in the workspace; record them so the next call
                // skips paths Roslyn already loaded for us.
                foreach (var p in ws.CurrentSolution.Projects)
                {
                    if (!string.IsNullOrEmpty(p.FilePath))
                    {
                        loadedPaths.Add(Path.GetFullPath(p.FilePath));
                    }
                }
            }
            catch (Exception ex)
            {
                log.LogDebug(ex, "OpenProjectAsync failed for {Path}", canon);
            }
        }
    }

    private static IEnumerable<string> ExpandToCsprojPaths(FileInfo entry, ILogger log)
    {
        var ext = entry.Extension.ToLowerInvariant();
        if (ext == ".csproj")
        {
            yield return entry.FullName;
            yield break;
        }
        if (ext != ".sln")
        {
            log.LogWarning("Unknown project file extension: {Path}", entry.FullName);
            yield break;
        }
        SolutionFile sln;
        try
        {
            sln = SolutionFile.Parse(entry.FullName);
        }
        catch (Exception ex)
        {
            log.LogWarning(ex, "Failed to parse solution {Path}", entry.FullName);
            yield break;
        }
        foreach (var p in sln.ProjectsInOrder)
        {
            // SolutionFolder / WebSite / etc. — only KnownToBeMSBuildFormat
            // projects are real .csproj/.vbproj files we can load.
            if (p.ProjectType != SolutionProjectType.KnownToBeMSBuildFormat) continue;
            yield return p.AbsolutePath;
        }
    }

    /// <summary>
    /// When MSBuild returns multiple Project objects per .csproj (one per target
    /// framework), prefer the net&lt;Major&gt;.0 one and skip the rest. This
    /// avoids indexing the same source file multiple times with different
    /// symbol identities.
    /// </summary>
    public static IEnumerable<Project> DedupeTargetFrameworks(IEnumerable<Project> projects)
    {
        var preferred = $"(net{Environment.Version.Major}.0)";
        return projects
            .GroupBy(p => p.FilePath)
            .Select(g => g.FirstOrDefault(p => p.Name.Contains(preferred, StringComparison.OrdinalIgnoreCase))
                         ?? g.First());
    }
}
