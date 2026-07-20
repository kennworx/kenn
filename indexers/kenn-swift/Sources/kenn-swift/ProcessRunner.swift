#if canImport(Glibc)
    import Glibc
#elseif canImport(Darwin)
    import Darwin
#endif
import Foundation

/// Subprocess spawning for kenn-swift.
///
/// On POSIX this uses `posix_spawn` + `waitpid` directly rather than
/// `Foundation.Process`, whose `run()` + pipe + `waitUntilExit()` path
/// deadlocks in `swift:6.x` Linux containers — a 5-line `Foundation.Process`
/// repro hangs there (not the sandbox, bind mounts, arch emulation, `--init`,
/// or seccomp; see the `docker-swift-foundation-process-hang` note). The libc
/// primitives do not hang, so this is what lets kenn-swift drive `swift build`
/// inside its container. On Windows `posix_spawn` is unavailable and
/// `Foundation.Process` has no such bug, so it is kept there.
enum ProcessRunner {
    /// Run `launchPath args`, routing the child's stdout AND stderr to the
    /// parent's stderr. kenn-swift's own stdout is the JSONL channel, so child
    /// output must never reach it. Returns whether the child exited 0.
    static func run(_ launchPath: String, _ args: [String]) -> Bool {
        #if os(Windows)
            return foundationRun(launchPath, args)
        #else
            // Child stdout(1) and stderr(2) both → the parent's stderr(2).
            guard let pid = posixSpawn(launchPath, args, [.dup2(2, 1), .dup2(2, 2)]) else {
                logError("spawn \(launchPath): posix_spawn failed")
                return false
            }
            return waitFor(pid) == 0
        #endif
    }

    /// Run `launchPath args` capturing the child's stdout; its stderr inherits
    /// the parent's. Returns the captured bytes on exit 0, else nil.
    static func capture(_ launchPath: String, _ args: [String]) -> Data? {
        #if os(Windows)
            return foundationCapture(launchPath, args)
        #else
            var fds: [Int32] = [0, 0]
            guard pipe(&fds) == 0 else { return nil }
            let (readEnd, writeEnd) = (fds[0], fds[1])
            // Child stdout(1) → the pipe's write end; the child needs no read end.
            guard let pid = posixSpawn(launchPath, args, [.dup2(writeEnd, 1), .close(readEnd)])
            else {
                close(readEnd)
                close(writeEnd)
                logError("spawn \(launchPath): posix_spawn failed")
                return nil
            }
            // The parent keeps only the read end and drains it to EOF BEFORE
            // waiting, so a child that fills the pipe buffer can't block (the
            // classic pipe deadlock).
            close(writeEnd)
            let data = drain(readEnd)
            close(readEnd)
            return waitFor(pid) == 0 ? data : nil
        #endif
    }
}

#if !os(Windows)

    /// A file-descriptor setup step applied to the child before exec.
    private enum FdAction {
        case dup2(Int32, Int32)  // dup2(src, dstInChild)
        case close(Int32)
    }

    /// `posix_spawn` `path` with `args` (inheriting the parent environment) after
    /// applying `fdActions` to the child. `path` must be absolute — callers
    /// resolve via PATH first, and this is `posix_spawn`, not `posix_spawnp`, so
    /// it performs no PATH search. Returns the child pid, or nil on failure.
    private func posixSpawn(_ path: String, _ args: [String], _ fdActions: [FdAction]) -> pid_t? {
        // `posix_spawn_file_actions_t` differs by platform: a heap pointer on
        // Darwin (nil until `_init` allocates), a struct on Glibc. Every
        // file-actions call (and `posix_spawn` itself) takes `&actions`
        // uniformly once it's declared to match the platform.
        #if canImport(Darwin)
            var actions: posix_spawn_file_actions_t?
        #else
            var actions = posix_spawn_file_actions_t()
        #endif
        guard posix_spawn_file_actions_init(&actions) == 0 else { return nil }
        defer { posix_spawn_file_actions_destroy(&actions) }
        for action in fdActions {
            switch action {
            case .dup2(let src, let dst): posix_spawn_file_actions_adddup2(&actions, src, dst)
            case .close(let fd): posix_spawn_file_actions_addclose(&actions, fd)
            }
        }

        var argv: [UnsafeMutablePointer<CChar>?] = ([path] + args).map { strdup($0) }
        argv.append(nil)
        var envp: [UnsafeMutablePointer<CChar>?] = ProcessInfo.processInfo.environment.map {
            strdup("\($0.key)=\($0.value)")
        }
        envp.append(nil)
        defer {
            for p in argv { free(p) }
            for p in envp { free(p) }
        }

        var pid: pid_t = 0
        let rc = posix_spawn(&pid, path, &actions, nil, argv, envp)
        return rc == 0 ? pid : nil
    }

    /// Read `fd` to EOF.
    private func drain(_ fd: Int32) -> Data {
        var out = Data()
        var buf = [UInt8](repeating: 0, count: 8192)
        while true {
            let n = read(fd, &buf, buf.count)
            if n > 0 {
                out.append(buf, count: n)
            } else if n == 0 {
                break  // EOF
            } else if errno == EINTR {
                continue
            } else {
                break  // read error
            }
        }
        return out
    }

    /// Wait for `pid` and return its exit code, or -1 if it did not exit
    /// normally (signalled). Retries across `EINTR`. The `WIFEXITED` /
    /// `WEXITSTATUS` macros are not imported into Swift, so they are inlined.
    private func waitFor(_ pid: pid_t) -> Int32 {
        var status: Int32 = 0
        while waitpid(pid, &status, 0) == -1 && errno == EINTR {}
        if status & 0x7f == 0 {  // WIFEXITED
            return (status >> 8) & 0xff  // WEXITSTATUS
        }
        return -1
    }

#endif

#if os(Windows)

    /// Windows retains `Foundation.Process` (no container deadlock there). Child
    /// stdout + stderr both go to the parent's stderr.
    private func foundationRun(_ launchPath: String, _ args: [String]) -> Bool {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: launchPath)
        proc.arguments = args
        proc.standardOutput = FileHandle.standardError
        proc.standardError = FileHandle.standardError
        do {
            try proc.run()
            proc.waitUntilExit()
        } catch {
            logError("spawn \(launchPath): \(error)")
            return false
        }
        return proc.terminationStatus == 0
    }

    /// Windows `Foundation.Process` capturing the child's stdout.
    private func foundationCapture(_ launchPath: String, _ args: [String]) -> Data? {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: launchPath)
        proc.arguments = args
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = FileHandle.standardError
        do {
            try proc.run()
        } catch {
            logError("spawn \(launchPath): \(error)")
            return nil
        }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        proc.waitUntilExit()
        return proc.terminationStatus == 0 ? data : nil
    }

#endif
