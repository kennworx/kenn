using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.Extensions.Logging;

namespace Kenn.Dotnet.Indexing;

/// <summary>
/// Roslyn 4.7+ MSBuildWorkspace launches an out-of-process
/// `Microsoft.CodeAnalysis.Workspaces.MSBuild.BuildHost.dll` child for project
/// evaluation. When kenn-dotnet exits cleanly the workspace's Dispose closes
/// the IPC pipes and the child exits; when kenn-dotnet crashes (or is killed)
/// the child is re-parented to PID 1 and survives indefinitely, holding pipe
/// state, NuGet locks, and `obj` file handles. Sweeping it is still worth doing.
///
/// NOTE: the AccessViolationException this guard was written to prevent did NOT
/// come from a lingering BuildHost. It came from draining `dotnet restore`'s
/// redirected pipes with CopyToAsync — see the comment in
/// SolutionLoader.RunRestoreAsync. `pgrep -f BuildHost` is empty immediately
/// before a crashing run; the crash lands inside RunRestoreAsync; and switching
/// that drain to a blocking read moved the abort rate from 6/12 to 0/32 with no
/// change to this guard. When an AVE appears, look for an async read on a pipe
/// whose stream is about to be disposed — not for an orphan.
///
/// This guard makes two best-effort sweeps:
///   - <see cref="KillAllExisting"/>: at startup, SIGKILL any BuildHost.dll
///     process re-parented to PID 1. We never kill a BuildHost owned by a
///     different live process (e.g. VS Code's C# extension): orphans are
///     identified strictly by `PPID == 1`.
///   - <see cref="KillOurChildren"/>: on graceful shutdown, ProcessExit, and
///     unhandled exception, SIGKILL any BuildHost.dll process whose PPID
///     equals our own. Belt and braces with `MSBuildWorkspace.Dispose()`.
///
/// Cross-platform: Unix uses `pgrep`/`ps`; Windows uses PowerShell's
/// `Get-CimInstance Win32_Process`. On Windows the orphan re-parenting goes
/// to PID 0 (System Idle) when its parent dies; we sweep that instead of 1.
/// </summary>
internal static class BuildHostGuard
{
    private const string BuildHostMarker = "Microsoft.CodeAnalysis.Workspaces.MSBuild.BuildHost.dll";

    /// <summary>Install ProcessExit + UnhandledException hooks so we
    /// kill our BuildHost children on the way out, even on a crash.</summary>
    public static void Install(ILogger log)
    {
        var ourPid = Environment.ProcessId;
        AppDomain.CurrentDomain.ProcessExit += (_, _) => KillOurChildren(ourPid, log);
        AppDomain.CurrentDomain.UnhandledException += (_, _) => KillOurChildren(ourPid, log);
    }

    /// <summary>Best-effort: kill ANY existing BuildHost.dll process at
    /// startup, regardless of PPID. The DLL is uniquely Roslyn 4.7+'s
    /// out-of-process MSBuildWorkspace child — no other tool uses it. Two
    /// reasons not to filter by PPID: (1) PPID-based detection of orphans
    /// races with kernel reparenting, so a freshly-orphaned child might
    /// briefly look not-orphaned, and (2) even a non-orphaned BuildHost
    /// (still parented to a dying kenn-dotnet sibling) corrupts the next
    /// run's IPC because it's still holding the AF_UNIX socket pair.</summary>
    public static void KillAllExisting(ILogger log)
    {
        foreach (var pid in FindBuildHosts(log))
        {
            log.LogDebug("BuildHostGuard: killing pre-existing BuildHost pid={Pid}", pid);
            TryKill(pid);
        }
    }

    /// <summary>Best-effort: kill any BuildHost.dll whose PPID equals
    /// <paramref name="ourPid"/>. Called explicitly at end of Run() and
    /// from ProcessExit/UnhandledException as a fallback; safe to call when
    /// there are none.</summary>
    public static void KillOurChildren(int ourPid, ILogger log)
    {
        var pids = IsWindows()
            ? FindBuildHostsWithPpidWindows(ourPid, log)
            : FindBuildHostsWithPpidUnix(ourPid, log);
        foreach (var pid in pids)
        {
            log.LogDebug("BuildHostGuard: killing our BuildHost child pid={Pid}", pid);
            TryKill(pid);
        }
    }

    private static bool IsWindows() => RuntimeInformation.IsOSPlatform(OSPlatform.Windows);

    /// <summary>List PIDs of every running BuildHost.dll, regardless of
    /// PPID. Excludes our own PID for safety. Cross-platform.</summary>
    private static IEnumerable<int> FindBuildHosts(ILogger log)
    {
        return IsWindows() ? FindBuildHostsWindows(log) : FindBuildHostsUnix(log);
    }

    private static IEnumerable<int> FindBuildHostsUnix(ILogger log)
    {
        var pgrep = RunCapture("pgrep", $"-af {BuildHostMarker.Replace(".", @"\.")}", log);
        if (pgrep is null) yield break;
        foreach (var line in pgrep.Split('\n', StringSplitOptions.RemoveEmptyEntries))
        {
            var sp = line.IndexOf(' ');
            if (sp <= 0) continue;
            if (!int.TryParse(line.AsSpan(0, sp), out var pid)) continue;
            if (pid == Environment.ProcessId) continue;
            yield return pid;
        }
    }

    private static IEnumerable<int> FindBuildHostsWindows(ILogger log)
    {
        const string script =
            "Get-CimInstance Win32_Process -Filter \"CommandLine LIKE '%BuildHost.dll%'\""
            + " | ForEach-Object { $_.ProcessId }";
        return RunPowerShellPids(script, log);
    }

    // ─── Unix (macOS / Linux) ───────────────────────────────────────────────

    /// <summary>Returns PIDs of `dotnet` processes running BuildHost.dll
    /// whose PPID matches the given target. Implemented via `pgrep -af` for
    /// argv search + `ps -o ppid=` for parent lookup — both available on
    /// macOS and Linux without additional packages.</summary>
    private static IEnumerable<int> FindBuildHostsWithPpidUnix(int targetPpid, ILogger log)
    {
        var pgrep = RunCapture("pgrep", $"-af {BuildHostMarker.Replace(".", @"\.")}", log);
        if (pgrep is null) yield break;

        foreach (var line in pgrep.Split('\n', StringSplitOptions.RemoveEmptyEntries))
        {
            var sp = line.IndexOf(' ');
            if (sp <= 0) continue;
            if (!int.TryParse(line.AsSpan(0, sp), out var pid)) continue;
            if (pid == Environment.ProcessId) continue;

            var ppidStr = RunCapture("ps", $"-o ppid= -p {pid}", log)?.Trim();
            if (!int.TryParse(ppidStr, out var ppid)) continue;
            if (ppid != targetPpid) continue;
            yield return pid;
        }
    }

    // ─── Windows (PowerShell + CIM) ─────────────────────────────────────────

    /// <summary>List BuildHost processes whose parent process is no longer
    /// alive. Windows doesn't reparent to PID 1 like Unix; PPID stays as the
    /// dead parent's old PID. The query joins to Win32_Process to filter
    /// rows whose ParentProcessId is missing.</summary>
    private static IEnumerable<int> FindOrphanedBuildHostsWindows(ILogger log)
    {
        const string script =
            "$bh = Get-CimInstance Win32_Process -Filter \"CommandLine LIKE '%BuildHost.dll%'\";"
            + "$alive = (Get-CimInstance Win32_Process | Select-Object -ExpandProperty ProcessId);"
            + "$bh | Where-Object { $alive -notcontains $_.ParentProcessId } | ForEach-Object { $_.ProcessId }";
        return RunPowerShellPids(script, log);
    }

    private static IEnumerable<int> FindBuildHostsWithPpidWindows(int targetPpid, ILogger log)
    {
        var script =
            $"Get-CimInstance Win32_Process -Filter \"CommandLine LIKE '%BuildHost.dll%' AND ParentProcessId = {targetPpid}\""
            + " | ForEach-Object { $_.ProcessId }";
        return RunPowerShellPids(script, log);
    }

    private static IEnumerable<int> RunPowerShellPids(string script, ILogger log)
    {
        var stdout = RunCapture("powershell", "-NoProfile -NonInteractive -Command \"" + script.Replace("\"", "\\\"") + "\"", log);
        if (stdout is null) yield break;
        foreach (var line in stdout.Split('\n', StringSplitOptions.RemoveEmptyEntries))
        {
            if (int.TryParse(line.Trim(), out var pid) && pid != Environment.ProcessId)
                yield return pid;
        }
    }

    // ─── Kill / process spawn ───────────────────────────────────────────────

    private static void TryKill(int pid)
    {
        try
        {
            // Cross-platform via System.Diagnostics: SIGKILL on Unix, TerminateProcess on Windows.
            using var p = Process.GetProcessById(pid);
            p.Kill();
            p.WaitForExit(2000);
        }
        catch
        {
            // Best-effort: dead/gone/permission-denied targets are not worth surfacing.
        }
    }

    private static string? RunCapture(string fileName, string arguments, ILogger log)
    {
        try
        {
            using var p = Process.Start(new ProcessStartInfo
            {
                FileName = fileName,
                Arguments = arguments,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
            });
            if (p is null) return null;
            var stdout = p.StandardOutput.ReadToEnd();
            p.WaitForExit(5000);
            return stdout;
        }
        catch (Exception ex)
        {
            log.LogDebug(ex, "BuildHostGuard: {File} {Args} failed", fileName, arguments);
            return null;
        }
    }
}
